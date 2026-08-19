//! Dependency-free full-page preview for the firmware's e-ink screens.
//!
//! Renders the Home, Calendar, Alarms and Todos screens with representative
//! data into `tmp/` as grayscale PNGs, reusing the *same* drawing code the
//! firmware runs: the preview parses `font8x16.rs`'s glyph tables and
//! `icons.rs`'s bitmaps straight out of the source, and the layout constants
//! here mirror `display.rs`/`screens.rs`/`ui.rs`.
//!
//! Run from the `inkpaper` repository root:
//!   rustc scripts/ui_preview.rs -o /tmp/ui-preview
//!   /tmp/ui-preview rust-firmware/src tmp

use std::{cell::RefCell, env, fs, io, path::Path};

const WIDTH: usize = 400;
const HEIGHT: usize = 300;
const SCALE: usize = 3;

// ============================== font =====================================

struct Font {
    widths: Vec<u8>,
    glyphs: Vec<[u16; 16]>,
}

impl Font {
    fn glyph(&self, character: char) -> (&[u16; 16], u8) {
        let code = character as u32;
        if !(0x20..=0x7E).contains(&code) {
            return (&self.glyphs[0], self.widths[0]);
        }
        let index = (code - 0x20) as usize;
        (&self.glyphs[index], self.widths[index])
    }
}

// The firmware exposes `Canvas::text_prop_width` as a free function with no
// canvas, so the preview mirrors that with a lazily-set global font.
thread_local! {
    static FONT: RefCell<Option<Font>> = const { RefCell::new(None) };
}

fn with_font<T>(f: impl FnOnce(&Font) -> T) -> T {
    FONT.with(|slot| {
        let slot = slot.borrow();
        let font = slot.as_ref().expect("font not loaded before drawing");
        f(font)
    })
}

fn block_after<'a>(source: &'a str, marker: &str) -> &'a str {
    source
        .split_once(marker)
        .unwrap_or_else(|| panic!("marker {} not found", marker))
        .1
        .split_once("];")
        .expect("unterminated array")
        .0
}

fn hex_tokens(s: &str) -> Vec<u32> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'0' && i + 1 < bytes.len() && bytes[i + 1] == b'x' {
            let mut j = i + 2;
            let mut value: u32 = 0;
            while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                value = value * 16 + (bytes[j] as char).to_digit(16).unwrap();
                j += 1;
            }
            out.push(value);
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Strips `// ...` line comments so they can't be mistaken for data - each
/// row of `GLYPHS` ends with a `// 0x..` comment naming its char code, which
/// `hex_tokens` would otherwise read as a spurious extra glyph value and
/// misalign every row after it.
fn strip_line_comments(s: &str) -> String {
    s.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

fn load_font(source: &str) -> Font {
    let widths = block_after(source, "const GLYPH_WIDTHS: [u8; GLYPH_COUNT] = [")
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<u8>().expect("invalid width"))
        .collect::<Vec<_>>();
    let hexes = hex_tokens(&strip_line_comments(block_after(
        source,
        "const GLYPHS: [[u16; GLYPH_HEIGHT]; GLYPH_COUNT] = [",
    )));
    let glyphs = hexes
        .chunks_exact(16)
        .map(|chunk| {
            let mut rows = [0u16; 16];
            for (slot, value) in rows.iter_mut().zip(chunk) {
                *slot = *value as u16;
            }
            rows
        })
        .collect::<Vec<_>>();
    Font { widths, glyphs }
}

// ============================== icons ====================================

struct Icon {
    width: usize,
    rows: Vec<u32>,
}

fn load_icon(source: &str, name: &str) -> Icon {
    let marker = format!("pub const {name}: Icon");
    let block = source
        .split_once(&marker)
        .unwrap_or_else(|| panic!("icon {} not found", name))
        .1
        .split_once("};")
        .expect("unterminated icon block")
        .0;
    let width = block
        .split_once("width:")
        .expect("missing width")
        .1
        .split(',')
        .next()
        .unwrap()
        .trim()
        .parse()
        .expect("invalid width");
    let row_text = block
        .split_once("rows: &[")
        .expect("missing rows")
        .1
        .split_once(']')
        .expect("unterminated rows")
        .0;
    let rows = row_text
        .split(',')
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| {
                u32::from_str_radix(value.trim_start_matches("0x"), 16)
                    .expect("invalid bitmap row")
            })
        })
        .collect();
    Icon { width, rows }
}

fn draw_icon(canvas: &mut Canvas, x: usize, y: usize, icon: &Icon) {
    for (row, bits) in icon.rows.iter().enumerate() {
        for col in 0..icon.width {
            if bits & (1 << (31 - col)) != 0 {
                canvas.set_pixel(x + col, y + row, true);
            }
        }
    }
}

// ============================== canvas ===================================

struct Canvas {
    frame: Vec<u8>,
}

const BYTES_PER_ROW: usize = WIDTH / 8;
const FRAME_SIZE: usize = BYTES_PER_ROW * HEIGHT;

impl Canvas {
    fn new() -> Self {
        Self {
            frame: vec![0xFF; FRAME_SIZE],
        }
    }

    fn clear(&mut self) {
        self.frame.fill(0xFF);
    }

    fn set_pixel(&mut self, x: usize, y: usize, black: bool) {
        if x >= WIDTH || y >= HEIGHT {
            return;
        }
        let index = y * BYTES_PER_ROW + x / 8;
        let mask = 1 << (7 - (x & 7));
        if black {
            self.frame[index] &= !mask;
        } else {
            self.frame[index] |= mask;
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, black: bool) {
        for yy in y..y.saturating_add(height) {
            for xx in x..x.saturating_add(width) {
                self.set_pixel(xx, yy, black);
            }
        }
    }

    fn stroke_rect(&mut self, x: usize, y: usize, width: usize, height: usize, thickness: usize) {
        if width == 0 || height == 0 || thickness == 0 {
            return;
        }
        let t = thickness.min(width).min(height);
        self.fill_rect(x, y, width, t, true);
        self.fill_rect(x, y + height.saturating_sub(t), width, t, true);
        self.fill_rect(x, y, t, height, true);
        self.fill_rect(x + width.saturating_sub(t), y, t, height, true);
    }

    fn text_prop_width(text: &str, scale: usize) -> usize {
        with_font(|f| {
            text.chars()
                .map(|c| (f.glyph(c).1 as usize + 1) * scale)
                .sum()
        })
    }

    fn draw_text_prop(&mut self, x: usize, y: usize, scale: usize, text: &str) -> usize {
        with_font(|f| {
            let mut cursor = x;
            for character in text.chars() {
                let (rows, width) = f.glyph(character);
                for (row, bits) in rows.iter().enumerate() {
                    for column in 0..width as usize {
                        if bits & (1 << (15 - column)) != 0 {
                            self.fill_rect(cursor + column * scale, y + row * scale, scale, scale, true);
                        }
                    }
                }
                cursor += (width as usize + 1) * scale;
            }
            cursor - x
        })
    }
}

// ============================ shared chrome ==============================

fn header(canvas: &mut Canvas, title: &str) {
    canvas.draw_text_prop(16, 8, 1, "INKPAPER");
    let width = Canvas::text_prop_width(title, 1);
    canvas.draw_text_prop(384usize.saturating_sub(width), 8, 1, title);
    canvas.fill_rect(16, 29, 368, 1, true);
}

const MAX_LISTED_ITEMS: usize = 7;
const LIST_ROW_HEIGHT: usize = 37;
const LIST_TEXT_X: usize = 50;

fn draw_rows(canvas: &mut Canvas, title: &str, items: &[String], selected: usize) {
    canvas.clear();
    header(canvas, title);
    let selected = selected.min(items.len().saturating_sub(1));
    let first = selected.saturating_sub(MAX_LISTED_ITEMS - 1);
    let mut y = 39usize;
    for (index, item) in items.iter().enumerate().skip(first).take(MAX_LISTED_ITEMS) {
        if index == selected {
            canvas.stroke_rect(16, y, 368, LIST_ROW_HEIGHT - 2, 2);
            canvas.fill_rect(16, y, 5, LIST_ROW_HEIGHT - 2, true);
        }
        canvas.draw_text_prop(LIST_TEXT_X, y + 10, 1, item);
        y += LIST_ROW_HEIGHT;
    }
}

// ================================ home ===================================

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

#[derive(Clone, Copy)]
struct Now {
    year: u16,
    month: u8,
    day: u8,
    weekday: u8,
    hour: u8,
    minute: u8,
}

fn draw_value_centered(canvas: &mut Canvas, x: usize, y: usize, max_width: usize, text: &str) {
    let scale = (1..=3)
        .rev()
        .find(|&s| Canvas::text_prop_width(text, s) <= max_width)
        .unwrap_or(1);
    let width = Canvas::text_prop_width(text, scale);
    canvas.draw_text_prop(x + (max_width.saturating_sub(width)) / 2, y, scale, text);
}

fn draw_clock(canvas: &mut Canvas, dt: Now) {
    let time = format!("{:02}:{:02}", dt.hour, dt.minute);
    canvas.draw_text_prop(24, 44, 4, &time);
    let weekday = WEEKDAYS[(dt.weekday as usize).min(6)];
    canvas.draw_text_prop(24, 114, 1, weekday);
    let m_idx = (dt.month as usize).saturating_sub(1).min(11);
    let md = format!("{} {}", MONTH_NAMES[m_idx], dt.day);
    let year = format!("{}", dt.year);
    let md_w = Canvas::text_prop_width(&md, 2);
    let year_w = Canvas::text_prop_width(&year, 2);
    canvas.draw_text_prop(WIDTH - md_w - 24, 46, 2, &md);
    canvas.draw_text_prop(WIDTH - year_w - 24, 84, 2, &year);
}

/// Mirrors `display.rs`'s `render_home`; icons parsed from `icons.rs`.
fn render_home(
    canvas: &mut Canvas,
    icons: &HomeIcons,
    clock: Option<Now>,
    next_alarm_time: Option<&str>,
    next_alarm_repeat: Option<&str>,
    next_alarm_date: Option<&str>,
    next_alarm_days_left: Option<i64>,
    todo_pending: usize,
    todo_due_today: usize,
    todo_high_pending: usize,
    wifi_configured: bool,
    battery_percent: Option<u8>,
    charging: bool,
    full: bool,
) {
    canvas.clear();
    canvas.draw_text_prop(16, 8, 1, "INKPAPER");

    let percent = battery_percent.unwrap_or(0);
    let battery_icon = if charging {
        if percent < 34 {
            &icons.charging_low
        } else if percent < 67 {
            &icons.charging_medium
        } else {
            &icons.charging_high
        }
    } else if full {
        &icons.battery_full
    } else if percent < 10 {
        &icons.battery_outline
    } else if percent < 40 {
        &icons.battery_low
    } else if percent < 70 {
        &icons.battery_medium
    } else if percent < 95 {
        &icons.battery_high
    } else {
        &icons.battery_full
    };
    let battery_x = 384usize.saturating_sub(battery_icon.width as usize);
    draw_icon(canvas, battery_x, 7, battery_icon);
    if wifi_configured {
        let wifi_x = battery_x.saturating_sub(8 + icons.wifi.width as usize);
        let wifi_y = 7 + battery_icon.rows.len() - icons.wifi.rows.len();
        draw_icon(canvas, wifi_x, wifi_y, &icons.wifi);
    }

    canvas.fill_rect(16, 29, 368, 1, true);
    if let Some(dt) = clock {
        draw_clock(canvas, dt);
    } else {
        canvas.draw_text_prop(180, 52, 3, "--:--");
    }

    const CARD_TOP: usize = 139;
    const CARD_H: usize = 130;
    const CARD_W: usize = 176;
    const VALUE_MAX_WIDTH: usize = 152;

    canvas.stroke_rect(16, CARD_TOP, CARD_W, CARD_H, 2);
    canvas.fill_rect(16, CARD_TOP, 5, CARD_H, true);
    canvas.draw_text_prop(32, CARD_TOP + 14, 1, "NEXT ALARM");
    match next_alarm_time {
        Some(time) => {
            draw_value_centered(canvas, 32, CARD_TOP + 44, VALUE_MAX_WIDTH, time);
            if let Some(repeat) = next_alarm_repeat {
                let w = Canvas::text_prop_width(repeat, 1);
                canvas.draw_text_prop(
                    32 + (VALUE_MAX_WIDTH.saturating_sub(w)) / 2,
                    CARD_TOP + 88,
                    1,
                    repeat,
                );
            }
            let caption = match (next_alarm_date, next_alarm_days_left) {
                (None, _) => "EVERY DAY".to_string(),
                (Some(_), Some(0)) => "TODAY".to_string(),
                (Some(date), Some(n)) if n > 0 => format!("NEXT {date}  D+{n}"),
                _ => "EVERY DAY".to_string(),
            };
            let w = Canvas::text_prop_width(&caption, 1);
            canvas.draw_text_prop(
                32 + (VALUE_MAX_WIDTH.saturating_sub(w)) / 2,
                CARD_TOP + 110,
                1,
                &caption,
            );
        }
        None => {
            draw_value_centered(canvas, 32, CARD_TOP + 44, VALUE_MAX_WIDTH, "NONE");
            let w = Canvas::text_prop_width("NO ALARMS SET", 1);
            let card_center_x = 16 + CARD_W / 2;
            canvas.draw_text_prop(card_center_x - w / 2, CARD_TOP + 96, 1, "NO ALARMS SET");
        }
    }

    let right_x = 16 + CARD_W + 16;
    canvas.stroke_rect(right_x, CARD_TOP, CARD_W, CARD_H, 2);
    canvas.fill_rect(right_x, CARD_TOP, 5, CARD_H, true);
    canvas.draw_text_prop(right_x + 16, CARD_TOP + 14, 1, "OPEN TODOS");
    draw_value_centered(canvas, right_x + 16, CARD_TOP + 44, VALUE_MAX_WIDTH, &todo_pending.to_string());
    let due_caption = format!("DUE TODAY {}", todo_due_today);
    let w = Canvas::text_prop_width(&due_caption, 1);
    canvas.draw_text_prop(
        right_x + 16 + (VALUE_MAX_WIDTH.saturating_sub(w)) / 2,
        CARD_TOP + 88,
        1,
        &due_caption,
    );
    let high_caption = format!("HIGH {}", todo_high_pending);
    let w = Canvas::text_prop_width(&high_caption, 1);
    canvas.draw_text_prop(
        right_x + 16 + (VALUE_MAX_WIDTH.saturating_sub(w)) / 2,
        CARD_TOP + 110,
        1,
        &high_caption,
    );
}

// ============================= calendar ==================================

#[derive(Clone, Copy, Default)]
struct DayMark {
    todo_high: bool,
    todo_low: bool,
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: u16, month: u8) -> u8 {
    const DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 && is_leap_year(year as i64) {
        29
    } else {
        DAYS[(month - 1) as usize]
    }
}

fn weekday_of(year: u16, month: u8, day: u8) -> u8 {
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year as i64;
    if month < 3 {
        y -= 1;
    }
    ((y + y / 4 - y / 100 + y / 400 + T[(month - 1) as usize] + day as i64) % 7) as u8
}

/// Mirrors `alarms::days_since_epoch` - absolute day number, epoch 1970-01-01.
fn days_since_epoch(year: u16, month: u8, day: u8) -> i64 {
    let mut days: i64 = 0;
    for y in 1970..year as i64 {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    let month_days = if is_leap_year(year as i64) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for m in month_days.iter().take(month as usize - 1) {
        days += *m as i64;
    }
    days + day as i64 - 1
}

/// Mirrors `alarms::date_from_days` - inverse of `days_since_epoch`.
fn date_from_days(mut days: i64) -> (u16, u8, u8) {
    let mut year = 1970i64;
    loop {
        let dim = if is_leap_year(year) { 366 } else { 365 };
        if days < dim {
            break;
        }
        days -= dim;
        year += 1;
    }
    for month in 1..=12u8 {
        let dim = days_in_month(year as u16, month) as i64;
        if days < dim {
            return (year as u16, month, (days + 1) as u8);
        }
        days -= dim;
    }
    unreachable!("date_from_days ran past a year's day count")
}

/// Mirrors `screens::draw_month_grid`. UP/DOWN would move `selected_day`;
/// ENTER opens `render_week` for it. `today` gets a thin underline so it
/// stays visible even once the cursor (box + accent bar) moves off it.
fn draw_month_grid(
    canvas: &mut Canvas,
    year: u16,
    month: u8,
    today: Option<Now>,
    selected_day: u8,
    marks: &[DayMark; 32],
) {
    let title = format!("{:04} / {:02}", year, month);
    canvas.draw_text_prop(16, 38, 2, &title);

    const LABELS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
    const COL_WIDTH: usize = 53;
    const ROW_HEIGHT: usize = 32;
    const ORIGIN_X: usize = 18;
    const ORIGIN_Y: usize = 75;
    const MARKER_Y: usize = 18;

    for (i, label) in LABELS.iter().enumerate() {
        let x = ORIGIN_X + i * COL_WIDTH;
        canvas.draw_text_prop(x, ORIGIN_Y, 1, label);
    }
    canvas.fill_rect(16, 99, 368, 1, true);

    let dim = days_in_month(year, month);
    let mut col = weekday_of(year, month, 1) as usize;
    let mut row = 1usize;
    for day in 1..=dim {
        let x = ORIGIN_X + col * COL_WIDTH;
        let y = ORIGIN_Y + row * ROW_HEIGHT;
        let text = day.to_string();
        let is_today = today
            .map(|dt| dt.year == year && dt.month == month && dt.day == day)
            .unwrap_or(false);
        if day == selected_day {
            canvas.stroke_rect(x.saturating_sub(6), y.saturating_sub(4), 34, 30, 2);
            canvas.fill_rect(x.saturating_sub(6), y.saturating_sub(4), 4, 30, true);
        }
        canvas.draw_text_prop(x, y, 1, &text);
        if is_today {
            let w = Canvas::text_prop_width(&text, 1);
            canvas.fill_rect(x, y + 16, w, 1, true);
        }
        let mark = marks[day as usize];
        if mark.todo_high {
            canvas.fill_rect(x, y + MARKER_Y, 6, 6, true);
        } else if mark.todo_low {
            canvas.fill_rect(x, y + MARKER_Y, 4, 4, true);
        }
        col += 1;
        if col > 6 {
            col = 0;
            row += 1;
        }
    }
}

// ============================ alarms page ================================

const WEEKDAY_SHORT: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

enum Repeat {
    Daily,
    Weekly(Vec<u8>),
    Monthly(Vec<u8>),
    Once(u8, u8),
}

struct StoredAlarm {
    hour: u8,
    minute: u8,
    repeat: Repeat,
    enabled: bool,
    label: String,
}

fn format_alarm_row(alarm: &StoredAlarm) -> String {
    let mark = if alarm.enabled { "[X]" } else { "[ ]" };
    let when = match &alarm.repeat {
        Repeat::Daily => format!("{:02}:{:02} DAILY", alarm.hour, alarm.minute),
        Repeat::Weekly(days) => {
            let weekdays: Vec<&str> = days.iter().map(|d| WEEKDAY_SHORT[*d as usize]).collect();
            format!("{:02}:{:02} {}", alarm.hour, alarm.minute, weekdays.join(","))
        }
        Repeat::Monthly(days) => {
            let list: Vec<String> = days.iter().map(|d| d.to_string()).collect();
            format!("{:02}:{:02} DAY {}", alarm.hour, alarm.minute, list.join(","))
        }
        Repeat::Once(month, day) => {
            format!("{:02}:{:02} {:02}/{:02}", alarm.hour, alarm.minute, month, day)
        }
    };
    let mut row = format!("{mark} {when}");
    if !alarm.label.is_empty() {
        row.push(' ');
        row.push_str(&alarm.label);
    }
    row.chars().take(34).collect()
}

fn render_alarms(canvas: &mut Canvas, alarms: &[StoredAlarm], selected: usize) {
    let mut items: Vec<String> = alarms.iter().map(format_alarm_row).collect();
    items.push("+ ADD ALARM".to_string());
    draw_rows(canvas, "ALARMS", &items, selected);
}

// ============================= todos page ================================

enum Importance {
    Low,
    Medium,
    High,
}

struct Todo {
    text: String,
    done: bool,
    importance: Importance,
    due: Option<(u8, u8)>,
    repeat: Option<Repeat>,
}

fn todo_due_today(todo: &Todo, now: Option<Now>) -> bool {
    let Some(dt) = now else {
        return false;
    };
    match &todo.repeat {
        Some(Repeat::Weekly(days)) => days.contains(&dt.weekday),
        Some(Repeat::Monthly(days)) => days.contains(&dt.day),
        Some(Repeat::Daily) => true,
        Some(Repeat::Once(month, day)) => todo
            .due
            .map_or(false, |(dm, dd)| dm == *month && dd == *day),
        None => todo.due.map_or(false, |(dm, dd)| dm == dt.month && dd == dt.day),
    }
}

fn format_todo_row(todo: &Todo, now: Option<Now>) -> String {
    let mark = if todo.done { "[X]" } else { "[ ]" };
    let imp = match todo.importance {
        Importance::Low => "",
        Importance::Medium => "! ",
        Importance::High => "!! ",
    };
    let mut row = format!("{mark} {imp}{}", todo.text);
    if !todo.done {
        if todo_due_today(todo, now) {
            row.push_str(" - DUE TODAY");
        } else if let Some((month, day)) = todo.due {
            row.push_str(&format!(" - {:02}/{:02}", month, day));
        } else if let Some(Repeat::Weekly(days)) = &todo.repeat {
            let weekdays: Vec<&str> = days.iter().map(|d| WEEKDAY_SHORT[*d as usize]).collect();
            row.push_str(" - ");
            row.push_str(&weekdays.join(","));
        }
    }
    row.chars().take(34).collect()
}

fn render_todos(canvas: &mut Canvas, todos: &[Todo], selected: usize, now: Option<Now>) {
    let items: Vec<String> = todos.iter().map(|t| format_todo_row(t, now)).collect();
    draw_rows(canvas, "TODOS", &items, selected);
}

// ============================ nav drawer ==================================

/// Mirrors `screens::pick_navigation`'s drawing, overlaid on whatever page
/// is already on `canvas` (deliberately doesn't clear - the current page
/// stays visible to the right, same as the real drawer). `current` is the
/// destination index the drawer opens pre-selected on now that it starts
/// on wherever you already are instead of always resetting to HOME.
fn draw_navigation_drawer(canvas: &mut Canvas, current: usize) {
    const DESTINATIONS: [&str; 5] = ["HOME", "CALENDAR", "ALARMS", "TODOS", "SETTINGS"];
    canvas.fill_rect(0, 0, 224, 300, false);
    canvas.fill_rect(216, 0, 2, 300, true);
    canvas.draw_text_prop(16, 8, 1, "INKPAPER");
    canvas.fill_rect(16, 29, 192, 1, true);
    canvas.draw_text_prop(16, 40, 2, "GO TO");

    let mut y = 88usize;
    for (index, destination) in DESTINATIONS.iter().enumerate() {
        if index == current {
            canvas.stroke_rect(12, y, 194, 34, 2);
            canvas.fill_rect(12, y, 5, 34, true);
        } else {
            canvas.fill_rect(12, y, 194, 34, false);
        }
        canvas.draw_text_prop(28, y + 8, 1, destination);
        y += 38;
    }
}

// ================================ week ====================================

fn repeat_fires_on(repeat: &Repeat, month: u8, day: u8, weekday: u8) -> bool {
    match repeat {
        Repeat::Daily => true,
        Repeat::Weekly(days) => days.contains(&weekday),
        Repeat::Monthly(days) => days.contains(&day),
        Repeat::Once(m, d) => *m == month && *d == day,
    }
}

/// Sample todos for the week-view preview frame - independent of the list
/// used by the Todos-page preview above, chosen to land across the week
/// containing August 21st.
fn todos_for_week() -> Vec<Todo> {
    vec![
        Todo {
            text: "Buy groceries".into(),
            done: false,
            importance: Importance::Medium,
            due: Some((8, 19)),
            repeat: None,
        },
        Todo {
            text: "Call home".into(),
            done: false,
            importance: Importance::High,
            due: None,
            repeat: Some(Repeat::Weekly(vec![0, 3])),
        },
        Todo {
            text: "Gym".into(),
            done: false,
            importance: Importance::Low,
            due: None,
            repeat: Some(Repeat::Weekly(vec![1, 3, 5])),
        },
        Todo {
            text: "Pay rent".into(),
            done: false,
            importance: Importance::High,
            due: Some((8, 21)),
            repeat: None,
        },
        Todo {
            text: "Dentist".into(),
            done: false,
            importance: Importance::High,
            due: Some((9, 2)),
            repeat: None,
        },
    ]
}

/// Mirrors `screens::week_view`, rendered as a static frame (no
/// interactive selection loop here) via `draw_rows`, with the row for
/// `today` pre-selected the way the real screen opens on the day the
/// cursor was on.
/// Greedy word-wrap: packs whitespace-separated words into lines no wider
/// than `max_width` px at `scale`, measured with the real proportional
/// font (not a fixed char count) - a day column is only ~44px of usable
/// width, too narrow for most todo text on one line. A single word longer
/// than `max_width` on its own (e.g. "groceries") gets hard character-split
/// across lines instead of overflowing the column.
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

/// Mirrors `screens::week_view`: one column per day (Sun-Sat), matching
/// the month grid's column rhythm - "a week is 7 days side by side", not
/// a 7-row list. Each column carries its weekday/date and a word-wrapped
/// stack of that day's open todo text; `today` gets the same thin
/// underline the month grid uses, independent of which day (`day`) the
/// view was opened for. Read-only - any button closes it, there's nothing
/// further to drill into.
fn render_week(canvas: &mut Canvas, year: u16, month: u8, day: u8, today: Option<Now>, todos: &[Todo]) {
    // Header titles are all-caps everywhere else in the app ("CALENDAR",
    // "GO TO") - a separate array from the clock's mixed-case MONTH_NAMES,
    // which is a deliberate exception for that one big personal-clock
    // element, not the house style for chrome/labels.
    const MONTH_NAMES_CAPS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    let start = days_since_epoch(year, month, day) - weekday_of(year, month, day) as i64;
    let (_, sm, sd) = date_from_days(start);
    let (_, em, ed) = date_from_days(start + 6);
    let title = if sm == em {
        format!("{} {}-{}", MONTH_NAMES_CAPS[(sm - 1) as usize], sd, ed)
    } else {
        format!(
            "{} {} - {} {}",
            MONTH_NAMES_CAPS[(sm - 1) as usize],
            sd,
            MONTH_NAMES_CAPS[(em - 1) as usize],
            ed
        )
    };

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
        let (y, m, d) = date_from_days(start + i as i64);
        let weekday = weekday_of(y, m, d);
        let x = ORIGIN_X + i * COL_WIDTH;
        let is_today = today.is_some_and(|dt| dt.year == y && dt.month == m && dt.day == d);

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
                Some(r) => repeat_fires_on(r, m, d, weekday),
                None => t.due.map_or(false, |(dm, dd)| dm == m && dd == d),
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
}

// ================================ png ====================================

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320u32 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut checked = Vec::with_capacity(4 + data.len());
    checked.extend_from_slice(kind);
    checked.extend_from_slice(data);
    png.extend_from_slice(&crc32(&checked).to_be_bytes());
}

fn write_png(path: &Path, frame: &[u8]) -> io::Result<()> {
    let width = WIDTH * SCALE;
    let height = HEIGHT * SCALE;
    let mut raw = Vec::with_capacity((width + 1) * height);
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let lx = x / SCALE;
            let ly = y / SCALE;
            let byte = frame[ly * BYTES_PER_ROW + lx / 8];
            let mask = 1 << (7 - (lx & 7));
            raw.push(if byte & mask == 0 { 0 } else { 255 });
        }
    }

    let mut zlib = vec![0x78, 0x01];
    for (index, block) in raw.chunks(65_535).enumerate() {
        let final_block = index == raw.len().div_ceil(65_535) - 1;
        zlib.push(u8::from(final_block));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    fs::write(path, png)
}

// ================================= main ==================================

struct HomeIcons {
    wifi: Icon,
    charging_low: Icon,
    charging_medium: Icon,
    charging_high: Icon,
    battery_full: Icon,
    battery_outline: Icon,
    battery_low: Icon,
    battery_medium: Icon,
    battery_high: Icon,
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    let src_dir = args.get(1).map(String::as_str).unwrap_or("rust-firmware/src");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("tmp");
    let font_source = fs::read_to_string(format!("{src_dir}/font8x16.rs"))?;
    let icon_source = fs::read_to_string(format!("{src_dir}/icons.rs"))?;

    let font = load_font(&font_source);
    FONT.with(|slot| *slot.borrow_mut() = Some(font));

    let icons = HomeIcons {
        wifi: load_icon(&icon_source, "WIFI"),
        charging_low: load_icon(&icon_source, "CHARGING_LOW"),
        charging_medium: load_icon(&icon_source, "CHARGING_MEDIUM"),
        charging_high: load_icon(&icon_source, "CHARGING_HIGH"),
        battery_full: load_icon(&icon_source, "BATTERY_FULL"),
        battery_outline: load_icon(&icon_source, "BATTERY_OUTLINE"),
        battery_low: load_icon(&icon_source, "BATTERY_LOW"),
        battery_medium: load_icon(&icon_source, "BATTERY_MEDIUM"),
        battery_high: load_icon(&icon_source, "BATTERY_HIGH"),
    };

    let mut canvas = Canvas::new();
    let now = Now { year: 2026, month: 8, day: 19, weekday: 3, hour: 7, minute: 30 };
    let mut frames = Vec::new();
    let mut names = Vec::new();

    render_home(
        &mut canvas,
        &icons,
        Some(now),
        Some("07:30"),
        Some("SU,WE,FR"),
        Some("08/21"),
        Some(2),
        3,
        2,
        1,
        true,
        Some(80),
        false,
        false,
    );
    frames.push(canvas.frame.clone());
    names.push("ui_home_alarm.png".to_string());

    render_home(
        &mut canvas,
        &icons,
        Some(now),
        None,
        None,
        None,
        None,
        5,
        0,
        2,
        true,
        Some(35),
        false,
        false,
    );
    frames.push(canvas.frame.clone());
    names.push("ui_home_none.png".to_string());

    // Home with no alarms AND no todos at all - the "nothing to look at" state.
    render_home(
        &mut canvas,
        &icons,
        Some(now),
        None,
        None,
        None,
        None,
        0,
        0,
        0,
        false,
        None,
        false,
        false,
    );
    frames.push(canvas.frame.clone());
    names.push("ui_home_empty.png".to_string());

    // Calendar: August 2026 - todos due on a handful of days. `now`.day
    // (19) is today; selected_day (21) is a cursor moved off today, to
    // show the underline and the box/accent-bar cursor are independent.
    let mut marks = [DayMark::default(); 32];
    marks[1].todo_high = true;
    marks[10].todo_low = true;
    marks[15].todo_high = true;
    marks[19].todo_low = true;
    marks[21].todo_low = true;
    marks[31].todo_high = true;
    canvas.clear();
    header(&mut canvas, "CALENDAR");
    draw_month_grid(&mut canvas, 2026, 8, Some(now), 21, &marks);
    frames.push(canvas.frame.clone());
    names.push("ui_calendar.png".to_string());

    // Week view for the week containing the selected day (21st).
    render_week(&mut canvas, 2026, 8, 21, Some(now), &todos_for_week());
    frames.push(canvas.frame.clone());
    names.push("ui_week.png".to_string());

    let alarms = vec![
        StoredAlarm { hour: 7, minute: 30, repeat: Repeat::Weekly(vec![0, 2, 4]), enabled: true, label: "wake".into() },
        StoredAlarm { hour: 9, minute: 0, repeat: Repeat::Daily, enabled: true, label: "standup".into() },
        StoredAlarm { hour: 22, minute: 0, repeat: Repeat::Once(12, 25), enabled: true, label: "xmas".into() },
        StoredAlarm { hour: 21, minute: 15, repeat: Repeat::Monthly(vec![1, 15]), enabled: true, label: "rent".into() },
        StoredAlarm { hour: 6, minute: 0, repeat: Repeat::Daily, enabled: false, label: String::new() },
    ];
    render_alarms(&mut canvas, &alarms, 1);
    frames.push(canvas.frame.clone());
    names.push("ui_alarms.png".to_string());

    let todos = vec![
        Todo { text: "Buy groceries".into(), done: false, importance: Importance::Medium, due: Some((8, 19)), repeat: None },
        Todo { text: "Call home".into(), done: false, importance: Importance::High, due: None, repeat: None },
        Todo { text: "Gym".into(), done: false, importance: Importance::Low, due: None, repeat: Some(Repeat::Weekly(vec![0, 2, 4])) },
        Todo { text: "Pay rent".into(), done: true, importance: Importance::High, due: Some((8, 1)), repeat: Some(Repeat::Monthly(vec![1])) },
        Todo { text: "Dentist".into(), done: false, importance: Importance::High, due: Some((9, 2)), repeat: None },
    ];
    render_todos(&mut canvas, &todos, 0, Some(now));
    frames.push(canvas.frame.clone());
    names.push("ui_todos.png".to_string());

    // GO TO drawer, opened from the Todos page (index 3) - pre-selected on
    // TODOS instead of always resetting to HOME.
    draw_navigation_drawer(&mut canvas, 3);
    frames.push(canvas.frame.clone());
    names.push("ui_goto.png".to_string());

    fs::create_dir_all(out_dir)?;
    for (frame, name) in frames.iter().zip(names) {
        let path = Path::new(out_dir).join(name);
        write_png(&path, frame)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
