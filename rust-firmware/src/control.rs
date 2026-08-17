//! Shared command/reply protocol for USB and BLE control channels.
//!
//! **Wire format:** Commands and replies are framed by the transport layer
//! (e.g., `usb_console.rs`) with a sentinel prefix to distinguish them from
//! ordinary log output:
//! - Command frame: `>>IP {json}\n` (one JSON object per line)
//! - Reply frame: `<<IP {json}\n` (one JSON object per line)
//!
//! See `docs/control-protocol.md` for the complete specification.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::alarms::AlarmStore;
use crate::board::Note4Board;
use crate::rtc::DateTime;
use crate::storage::{DeviceConfig, PersistedCounters, WifiCreds};
use crate::sync;
use crate::todos::TodoStore;
use crate::wifi;

/// Incoming command from a USB/BLE client.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Configure Wi-Fi credentials. Will attempt to connect to verify before
    /// saving to NVS - only credentials we know work end up persisted.
    SetWifi { ssid: String, password: String },

    /// Configure the server URL and authentication token for syncing alarms
    /// and todos. Saves immediately without verification (URLs are hard to
    /// validate without attempting a network request).
    SetServer { url: String, token: String },

    /// Trigger an immediate sync with the configured server. Requires a
    /// live Wi-Fi connection and a valid system time; returns an error
    /// if either is unavailable.
    SyncNow,

    /// Query the device's current configuration and connectivity state.
    GetStatus,
}

/// Reply sent back to a USB/BLE client.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Reply {
    /// Command succeeded.
    Ok,

    /// Device status snapshot.
    Status {
        wifi_configured: bool,
        server_configured: bool,
        wifi_connected: bool,
    },

    /// Command failed.
    Error { message: String },
}

/// Parses a command from a JSON string. Returns `Err` with a descriptive
/// message if parsing fails (e.g., invalid JSON, missing required field,
/// unknown command).
pub fn parse_command(line: &str) -> Result<Command> {
    serde_json::from_str(line).map_err(|e| anyhow!("Failed to parse command: {e}"))
}

/// Renders a reply as JSON. Falls back to a hardcoded error JSON string if
/// serialization somehow fails (should be extremely rare, but we don't want
/// to panic here).
pub fn render_reply(reply: &Reply) -> String {
    match serde_json::to_string(reply) {
        Ok(s) => s,
        Err(_) => r#"{"status":"error","message":"Failed to serialize reply"}"#.to_string(),
    }
}

/// Executes a command and returns a reply. This is the single point where
/// both USB (Phase 4) and BLE (Phase 5) control channels will call into,
/// so it must remain transport-agnostic. It accesses the board (for
/// RTC and Wi-Fi state), the stores (for syncing), and the time.
pub fn dispatch(
    cmd: Command,
    board: &mut Note4Board,
    counters: &PersistedCounters,
    wifi_mgr: &mut wifi::WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    now: Option<&DateTime>,
) -> Reply {
    match cmd {
        Command::SetWifi { ssid, password } => {
            let creds = WifiCreds { ssid, password };

            // A second in-process Wi-Fi connect crashes (see
            // `WifiManager`'s doc comment). Unlike `sync::sync_now`, there's
            // no clean way to verify-then-reply-then-restart here (the
            // restart happens before any reply could reach the client), so
            // this path skips the usual verify-before-save step and just
            // saves the credentials directly - the next boot's normal
            // `needs_wifi_sync` connect attempt effectively verifies them
            // instead. Documented in docs/control-protocol.md.
            if wifi_mgr.used() {
                return match counters.save_wifi_creds(&creds) {
                    Ok(()) => {
                        log::warn!(
                            "USB control: Wi-Fi already used this session; saved '{}' unverified and restarting",
                            creds.ssid
                        );
                        wifi::restart_for_fresh_wifi_session();
                    }
                    Err(err) => Reply::Error {
                        message: format!("Failed to save credentials: {err}"),
                    },
                };
            }

            // Attempt to connect and verify the credentials work before saving.
            // Only credentials we know are valid end up in NVS; if the
            // connection fails, return an error without persisting.
            match wifi_mgr.connect(&creds) {
                Ok(()) => {
                    // Connection succeeded; disconnect (we're not keeping a
                    // persistent connection) and save to NVS.
                    wifi_mgr.disconnect();
                    match counters.save_wifi_creds(&creds) {
                        Ok(()) => {
                            log::info!("USB control: Wi-Fi credentials saved for '{}'", creds.ssid);
                            Reply::Ok
                        }
                        Err(err) => {
                            log::warn!("USB control: Wi-Fi credentials verified but NVS save failed: {err}");
                            Reply::Error {
                                message: format!("Failed to save credentials: {err}"),
                            }
                        }
                    }
                }
                Err(err) => {
                    log::warn!("USB control: Wi-Fi connection verification failed: {err}");
                    Reply::Error {
                        message: format!("Connection verification failed: {err}"),
                    }
                }
            }
        }

        Command::SetServer { url, token } => {
            // Server config is saved immediately without verification -
            // it's hard to validate a URL without a network request, and
            // we want this command to be fast.
            let cfg = DeviceConfig {
                server_url: url,
                auth_token: token,
            };
            match counters.save_device_config(&cfg) {
                Ok(()) => {
                    if let Err(err) = counters.clear_sync_etag() {
                        log::warn!(
                            "Server config saved but old sync ETag could not be cleared: {err}"
                        );
                    }
                    log::info!("USB control: server config saved");
                    Reply::Ok
                }
                Err(err) => {
                    log::warn!("USB control: server config save failed: {err}");
                    Reply::Error {
                        message: format!("Failed to save server config: {err}"),
                    }
                }
            }
        }

        Command::SyncNow => {
            // `sync::sync_now` connects Wi-Fi (main.rs drops the boot-time
            // connection once NTP sync is done, so one usually isn't
            // already up by the time this command arrives), then does the
            // fetch/apply/etag-save cycle - shared with the on-device
            // "SYNC NOW" menu screen so the two paths can't drift.
            let Some(now_dt) = now else {
                return Reply::Error {
                    message: "System time not available".to_string(),
                };
            };
            match sync::sync_now(
                counters,
                wifi_mgr,
                alarm_store,
                todo_store,
                &mut board.rtc,
                now_dt,
            ) {
                Ok(sync::SyncOutcome::Applied {
                    alarm_count,
                    todo_count,
                    ..
                }) => {
                    log::info!(
                        "USB control sync completed: {alarm_count} alarms, {todo_count} todos"
                    );
                    Reply::Ok
                }
                Err(err) => {
                    log::warn!("USB control sync failed: {err}");
                    Reply::Error {
                        message: format!("Sync failed: {err}"),
                    }
                }
            }
        }

        Command::GetStatus => {
            // Report whatever we can cheaply check without network activity.
            let wifi_configured = counters
                .wifi_creds()
                .map(|opt| opt.is_some())
                .unwrap_or(false);
            let server_configured = counters
                .device_config()
                .map(|opt| opt.is_some())
                .unwrap_or(false);

            // TODO: check live Wi-Fi connection state if readily available.
            // For now, we just report static config, not current connection status.
            let wifi_connected = false;

            Reply::Status {
                wifi_configured,
                server_configured,
                wifi_connected,
            }
        }
    }
}
