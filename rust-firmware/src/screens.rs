//! Menu/Calendar/Alarms/Todos screens, entered from the Home screen's
//! long UP/DOWN navigation drawer (see `main.rs`). Each screen is a
//! self-contained blocking function.

use crate::alarms::{self, AlarmStore, Repeat, StoredAlarm};
use crate::board::Note4Board;
use crate::display::Rect;
use crate::rtc::{is_leap, DateTime};
use crate::storage::PersistedCounters;
use crate::sync;
use crate::todos::TodoStore;
use crate::ui::{footer, header, pick_from_list, pick_number, poll_nav, show_message, tick, Nav};
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
        "SYNC NOW".to_string(),
        "BLE PAIRING".to_string(),
        "SLEEP".to_string(),
    ];
    loop {
        let Some(index) = pick_from_list(
            board,
            "SETTINGS",
            &items,
            "UP/DOWN MOVE   ENTER OK   HOLD ENTER BACK",
        ) else {
            return;
        };
        match index {
            0 => sync_now_screen(board, counters, wifi_mgr, alarm_store, todo_store, now),
            1 => ble_pairing_screen(
                board,
                counters,
                wifi_mgr,
                alarm_store,
                todo_store,
                now,
                ble_control,
            ),
            2 => {
                show_message(
                    board,
                    "SLEEP",
                    &["GOING TO SLEEP"],
                    std::time::Duration::from_millis(500),
                );
                crate::power::enter_deep_sleep_with_wakeups(None);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Calendar,
    Alarms,
    Todos,
}

fn pick_navigation(board: &mut Note4Board) -> Option<usize> {
    let destinations = [
        "HOME".to_string(),
        "CALENDAR".to_string(),
        "ALARMS".to_string(),
        "TODOS".to_string(),
        "SETTINGS".to_string(),
    ];
    let mut selected = 0usize;
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();

            // Left navigation drawer. Deliberately do not clear the canvas:
            // the current page remains visible to the right of the drawer.
            canvas.fill_rect(0, 0, 224, 300, false);
            canvas.fill_rect(216, 0, 2, 300, true);
            canvas.draw_text_prop(20, 20, 1, "INKPAPER");
            canvas.draw_text_prop(20, 40, 2, "GO TO");
            canvas.fill_rect(20, 75, 178, 1, true);

            let mut y = 88usize;
            for (index, destination) in destinations.iter().enumerate() {
                if index == selected {
                    canvas.stroke_rect(12, y, 194, 34, 2);
                    canvas.fill_rect(12, y, 5, 34, true);
                    canvas.draw_text_prop(28, y + 8, 1, destination);
                } else {
                    canvas.fill_rect(12, y, 194, 34, false);
                    canvas.draw_text_prop(28, y + 8, 1, destination);
                }
                y += 38;
            }
            let _ = board.display.refresh_partial(Rect {
                x: 0,
                y: 0,
                width: 224,
                height: 300,
            });
            needs_redraw = false;
        }

        match poll_nav(board) {
            Nav::Up => {
                selected = if selected == 0 {
                    destinations.len() - 1
                } else {
                    selected - 1
                };
                needs_redraw = true;
            }
            Nav::Down => {
                selected = (selected + 1) % destinations.len();
                needs_redraw = true;
            }
            Nav::Enter => return Some(selected),
            Nav::Cancel => return None,
            Nav::PageUp | Nav::PageDown | Nav::None => {}
        }
        tick();
    }
}

/// Opens the global navigation directory. Both long UP and long DOWN enter
/// this directory; short UP/DOWN selects a destination and ENTER opens it.
pub fn open_navigation(
    board: &mut Note4Board,
    counters: &PersistedCounters,
    wifi_mgr: &mut WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    now: Option<&DateTime>,
    ble_control: &mut Option<crate::ble_control::BleControl>,
) {
    loop {
        let Some(selected) = pick_navigation(board) else {
            return;
        };
        match selected {
            0 => return,
            1..=3 => {
                let page = match selected {
                    1 => Page::Calendar,
                    2 => Page::Alarms,
                    _ => Page::Todos,
                };
                browse_page(
                    board,
                    page,
                    counters,
                    wifi_mgr,
                    alarm_store,
                    todo_store,
                    now,
                    ble_control,
                );
                return;
            }
            4 => open_menu(
                board,
                counters,
                wifi_mgr,
                alarm_store,
                todo_store,
                now,
                ble_control,
            ),
            _ => {}
        }
    }
}

/// Runs one peer content page. Long UP/DOWN opens the navigation overlay;
/// cancelling that overlay restores this page.
fn browse_page(
    board: &mut Note4Board,
    mut page: Page,
    counters: &PersistedCounters,
    wifi_mgr: &mut WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    now: Option<&DateTime>,
    ble_control: &mut Option<crate::ble_control::BleControl>,
) {
    let mut alarm_selected = 0usize;
    let mut todo_selected = 0usize;
    let mut needs_redraw = true;
    let mut first_draw = true;
    loop {
        if needs_redraw {
            match page {
                Page::Home => {
                    let next_alarm = now.and_then(|dt| next_alarm_label(alarm_store, dt));
                    board.display.render_home(
                        now,
                        next_alarm.as_deref(),
                        pending_todo_count(todo_store),
                    );
                }
                Page::Calendar => {
                    let canvas = board.display.canvas_mut();
                    canvas.clear();
                    header(canvas, "CALENDAR");
                    if let Some(dt) = now {
                        draw_month_grid(canvas, dt.year, dt.month, now);
                    }
                    footer(canvas, "HOLD UP/DOWN SWITCH PAGE");
                }
                Page::Alarms => render_alarm_page(board, alarm_store, alarm_selected),
                Page::Todos => render_todo_page(board, todo_store, todo_selected),
            }
            if first_draw {
                let _ = board.display.refresh_full();
                first_draw = false;
            } else {
                let _ = board.display.refresh_partial(Rect {
                    x: 0,
                    y: 0,
                    width: 400,
                    height: 300,
                });
            }
            needs_redraw = false;
        }

        match poll_nav(board) {
            Nav::PageUp | Nav::PageDown => {
                match pick_navigation(board) {
                    Some(0) => return,
                    Some(1) => page = Page::Calendar,
                    Some(2) => page = Page::Alarms,
                    Some(3) => page = Page::Todos,
                    Some(4) => open_menu(
                        board,
                        counters,
                        wifi_mgr,
                        alarm_store,
                        todo_store,
                        now,
                        ble_control,
                    ),
                    Some(_) | None => {}
                }
                needs_redraw = true;
            }
            Nav::Cancel => {
                if page == Page::Home {
                    return;
                }
                page = Page::Home;
                needs_redraw = true;
            }
            Nav::Up => match page {
                Page::Alarms => {
                    alarm_selected = alarm_selected.saturating_sub(1);
                    needs_redraw = true;
                }
                Page::Todos => {
                    todo_selected = todo_selected.saturating_sub(1);
                    needs_redraw = true;
                }
                _ => {}
            },
            Nav::Down => match page {
                Page::Alarms => {
                    let len = alarm_store.load().map(|v| v.len()).unwrap_or(0) + 1;
                    alarm_selected = (alarm_selected + 1).min(len - 1);
                    needs_redraw = true;
                }
                Page::Todos => {
                    let len = todo_store.load().map(|v| v.len()).unwrap_or(0);
                    if len > 0 {
                        todo_selected = (todo_selected + 1).min(len - 1);
                        needs_redraw = true;
                    }
                }
                _ => {}
            },
            Nav::Enter => {
                match page {
                    Page::Home => {}
                    Page::Alarms => activate_alarm_row(board, alarm_store, now, alarm_selected),
                    Page::Todos => activate_todo_row(todo_store, todo_selected),
                    Page::Calendar => {}
                }
                needs_redraw = true;
            }
            Nav::None => {}
        }
        tick();
    }
}

/// Read-only current-month grid, today highlighted. No month navigation in
/// v1 - the device always shows "now".
fn draw_month_grid(
    canvas: &mut crate::canvas::Canvas,
    year: u16,
    month: u8,
    today: Option<&DateTime>,
) {
    let title = format!("{:04} / {:02}", year, month);
    canvas.draw_text_prop(16, 38, 2, &title);

    const LABELS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
    const COL_WIDTH: usize = 52;
    const ROW_HEIGHT: usize = 27;
    const ORIGIN_X: usize = 18;
    const ORIGIN_Y: usize = 75;

    for (i, label) in LABELS.iter().enumerate() {
        let x = ORIGIN_X + i * COL_WIDTH;
        if i == 0 || i == 6 {
            canvas.stroke_rect(x.saturating_sub(4), ORIGIN_Y - 5, 34, 25, 1);
            canvas.draw_text_prop(x, ORIGIN_Y, 1, label);
        } else {
            canvas.draw_text_prop(x, ORIGIN_Y, 1, label);
        }
    }
    canvas.fill_rect(16, 99, 368, 1, true);

    let days_in_month = days_in_month(year, month);
    let mut col = weekday_of(year, month, 1) as usize;
    let mut row = 1usize;
    for day in 1..=days_in_month {
        let x = ORIGIN_X + col * COL_WIDTH;
        let y = ORIGIN_Y + row * ROW_HEIGHT;
        let text = day.to_string();
        let is_today = today
            .map(|dt| dt.year == year && dt.month == month && dt.day == day)
            .unwrap_or(false);
        if is_today {
            canvas.stroke_rect(x.saturating_sub(6), y.saturating_sub(5), 34, 25, 2);
            canvas.fill_rect(x.saturating_sub(6), y.saturating_sub(5), 4, 25, true);
            canvas.draw_text_prop(x, y, 1, &text);
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

fn render_rows(canvas: &mut crate::canvas::Canvas, title: &str, items: &[String], selected: usize) {
    canvas.clear();
    header(canvas, title);
    let selected = selected.min(items.len().saturating_sub(1));
    let first = selected.saturating_sub(crate::ui::MAX_LISTED_ITEMS - 1);
    let mut y = 39usize;
    for (index, item) in items
        .iter()
        .enumerate()
        .skip(first)
        .take(crate::ui::MAX_LISTED_ITEMS)
    {
        if index == selected {
            canvas.stroke_rect(16, y, 368, 29, 2);
            canvas.fill_rect(16, y, 5, 29, true);
            canvas.draw_text_prop(30, y + 6, 1, ">");
            canvas.draw_text_prop(50, y + 6, 1, item);
        } else {
            canvas.draw_text_prop(24, y + 6, 1, item);
        }
        y += 31;
    }
    footer(canvas, "UP/DOWN MOVE   ENTER OK   HOLD UP/DOWN PAGE");
}

fn render_alarm_page(board: &mut Note4Board, store: &AlarmStore, selected: usize) {
    let mut items: Vec<String> = store
        .load()
        .unwrap_or_default()
        .iter()
        .map(format_alarm_row)
        .collect();
    items.push("+ ADD ALARM".to_string());
    render_rows(board.display.canvas_mut(), "ALARMS", &items, selected);
}

fn activate_alarm_row(
    board: &mut Note4Board,
    store: &AlarmStore,
    now: Option<&DateTime>,
    selected: usize,
) {
    let mut list = store.load().unwrap_or_default();
    if selected >= list.len() {
        if let Some(alarm) = add_alarm_screen(board, &list) {
            list.push(alarm);
        } else {
            return;
        }
    } else {
        list[selected].enabled = !list[selected].enabled;
    }
    if let Err(err) = store.save(&list) {
        log::warn!("Failed to save alarms: {err}");
    }
    if let Some(dt) = now {
        if let Err(err) = alarms::program_hardware_alarm(&mut board.rtc, &list, dt) {
            log::warn!("Failed to reprogram hardware alarm: {err}");
        }
    }
}

fn render_todo_page(board: &mut Note4Board, store: &TodoStore, selected: usize) {
    let items: Vec<String> = store
        .load()
        .unwrap_or_default()
        .iter()
        .map(|todo| {
            let mark = if todo.done { "[X]" } else { "[ ]" };
            format!("{mark} {}", todo.text)
        })
        .collect();
    render_rows(board.display.canvas_mut(), "TODOS", &items, selected);
}

fn activate_todo_row(store: &TodoStore, selected: usize) {
    let mut list = store.load().unwrap_or_default();
    let Some(todo) = list.get_mut(selected) else {
        return;
    };
    todo.done = !todo.done;
    if let Err(err) = store.save(&list) {
        log::warn!("Failed to save todos: {err}");
    }
}

/// Two-stage hour/minute stepper for a new daily alarm - editing repeat
/// mode or a specific one-shot date is left to the PC tool/server sync
/// path, not this on-device screen.
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
    counters: &PersistedCounters,
    wifi_mgr: &mut WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    now: Option<&DateTime>,
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
            footer(canvas, "HOLD ENTER BACK");
            let _ = board.display.refresh_full();

            // This screen owns the main thread while pairing is active, so it
            // must drain BLE commands here. Waiting for the outer Home loop
            // would deadlock the control session because leaving this screen
            // tears BLE down.
            let started_at = std::time::Instant::now();
            loop {
                if let Some(cmd) = ble_control.as_ref().and_then(|ble| ble.poll_command()) {
                    let reply = crate::control::dispatch(
                        cmd,
                        board,
                        counters,
                        wifi_mgr,
                        alarm_store,
                        todo_store,
                        now,
                    );
                    if let Some(ble) = ble_control.as_ref() {
                        ble.write_reply(&reply);
                    }
                }
                // This is a passive status page with no confirm action, so
                // either ENTER gesture exits. The timeout is a final escape
                // hatch if a button event is ever lost while the radio stack
                // is active.
                if matches!(poll_nav(board), Nav::Enter | Nav::Cancel)
                    || started_at.elapsed() >= std::time::Duration::from_secs(120)
                {
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
