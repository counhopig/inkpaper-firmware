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
use serde::Deserialize;

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

/// Outcome of a sync operation: either new data was applied with counts, or
/// the server had no changes (HTTP 304).
#[derive(Clone, Debug)]
pub enum SyncOutcome {
    Applied {
        alarm_count: usize,
        todo_count: usize,
        etag: Option<String>,
    },
    NotModified,
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

/// Fetches alarms and todos from `server_url`, applying conditional-request
/// semantics via `If-None-Match` with the cached `etag` if present. Returns
/// either the fetched data's counts and a new ETag (if any), or
/// `NotModified` if the server sent HTTP 304. Any HTTP error, TLS error, or
/// JSON parse error returns `Err(...)` with a descriptive message rather
/// than panicking.
pub fn fetch_and_apply(
    server_url: &str,
    token: &str,
    etag: Option<&str>,
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

    let mut headers: Vec<(&str, &str)> = vec![("accept", "application/json")];
    let auth_header;
    if !token.is_empty() {
        auth_header = format!("Bearer {token}");
        headers.push(("authorization", &auth_header));
    }
    if let Some(etag) = etag {
        headers.push(("if-none-match", etag));
    }

    let request = client
        .request(Method::Get, server_url, &headers)
        .map_err(|e| anyhow!("GET {server_url} failed to start: {e}"))?;
    let mut response = request
        .submit()
        .map_err(|e| anyhow!("GET {server_url} failed: {e}"))?;

    let status = response.status();
    watchdog::feed();
    if status == 304 {
        log::info!("Sync: server reports no changes (304)");
        return Ok(SyncOutcome::NotModified);
    }
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

    if wifi_mgr.used() {
        // A second in-process Wi-Fi connect crashes even with raw
        // `esp_wifi_connect()`/`esp_wifi_disconnect()` FFI calls, bypassing
        // `EspWifi`'s own connect/status-tracking entirely (confirmed by
        // deliberately testing a real second connect here: identical crash,
        // same PC address, with or without the wrapper) - so this isn't an
        // esp-idf-svc-specific bug, it's lower-level than that. Restart
        // cleanly instead of attempting it; see `WifiManager`'s doc comment
        // for the full investigation and why `restart_for_fresh_wifi_session`
        // goes through deep sleep rather than `esp_restart()`.
        wifi::restart_for_fresh_wifi_session();
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

    outcome
}
