//! Shared 3-button primitives: short UP/DOWN moves, short ENTER confirms,
//! long ENTER goes back, and long UP/DOWN switches a page at a time.
//! Shared by all on-device screens. Text entry is intentionally absent:
//! user-authored strings are supplied by Desktop/Server only.

use std::thread;
use std::time::Duration;

use crate::board::Note4Board;
use crate::button::{ButtonEvent, POLL_INTERVAL_MS};
use crate::canvas::Canvas;
use crate::display::Rect;
use crate::watchdog;

/// Outcome of one blocking poll wait: which control fired, if any.
pub enum Nav {
    None,
    Up,
    Down,
    Enter,
    /// Long ENTER: return to the previous screen.
    Cancel,
    /// Long UP/DOWN: switch a page at a time.
    PageUp,
    PageDown,
}

pub fn poll_nav(board: &mut Note4Board) -> Nav {
    if let Some(event) = board.key_enter.poll() {
        match event {
            ButtonEvent::Pressed => return Nav::Enter,
            ButtonEvent::LongPressed => return Nav::Cancel,
            ButtonEvent::Released => {}
        }
    }
    if let Some(event) = board.key_up.poll() {
        match event {
            ButtonEvent::Pressed => return Nav::Up,
            ButtonEvent::LongPressed => return Nav::PageUp,
            ButtonEvent::Released => {}
        }
    }
    if let Some(event) = board.key_down.poll() {
        match event {
            ButtonEvent::Pressed => return Nav::Down,
            ButtonEvent::LongPressed => return Nav::PageDown,
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
    canvas.draw_text_prop(16, 8, 1, "INKPAPER");
    let width = Canvas::text_prop_width(title, 1);
    canvas.draw_text_prop(384usize.saturating_sub(width), 8, 1, title);
    canvas.fill_rect(16, 29, 368, 1, true);
}

/// Reserved for screen call sites that still describe their controls in
/// code. The device UI intentionally has no persistent button-hint footer.
pub fn footer(_canvas: &mut Canvas, _hint: &str) {}

pub fn show_message(board: &mut Note4Board, title: &str, lines: &[&str], pause: Duration) {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    header(canvas, title);
    let mut y = 62usize;
    for line in lines {
        canvas.draw_text_prop(24, y, 2, line);
        y += 38;
    }
    let _ = board.display.refresh_partial(Rect {
        x: 0,
        y: 0,
        width: 400,
        height: 300,
    });
    thread::sleep(pause);
}

/// Rows visible at once. Longer lists scroll around the selection.
pub const MAX_LISTED_ITEMS: usize = 7;

/// Blocking wheel-list picker: draws `items` (already formatted by the
/// caller, e.g. with a "[x] " done marker baked in) under `title`,
/// highlights the selected row with an outlined rect + side bar, and returns
/// the chosen index on ENTER or `None` on hold-to-cancel. Shared by every
/// screen that's "a list of things, pick one" - the menu, alarms list,
/// todos list.
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
    let mut first_draw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, title);
            let first = if selected < MAX_LISTED_ITEMS {
                0
            } else {
                selected + 1 - MAX_LISTED_ITEMS
            };
            let mut y = 39usize;
            for (i, item) in items.iter().enumerate().skip(first).take(MAX_LISTED_ITEMS) {
                if i == selected {
                    canvas.stroke_rect(16, y, 368, 29, 2);
                    canvas.fill_rect(16, y, 5, 29, true);
                    canvas.draw_text_prop(30, y + 6, 1, ">");
                    canvas.draw_text_prop(50, y + 6, 1, item);
                } else {
                    canvas.draw_text_prop(24, y + 6, 1, &format!("{:02}", i + 1));
                    canvas.fill_rect(50, y + 14, 8, 1, true);
                    canvas.draw_text_prop(68, y + 6, 1, item);
                }
                y += 31;
            }
            footer(canvas, hint);
            if first_draw {
                let _ = board.display.refresh_full();
                first_draw = false;
            } else {
                let _ = board.display.refresh_partial(Rect {
                    x: 8,
                    y: 34,
                    width: 384,
                    height: 226,
                });
            }
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
            Nav::PageUp => {
                selected = selected.saturating_sub(MAX_LISTED_ITEMS);
                needs_redraw = true;
            }
            Nav::PageDown => {
                selected = (selected + MAX_LISTED_ITEMS).min(items.len() - 1);
                needs_redraw = true;
            }
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
    let mut first_draw = true;
    loop {
        if needs_redraw {
            let canvas = board.display.canvas_mut();
            canvas.clear();
            header(canvas, title);
            let label = format!("{value:02}");
            let number_width = Canvas::text_prop_width(&label, 5);
            let box_width = number_width + 64;
            let box_x = 200usize.saturating_sub(box_width / 2);
            canvas.draw_text_prop(156, 54, 1, "CHOOSE VALUE");
            canvas.stroke_rect(box_x, 82, box_width, 100, 3);
            canvas.fill_rect(box_x, 82, 7, 100, true);
            canvas.draw_text_prop(box_x + 32, 92, 5, &label);
            footer(canvas, "UP/DOWN CHANGE   ENTER OK   HOLD ENTER BACK");
            if first_draw {
                let _ = board.display.refresh_full();
                first_draw = false;
            } else {
                let _ = board.display.refresh_partial(Rect {
                    x: 96,
                    y: 76,
                    width: 208,
                    height: 114,
                });
            }
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
            Nav::PageUp => {
                value = value.saturating_add(10).min(max);
                needs_redraw = true;
            }
            Nav::PageDown => {
                value = value.saturating_sub(10).max(min);
                needs_redraw = true;
            }
            Nav::None => {}
        }
        tick();
    }
}
