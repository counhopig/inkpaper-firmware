//! Multi-alarm store, backed by one NVS blob. The PCF8563 only has a single
//! live hardware alarm slot, so `program_hardware_alarm` always figures out
//! which stored alarm is chronologically nearest and reprograms the RTC to
//! just that one - see `rtc::Pcf8563::set_alarm`.

use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use serde::{Deserialize, Serialize};

use crate::rtc::{is_leap, AlarmRegs, DateTime, Pcf8563};

const NAMESPACE: &str = "inkpaper_alrm";
const KEY_ALARMS: &str = "alarms";
/// Generous headroom over what a few dozen short JSON alarm records need;
/// NVS blob entries top out around ~4000 bytes on this partition anyway.
const BLOB_BUF_LEN: usize = 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Repeat {
    Daily,
    /// Fires once on this calendar date, then the caller is expected to
    /// drop it from the store (see `main.rs`'s ack-and-rearm flow).
    Once {
        year: u16,
        month: u8,
        day: u8,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredAlarm {
    pub id: u8,
    pub hour: u8,
    pub minute: u8,
    pub repeat: Repeat,
    pub enabled: bool,
    pub label: String,
}

pub struct AlarmStore {
    nvs: EspDefaultNvs,
}

impl AlarmStore {
    /// `partition` must be a clone of the one shared `EspDefaultNvsPartition`
    /// handle `main.rs` takes once - see the doc comment on
    /// `storage::PersistedCounters::open` for why a second independent
    /// `EspDefaultNvsPartition::take()` here would fail at boot.
    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        let nvs = EspDefaultNvs::new(partition, NAMESPACE, true)
            .map_err(|e| anyhow!("failed to open NVS namespace '{NAMESPACE}': {e}"))?;
        Ok(Self { nvs })
    }

    /// Empty list if nothing has been saved yet.
    pub fn load(&self) -> Result<Vec<StoredAlarm>> {
        let mut buf = [0u8; BLOB_BUF_LEN];
        let bytes = self
            .nvs
            .get_blob(KEY_ALARMS, &mut buf)
            .map_err(|e| anyhow!("NVS get_blob({KEY_ALARMS}) failed: {e}"))?;
        match bytes {
            Some(bytes) => {
                serde_json::from_slice(bytes).map_err(|e| anyhow!("alarms JSON decode failed: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    pub fn save(&self, alarms: &[StoredAlarm]) -> Result<()> {
        let bytes =
            serde_json::to_vec(alarms).map_err(|e| anyhow!("alarms JSON encode failed: {e}"))?;
        if bytes.len() > BLOB_BUF_LEN {
            return Err(anyhow!(
                "alarms blob too large: {} bytes (max {BLOB_BUF_LEN})",
                bytes.len()
            ));
        }
        self.nvs
            .set_blob(KEY_ALARMS, &bytes)
            .map_err(|e| anyhow!("NVS set_blob({KEY_ALARMS}) failed: {e}"))
    }
}

/// Absolute day number (proleptic Gregorian, epoch 1970-01-01) - only used
/// to order alarms against each other, matching the calendar math already
/// in `rtc::DateTime::from_unix`.
fn days_since_epoch(year: u16, month: u8, day: u8) -> i64 {
    let mut days: i64 = 0;
    for y in 1970..year as i64 {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days = if is_leap(year as i64) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for m in month_days.iter().take(month as usize - 1) {
        days += *m as i64;
    }
    days + day as i64 - 1
}

/// Minutes from `now` until `alarm` next fires, or `i64::MAX` if it's a
/// `Once` alarm whose date has already passed (a live store shouldn't have
/// these - `main.rs` drops fired one-shots - but `next_due` stays correct
/// even if one lingers).
fn minutes_until(alarm: &StoredAlarm, now: &DateTime) -> i64 {
    let now_minutes = now.hour as i64 * 60 + now.minute as i64;
    let alarm_minutes = alarm.hour as i64 * 60 + alarm.minute as i64;
    match alarm.repeat {
        Repeat::Daily => {
            let mut delta = alarm_minutes - now_minutes;
            if delta < 0 {
                delta += 24 * 60;
            }
            delta
        }
        Repeat::Once { year, month, day } => {
            let now_days = days_since_epoch(now.year, now.month, now.day);
            let alarm_days = days_since_epoch(year, month, day);
            let delta = (alarm_days - now_days) * 24 * 60 + (alarm_minutes - now_minutes);
            if delta < 0 {
                i64::MAX
            } else {
                delta
            }
        }
    }
}

/// Next unused id, so callers adding an alarm don't have to track a counter
/// themselves - just `id: next_id(&alarms)`.
pub fn next_id(alarms: &[StoredAlarm]) -> u8 {
    alarms.iter().map(|a| a.id).max().map_or(0, |m| m + 1)
}

/// True for a `Once` alarm whose date has already passed relative to `now`
/// - i.e. it fired (or was skipped over) and should be dropped from the
///   store so it doesn't linger and confuse `next_due` forever. Daily alarms
///   recur on their own and are never "expired".
pub fn is_expired_once(alarm: &StoredAlarm, now: &DateTime) -> bool {
    matches!(alarm.repeat, Repeat::Once { .. }) && minutes_until(alarm, now) == i64::MAX
}

/// Picks the chronologically nearest enabled alarm relative to `now`.
pub fn next_due<'a>(alarms: &'a [StoredAlarm], now: &DateTime) -> Option<&'a StoredAlarm> {
    alarms
        .iter()
        .filter(|a| a.enabled)
        .min_by_key(|a| minutes_until(a, now))
}

/// Reprograms the PCF8563's single hardware alarm slot to whichever stored
/// alarm is nearest, or clears it if none are enabled. Call after boot,
/// after any alarm-list edit, and immediately after acking a fired alarm.
pub fn program_hardware_alarm(
    rtc: &mut Pcf8563,
    alarms: &[StoredAlarm],
    now: &DateTime,
) -> Result<()> {
    match next_due(alarms, now) {
        Some(alarm) => {
            let day = match alarm.repeat {
                Repeat::Daily => None,
                Repeat::Once { year, month, day } => {
                    // The PCF8563 alarm slot has no month/year register - it
                    // only compares day-of-month, hour, and minute. Arming
                    // `day` while `now` is outside the alarm's target month
                    // would make the chip fire on this month's (or an
                    // earlier month's) occurrence of that day-of-month,
                    // ringing the alarm months before the real date. Only
                    // arm the hardware once we're actually in the target
                    // month; otherwise leave the slot cleared until a later
                    // boot/sync/edit re-evaluates (periodic auto-sync calls
                    // this function on every successful sync, so a
                    // configured device re-checks at least that often).
                    if now.year == year && now.month == month {
                        Some(day)
                    } else {
                        log::info!(
                            "Once alarm id={} scheduled for {:04}-{:02}-{:02} {:02}:{:02} is outside the current month ({:04}-{:02}); deferring hardware arm to avoid an early false ring",
                            alarm.id, year, month, day, alarm.hour, alarm.minute, now.year, now.month
                        );
                        rtc.clear_alarm()?;
                        return Ok(());
                    }
                }
            };
            rtc.set_alarm(&AlarmRegs {
                minute: alarm.minute,
                hour: alarm.hour,
                day,
                weekday: None,
            })?;
            log::info!(
                "Hardware alarm armed: id={} {:02}:{:02} ({:?})",
                alarm.id,
                alarm.hour,
                alarm.minute,
                alarm.repeat
            );
        }
        None => {
            rtc.clear_alarm()?;
            log::info!("No enabled alarms; hardware alarm cleared");
        }
    }
    Ok(())
}
