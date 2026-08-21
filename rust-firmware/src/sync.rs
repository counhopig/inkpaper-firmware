//! HTTPS sync client for pulling alarms and todos from the inkwash-server
//! (contract: `docs/sync-api.md`).
//!
//! `fetch_and_apply` handles conditional requests via `If-None-Match`/ETag,
//! parses the sync response, writes the fetched data into the local NVS
//! stores, and re-arms the RTC hardware alarm to whichever is now nearest -
//! this is the only place outside `screens.rs`'s on-device edit paths that
//! mutates `alarms::AlarmStore`/`todos::TodoStore`.

use anyhow::{anyhow, Result};
use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use embedded_svc::utils::io;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::alarms::{self, AlarmStore, StoredAlarm};
use crate::inbox::{InboxItem, InboxStore};
use crate::rtc::{DateTime, Pcf8563};
use crate::storage::PersistedCounters;
use crate::todos::{Importance, Todo, TodoStore};
use crate::watchdog;
use crate::wifi;

/// Response bodies from a compliant server are small (alarms/todos are
/// themselves capped to a couple KB each in NVS - see `alarms::BLOB_BUF_LEN`
/// / `todos::BLOB_BUF_LEN`); the inbox adds up to 20 small items. A fixed
/// buffer avoids a heap-growing read loop for a payload this bounded.
const RESPONSE_BUF_LEN: usize = 16384;

/// Outcome of a bidirectional sync after the merged server state is applied.
#[derive(Clone, Debug)]
pub enum SyncOutcome {
    Applied {
        alarm_count: usize,
        todo_count: usize,
        inbox_count: usize,
        inbox_truncated: bool,
        etag: Option<String>,
    },
}

/// Sync response body shape, deserialized directly from the server's JSON -
/// `StoredAlarm`/`Todo` already derive `Deserialize`, so no separate wire
/// DTO is needed. See `docs/sync-api.md` for the exact contract.
#[derive(Debug, Deserialize)]
struct SyncResponse {
    #[serde(default)]
    alarms: Vec<StoredAlarm>,
    #[serde(default)]
    todos: Vec<Todo>,
    #[serde(default)]
    inbox: Vec<InboxItem>,
    #[serde(default)]
    inbox_read_acked: Vec<u64>,
    #[serde(default)]
    inbox_truncated: bool,
}

/// Rejects malformed or internally inconsistent server state before any NVS
/// blob is replaced. Wire DTOs deliberately use plain integers for protocol
/// compatibility; the firmware must establish their invariants at this
/// boundary instead of letting invalid dates become array indices or RTC BCD.
fn validate_sync_response(response: &SyncResponse) -> Result<()> {
    let mut alarm_ids = HashSet::new();
    for alarm in &response.alarms {
        if !alarm_ids.insert(alarm.id) {
            return Err(anyhow!("duplicate alarm id {}", alarm.id));
        }
        if alarm.hour > 23 || alarm.minute > 59 {
            return Err(anyhow!(
                "alarm {} has invalid time {:02}:{:02}",
                alarm.id,
                alarm.hour,
                alarm.minute
            ));
        }
        validate_repeat(&alarm.repeat)
            .map_err(|err| anyhow!("alarm {} has invalid repeat: {err}", alarm.id))?;
    }

    let mut todo_ids = HashSet::new();
    for todo in &response.todos {
        if !todo_ids.insert(todo.id) {
            return Err(anyhow!("duplicate todo id {}", todo.id));
        }
        if let Some(due) = todo.due_date {
            validate_date(due.year, due.month, due.day)
                .map_err(|err| anyhow!("todo {} has invalid due date: {err}", todo.id))?;
        }
        if let Some(repeat) = &todo.repeat {
            if matches!(repeat, alarms::Repeat::Once { .. }) {
                return Err(anyhow!("todo {} uses unsupported Once repeat", todo.id));
            }
            validate_repeat(repeat)
                .map_err(|err| anyhow!("todo {} has invalid repeat: {err}", todo.id))?;
        }
    }

    let mut inbox_ids = HashSet::new();
    for item in &response.inbox {
        if !inbox_ids.insert(item.id) {
            return Err(anyhow!("duplicate inbox id {}", item.id));
        }
    }

    // Preflight the two fixed-size stores before writing either one. Without
    // this, an oversized todo response could replace alarms and then fail,
    // leaving a mixed-generation snapshot behind.
    if serde_json::to_vec(&response.alarms)?.len() > 1024 {
        return Err(anyhow!("alarm list exceeds device storage capacity"));
    }
    if serde_json::to_vec(&response.todos)?.len() > 2048 {
        return Err(anyhow!("todo list exceeds device storage capacity"));
    }
    Ok(())
}

fn validate_repeat(repeat: &alarms::Repeat) -> Result<()> {
    match repeat {
        alarms::Repeat::Daily => Ok(()),
        alarms::Repeat::Weekly { days } => {
            if days.is_empty() || days.iter().any(|day| *day > 6) {
                return Err(anyhow!("weekly days must be non-empty and within 0..=6"));
            }
            Ok(())
        }
        alarms::Repeat::Monthly { days } => {
            if days.is_empty() || days.iter().any(|day| !(1..=31).contains(day)) {
                return Err(anyhow!("monthly days must be non-empty and within 1..=31"));
            }
            Ok(())
        }
        alarms::Repeat::Once { year, month, day } => validate_date(*year, *month, *day),
    }
}

fn validate_date(year: u16, month: u8, day: u8) -> Result<()> {
    if !(2000..=2099).contains(&year) || !(1..=12).contains(&month) {
        return Err(anyhow!("date must be within 2000-01-01..=2099-12-31"));
    }
    let days = match month {
        2 if crate::rtc::is_leap(year as i64) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days).contains(&day) {
        return Err(anyhow!("day {day} is invalid for {year:04}-{month:02}"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct DeviceSyncRequest {
    alarms: Vec<DeviceAlarmState>,
    todos: Vec<DeviceTodoState>,
    #[serde(default)]
    inbox_read: Vec<u64>,
}

#[derive(Debug, Serialize)]
struct DeviceAlarmState {
    id: u8,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct DeviceTodoState {
    id: u8,
    done: bool,
    /// The device may cycle importance on-device; the server merges it.
    /// Serialized as `Some(..)` always by the current firmware, but kept
    /// optional so an older firmware's upload (without the field) is still
    /// wire-compatible on the server side.
    importance: Option<Importance>,
}

/// Fetches alarms and todos from `server_url`, applying conditional-request
/// semantics. Uploads local mutable flags first and returns the merged
/// server data's counts and new ETag. Any HTTP error, TLS error, or
/// JSON parse error returns `Err(...)` with a descriptive message rather
/// than panicking.
#[allow(clippy::too_many_arguments)]
pub fn fetch_and_apply(
    server_url: &str,
    token: &str,
    _etag: Option<&str>,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    inbox_store: &InboxStore,
    rtc: &mut Pcf8563,
    now: &DateTime,
) -> Result<SyncOutcome> {
    watchdog::feed();

    let config = HttpConfiguration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut client = HttpClient::wrap(
        EspHttpConnection::new(&config)
            .map_err(|e| anyhow!("HTTP connection setup failed: {e}"))?,
    );

    let local_alarms = alarm_store
        .load()
        .map_err(|e| anyhow!("failed to load local alarms for upload: {e}"))?;
    let local_todos = todo_store
        .load()
        .map_err(|e| anyhow!("failed to load local todos for upload: {e}"))?;
    let pending_read = inbox_store
        .pending_read()
        .map_err(|e| anyhow!("failed to load pending inbox reads for upload: {e}"))?;
    // Two-way sync: only items the user actually changed on-device are
    // uploaded (alarm `enabled`, todo `done`/`importance`). Server-side
    // edits therefore survive the next sync instead of being clobbered by
    // the device's stale copy; the dirty sets are cleared on success below.
    let dirty_alarms = alarm_store
        .dirty_ids()
        .map_err(|e| anyhow!("failed to load alarm dirty set: {e}"))?;
    let dirty_todos = todo_store
        .dirty_ids()
        .map_err(|e| anyhow!("failed to load todo dirty set: {e}"))?;
    let upload = DeviceSyncRequest {
        alarms: local_alarms
            .iter()
            .filter(|alarm| dirty_alarms.contains(&alarm.id))
            .map(|alarm| DeviceAlarmState {
                id: alarm.id,
                enabled: alarm.enabled,
            })
            .collect(),
        todos: local_todos
            .iter()
            .filter(|todo| dirty_todos.contains(&todo.id))
            .map(|todo| DeviceTodoState {
                id: todo.id,
                done: todo.done,
                // The device owns the todo's importance (long-ENTER cycles
                // it in `screens.rs`), so it's uploaded for the server to
                // merge.
                importance: Some(todo.importance),
            })
            .collect(),
        inbox_read: pending_read,
    };
    let request_body =
        serde_json::to_vec(&upload).map_err(|e| anyhow!("device sync JSON encode failed: {e}"))?;
    let content_length = request_body.len().to_string();
    let mut headers: Vec<(&str, &str)> = vec![
        ("accept", "application/json"),
        ("content-type", "application/json"),
        ("content-length", &content_length),
    ];
    let auth_header;
    if !token.is_empty() {
        auth_header = format!("Bearer {token}");
        headers.push(("authorization", &auth_header));
    }
    let mut request = client
        .request(Method::Post, server_url, &headers)
        .map_err(|e| anyhow!("POST {server_url} failed to start: {e}"))?;
    let mut written = 0usize;
    while written < request_body.len() {
        let count = request
            .write(&request_body[written..])
            .map_err(|e| anyhow!("POST {server_url} body write failed: {e}"))?;
        if count == 0 {
            return Err(anyhow!("POST {server_url} body write made no progress"));
        }
        written += count;
    }
    let mut response = request
        .submit()
        .map_err(|e| anyhow!("POST {server_url} failed: {e}"))?;

    let status = response.status();
    watchdog::feed();
    if status != 200 {
        return Err(anyhow!("sync request failed: HTTP {status}"));
    }

    let new_etag = response.header("etag").map(|s| s.to_string());

    let mut buf = [0u8; RESPONSE_BUF_LEN];
    let bytes_read = io::try_read_full(&mut response, &mut buf).map_err(|e| e.0)?;
    let body = &buf[..bytes_read];

    let parsed: SyncResponse = serde_json::from_slice(body)
        .map_err(|e| anyhow!("sync response JSON decode failed: {e}"))?;
    validate_sync_response(&parsed).map_err(|e| anyhow!("sync response validation failed: {e}"))?;

    alarm_store
        .save(&parsed.alarms)
        .map_err(|e| anyhow!("failed to save synced alarms: {e}"))?;
    todo_store
        .save(&parsed.todos)
        .map_err(|e| anyhow!("failed to save synced todos: {e}"))?;
    inbox_store
        .save(&parsed.inbox)
        .map_err(|e| anyhow!("failed to save synced inbox: {e}"))?;
    inbox_store
        .ack_read(&parsed.inbox_read_acked)
        .map_err(|e| anyhow!("failed to ack inbox reads: {e}"))?;
    // The merged server state now reflects everything we uploaded, so the
    // pending local changes are spent. Cleared only here, after a
    // successful round-trip - a failed sync keeps them for the retry.
    // The hardware alarm is part of the committed device state, not an
    // optional side effect. Do not acknowledge local mutations when the RTC
    // could not be brought in line with the newly stored alarm list.
    alarms::program_hardware_alarm(rtc, &parsed.alarms, now)
        .map_err(|e| anyhow!("failed to reprogram hardware alarm after sync: {e}"))?;
    alarm_store
        .clear_dirty()
        .map_err(|e| anyhow!("failed to clear alarm dirty set: {e}"))?;
    todo_store
        .clear_dirty()
        .map_err(|e| anyhow!("failed to clear todo dirty set: {e}"))?;

    log::info!(
        "Sync applied: {} alarms, {} todos, {} inbox (truncated={})",
        parsed.alarms.len(),
        parsed.todos.len(),
        parsed.inbox.len(),
        parsed.inbox_truncated
    );

    Ok(SyncOutcome::Applied {
        alarm_count: parsed.alarms.len(),
        todo_count: parsed.todos.len(),
        inbox_count: parsed.inbox.len(),
        inbox_truncated: parsed.inbox_truncated,
        etag: new_etag,
    })
}

/// Full "Sync Now" flow, shared by the on-device menu (`screens.rs`) and
/// the USB/BLE `SyncNow` command (`control.rs`): connects Wi-Fi using the
/// stored credentials (via the process's one shared `WifiManager` - see
/// its doc comment for why a fresh `EspWifi` per call crashes), loads
/// server config + cached ETag, calls `fetch_and_apply`, persists any new
/// ETag, then disconnects Wi-Fi again - mirroring `main.rs`'s boot-time
/// "connect only for as long as needed" pattern. `fetch_and_apply` itself
/// has no idea whether Wi-Fi is up; both call sites used to assume it
/// already was (main.rs disconnects after the initial NTP sync, so by the
/// time a user actually presses "Sync Now" minutes or hours later, there
/// is no active STA connection) - confirmed as a real bug via an
/// end-to-end hardware test (`ESP_ERR_HTTP_CONNECT` / "Host is
/// unreachable"), not just a hypothetical.
pub fn sync_now(
    counters: &PersistedCounters,
    wifi_mgr: &mut wifi::WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    inbox_store: &InboxStore,
    rtc: &mut Pcf8563,
    now: &DateTime,
) -> Result<SyncOutcome> {
    let creds = counters
        .wifi_creds()
        .map_err(|e| anyhow!("failed to load Wi-Fi credentials: {e}"))?
        .ok_or_else(|| {
            anyhow!("Wi-Fi not configured; use SetWifi or the on-device wizard first")
        })?;
    let cfg = counters
        .device_config()
        .map_err(|e| anyhow!("failed to load server config: {e}"))?
        .ok_or_else(|| anyhow!("Server not configured; use SetServer first"))?;
    let etag = counters.sync_etag().ok().flatten();

    // A second in-process Wi-Fi connect used to crash; the codebase once
    // worked around it by deep-sleep restarting for a fresh session (see
    // `wifi::WifiManager`'s doc comment for the full history). The manual
    // `esp_wifi_scan_start()` that `connect()` ran before every connection
    // turned out to be the trigger - credentials here always come from the
    // desktop's `SetWifi` command, so that scan was never needed and has
    // been removed. Multiple connects per boot now work; no restart needed.
    if wifi_mgr.used() {
        log::warn!("Wi-Fi already used this boot session; attempting a second connect (scan-free)");
    }

    wifi_mgr
        .connect(&creds)
        .map_err(|e| anyhow!("Wi-Fi connect failed: {e}"))?;

    let outcome = fetch_and_apply(
        &cfg.server_url,
        &cfg.auth_token,
        etag.as_deref(),
        alarm_store,
        todo_store,
        inbox_store,
        rtc,
        now,
    );

    // Daily NTP RTC alignment rides the live connection (SNTP needs Wi-Fi
    // up); skipped on failed syncs so a dead network retries next time.
    if outcome.is_ok() {
        maybe_align_rtc(counters, rtc, now);
    }

    // Disconnect regardless of outcome - nothing else needs Wi-Fi to stay
    // connected after this.
    wifi_mgr.disconnect();

    if let Ok(SyncOutcome::Applied {
        etag: Some(ref new_etag),
        ..
    }) = outcome
    {
        if let Err(err) = counters.save_sync_etag(new_etag) {
            log::warn!("Failed to save sync ETag: {err}");
        }
    }
    // Record when this sync ran, so the periodic auto-sync checker
    // (`main.rs`) knows how long it has been since the last one.
    if outcome.is_ok() {
        if let Err(err) = counters.set_last_sync_epoch(now.to_unix()) {
            log::warn!("Failed to record last-sync time: {err}");
        }
    }

    outcome
}

/// Resyncs the PCF8563 over NTP once per day. Must be called while Wi-Fi
/// is still connected (SNTP cannot start on a dead link) - `sync_now`
/// calls it before its final disconnect, so the daily check piggybacks on
/// whichever sync path happened to run. A failed NTP round-trip retries on
/// the next sync; the alignment timestamp is only recorded on success.
fn maybe_align_rtc(counters: &PersistedCounters, rtc: &mut Pcf8563, now: &DateTime) {
    let align_due = match counters.rtc_align_epoch() {
        Ok(Some(last)) => now.to_unix().saturating_sub(last) >= 24 * 3600,
        Ok(None) => true,
        Err(err) => {
            log::warn!("Failed to read RTC alignment time: {err}");
            false
        }
    };
    if !align_due {
        return;
    }
    let timezone_offset = counters.timezone_offset_minutes().unwrap_or(0);
    match wifi::ntp_sync_and_set_rtc(rtc, timezone_offset) {
        Ok(()) => {
            if let Err(err) = counters.set_rtc_align_epoch(now.to_unix()) {
                log::warn!("Failed to record RTC alignment time: {err}");
            }
        }
        Err(err) => log::warn!("Periodic RTC alignment failed: {err}"),
    }
}

/// Lightweight urgent-message poll: connects, sends a `POST` with the
/// `X-Inkpaper-Poll` header, reads a tiny `{"urgent": bool}` response, and
/// disconnects. The server answers immediately (no hold), so the firmware can
/// call this on a short timer to detect high-priority messages without
/// keeping a long connection open or blocking the main loop for long.
///
/// Returns `Ok(true)` when the server has an unread high-priority message.
/// Errors (no Wi-Fi, no server config, network failure) are returned so the
/// caller can fall back to a regular sync or log.
pub fn poll_urgent(counters: &PersistedCounters, wifi_mgr: &mut wifi::WifiManager) -> Result<bool> {
    let creds = counters
        .wifi_creds()?
        .ok_or_else(|| anyhow!("Wi-Fi not configured"))?;
    let cfg = counters
        .device_config()?
        .ok_or_else(|| anyhow!("Server not configured"))?;

    wifi_mgr.connect(&creds)?;

    let result = (|| -> Result<bool> {
        let config = HttpConfiguration {
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let mut client = HttpClient::wrap(
            EspHttpConnection::new(&config)
                .map_err(|e| anyhow!("HTTP connection setup failed: {e}"))?,
        );
        let mut headers: Vec<(&str, &str)> = vec![
            ("accept", "application/json"),
            ("content-type", "application/json"),
            ("x-inkwash-poll", "1"),
            ("content-length", "2"),
        ];
        let auth_header;
        if !cfg.auth_token.is_empty() {
            auth_header = format!("Bearer {}", cfg.auth_token);
            headers.push(("authorization", &auth_header));
        }
        let mut request = client
            .request(Method::Post, &cfg.server_url, &headers)
            .map_err(|e| anyhow!("POST {} failed to start: {e}", cfg.server_url))?;
        let mut written = 0usize;
        let body = b"{}";
        while written < body.len() {
            let count = request
                .write(&body[written..])
                .map_err(|e| anyhow!("POST body write failed: {e}"))?;
            if count == 0 {
                return Err(anyhow!("POST body write made no progress"));
            }
            written += count;
        }
        let mut response = request
            .submit()
            .map_err(|e| anyhow!("POST {} failed: {e}", cfg.server_url))?;
        watchdog::feed();
        if response.status() != 200 {
            return Err(anyhow!("urgent poll failed: HTTP {}", response.status()));
        }
        let mut buf = [0u8; 256];
        let bytes_read = io::try_read_full(&mut response, &mut buf).map_err(|e| e.0)?;
        let parsed: serde_json::Value = serde_json::from_slice(&buf[..bytes_read])
            .map_err(|e| anyhow!("urgent poll JSON decode failed: {e}"))?;
        Ok(parsed
            .get("urgent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    })();

    wifi_mgr.disconnect();
    result
}
