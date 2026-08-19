//! Menu/Calendar/Alarms/Todos screens, entered from the Home screen's
//! long UP/DOWN navigation drawer (see `main.rs`). Each screen is a
//! self-contained blocking function.

use crate::alarms::{self, AlarmStore, Repeat, StoredAlarm};
use crate::board::Note4Board;
use crate::canvas::Canvas;
use crate::display::Rect;
use crate::rtc::{is_leap, DateTime};
use crate::storage::PersistedCounters;
use crate::sync;
use crate::todos::{Importance, TodoStore};
use crate::ui::{
    draw_rows, footer, header, pick_from_list, pick_number, poll_nav, show_message, tick, Nav,
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
        "SYNC NOW".to_string(),
        "SYNC INTERVAL".to_string(),
        "BLE PAIRING".to_string(),
        "SLEEP".to_string(),
    ];
    loop {
        let Some(index) = pick_from_list(
            board,
            "SETTINGS",
            &items,
            "UP/DOWN MOVE   ENTER OK   HOLD ENTER BACK",
            0,
        ) else {
            return;
        };
        match index {
            0 => sync_now_screen(board, counters, wifi_mgr, alarm_store, todo_store, now),
            1 => sync_interval_screen(board, counters),
            2 => ble_pairing_screen(
                board,
                counters,
                wifi_mgr,
                alarm_store,
                todo_store,
                now,
                ble_control,
            ),
            3 => {
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

/// Pick the automatic sync interval from preset options (1/5/10/30/60
/// minutes). The device re-syncs with the configured server every this many
/// minutes while idle on Home (see `main.rs`'s `maybe_auto_sync`).
fn sync_interval_screen(board: &mut Note4Board, counters: &PersistedCounters) {
    const OPTIONS: [(&str, u16); 5] = [
        ("1 MIN", 1),
        ("5 MIN", 5),
        ("10 MIN", 10),
        ("30 MIN", 30),
        ("60 MIN", 60),
    ];
    let items: Vec<String> = OPTIONS.iter().map(|(label, _)| label.to_string()).collect();
    let Some(index) = pick_from_list(
        board,
        "SYNC INTERVAL",
        &items,
        "UP/DOWN MOVE   ENTER OK   HOLD ENTER BACK",
        0,
    ) else {
        return;
    };
    let (_, minutes) = OPTIONS[index];
    match counters.set_sync_interval_minutes(minutes) {
        Ok(()) => {
            log::info!("Sync interval set to {minutes} min");
            show_message(
                board,
                "SYNC INTERVAL",
                &[&format!("{minutes} MIN")],
                std::time::Duration::from_secs(1),
            );
        }
        Err(err) => log::warn!("Failed to save sync interval: {err}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Calendar,
    Alarms,
    Todos,
}

/// Opens the GO TO destination list, pre-selected on wherever you already
/// are (`current`) instead of always resetting to HOME - so pressing ENTER
/// with no further input just closes the drawer back onto the same page,
/// and the highlighted row itself doubles as the "you are here" indicator.
/// This used to hand-roll a partial-width overlay that "deliberately"
/// didn't clear the canvas so the current page stayed visible to its
/// right - in practice that just chopped whatever content started left of
/// x=224 (a list row's text, a card) off mid-word, which read as broken
/// rather than as useful context. A full-screen `pick_from_list`, the same
/// component every other list in the app uses, reads as a normal screen
/// instead.
fn pick_navigation(board: &mut Note4Board, current: Page) -> Option<usize> {
    let destinations = [
        "HOME".to_string(),
        "CALENDAR".to_string(),
        "ALARMS".to_string(),
        "TODOS".to_string(),
        "SETTINGS".to_string(),
    ];
    let current_index = match current {
        Page::Home => 0,
        Page::Calendar => 1,
        Page::Alarms => 2,
        Page::Todos => 3,
    };
    pick_from_list(
        board,
        "GO TO",
        &destinations,
        "UP/DOWN MOVE   ENTER OK   HOLD ENTER BACK",
        current_index,
    )
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
        let Some(selected) = pick_navigation(board, Page::Home) else {
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
#[allow(clippy::too_many_arguments)]
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
    // Calendar day cursor - starts on today, moves with UP/DOWN, ENTER
    // opens that day's week view. Only meaningful while `now` is known.
    let mut cal_selected_day: u8 = now.map(|dt| dt.day).unwrap_or(1);
    let mut needs_redraw = true;
    let mut first_draw = true;
    loop {
        if needs_redraw {
            match page {
                Page::Home => {
                    let next_alarm = now.and_then(|dt| next_alarm_label(alarm_store, dt));
                    let todo_summary = todo_summary(todo_store, now);
                    let wifi_configured = counters
                        .wifi_creds()
                        .map(|creds| creds.is_some())
                        .unwrap_or(false);
                    let battery_percent = board
                        .battery_millivolts()
                        .ok()
                        .map(crate::board::battery_percent_from_mv);
                    let charge = board.charge_snapshot();
                    board.display.render_home(
                        now,
                        next_alarm.as_ref().map(|label| label.time.as_str()),
                        next_alarm.as_ref().map(|label| label.repeat.as_str()),
                        next_alarm.as_ref().and_then(|label| label.date.as_deref()),
                        next_alarm.as_ref().map(|label| label.days_left),
                        todo_summary.pending,
                        todo_summary.due_today,
                        todo_summary.high_pending,
                        wifi_configured,
                        battery_percent,
                        charge,
                    );
                }
                Page::Calendar => {
                    let canvas = board.display.canvas_mut();
                    canvas.clear();
                    header(canvas, "CALENDAR");
                    if let Some(dt) = now {
                        let todos = todo_store.load().unwrap_or_default();
                        // Day markers for the visible month: whether a todo
                        // is due that day (repeat schedule, or its single
                        // due date), carrying importance so the marker can
                        // be sized by it. Alarms don't get a mark here - the
                        // month grid is a todo-due overview; ENTER on a day
                        // opens the week view for the specifics, and alarms
                        // already have their own page.
                        let mut marks = [DayMark::default(); 32];
                        let days_in_month = days_in_month(dt.year, dt.month);
                        for todo in todos.iter() {
                            for day in 1..=days_in_month {
                                let fires = match &todo.repeat {
                                    Some(r) => r.fires_on(
                                        dt.year,
                                        dt.month,
                                        day,
                                        weekday_of(dt.year, dt.month, day),
                                    ),
                                    None => todo.due_date.is_some_and(|d| {
                                        d.year == dt.year && d.month == dt.month && d.day == day
                                    }),
                                };
                                if fires {
                                    marks[day as usize].todo = Some(todo.importance);
                                }
                            }
                        }
                        cal_selected_day = cal_selected_day.min(days_in_month).max(1);
                        draw_month_grid(canvas, dt.year, dt.month, now, cal_selected_day, &marks);
                    }
                    footer(canvas, "UP/DOWN MOVE   ENTER WEEK VIEW   HOLD UP/DOWN SWITCH PAGE");
                }
                Page::Alarms => render_alarm_page(board, alarm_store, alarm_selected),
                Page::Todos => render_todo_page(board, todo_store, todo_selected, now),
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
                match pick_navigation(board, page) {
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
                if page == Page::Todos {
                    // Long ENTER cycles a todo's importance (Low -> Medium
                    // -> High) instead of leaving the page - long UP/DOWN
                    // opens the navigation drawer, which is the way out.
                    cycle_todo_importance(todo_store, todo_selected);
                    needs_redraw = true;
                } else if page == Page::Home {
                    return;
                } else {
                    page = Page::Home;
                    needs_redraw = true;
                }
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
                Page::Calendar => {
                    cal_selected_day = cal_selected_day.saturating_sub(1).max(1);
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
                Page::Calendar => {
                    if let Some(dt) = now {
                        let dim = days_in_month(dt.year, dt.month);
                        cal_selected_day = (cal_selected_day + 1).min(dim);
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
                    Page::Calendar => {
                        if let Some(dt) = now {
                            week_view(board, todo_store, dt.year, dt.month, cal_selected_day, now);
                        }
                    }
                }
                needs_redraw = true;
            }
            Nav::None => {}
        }
        tick();
    }
}

/// Per-day cell marker for the month grid: the importance of a todo due
/// that day, if any. Alarms don't get a month-grid mark - see the ENTER ->
/// week view flow below for schedule detail.
#[derive(Clone, Copy, Default)]
struct DayMark {
    todo: Option<Importance>,
}

/// Read-only current-month grid. No month navigation in v1 - the device
/// always shows "now". UP/DOWN moves `selected_day` (a linear cursor over
/// 1..=days_in_month, wrapping row/col); ENTER on it opens that day's week
/// view (`week_view`) - the grid itself only has room for a due/not-due
/// dot, so that's where "what exactly is due Wednesday" gets answered.
/// `today` gets a thin underline so it stays visible even when the cursor
/// (the box + accent bar) has moved off it.
fn draw_month_grid(
    canvas: &mut crate::canvas::Canvas,
    year: u16,
    month: u8,
    today: Option<&DateTime>,
    selected_day: u8,
    marks: &[DayMark; 32],
) {
    let title = format!("{:04} / {:02}", year, month);
    canvas.draw_text_prop(16, 38, 2, &title);

    const LABELS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
    const COL_WIDTH: usize = 53;
    // Rows must fit all 6 possible week rows, each with clearance below
    // the day number for its todo dot, within the 300px display.
    const ROW_HEIGHT: usize = 32;
    const ORIGIN_X: usize = 18;
    const ORIGIN_Y: usize = 75;
    // The todo dot sits below the day number's 16px glyph height, not
    // overlapping it.
    const MARKER_Y: usize = 18;

    for (i, label) in LABELS.iter().enumerate() {
        let x = ORIGIN_X + i * COL_WIDTH;
        canvas.draw_text_prop(x, ORIGIN_Y, 1, label);
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
        // The cursor gets the same box + left accent bar every selection
        // in the app uses (nav drawer, list rows) - "you are here, ENTER
        // acts on it".
        if day == selected_day {
            canvas.stroke_rect(x.saturating_sub(6), y.saturating_sub(4), 34, 30, 2);
            canvas.fill_rect(x.saturating_sub(6), y.saturating_sub(4), 4, 30, true);
        }
        canvas.draw_text_prop(x, y, 1, &text);
        if is_today {
            let w = Canvas::text_prop_width(&text, 1);
            canvas.fill_rect(x, y + 16, w, 1, true);
        }
        if let Some(importance) = marks[day as usize].todo {
            let size = if importance == Importance::High { 6 } else { 4 };
            canvas.fill_rect(x, y + MARKER_Y, size, size, true);
        }
        col += 1;
        if col > 6 {
            col = 0;
            row += 1;
        }
    }
}

const MONTH_NAMES: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Greedy word-wrap: packs whitespace-separated words into lines no wider
/// than `max_width` px at `scale`, measured with the real proportional
/// font (not a fixed char count) - a week-view day column is only ~44px
/// of usable width, too narrow for most todo text on one line. A single
/// word longer than `max_width` on its own (e.g. "groceries") gets hard
/// character-split across lines instead of overflowing into the next
/// column.
fn wrap_text(text: &str, max_width: usize, scale: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let mut remaining = word;
        loop {
            let candidate = if current.is_empty() {
                remaining.to_string()
            } else {
                format!("{current} {remaining}")
            };
            if Canvas::text_prop_width(&candidate, scale) <= max_width {
                current = candidate;
                break;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                continue;
            }
            let mut split = remaining.len();
            while split > 1 && Canvas::text_prop_width(&remaining[..split], scale) > max_width {
                split -= 1;
            }
            lines.push(remaining[..split].to_string());
            remaining = &remaining[split..];
            if remaining.is_empty() {
                break;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Opens a read-only week view for the week (Sun-Sat) containing
/// `year`/`month`/`day`: one column per day, matching the month grid's own
/// column rhythm - a week is 7 days side by side, not a 7-row list. Each
/// column carries its weekday/date and a word-wrapped stack of that day's
/// open todo text - the detail the month grid's single dot can't show.
/// `now` gets the same thin underline the month grid uses for today,
/// independent of which day (`day`, the cursor position when ENTER was
/// pressed) the view was opened for. Any button closes it - read-only,
/// there's nothing further to drill into.
fn week_view(
    board: &mut Note4Board,
    todo_store: &TodoStore,
    year: u16,
    month: u8,
    day: u8,
    now: Option<&DateTime>,
) {
    let todos = todo_store.load().unwrap_or_default();
    let start = alarms::days_since_epoch(year, month, day) - weekday_of(year, month, day) as i64;
    let (_, sm, sd) = alarms::date_from_days(start);
    let (_, em, ed) = alarms::date_from_days(start + 6);
    let title = if sm == em {
        format!("{} {}-{}", MONTH_NAMES[(sm - 1) as usize], sd, ed)
    } else {
        format!(
            "{} {} - {} {}",
            MONTH_NAMES[(sm - 1) as usize],
            sd,
            MONTH_NAMES[(em - 1) as usize],
            ed
        )
    };

    let canvas = board.display.canvas_mut();
    canvas.clear();
    header(canvas, &title);

    const ORIGIN_X: usize = 16;
    const COL_WIDTH: usize = 53;
    const WEEKDAY_Y: usize = 38;
    const DATE_Y: usize = 56;
    const LIST_TOP: usize = 90;
    const LINE_H: usize = 15;
    const BOTTOM: usize = 296;

    for i in 0..7usize {
        let (y, m, d) = alarms::date_from_days(start + i as i64);
        let weekday = weekday_of(y, m, d);
        let x = ORIGIN_X + i * COL_WIDTH;
        let is_today = now.is_some_and(|dt| dt.year == y && dt.month == m && dt.day == d);

        canvas.draw_text_prop(x, WEEKDAY_Y, 1, WEEKDAY_SHORT[weekday as usize]);
        let date_text = d.to_string();
        canvas.draw_text_prop(x, DATE_Y, 1, &date_text);
        if is_today {
            let w = Canvas::text_prop_width(&date_text, 1);
            canvas.fill_rect(x, DATE_Y + 16, w, 1, true);
        }
        if i > 0 {
            canvas.fill_rect(x - 4, WEEKDAY_Y, 1, BOTTOM - WEEKDAY_Y, true);
        }

        let due: Vec<&str> = todos
            .iter()
            .filter(|t| !t.done)
            .filter(|t| match &t.repeat {
                Some(r) => r.fires_on(y, m, d, weekday),
                None => t
                    .due_date
                    .is_some_and(|dd| dd.year == y && dd.month == m && dd.day == d),
            })
            .map(|t| t.text.as_str())
            .collect();

        let max_w = COL_WIDTH.saturating_sub(6);
        let mut y_cursor = LIST_TOP;
        'day: for text in due {
            for line in wrap_text(text, max_w, 1) {
                if y_cursor + LINE_H > BOTTOM {
                    break 'day;
                }
                canvas.draw_text_prop(x, y_cursor, 1, &line);
                y_cursor += LINE_H;
            }
        }
    }

    let _ = board.display.refresh_full();
    loop {
        match poll_nav(board) {
            Nav::None => {}
            _ => return,
        }
        tick();
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

/// `[X]`/`[ ]` + time + compact repeat summary, then the label (when set)
/// appended and truncated to a single row's width - the alarm page now
/// shows *when it repeats* and *what it's called*, not just the time.
fn format_alarm_row(alarm: &StoredAlarm) -> String {
    let mark = if alarm.enabled { "[X]" } else { "[ ]" };
    let when = match &alarm.repeat {
        Repeat::Daily => format!("{:02}:{:02} DAILY", alarm.hour, alarm.minute),
        Repeat::Weekly { days } => {
            let weekdays: Vec<&str> = days.iter().map(|d| WEEKDAY_SHORT[*d as usize]).collect();
            format!(
                "{:02}:{:02} {}",
                alarm.hour,
                alarm.minute,
                weekdays.join(",")
            )
        }
        Repeat::Monthly { days } => {
            let list: Vec<String> = days.iter().map(|d| d.to_string()).collect();
            format!(
                "{:02}:{:02} DAY {}",
                alarm.hour,
                alarm.minute,
                list.join(",")
            )
        }
        Repeat::Once { month, day, .. } => {
            format!(
                "{:02}:{:02} {:02}/{:02}",
                alarm.hour, alarm.minute, month, day
            )
        }
    };
    let mut row = format!("{mark} {when}");
    if !alarm.label.is_empty() {
        row.push(' ');
        row.push_str(&alarm.label);
    }
    row.chars().take(34).collect()
}

const WEEKDAY_SHORT: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

fn render_alarm_page(board: &mut Note4Board, store: &AlarmStore, selected: usize) {
    let mut items: Vec<String> = store
        .load()
        .unwrap_or_default()
        .iter()
        .map(format_alarm_row)
        .collect();
    items.push("+ ADD ALARM".to_string());
    let canvas = board.display.canvas_mut();
    draw_rows(canvas, "ALARMS", &items, selected);
    footer(canvas, "UP/DOWN MOVE   ENTER OK   HOLD UP/DOWN PAGE");
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

fn render_todo_page(
    board: &mut Note4Board,
    store: &TodoStore,
    selected: usize,
    now: Option<&DateTime>,
) {
    let items: Vec<String> = store
        .load()
        .unwrap_or_default()
        .iter()
        .map(|t| format_todo_row(t, now))
        .collect();
    let canvas = board.display.canvas_mut();
    draw_rows(canvas, "TODOS", &items, selected);
    footer(canvas, "ENTER DONE   HOLD ENTER IMPORTANCE");
}

/// Whether `todo` (repeating or one-off) is due on `now`'s date.
fn todo_due_today(todo: &crate::todos::Todo, now: Option<&DateTime>) -> bool {
    now.is_some_and(|dt| match &todo.repeat {
        Some(r) => r.fires_on(dt.year, dt.month, dt.day, dt.weekday),
        None => todo
            .due_date
            .is_some_and(|d| d.year == dt.year && d.month == dt.month && d.day == dt.day),
    })
}

/// `[X]`/`[ ]` plus a `!!`/`!` importance suffix (low gets none), then the
/// text; a trailing `- MM/DD`/`- DUE TODAY` marks the due date / repeat so
/// the open-items page also shows *when* something needs doing, not just
/// what.
fn format_todo_row(todo: &crate::todos::Todo, now: Option<&DateTime>) -> String {
    let mark = if todo.done { "[X]" } else { "[ ]" };
    let imp = match todo.importance {
        crate::todos::Importance::Low => "",
        crate::todos::Importance::Medium => "! ",
        crate::todos::Importance::High => "!! ",
    };
    let mut row = format!("{mark} {imp}{}", todo.text);
    if !todo.done {
        if todo_due_today(todo, now) {
            row.push_str(" - DUE TODAY");
        } else if let Some(due) = todo.due_date {
            row.push_str(&format!(" - {:02}/{:02}", due.month, due.day));
        } else if let Some(Repeat::Weekly { days }) = &todo.repeat {
            let weekdays: Vec<&str> = days.iter().map(|d| WEEKDAY_SHORT[*d as usize]).collect();
            row.push_str(" - ");
            row.push_str(&weekdays.join(","));
        }
    }
    row.chars().take(34).collect()
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

/// Long-ENTER action: cycle the selected todo's importance
/// Low -> Medium -> High -> Low and persist. The device can't edit text,
/// but importance is cheap to express with the three-button set.
fn cycle_todo_importance(store: &TodoStore, selected: usize) {
    let mut list = store.load().unwrap_or_default();
    let Some(todo) = list.get_mut(selected) else {
        return;
    };
    todo.importance = match todo.importance {
        crate::todos::Importance::Low => crate::todos::Importance::Medium,
        crate::todos::Importance::Medium => crate::todos::Importance::High,
        crate::todos::Importance::High => crate::todos::Importance::Low,
    };
    log::info!("Todo id={} importance now {:?}", todo.id, todo.importance);
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

/// Next enabled alarm's summary for the Home screen card: the time plus
/// enough schedule detail to say *when* without opening the alarm page -
/// repeat pattern, next firing date, and how many days out it is.
pub struct NextAlarmLabel {
    pub time: String,
    /// Next firing date as `MM/DD` (only when the schedule has a specific
    /// date to name - Weekly/Monthly/Once).
    pub date: Option<String>,
    /// Repeat summary: `DAILY`, `SU,WE,FR`, `DAY 1,15`, or `ONCE`.
    pub repeat: String,
    /// Whole days from today until the next firing (0 = today).
    pub days_left: i64,
}

pub fn next_alarm_label(store: &AlarmStore, now: &DateTime) -> Option<NextAlarmLabel> {
    let list = match store.load() {
        Ok(list) => list,
        Err(err) => {
            log::warn!("Failed to load alarms for next-alarm label: {err}");
            return None;
        }
    };
    alarms::next_due(&list, now).map(|alarm| {
        let time = format!("{:02}:{:02}", alarm.hour, alarm.minute);
        let (date, repeat, days_left) = match &alarm.repeat {
            Repeat::Daily => (None, "DAILY".to_string(), 0),
            Repeat::Weekly { days } => {
                let (year, month, day, _) = alarms::next_occurrence_date(&alarm.repeat, now);
                let summary = days
                    .iter()
                    .map(|d| WEEKDAY_SHORT[*d as usize])
                    .collect::<Vec<_>>()
                    .join(",");
                (
                    Some(format!("{:02}/{:02}", month, day)),
                    summary,
                    alarms::days_until(year, month, day, now),
                )
            }
            Repeat::Monthly { days } => {
                let (year, month, day, _) = alarms::next_occurrence_date(&alarm.repeat, now);
                let summary = days
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                (
                    Some(format!("{:02}/{:02}", month, day)),
                    format!("DAY {summary}"),
                    alarms::days_until(year, month, day, now),
                )
            }
            Repeat::Once { year, month, day } => (
                Some(format!("{:02}/{:02}", month, day)),
                "ONCE".to_string(),
                alarms::days_until(*year, *month, *day, now),
            ),
        };
        NextAlarmLabel {
            time,
            date,
            repeat,
            days_left,
        }
    })
}

/// Aggregated todo stats for the Home screen's OPEN TODOS card: how many
/// are still open, how many of those are due today, and how many of those
/// are high priority.
pub struct TodoSummary {
    pub pending: usize,
    pub due_today: usize,
    pub high_pending: usize,
}

pub fn todo_summary(store: &TodoStore, now: Option<&DateTime>) -> TodoSummary {
    let Ok(list) = store.load() else {
        return TodoSummary {
            pending: 0,
            due_today: 0,
            high_pending: 0,
        };
    };
    let mut summary = TodoSummary {
        pending: 0,
        due_today: 0,
        high_pending: 0,
    };
    for todo in &list {
        if todo.done {
            continue;
        }
        summary.pending += 1;
        if todo.importance == Importance::High {
            summary.high_pending += 1;
        }
        if todo_due_today(todo, now) {
            summary.due_today += 1;
        }
    }
    summary
}
