//! On-device Wi-Fi setup: pick an AP from a scan (no SSID typing - see the
//! module doc on `wifi::scan_networks` for why) and enter its password with
//! a UP/DOWN character wheel, then verify the connection before saving.
//!
//! Entered from the main loop by holding UP for `ENTER_HOLD`. Runs its own
//! blocking poll loop and returns once the user finishes or cancels, so the
//! main loop's counters/clock logic doesn't need to know about it.

use std::time::Duration;

use esp_idf_svc::wifi::AccessPointInfo;

use crate::board::Note4Board;
use crate::storage::{PersistedCounters, WifiCreds};
use crate::ui::{enter_text, footer, header, pick_from_list, poll_nav, show_message, tick, Nav};
use crate::wifi::WifiManager;

/// How long UP must be held from the main screen to enter setup. Wall-clock,
/// matching the DOWN-hold-for-deep-sleep gesture in `main.rs` (see that
/// constant's doc comment for why this isn't a poll-cycle count).
pub const ENTER_HOLD: Duration = Duration::from_secs(3);

/// Longest password this UI will build. WPA2-PSK tops out at 63 chars.
const MAX_PASSWORD_LEN: usize = 63;
/// Longest AP list shown at once; the screen has no scroll/paging in v1.
const MAX_LISTED_APS: usize = 10;

/// Blocks until the user picks an AP, or returns `None` if they cancel.
fn pick_access_point(board: &mut Note4Board, wifi_mgr: &mut WifiManager) -> Option<String> {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    header(canvas, "WIFI SETUP");
    canvas.draw_text(8, 40, 2, "SCANNING...");
    if let Err(err) = board.display.refresh_full() {
        log::warn!("Wi-Fi setup: scanning-screen refresh failed: {err}");
    }

    let aps: Vec<AccessPointInfo> = match wifi_mgr.scan_networks() {
        Ok(aps) => aps,
        Err(err) => {
            log::warn!("Wi-Fi setup: scan failed: {err}");
            Vec::new()
        }
    };
    let names: Vec<String> = aps
        .iter()
        .take(MAX_LISTED_APS)
        .map(|ap| ap.ssid.as_str().to_string())
        .collect();

    if names.is_empty() {
        let canvas = board.display.canvas_mut();
        canvas.clear();
        header(canvas, "WIFI SETUP");
        canvas.draw_text(8, 40, 2, "NO APS FOUND");
        footer(canvas, "HOLD UP OR DOWN TO GO BACK");
        let _ = board.display.refresh_full();
        loop {
            if matches!(poll_nav(board), Nav::Cancel) {
                return None;
            }
            tick();
        }
    }

    let index = pick_from_list(
        board,
        "WIFI SETUP - PICK AP",
        &names,
        "UP/DOWN=MOVE ENTER=PICK HOLD=BACK",
    )?;
    Some(names[index].clone())
}

/// Outcome of the password screen.
enum PasswordResult {
    Done(String),
    Cancel,
}

/// Blocks until the user finishes (or cancels) entering the password for
/// `ssid`, via the shared character-wheel text entry (see `ui::enter_text`).
fn enter_password(board: &mut Note4Board, ssid: &str) -> PasswordResult {
    match enter_text(board, "WIFI PASSWORD", Some(ssid), MAX_PASSWORD_LEN) {
        Some(password) => PasswordResult::Done(password),
        None => PasswordResult::Cancel,
    }
}

/// Runs the whole setup wizard: scan -> pick AP -> enter password -> verify
/// -> save. Always leaves the caller with a clean screen state (the caller
/// is expected to force a full refresh of its own UI afterwards).
pub fn run(board: &mut Note4Board, counters: &PersistedCounters, wifi_mgr: &mut WifiManager) {
    log::info!("Entering Wi-Fi setup wizard");

    // A second in-process Wi-Fi connect crashes (see `WifiManager`'s doc
    // comment). Unlike `sync::sync_now`, this wizard is interactive (the
    // user has to pick from a freshly scanned AP list), so there's no
    // clean way to auto-resume it after a reboot - just restart and let
    // the user hold UP again, which will then be the fresh session's safe
    // first connect.
    if wifi_mgr.used() {
        show_message(
            board,
            "RESTARTING",
            &[
                "Wi-Fi already used",
                "this session - hold UP",
                "again after restart",
            ],
            Duration::from_secs(3),
        );
        crate::wifi::restart_for_fresh_wifi_session();
    }

    let Some(ssid) = pick_access_point(board, wifi_mgr) else {
        log::info!("Wi-Fi setup: cancelled at AP picker");
        return;
    };

    loop {
        let password = match enter_password(board, &ssid) {
            PasswordResult::Done(pw) => pw,
            PasswordResult::Cancel => {
                log::info!("Wi-Fi setup: cancelled at password entry");
                return;
            }
        };

        show_message(
            board,
            "WIFI SETUP",
            &["CONNECTING TO", ssid.as_str()],
            Duration::from_millis(200),
        );

        let creds = WifiCreds {
            ssid: ssid.clone(),
            password,
        };
        match wifi_mgr.connect(&creds) {
            Ok(()) => {
                wifi_mgr.disconnect();
                match counters.save_wifi_creds(&creds) {
                    Ok(()) => {
                        log::info!("Wi-Fi setup: connected and saved credentials for '{ssid}'");
                        show_message(
                            board,
                            "WIFI SETUP",
                            &["CONNECTED", "CREDENTIALS SAVED"],
                            Duration::from_secs(2),
                        );
                    }
                    Err(err) => {
                        log::warn!("Wi-Fi setup: connected but NVS save failed: {err}");
                        show_message(
                            board,
                            "WIFI SETUP",
                            &["CONNECTED BUT", "SAVE TO NVS FAILED"],
                            Duration::from_secs(2),
                        );
                    }
                }
                return;
            }
            Err(err) => {
                log::warn!("Wi-Fi setup: connect failed: {err}");
                show_message(
                    board,
                    "WIFI SETUP",
                    &["CONNECT FAILED", "CHECK PASSWORD, RETRY"],
                    Duration::from_secs(2),
                );
                // Loop back into password entry for the same AP.
            }
        }
    }
}
