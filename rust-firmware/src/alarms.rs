//! Multi-alarm store, backed by one NVS blob. The PCF8563 only has a single
//! live hardware alarm slot, so `program_hardware_alarm` always figures out
//! which stored alarm is chronologically nearest and reprograms the RTC to
//! just that one - see `rtc::Pcf8563::set_alarm`.

use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::rtc::{is_leap, AlarmRegs, DateTime, Pcf8563};

const NAMESPACE: &str = "inkwash_alrm";
const KEY_ALARMS: &str = "alarms";
/// Locally-changed `local_id`s pending upload (two-way sync dirty set).
const KEY_DIRTY: &str = "dirty";
/// Generous headroom over what a few dozen short JSON alarm records need;
/// NVS blob entries top out around ~4000 bytes on this partition anyway.
const BLOB_BUF_LEN: usize = 1024;

/// Recurrence schedule, wire-compatible with the server's
/// `models::Repeat`. Externally tagged by serde: `"Daily"`, `{"Weekly":
/// {"days": [0, 2, 4]}}`, `{"Monthly": {"days": [1, 15]}}`, or `{"Once":
/// {"year": 2026, "month": 8, "day": 19}}`. Weekdays are 0=Sunday ..
/// 6=Saturday (matching the RTC's `DateTime.weekday` and JS
/// `Date.getDay()`); month days are 1..=31.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Repeat {
    /// Every day at the alarm's time.
    Daily,
    /// Days of the week at the alarm's time. Never empty once created.
    Weekly { days: Vec<u8> },
    /// Days of the month at the alarm's time. Never empty once created.
    Monthly { days: Vec<u8> },
    /// Fires once on this calendar date, then the caller is expected to
    /// drop it from the store (see `main.rs`'s ack-and-rearm flow).
    Once { year: u16, month: u8, day: u8 },
}

impl Repeat {
    /// Whether this schedule covers the given calendar date. `weekday`
    /// is 0=Sunday..6=Saturday.
    pub fn fires_on(&self, year: u16, month: u8, day: u8, weekday: u8) -> bool {
        match self {
            Repeat::Daily => true,
            Repeat::Weekly { days } => days.contains(&weekday),
            Repeat::Monthly { days } => days.contains(&day),
            Repeat::Once {
                year: y,
                month: m,
                day: d,
            } => *y == year && *m == month && *d == day,
        }
    }
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

    // --- Two-way sync dirty tracking -------------------------------------
    //
    // Same contract as `TodoStore::mark_dirty`: only `local_id`s whose
    // `enabled` flag changed *locally* are uploaded, so a Server/Desktop
    // edit isn't clobbered by the device's stale copy on the next sync.

    /// Marks `id` as locally changed (enabled flag) and pending upload.
    pub fn mark_dirty(&self, id: u8) -> Result<()> {
        let mut dirty = self.dirty_ids()?;
        if !dirty.contains(&id) {
            dirty.push(id);
        }
        let bytes =
            serde_json::to_vec(&dirty).map_err(|e| anyhow!("dirty JSON encode failed: {e}"))?;
        self.nvs
            .set_blob(KEY_DIRTY, &bytes)
            .map_err(|e| anyhow!("NVS set_blob({KEY_DIRTY}) failed: {e}"))
    }

    /// `local_id`s changed locally since the last successful sync.
    pub fn dirty_ids(&self) -> Result<Vec<u8>> {
        let mut buf = [0u8; BLOB_BUF_LEN];
        let bytes = self
            .nvs
            .get_blob(KEY_DIRTY, &mut buf)
            .map_err(|e| anyhow!("NVS get_blob({KEY_DIRTY}) failed: {e}"))?;
        match bytes {
            Some(bytes) => {
                serde_json::from_slice(bytes).map_err(|e| anyhow!("dirty JSON decode failed: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    /// Drops the dirty set after a successful sync.
    pub fn clear_dirty(&self) -> Result<()> {
        self.nvs
            .remove(KEY_DIRTY)
            .map(|_| ())
            .map_err(|e| anyhow!("NVS remove({KEY_DIRTY}) failed: {e}"))
    }
}

/// Absolute day number (proleptic Gregorian, epoch 1970-01-01) - only used
/// to order alarms against each other, matching the calendar math already
/// in `rtc::DateTime::from_unix`.
pub(crate) fn days_since_epoch(year: u16, month: u8, day: u8) -> i64 {
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

/// Days per month for a given year (leap-aware).
fn month_lengths(year: i64) -> [i64; 12] {
    if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    }
}

/// Calendar date (year, month, day) for an absolute day number relative to
/// 1970-01-01. Inverse of `days_since_epoch`.
pub(crate) fn date_from_days(mut days: i64) -> (u16, u8, u8) {
    let mut year = 1970i64;
    loop {
        let dim = if is_leap(year) { 366 } else { 365 };
        if days < dim {
            break;
        }
        days -= dim;
        year += 1;
    }
    for (idx, dim) in month_lengths(year).iter().enumerate() {
        if days < *dim {
            return (year as u16, (idx + 1) as u8, (days + 1) as u8);
        }
        days -= *dim;
    }
    unreachable!("date_from_days ran past a year's day count")
}

/// Weekday (0=Sunday..6=Saturday) for an absolute day number. 1970-01-01
/// was a Thursday (3).
fn weekday_from_days(days: i64) -> u8 {
    ((days + 3).rem_euclid(7)) as u8
}

/// The next calendar date (year, month, day, weekday) that `repeat` covers,
/// at or after `now`'s date. Non-empty schedules always match within ~62
/// days, so the scan is bounded and the fallback is unreachable in practice.
pub(crate) fn next_occurrence_date(
    repeat: &Repeat,
    hour: u8,
    minute: u8,
    now: &DateTime,
) -> (u16, u8, u8, u8) {
    let now_days = days_since_epoch(now.year, now.month, now.day);
    let occurrence_minutes = hour as i64 * 60 + minute as i64;
    let now_minutes = now.hour as i64 * 60 + now.minute as i64;
    for offset in 0..370 {
        let days = now_days + offset;
        let (year, month, day) = date_from_days(days);
        let weekday = weekday_from_days(days);
        if repeat.fires_on(year, month, day, weekday)
            && !(offset == 0 && occurrence_minutes <= now_minutes)
        {
            return (year, month, day, weekday);
        }
    }
    (now.year, now.month, now.day, now.weekday)
}

/// Whole days from `now` until the given calendar date (0 = today,
/// negative = already passed). Used by the Home screen for the
/// "in N days" alarm countdown.
pub(crate) fn days_until(year: u16, month: u8, day: u8, now: &DateTime) -> i64 {
    days_since_epoch(year, month, day) - days_since_epoch(now.year, now.month, now.day)
}

/// Minutes from `now` until `alarm` next fires, or `i64::MAX` if it's a
/// `Once` alarm whose date has already passed (a live store shouldn't have
/// these - `main.rs` drops fired one-shots - but `next_due` stays correct
/// even if one lingers).
fn minutes_until(alarm: &StoredAlarm, now: &DateTime) -> i64 {
    let now_minutes = now.hour as i64 * 60 + now.minute as i64;
    let alarm_minutes = alarm.hour as i64 * 60 + alarm.minute as i64;
    match &alarm.repeat {
        Repeat::Daily => {
            let mut delta = alarm_minutes - now_minutes;
            if delta < 0 {
                delta += 24 * 60;
            }
            delta
        }
        Repeat::Weekly { .. } | Repeat::Monthly { .. } => {
            let now_days = days_since_epoch(now.year, now.month, now.day);
            for offset in 0..370 {
                let days = now_days + offset;
                let (year, month, day) = date_from_days(days);
                let weekday = weekday_from_days(days);
                if !alarm.repeat.fires_on(year, month, day, weekday) {
                    continue;
                }
                if offset == 0 && alarm_minutes <= now_minutes {
                    // Today's occurrence already passed; keep scanning for
                    // the next matching date.
                    continue;
                }
                return offset * 24 * 60 + (alarm_minutes - now_minutes);
            }
            i64::MAX
        }
        Repeat::Once { year, month, day } => {
            let now_days = days_since_epoch(now.year, now.month, now.day);
            let alarm_days = days_since_epoch(*year, *month, *day);
            let delta = (alarm_days - now_days) * 24 * 60 + (alarm_minutes - now_minutes);
            if delta < 0 {
                i64::MAX
            } else {
                delta
            }
        }
    }
}

/// First unused wire-compatible id. Searching gaps avoids overflow at 255 and
/// prevents the duplicate id that a wrapping `max + 1` would create.
pub fn next_id(alarms: &[StoredAlarm]) -> Option<u8> {
    (u8::MIN..=u8::MAX).find(|candidate| !alarms.iter().any(|a| a.id == *candidate))
}

/// True for a `Once` alarm whose date has already passed relative to `now`
/// - i.e. it fired (or was skipped over) and should be dropped from the
///   store so it doesn't linger and confuse `next_due` forever. Daily alarms
///   recur on their own and are never "expired".
pub fn is_expired_once(alarm: &StoredAlarm, now: &DateTime) -> bool {
    matches!(alarm.repeat, Repeat::Once { .. })
        && matches!(minutes_until(alarm, now), i64::MAX | ..=0)
}

/// Picks the chronologically nearest enabled alarm relative to `now`.
pub fn next_due<'a>(alarms: &'a [StoredAlarm], now: &DateTime) -> Option<&'a StoredAlarm> {
    alarms
        .iter()
        .filter(|a| a.enabled)
        .min_by_key(|a| minutes_until(a, now))
}

/// Timer wake needed to make a future-month one-shot alarm fully offline.
/// PCF8563 cannot compare month/year, so the ESP wakes one minute before the
/// earliest target month, remains in the normal main loop, and arms the RTC
/// alarm when the date boundary is observed. RTC alarm wake remains available
/// independently through EXT1 for already-armable alarms.
pub fn maintenance_wakeup_delay(alarms: &[StoredAlarm], now: &DateTime) -> Option<Duration> {
    let now_epoch = now.to_unix();
    alarms
        .iter()
        .filter(|alarm| alarm.enabled)
        .filter_map(|alarm| match alarm.repeat {
            Repeat::Once { year, month, .. }
                if (year, month) > (now.year, now.month) && (1..=12).contains(&month) =>
            {
                Some(
                    DateTime {
                        year,
                        month,
                        day: 1,
                        ..DateTime::default()
                    }
                    .to_unix(),
                )
            }
            _ => None,
        })
        .min()
        .map(|target_month| {
            // Wake before midnight so the ordinary date-boundary tick can do
            // the actual RTC programming without racing a 00:00 alarm.
            Duration::from_secs(target_month.saturating_sub(now_epoch + 60).max(1))
        })
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
            let day: Option<u8>;
            let weekday: Option<u8>;
            match &alarm.repeat {
                Repeat::Daily => {
                    day = None;
                    weekday = None;
                }
                Repeat::Weekly { .. } => {
                    // The PCF8563 supports weekday matching, so arm the
                    // next covered weekday; every ring is re-programmed by
                    // the ack-and-rearm flow, so it can't keep firing on
                    // later weeks that don't contain a nearer alarm.
                    let (_, _, _, dow) =
                        next_occurrence_date(&alarm.repeat, alarm.hour, alarm.minute, now);
                    day = None;
                    weekday = Some(dow);
                }
                Repeat::Monthly { .. } => {
                    let (_, _, day_of_month, _) =
                        next_occurrence_date(&alarm.repeat, alarm.hour, alarm.minute, now);
                    day = Some(day_of_month);
                    weekday = None;
                }
                Repeat::Once {
                    year,
                    month,
                    day: d,
                } => {
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
                    if now.year == *year && now.month == *month {
                        day = Some(*d);
                        weekday = None;
                    } else {
                        log::info!(
                            "Once alarm id={} scheduled for {:04}-{:02}-{:02} {:02}:{:02} is outside the current month ({:04}-{:02}); deferring hardware arm to avoid an early false ring",
                            alarm.id, year, month, d, alarm.hour, alarm.minute, now.year, now.month
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
                weekday,
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
