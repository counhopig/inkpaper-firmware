//! Menu/Calendar/Alarms/Todos screens, entered from the Home screen's ENTER
//! short-press (see `main.rs`). Each screen is a self-contained blocking
//! function, following the convention established in `provision.rs`.

use crate::alarms::{self, AlarmStore, Repeat, StoredAlarm};
use crate::board::Note4Board;
use crate::provision;
use crate::rtc::{is_leap, DateTime};
use crate::storage::{DeviceConfig, PersistedCounters};
use crate::sync;
use crate::todos::{self, Todo, TodoStore};
use crate::ui::{
    enter_text, footer, header, pick_from_list, pick_number, poll_nav, show_message, tick, Nav,
};
use crate::wifi::WifiManager;

/// Entry point from Home's ENTER short-press: shows the top-level menu and
/// recurses into whichever screen the user picks, returning once they back
/// all the way out to Home. Always leaves the caller (Home) to redraw its
/// own full screen afterwards - none of these screens know how to render
/// the home screen themselves.
pub fn open_menu(
    board: &mut Note4Board,
    counters: &PersistedCounters,
    wifi_mgr: &mut WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    now: Option<&DateTime>,
    ble_control: &mut Option<crate::ble_control::BleControl>,
) {
    let items = [
        "CALENDAR".to_string(),
        "ALARMS".to_string(),
        "TODOS".to_string(),
        "SERVER SETUP".to_string(),
        "SYNC NOW".to_string(),
        "WIFI SETUP".to_string(),
        "BLE PAIRING".to_string(),
    ];
    loop {
        let Some(index) =
            pick_from_list(board, "MENU", &items, "UP/DOWN=MOVE ENTER=PICK HOLD=BACK")
        else {
            return;
        };
        match index {
            0 => calendar_screen(board, now),
            1 => alarms_screen(board, alarm_store, now),
            2 => todos_screen(board, todo_store),
            3 => server_setup_screen(board, counters),
            4 => sync_now_screen(board, counters, wifi_mgr, alarm_store, todo_store, now),
            5 => provision::run(board, counters, wifi_mgr),
            6 => ble_pairing_screen(board, ble_control),
            _ => {}
        }
    }
}

fn calendar_screen(board: &mut Note4Board, now: Option<&DateTime>) {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    header(canvas, "CALENDAR");
    match now {
        Some(dt) => draw_month_grid(canvas, dt),
        None => {
            canvas.draw_text_prop(8, 40, 1, "NO CLOCK AVAILABLE");
        }
    }
    footer(canvas, "HOLD=BACK");
    let _ = board.display.refresh_full();
    loop {
        if matches!(poll_nav(board), Nav::Cancel) {
            return;
        }
        tick();
    }
}

/// Read-only current-month grid, today highlighted. No month navigation in
/// v1 - the device always shows "now".
fn draw_month_grid(canvas: &mut crate::canvas::Canvas, dt: &DateTime) {
    let title = format!("{:04}-{:02}", dt.year, dt.month);
    canvas.draw_text_prop(8, 32, 2, &title);

    const LABELS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
    const COL_WIDTH: usize = 54;
    const ROW_HEIGHT: usize = 28;
    const ORIGIN_X: usize = 8;
    const ORIGIN_Y: usize = 64;

    for (i, label) in LABELS.iter().enumerate() {
        canvas.draw_text_prop(ORIGIN_X + i * COL_WIDTH, ORIGIN_Y, 1, label);
    }

    let days_in_month = days_in_month(dt.year, dt.month);
    let mut col = weekday_of(dt.year, dt.month, 1) as usize;
    let mut row = 1usize;
    for day in 1..=days_in_month {
        let x = ORIGIN_X + col * COL_WIDTH;
        let y = ORIGIN_Y + row * ROW_HEIGHT;
        let text = day.to_string();
        if day == dt.day {
            canvas.fill_rect(x.saturating_sub(2), y.saturating_sub(2), 40, 22, true);
            canvas.draw_text_prop_white(x, y, 1, &text);
        } else {
            canvas.draw_text_prop(x, y, 1, &text);
        }
        col += 1;
        if col > 6 {
            col = 0;
            row += 1;
        }
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    const DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 && is_leap(year as i64) {
        29
    } else {
        DAYS[(month - 1) as usize]
    }
}

/// Sakamoto's algorithm; 0=Sunday, matching the `LABELS` order above. This
/// is independent of `DateTime::weekday`'s own (unrelated) convention -
/// nothing here reads that field.
fn weekday_of(year: u16, month: u8, day: u8) -> u8 {
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year as i64;
    if month < 3 {
        y -= 1;
    }
    ((y + y / 4 - y / 100 + y / 400 + T[(month - 1) as usize] + day as i64) % 7) as u8
}

fn alarms_screen(board: &mut Note4Board, store: &AlarmStore, now: Option<&DateTime>) {
    loop {
        let mut list = match store.load() {
            Ok(list) => list,
            Err(err) => {
                log::warn!("Failed to load alarms: {err}");
                Vec::new()
            }
        };
        let mut items: Vec<String> = list.iter().map(format_alarm_row).collect();
        items.push("+ ADD ALARM".to_string());

        let Some(index) = pick_from_list(board, "ALARMS", &items, "ENTER=TOGGLE HOLD=BACK") else {
            return;
        };

        if index == list.len() {
            let Some(new_alarm) = add_alarm_screen(board, &list) else {
                continue;
            };
            list.push(new_alarm);
        } else {
            list[index].enabled = !list[index].enabled;
        }

        if let Err(err) = store.save(&list) {
            log::warn!("Failed to save alarms: {err}");
        }
        if let Some(rtc_now) = now {
            if let Err(err) = alarms::program_hardware_alarm(&mut board.rtc, &list, rtc_now) {
                log::warn!("Failed to reprogram hardware alarm: {err}");
            }
        }
    }
}

fn format_alarm_row(alarm: &StoredAlarm) -> String {
    let mark = if alarm.enabled { "[X]" } else { "[ ]" };
    let when = match alarm.repeat {
        Repeat::Daily => format!("{:02}:{:02} DAILY", alarm.hour, alarm.minute),
        Repeat::Once { month, day, .. } => {
            format!(
                "{:02}:{:02} {:02}/{:02}",
                alarm.hour, alarm.minute, month, day
            )
        }
    };
    format!("{mark} {when}")
}

/// Two-stage hour/minute stepper for a new daily alarm - editing repeat
/// mode or a specific one-shot date is left to the PC tool/server sync
/// path (`docs/calendar-alarm-todo-plan.md` Phase 3), not this on-device
/// screen.
fn add_alarm_screen(board: &mut Note4Board, existing: &[StoredAlarm]) -> Option<StoredAlarm> {
    let hour = pick_number(board, "NEW ALARM - HOUR", 0, 23)?;
    let minute = pick_number(board, "NEW ALARM - MINUTE", 0, 59)?;
    Some(StoredAlarm {
        id: alarms::next_id(existing),
        hour,
        minute,
        repeat: Repeat::Daily,
        enabled: true,
        label: String::new(),
    })
}

fn todos_screen(board: &mut Note4Board, store: &TodoStore) {
    loop {
        let mut list = match store.load() {
            Ok(list) => list,
            Err(err) => {
                log::warn!("Failed to load todos: {err}");
                Vec::new()
            }
        };
        let mut items: Vec<String> = list
            .iter()
            .map(|t| {
                let mark = if t.done { "[X]" } else { "[ ]" };
                format!("{mark} {}", t.text)
            })
            .collect();
        items.push("+ ADD TODO".to_string());

        let Some(index) = pick_from_list(board, "TODOS", &items, "ENTER=TOGGLE HOLD=BACK") else {
            return;
        };

        if index == list.len() {
            let Some(text) = enter_text(board, "NEW TODO", None, 40) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            list.push(Todo {
                id: todos::next_id(&list),
                text,
                done: false,
            });
        } else {
            list[index].done = !list[index].done;
        }

        if let Err(err) = store.save(&list) {
            log::warn!("Failed to save todos: {err}");
        }
    }
}

/// Pending (not-done) todo count, for the Home screen summary line.
pub fn pending_todo_count(store: &TodoStore) -> usize {
    match store.load() {
        Ok(list) => list.iter().filter(|t| !t.done).count(),
        Err(err) => {
            log::warn!("Failed to load todos for pending count: {err}");
            0
        }
    }
}

/// Server configuration screen: prompts for server URL and auth token via
/// two-stage text entry (same pattern as add_alarm_screen's hour/minute),
/// then saves via PersistedCounters::save_device_config.
fn server_setup_screen(board: &mut Note4Board, counters: &PersistedCounters) {
    let server_url = enter_text(board, "SERVER URL", None, 128);
    let Some(server_url) = server_url else {
        return;
    };
    if server_url.is_empty() {
        return;
    }

    let auth_token = enter_text(board, "AUTH TOKEN", None, 128);
    let Some(auth_token) = auth_token else {
        return;
    };
    if auth_token.is_empty() {
        return;
    }

    let cfg = DeviceConfig {
        server_url,
        auth_token,
    };
    match counters.save_device_config(&cfg) {
        Ok(()) => {
            show_message(
                board,
                "SAVED",
                &["Server config saved"],
                std::time::Duration::from_secs(2),
            );
        }
        Err(err) => {
            show_message(
                board,
                "ERROR",
                &["Failed to save config"],
                std::time::Duration::from_secs(2),
            );
            log::warn!("Failed to save server config: {err}");
        }
    }
}

/// Sync screen: connects Wi-Fi, fetches alarms and todos from the
/// configured server, applies them to the local stores, and displays the
/// result. All the actual work is `sync::sync_now` - shared with the
/// USB/BLE `SyncNow` command so the two paths can't drift (in particular,
/// both must connect Wi-Fi first: `main.rs` drops the boot-time connection
/// once NTP sync is done, so by the time a user reaches this screen there
/// usually isn't an active Wi-Fi connection to piggyback on).
fn sync_now_screen(
    board: &mut Note4Board,
    counters: &PersistedCounters,
    wifi_mgr: &mut WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    now: Option<&DateTime>,
) {
    let Some(now_dt) = now else {
        show_message(
            board,
            "NO CLOCK",
            &["Clock not available"],
            std::time::Duration::from_secs(2),
        );
        return;
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
            let msg = format!("Alarms: {} Todos: {}", alarm_count, todo_count);
            show_message(board, "SYNC OK", &[&msg], std::time::Duration::from_secs(2));
        }
        Ok(sync::SyncOutcome::NotModified) => {
            show_message(
                board,
                "NOT MODIFIED",
                &["Server has no changes"],
                std::time::Duration::from_secs(2),
            );
        }
        Err(err) => {
            let err_msg = err.to_string();
            // Truncate long error messages for display - by chars, not
            // bytes, so this can't panic mid-UTF-8-codepoint.
            let truncated = if err_msg.chars().count() > 40 {
                format!("{}...", err_msg.chars().take(37).collect::<String>())
            } else {
                err_msg
            };
            show_message(
                board,
                "SYNC FAILED",
                &[&truncated],
                std::time::Duration::from_secs(2),
            );
            log::warn!("Sync failed: {err}");
        }
    }
}

fn ble_pairing_screen(
    board: &mut Note4Board,
    ble_control: &mut Option<crate::ble_control::BleControl>,
) {
    // Start BLE advertising on entry to the pairing screen.
    match crate::ble_control::BleControl::start() {
        Ok(ble) => {
            *ble_control = Some(ble);
            log::info!("BLE pairing screen: started advertising");

            // Display the pairing screen with instructions.
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, "BLE PAIRING");
            canvas.draw_text_prop(8, 40, 1, "CONNECTING...");
            canvas.draw_text_prop(8, 60, 1, "Service UUID:");
            canvas.draw_text_prop(8, 72, 1, "d2c25e50-");
            canvas.draw_text_prop(8, 84, 1, "5e22-48d8...");
            footer(canvas, "HOLD=BACK");
            let _ = board.display.refresh_full();

            // Poll for cancel (HOLD), but don't process commands - they only
            // run from Home per the documented limitations.
            loop {
                if matches!(poll_nav(board), Nav::Cancel) {
                    break;
                }
                tick();
            }
        }
        Err(err) => {
            log::warn!("Failed to start BLE: {err}");
            show_message(
                board,
                "BLE ERROR",
                &[&err.to_string()],
                std::time::Duration::from_secs(2),
            );
        }
    }

    // Stop BLE advertising on exit from the pairing screen.
    *ble_control = None;
    log::info!("BLE pairing screen: stopped advertising, reclaimed RAM");
}

/// Next enabled alarm's "HH:MM" (daily) or "HH:MM MM/DD" (one-shot) label,
/// for the Home screen summary line.
pub fn next_alarm_label(store: &AlarmStore, now: &DateTime) -> Option<String> {
    let list = match store.load() {
        Ok(list) => list,
        Err(err) => {
            log::warn!("Failed to load alarms for next-alarm label: {err}");
            return None;
        }
    };
    alarms::next_due(&list, now).map(|alarm| match alarm.repeat {
        Repeat::Daily => format!("{:02}:{:02}", alarm.hour, alarm.minute),
        Repeat::Once { month, day, .. } => {
            format!(
                "{:02}:{:02} {:02}/{:02}",
                alarm.hour, alarm.minute, month, day
            )
        }
    })
}
