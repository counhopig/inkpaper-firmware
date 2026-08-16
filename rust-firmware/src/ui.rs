//! Shared 3-button screen primitives (UP/DOWN=cycle, ENTER=pick, hold=back),
//! factored out of `provision.rs` so `screens.rs` doesn't have to duplicate
//! the polling/rendering conventions established there.

use std::thread;
use std::time::Duration;

use crate::board::Note4Board;
use crate::button::{ButtonEvent, POLL_INTERVAL_MS};
use crate::canvas::Canvas;
use crate::watchdog;

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

/// Characters first, control items last: the wheel starts on 'A' (see
/// `enter_text`'s initial index) so the first thing the user sees is an
/// obvious "cycle through letters" affordance rather than a "[ DONE ]"
/// control, which was easy to mistake for the whole screen not working.
fn wheel_item(index: usize) -> WheelItem {
    if index < CHARSET.len() {
        return WheelItem::Char(CHARSET[index]);
    }
    match index - CHARSET.len() {
        0 => WheelItem::Done,
        1 => WheelItem::Backspace,
        2 => WheelItem::Cancel,
        _ => WheelItem::ToggleCase,
    }
}

/// Outcome of one blocking poll wait: which control fired, if any.
pub enum Nav {
    None,
    Up,
    Down,
    Enter,
    /// Long-press on either UP or DOWN: the escape hatch out of any screen.
    Cancel,
}

pub fn poll_nav(board: &mut Note4Board) -> Nav {
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

pub fn tick() {
    watchdog::feed();
    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
}

pub fn header(canvas: &mut Canvas, title: &str) {
    canvas.draw_text(8, 4, 2, title);
    canvas.fill_rect(8, 24, 384, 2, true);
}

pub fn footer(canvas: &mut Canvas, hint: &str) {
    canvas.draw_text(8, 284, 1, hint);
}

pub fn show_message(board: &mut Note4Board, title: &str, lines: &[&str], pause: Duration) {
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

/// Longest list shown at once; no scroll/paging in v1.
pub const MAX_LISTED_ITEMS: usize = 10;

/// Blocking wheel-list picker: draws `items` (already formatted by the
/// caller, e.g. with a "[x] " done marker baked in) under `title`,
/// highlights the selected row with a solid black bar + white text, and
/// returns the chosen index on ENTER or `None` on hold-to-cancel. Shared by
/// every screen that's "a list of things, pick one" - the menu, alarms
/// list, todos list, and (via `provision.rs`) the Wi-Fi AP picker.
pub fn pick_from_list(
    board: &mut Note4Board,
    title: &str,
    items: &[String],
    hint: &str,
) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    let mut selected = 0usize;
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, title);
            let mut y = 32usize;
            for (i, item) in items.iter().take(MAX_LISTED_ITEMS).enumerate() {
                if i == selected {
                    canvas.fill_rect(4, y.saturating_sub(2), 392, 20, true);
                    canvas.draw_text_prop_white(8, y, 1, item);
                } else {
                    canvas.draw_text_prop(8, y, 1, item);
                }
                y += 20;
            }
            footer(canvas, hint);
            let _ = board.display.refresh_full();
            needs_redraw = false;
        }
        match poll_nav(board) {
            Nav::Up => {
                selected = if selected == 0 {
                    items.len() - 1
                } else {
                    selected - 1
                };
                needs_redraw = true;
            }
            Nav::Down => {
                selected = (selected + 1) % items.len();
                needs_redraw = true;
            }
            Nav::Enter => return Some(selected),
            Nav::Cancel => return None,
            Nav::None => {}
        }
        tick();
    }
}

/// Blocking UP/DOWN character-wheel text entry - the pattern this was
/// extracted from is `provision.rs`'s Wi-Fi-password screen. `subtitle`, if
/// given, is drawn as a context line above the text field (e.g. the SSID a
/// password belongs to). Returns `None` on hold-to-cancel or picking the
/// wheel's CANCEL item.
pub fn enter_text(
    board: &mut Note4Board,
    title: &str,
    subtitle: Option<&str>,
    max_len: usize,
) -> Option<String> {
    let mut buffer = String::new();
    // Start on 'A' (CHARSET[0] is a space), not the first control item, so
    // the first thing shown is an obvious letter to cycle through instead
    // of a "[ DONE ]" that reads like a dead end.
    let mut wheel_index = 1usize;
    let mut lowercase = false;
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, title);
            let mut y = 32usize;
            if let Some(subtitle) = subtitle {
                canvas.draw_text_prop(8, y, 1, subtitle);
                y += 20;
            }
            canvas.draw_text_prop(8, y, 1, &format!("TEXT: {buffer}"));
            canvas.draw_text_prop(8, y + 24, 1, "UP/DOWN=CYCLE  ENTER=ADD THIS:");
            let case_label = if lowercase { "lower" } else { "UPPER" };
            let candidate_label = match wheel_item(wheel_index) {
                WheelItem::Done => "DONE".to_string(),
                WheelItem::Backspace => "DEL".to_string(),
                WheelItem::Cancel => "CANCEL".to_string(),
                WheelItem::ToggleCase => format!("CASE:{case_label}"),
                WheelItem::Char(c) => c.to_string(),
            };
            // Same solid-bar-plus-white-text treatment as `pick_from_list`'s
            // selected row: the single biggest, hardest-to-miss thing on
            // the screen is what UP/DOWN is currently cycling to.
            let box_width = Canvas::text_prop_width(&candidate_label, 2) + 24;
            canvas.fill_rect(8, y + 42, box_width, 40, true);
            canvas.draw_text_prop_white(20, y + 50, 2, &candidate_label);
            footer(canvas, "HOLD=BACK");
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
                    WheelItem::Done => return Some(buffer),
                    WheelItem::Backspace => {
                        buffer.pop();
                    }
                    WheelItem::Cancel => return None,
                    WheelItem::ToggleCase => lowercase = !lowercase,
                    WheelItem::Char(c) => {
                        if buffer.chars().count() < max_len {
                            let c = if lowercase { c.to_ascii_lowercase() } else { c };
                            buffer.push(c);
                        }
                    }
                }
                wheel_index = 1; // back to 'A', not the space at index 0
                needs_redraw = true;
            }
            Nav::Cancel => return None,
            Nav::None => {}
        }
        tick();
    }
}

/// Blocking UP/DOWN-cycles-a-number, ENTER-confirms stepper, wrapping in
/// `[min, max]`. `None` on hold-to-cancel.
pub fn pick_number(board: &mut Note4Board, title: &str, min: u8, max: u8) -> Option<u8> {
    let mut value = min;
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, title);
            let label = format!("{value:02}");
            canvas.fill_rect(8, 60, 100, 60, true);
            canvas.draw_text_prop_white(24, 78, 4, &label);
            footer(canvas, "UP/DOWN=CHANGE ENTER=OK HOLD=BACK");
            let _ = board.display.refresh_full();
            needs_redraw = false;
        }
        match poll_nav(board) {
            Nav::Up => {
                value = if value == max { min } else { value + 1 };
                needs_redraw = true;
            }
            Nav::Down => {
                value = if value == min { max } else { value - 1 };
                needs_redraw = true;
            }
            Nav::Enter => return Some(value),
            Nav::Cancel => return None,
            Nav::None => {}
        }
        tick();
    }
}
