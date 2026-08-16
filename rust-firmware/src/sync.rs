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
use crate::todos::{Todo, TodoStore};
use crate::watchdog;

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
