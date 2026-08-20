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
    // Scene 1: everything populated, good battery, wifi.
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
    save("home-full.png", &c);

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
    c.fill_rect(16 + 176 - 3, 34, 3, 250, true);
    save("page-goto.png", &c);
}
