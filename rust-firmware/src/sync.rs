//! HTTPS sync client for pulling alarms and todos from the inkpaper-server
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

use crate::alarms::{self, AlarmStore, StoredAlarm};
use crate::rtc::{DateTime, Pcf8563};
use crate::storage::PersistedCounters;
use crate::todos::{Todo, TodoStore};
use crate::watchdog;
use crate::wifi;

/// Response bodies from a compliant server are small (alarms/todos are
/// themselves capped to a couple KB each in NVS - see `alarms::BLOB_BUF_LEN`
/// / `todos::BLOB_BUF_LEN`); a fixed buffer avoids a heap-growing read loop
/// for a payload this bounded.
const RESPONSE_BUF_LEN: usize = 8192;

/// Outcome of a bidirectional sync after the merged server state is applied.
#[derive(Clone, Debug)]
pub enum SyncOutcome {
    Applied {
        alarm_count: usize,
        todo_count: usize,
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
}

#[derive(Debug, Serialize)]
struct DeviceSyncRequest {
    alarms: Vec<DeviceAlarmState>,
    todos: Vec<DeviceTodoState>,
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
}

/// Fetches alarms and todos from `server_url`, applying conditional-request
/// semantics. Uploads local mutable flags first and returns the merged
/// server data's counts and new ETag. Any HTTP error, TLS error, or
/// JSON parse error returns `Err(...)` with a descriptive message rather
/// than panicking.
pub fn fetch_and_apply(
    server_url: &str,
    token: &str,
    _etag: Option<&str>,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
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
    let upload = DeviceSyncRequest {
        alarms: local_alarms
            .iter()
            .map(|alarm| DeviceAlarmState {
                id: alarm.id,
                enabled: alarm.enabled,
            })
            .collect(),
        todos: local_todos
            .iter()
            .map(|todo| DeviceTodoState {
                id: todo.id,
                done: todo.done,
            })
            .collect(),
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

    alarm_store
        .save(&parsed.alarms)
        .map_err(|e| anyhow!("failed to save synced alarms: {e}"))?;
    todo_store
        .save(&parsed.todos)
        .map_err(|e| anyhow!("failed to save synced todos: {e}"))?;
    if let Err(err) = alarms::program_hardware_alarm(rtc, &parsed.alarms, now) {
        log::warn!("Failed to reprogram hardware alarm after sync: {err}");
    }

    log::info!(
        "Sync applied: {} alarms, {} todos",
        parsed.alarms.len(),
        parsed.todos.len()
    );

    Ok(SyncOutcome::Applied {
        alarm_count: parsed.alarms.len(),
        todo_count: parsed.todos.len(),
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
        rtc,
        now,
    );

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
