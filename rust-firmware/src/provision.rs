//! On-device Wi-Fi setup: pick an AP from a scan (no SSID typing - see the
//! module doc on `wifi::scan_networks` for why) and enter its password with
//! a UP/DOWN character wheel, then verify the connection before saving.
//!
//! Entered from the main loop by holding UP for `ENTER_HOLD_POLLS`. Runs its
//! own blocking poll loop and returns once the user finishes or cancels, so
//! the main loop's counters/clock logic doesn't need to know about it.

use std::thread;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::wifi::AccessPointInfo;

use crate::board::Note4Board;
use crate::button::{ButtonEvent, POLL_INTERVAL_MS};
use crate::canvas::Canvas;
use crate::storage::{PersistedCounters, WifiCreds};
use crate::wifi;

/// Poll cycles (x20ms) UP must be held from the main screen to enter setup.
/// Matches the DOWN-hold-for-deep-sleep gesture already used in `main.rs`.
pub const ENTER_HOLD_POLLS: u32 = 150;

/// Longest password this UI will build. WPA2-PSK tops out at 63 chars.
const MAX_PASSWORD_LEN: usize = 63;
/// Longest AP list shown at once; the screen has no scroll/paging in v1.
const MAX_LISTED_APS: usize = 10;

const CONTROL_ITEMS: usize = 4; // Done, Backspace, Cancel, ToggleCase
const CHARSET: &[char] = &[
    ' ', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '-',
    ':', '/',
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum WheelItem {
    Done,
    Backspace,
    Cancel,
    ToggleCase,
    Char(char),
}

fn wheel_len() -> usize {
    CONTROL_ITEMS + CHARSET.len()
}

fn wheel_item(index: usize) -> WheelItem {
    if index >= CONTROL_ITEMS {
        return WheelItem::Char(CHARSET[index - CONTROL_ITEMS]);
    }
    match index {
        0 => WheelItem::Done,
        1 => WheelItem::Backspace,
        2 => WheelItem::Cancel,
        _ => WheelItem::ToggleCase,
    }
}

/// Outcome of one blocking poll wait: which control fired, if any.
enum Nav {
    None,
    Up,
    Down,
    Enter,
    /// Long-press on either UP or DOWN: the escape hatch out of any screen.
    Cancel,
}

fn poll_nav(board: &mut Note4Board) -> Nav {
    if let Some(event) = board.key_enter.poll() {
        if event == ButtonEvent::Pressed {
            return Nav::Enter;
        }
    }
    if let Some(event) = board.key_up.poll() {
        match event {
            ButtonEvent::Pressed => return Nav::Up,
            ButtonEvent::LongPressed => return Nav::Cancel,
            ButtonEvent::Released => {}
        }
    }
    if let Some(event) = board.key_down.poll() {
        match event {
            ButtonEvent::Pressed => return Nav::Down,
            ButtonEvent::LongPressed => return Nav::Cancel,
            ButtonEvent::Released => {}
        }
    }
    Nav::None
}

fn tick() {
    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
}

fn header(canvas: &mut Canvas, title: &str) {
    canvas.draw_text(8, 4, 2, title);
    canvas.fill_rect(8, 24, 384, 2, true);
}

fn footer(canvas: &mut Canvas, hint: &str) {
    canvas.draw_text(8, 284, 1, hint);
}

/// Blocks until the user picks an AP, or returns `None` if they cancel.
fn pick_access_point(board: &mut Note4Board, sysloop: &EspSystemEventLoop) -> Option<String> {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    header(canvas, "WIFI SETUP");
    canvas.draw_text(8, 40, 2, "SCANNING...");
    if let Err(err) = board.display.refresh_full() {
        log::warn!("Wi-Fi setup: scanning-screen refresh failed: {err}");
    }

    let aps: Vec<AccessPointInfo> = match wifi::scan_networks(sysloop) {
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

    let mut selected = 0usize;
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, "WIFI SETUP - PICK AP");
            let mut y = 32usize;
            for (i, name) in names.iter().enumerate() {
                let marker = if i == selected { ">" } else { " " };
                canvas.draw_text(8, y, 2, &format!("{marker}{name}"));
                y += 18;
            }
            footer(canvas, "UP/DOWN=MOVE ENTER=PICK HOLD=BACK");
            let _ = board.display.refresh_full();
            needs_redraw = false;
        }

        match poll_nav(board) {
            Nav::Up => {
                selected = if selected == 0 {
                    names.len() - 1
                } else {
                    selected - 1
                };
                needs_redraw = true;
            }
            Nav::Down => {
                selected = (selected + 1) % names.len();
                needs_redraw = true;
            }
            Nav::Enter => return Some(names[selected].clone()),
            Nav::Cancel => return None,
            Nav::None => {}
        }
        tick();
    }
}

/// Outcome of the password screen.
enum PasswordResult {
    Done(String),
    Cancel,
}

/// Blocks until the user finishes (or cancels) entering the password for
/// `ssid`. `Done` is one of the wheel items, selected the same way as any
/// character, rather than a separate button gesture - keeps every screen in
/// this wizard driven by the same three-button vocabulary (UP/DOWN=cycle,
/// ENTER=pick, hold=back).
fn enter_password(board: &mut Note4Board, ssid: &str) -> PasswordResult {
    let mut buffer = String::new();
    let mut wheel_index = 0usize;
    let mut lowercase = false;
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, "WIFI PASSWORD");
            canvas.draw_text(8, 32, 1, ssid);
            canvas.draw_text(8, 48, 2, &format!("PASS: {buffer}"));
            let case_label = if lowercase { "lower" } else { "UPPER" };
            let candidate_label = match wheel_item(wheel_index) {
                WheelItem::Done => "[ DONE ]".to_string(),
                WheelItem::Backspace => "[ DEL ]".to_string(),
                WheelItem::Cancel => "[ CANCEL ]".to_string(),
                WheelItem::ToggleCase => format!("[ CASE: {case_label} ]"),
                WheelItem::Char(c) => format!("< {c} >"),
            };
            canvas.draw_text(8, 90, 3, &candidate_label);
            footer(canvas, "UP/DOWN=CYCLE ENTER=PICK HOLD=BACK");
            let _ = board.display.refresh_full();
            needs_redraw = false;
        }

        match poll_nav(board) {
            Nav::Up => {
                wheel_index = (wheel_index + 1) % wheel_len();
                needs_redraw = true;
            }
            Nav::Down => {
                wheel_index = if wheel_index == 0 {
                    wheel_len() - 1
                } else {
                    wheel_index - 1
                };
                needs_redraw = true;
            }
            Nav::Enter => {
                match wheel_item(wheel_index) {
                    WheelItem::Done => return PasswordResult::Done(buffer),
                    WheelItem::Backspace => {
                        buffer.pop();
                    }
                    WheelItem::Cancel => return PasswordResult::Cancel,
                    WheelItem::ToggleCase => lowercase = !lowercase,
                    WheelItem::Char(c) => {
                        if buffer.chars().count() < MAX_PASSWORD_LEN {
                            let c = if lowercase {
                                c.to_ascii_lowercase()
                            } else {
                                c
                            };
                            buffer.push(c);
                        }
                    }
                }
                wheel_index = 0;
                needs_redraw = true;
            }
            Nav::Cancel => return PasswordResult::Cancel,
            Nav::None => {}
        }
        tick();
    }
}

fn show_message(board: &mut Note4Board, title: &str, lines: &[&str], pause: Duration) {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    header(canvas, title);
    let mut y = 40usize;
    for line in lines {
        canvas.draw_text(8, y, 2, line);
        y += 20;
    }
    let _ = board.display.refresh_full();
    thread::sleep(pause);
}

/// Runs the whole setup wizard: scan -> pick AP -> enter password -> verify
/// -> save. Always leaves the caller with a clean screen state (the caller
/// is expected to force a full refresh of its own UI afterwards).
pub fn run(board: &mut Note4Board, counters: &PersistedCounters, sysloop: &EspSystemEventLoop) {
    log::info!("Entering Wi-Fi setup wizard");
    let Some(ssid) = pick_access_point(board, sysloop) else {
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
        match wifi::WifiSta::connect(&creds, sysloop) {
            Ok(sta) => {
                drop(sta);
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
