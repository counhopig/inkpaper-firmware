//! PC preview of the on-device screens. Draws exactly the same pixels the
//! firmware would (the render modules are `#[path]`-included in place from
//! `rust-firmware/src/`) and writes PNGs, so home-screen layout can be
//! iterated without flashing a device.
//!
//! Run: `cargo run --release` from `tools/preview/` (host toolchain). Writes
//! `*.png` into the current directory.

#[path = "../../../rust-firmware/src/canvas.rs"]
mod canvas;

#[path = "../../../rust-firmware/src/font8x16.rs"]
mod font8x16;

#[path = "../../../rust-firmware/src/icons.rs"]
mod icons;

#[path = "../../../rust-firmware/src/home.rs"]
mod home;

/// Minimal stand-ins for the firmware-only types `home.rs` depends on.
mod board {
    pub struct ChargeSnapshot {
        pub power_present: bool,
        pub charging: bool,
        pub full: bool,
    }
}

mod rtc {
    #[derive(Clone, Copy)]
    pub struct DateTime {
        pub year: u16,
        pub month: u8,
        pub day: u8,
        pub weekday: u8,
        pub hour: u8,
        pub minute: u8,
        pub second: u8,
        pub voltage_low: bool,
    }
}

use canvas::{Canvas, HEIGHT, WIDTH};
use rtc::DateTime;

fn save(path: &str, canvas: &Canvas) {
    let img = image::GrayImage::from_fn(WIDTH as u32, HEIGHT as u32, |x, y| {
        // Frame bit 1 = white, 0 = black; map to 255 (white) / 0 (black).
        let byte = canvas.frame()[y as usize * (WIDTH / 8) + (x as usize / 8)];
        let white = byte & (1 << (7 - (x as usize & 7))) != 0;
        image::Luma([if white { 255 } else { 0 }])
    });
    img.save(path).expect("write png");
    println!("wrote {path}");
}

fn dt() -> DateTime {
    DateTime {
        year: 2026,
        month: 8,
        day: 20,
        weekday: 4, // THU
        hour: 14,
        minute: 50,
        second: 0,
        voltage_low: false,
    }
}

fn full_battery() -> board::ChargeSnapshot {
    board::ChargeSnapshot {
        power_present: false,
        charging: false,
        full: false,
    }
}

fn main() {
    // Scene 1: home screen, everything populated (README's home.png).
    let mut c = Canvas::new();
    home::render(
        &mut c,
        Some(&dt()),
        Some("07:30"),
        Some("AUG 20"),
        Some(0),
        3,
        1,
        true,
        Some(85),
        full_battery(),
    );
    save("home.png", &c);

    // Scene 2: no clock (RTC lost), no alarm, no wifi, low battery.
    let mut c = Canvas::new();
    home::render(
        &mut c,
        None,
        None,
        None,
        None,
        0,
        0,
        false,
        Some(8),
        board::ChargeSnapshot {
            power_present: false,
            charging: false,
            full: false,
        },
    );
    save("home-empty.png", &c);

    // Scene 3: charging + one-shot alarm in a few days.
    let mut c = Canvas::new();
    home::render(
        &mut c,
        Some(&dt()),
        Some("SEP 1"),
        Some("SEP 1"),
        Some(12),
        1,
        0,
        true,
        Some(45),
        board::ChargeSnapshot {
            power_present: true,
            charging: true,
            full: false,
        },
    );
    save("home-charging.png", &c);

    // Sub-screen scenes: share the home screen's header (brand mark +
    // wordmark + right-aligned title + rule) so every page matches the
    // home visual language. The list row chrome mirrors `ui::draw_rows`
    // (stroke + left accent bar on the selection).
    fn header(canvas: &mut Canvas, title: &str) {
        canvas.stroke_rect(16, 9, 14, 14, 2);
        canvas.fill_rect(21, 14, 4, 4, true);
        canvas.draw_text_prop(38, 8, 1, "INKPAPER");
        let width = canvas::Canvas::text_prop_width(title, 1);
        canvas.draw_text_prop(384usize.saturating_sub(width), 8, 1, title);
        canvas.fill_rect(16, 29, 368, 1, true);
    }

    // Settings list, selection on the second row.
    let mut c = Canvas::new();
    c.clear();
    header(&mut c, "SETTINGS");
    let items = ["SYNC INTERVAL", "TIME ZONE", "GO TO", "ABOUT"];
    let mut y = 39usize;
    for (i, item) in items.iter().enumerate() {
        if i == 1 {
            c.stroke_rect(16, y, 368, 35, 2);
            c.fill_rect(16, y, 5, 35, true);
        }
        c.draw_text_prop(50, y + 10, 1, item);
        y += 37;
    }
    save("page-list.png", &c);

    // Number picker (mirrors `ui::pick_number` layout).
    let mut c = Canvas::new();
    c.clear();
    header(&mut c, "HOUR");
    let label = "07";
    let nw = canvas::Canvas::text_prop_width(label, 5);
    let box_w = nw + 64;
    let box_x = 200usize.saturating_sub(box_w / 2);
    let cap_w = canvas::Canvas::text_prop_width("CHOOSE VALUE", 1);
    c.draw_text_prop((400usize.saturating_sub(cap_w)) / 2, 87, 1, "CHOOSE VALUE");
    c.stroke_rect(box_x, 123, box_w, 120, 3);
    c.fill_rect(box_x, 123, 7, 120, true);
    c.draw_text_prop(box_x + 7 + (box_w - 7 - nw) / 2, 133, 5, label);
    save("page-number.png", &c);

    // GO TO navigation drawer: overlaid on the home screen, with a thick
    // vertical rule down its right edge (mirrors screens::draw_navigation_bar).
    let mut c = Canvas::new();
    home::render(
        &mut c,
        Some(&dt()),
        Some("07:30"),
        Some("AUG 20"),
        Some(0),
        3,
        1,
        true,
        Some(85),
        full_battery(),
    );
    c.fill_rect(16, 34, 176, 250, false);
    c.draw_text_prop(24, 42, 1, "GO TO");
    c.fill_rect(24, 58, 160, 1, true);
    let dests = ["HOME", "CALENDAR", "ALARMS", "TODOS", "SETTINGS"];
    for (i, d) in dests.iter().enumerate() {
        let y = 64 + i * 37;
        if i == 1 {
            c.stroke_rect(22, y, 164, 35, 2);
            c.fill_rect(22, y, 5, 35, true);
        }
        c.draw_text_prop(30, y + 10, 1, d);
    }
    c.fill_rect(16 + 176 - 3, 34, 3, 266, true);
    save("goto.png", &c);

    // Calendar month grid (README's calendar.png, mirrors
    // screens::draw_month_grid). August 2026 starts on a Saturday.
    let mut c = Canvas::new();
    c.clear();
    header(&mut c, "CALENDAR");
    c.draw_text_prop(16, 38, 2, "2026 / 08");
    const CAL_LABELS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
    const CAL_COL: usize = 53;
    const CAL_ORIGIN_X: usize = 18;
    const CAL_ORIGIN_Y: usize = 75;
    const CAL_ROW: usize = 32;
    for (i, l) in CAL_LABELS.iter().enumerate() {
        c.draw_text_prop(CAL_ORIGIN_X + i * CAL_COL, CAL_ORIGIN_Y, 1, l);
    }
    c.fill_rect(16, 99, 368, 1, true);
    let mut col = 6usize; // 2026-08-01 = Saturday
    let mut row = 1usize;
    for day in 1..=31 {
        let x = CAL_ORIGIN_X + col * CAL_COL;
        let y = CAL_ORIGIN_Y + row * CAL_ROW;
        if day == 20 {
            c.stroke_rect(x.saturating_sub(6), y.saturating_sub(4), 34, 30, 2);
            c.fill_rect(x.saturating_sub(6), y.saturating_sub(4), 4, 30, true);
            let w = canvas::Canvas::text_prop_width(&day.to_string(), 1);
            c.fill_rect(x, y + 16, w, 1, true);
            c.fill_rect(x, y + 18, 4, 4, true);
        }
        c.draw_text_prop(x, y, 1, &day.to_string());
        col += 1;
        if col > 6 {
            col = 0;
            row += 1;
        }
    }
    save("calendar.png", &c);

    // Week view (README's week-view.png, mirrors screens::week_view):
    // one column per day, word-wrapped todo text under each date.
    let mut c = Canvas::new();
    c.clear();
    header(&mut c, "AUG 20-26");
    const WV_COL: usize = 53;
    const WV_WD: usize = 38;
    const WV_DATE: usize = 56;
    const WV_LIST: usize = 90;
    const WV_LINE: usize = 15;
    const WV_BOTTOM: usize = 296;
    let wdays = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
    let dates = [20, 21, 22, 23, 24, 25, 26];
    let cols: [&[&str]; 7] = [
        &[],
        &["call mum"],
        &[],
        &["water", "plants"],
        &["buy oats", "pay rent"],
        &[],
        &["weekend", "cleanup"],
    ];
    for i in 0..7usize {
        let x = 16 + i * WV_COL;
        c.draw_text_prop(x, WV_WD, 1, wdays[i]);
        c.draw_text_prop(x, WV_DATE, 1, &dates[i].to_string());
        if i == 3 {
            let w = canvas::Canvas::text_prop_width(&dates[i].to_string(), 1);
            c.fill_rect(x, WV_DATE + 16, w, 1, true);
        }
        if i > 0 {
            c.fill_rect(x - 4, WV_WD, 1, WV_BOTTOM - WV_WD, true);
        }
        let mut yy = WV_LIST;
        for line in cols[i] {
            if yy + WV_LINE > WV_BOTTOM {
                break;
            }
            c.draw_text_prop(x, yy, 1, line);
            yy += WV_LINE;
        }
    }
    save("week-view.png", &c);

    // Alarms list (README's alarms.png, mirrors screens::render_alarm_page).
    let mut c = Canvas::new();
    c.clear();
    header(&mut c, "ALARMS");
    let alarm_rows = [
        "[X] 07:30 DAILY  Wake up",
        "[ ] 21:15 SU,MO,WE  Water plants",
        "[X] 06:00 DAY 1,15  Pay bills",
        "+ ADD ALARM",
    ];
    for (i, r) in alarm_rows.iter().enumerate() {
        let y = 39 + i * 37;
        if i == 0 {
            c.stroke_rect(16, y, 368, 35, 2);
            c.fill_rect(16, y, 5, 35, true);
        }
        c.draw_text_prop(50, y + 10, 1, r);
    }
    save("alarms.png", &c);

    // Todos list (README's todos.png, mirrors screens::render_todo_page).
    let mut c = Canvas::new();
    c.clear();
    header(&mut c, "TODOS");
    let todo_rows = [
        "[ ] !! Buy oats - DUE TODAY",
        "[X] Water the plant",
        "[ ] ! Call mum - 08/24",
        "[ ] Review notes",
    ];
    for (i, r) in todo_rows.iter().enumerate() {
        let y = 39 + i * 37;
        if i == 2 {
            c.stroke_rect(16, y, 368, 35, 2);
            c.fill_rect(16, y, 5, 35, true);
        }
        c.draw_text_prop(50, y + 10, 1, r);
    }
    save("todos.png", &c);
}
