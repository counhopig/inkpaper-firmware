use anyhow::{bail, Result};
use esp_idf_svc::sys::zectrix_epd::{
    zectrix_epd_config_t, zectrix_epd_del, zectrix_epd_get_default_config, zectrix_epd_handle_t,
    zectrix_epd_new, zectrix_epd_power_off, zectrix_epd_power_on, zectrix_epd_rect_t,
    zectrix_epd_refresh_full_1bpp, zectrix_epd_refresh_partial_1bpp,
};

use crate::canvas::Canvas;
use crate::canvas::WIDTH;
use crate::icons::{self, Icon};
use crate::rtc::DateTime;

pub use crate::canvas::Rect;

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub struct EpdDisplay {
    handle: zectrix_epd_handle_t,
    canvas: Canvas,
}

impl EpdDisplay {
    pub fn new() -> Result<Self> {
        let mut config = unsafe { std::mem::zeroed::<zectrix_epd_config_t>() };
        unsafe { zectrix_epd_get_default_config(&mut config) };

        let mut handle: zectrix_epd_handle_t = std::ptr::null_mut();
        check_epd("initialize official Zectrix EPD driver", unsafe {
            zectrix_epd_new(&config, &mut handle)
        })?;

        Ok(Self {
            handle,
            canvas: Canvas::new(),
        })
    }

    /// Direct canvas access for screens that don't fit the fixed
    /// `render_home` layout, e.g. the navigation drawer and `screens.rs`.
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        &mut self.canvas
    }

    /// The idle/background screen: clock, Wi-Fi/battery status, next-alarm
    /// summary, pending-todo count. `main.rs` redraws this after returning
    /// from any modal screen (the navigation drawer, settings menu, alarm
    /// ring).
    ///
    /// `next_alarm_time`/`next_alarm_date` are kept as two separate values
    /// (rather than one pre-joined "HH:MM MM/DD" string) so the card can
    /// always draw the time at the same confident scale and the date, only
    /// present for a one-shot alarm, as a smaller caption underneath -
    /// joining them into one line forced the whole value down to whatever
    /// scale fit the longer one-shot format, which made a plain daily
    /// "07:30" and a dated "22:00 10/25" look like two different kinds of
    /// number instead of the same field.
    #[allow(clippy::too_many_arguments)]
    pub fn render_home(
        &mut self,
        clock: Option<&DateTime>,
        next_alarm_time: Option<&str>,
        next_alarm_date: Option<&str>,
        todo_pending: usize,
        wifi_configured: bool,
        battery_percent: Option<u8>,
        charging: bool,
    ) {
        self.canvas.clear();
        self.canvas.draw_text_prop(16, 8, 1, "INKPAPER");

        // Status cluster is right-aligned like every other header, built
        // from icon glyphs (right to left: battery, wifi) - see `icons.rs`.
        // Wi-Fi's icon is omitted entirely when not configured (absence is
        // the "off" signal) rather than drawn in some fainter style: at
        // this pixel size a hollow/outlined variant was tried and was
        // visually indistinguishable from the filled one once actually
        // rendered.
        let percent = battery_percent.unwrap_or(0);
        let battery_icon: &Icon = if charging {
            if percent < 34 {
                &icons::CHARGING_LOW
            } else if percent < 67 {
                &icons::CHARGING_MEDIUM
            } else {
                &icons::CHARGING_HIGH
            }
        } else if percent < 10 {
            &icons::BATTERY_OUTLINE
        } else if percent < 40 {
            &icons::BATTERY_LOW
        } else if percent < 70 {
            &icons::BATTERY_MEDIUM
        } else if percent < 95 {
            &icons::BATTERY_HIGH
        } else {
            &icons::BATTERY_FULL
        };
        let battery_x = 384usize.saturating_sub(battery_icon.width as usize);
        icons::draw_icon(&mut self.canvas, battery_x, 7, battery_icon);
        if wifi_configured {
            let wifi_x = battery_x.saturating_sub(8 + icons::WIFI.width as usize);
            let wifi_y = 7 + battery_icon.rows.len() - icons::WIFI.rows.len();
            icons::draw_icon(&mut self.canvas, wifi_x, wifi_y, &icons::WIFI);
        }

        self.canvas.fill_rect(16, 29, 368, 1, true);
        if let Some(dt) = clock {
            self.draw_clock(dt);
        } else {
            let dash_w = Canvas::text_prop_width("--:--", 3);
            let dash_x = (WIDTH - dash_w) / 2;
            self.canvas.draw_text_prop(dash_x, 52, 3, "--:--");
        }

        // Cards reach down to a real bottom margin (300-269=31px, matching
        // the header's own rhythm) instead of stopping at a fixed height
        // that left ~67px of unstructured void below them. The values
        // that actually matter (next alarm time, pending todo count) are
        // drawn bigger so they read as confident numbers rather than
        // floating small inside an oversized box.
        const CARD_TOP: usize = 139;
        const CARD_H: usize = 130;
        const CARD_W: usize = 176;
        // Value column width available before text would run into the
        // card's own right edge (or, worse, the neighboring card).
        const VALUE_MAX_WIDTH: usize = 152;

        self.canvas.stroke_rect(16, CARD_TOP, CARD_W, CARD_H, 2);
        self.canvas.fill_rect(16, CARD_TOP, 5, CARD_H, true);
        self.canvas
            .draw_text_prop(32, CARD_TOP + 14, 1, "NEXT ALARM");
        match next_alarm_time {
            Some(time) => {
                draw_value_centered(&mut self.canvas, 32, CARD_TOP + 50, VALUE_MAX_WIDTH, time);
                if let Some(date) = next_alarm_date {
                    let date_w = Canvas::text_prop_width(date, 1);
                    self.canvas.draw_text_prop(
                        32 + (VALUE_MAX_WIDTH.saturating_sub(date_w)) / 2,
                        CARD_TOP + 104,
                        1,
                        date,
                    );
                }
            }
            None => {
                draw_value_centered(&mut self.canvas, 32, CARD_TOP + 50, VALUE_MAX_WIDTH, "NONE");
            }
        }

        let right_x = 16 + CARD_W + 16;
        self.canvas
            .stroke_rect(right_x, CARD_TOP, CARD_W, CARD_H, 2);
        self.canvas.fill_rect(right_x, CARD_TOP, 5, CARD_H, true);
        self.canvas
            .draw_text_prop(right_x + 16, CARD_TOP + 14, 1, "OPEN TODOS");
        let todo_count = todo_pending.to_string();
        draw_value_centered(
            &mut self.canvas,
            right_x + 16,
            CARD_TOP + 50,
            VALUE_MAX_WIDTH,
            &todo_count,
        );
    }

    #[allow(dead_code)]
    pub fn render_clock(&mut self, clock: &DateTime) {
        self.canvas.clear();
        self.draw_clock(clock);
    }

    fn draw_clock(&mut self, dt: &DateTime) {
        let time = format!("{:02}:{:02}", dt.hour, dt.minute);
        self.canvas.draw_text_prop(24, 44, 4, &time);

        let m_idx = (dt.month as usize).saturating_sub(1).min(11);
        let md = format!("{} {}", MONTH_NAMES[m_idx], dt.day);
        let year = format!("{}", dt.year);
        let md_w = Canvas::text_prop_width(&md, 2);
        let year_w = Canvas::text_prop_width(&year, 2);
        self.canvas.draw_text_prop(WIDTH - md_w - 24, 46, 2, &md);
        self.canvas
            .draw_text_prop(WIDTH - year_w - 24, 84, 2, &year);
    }

    pub fn refresh_full(&mut self) -> Result<()> {
        check_epd("power on EPD", unsafe { zectrix_epd_power_on(self.handle) })?;
        let refresh = check_epd("refresh EPD", unsafe {
            zectrix_epd_refresh_full_1bpp(
                self.handle,
                self.canvas.frame().as_ptr(),
                self.canvas.frame().len(),
            )
        });
        let power_off = check_epd("power off EPD", unsafe {
            zectrix_epd_power_off(self.handle)
        });
        refresh.and(power_off)
    }

    /// Always refreshes only `rect`, never promotes to a full refresh.
    /// The home clock ticks every ~1.2s so a periodic full flash every
    /// few seconds would be very visible; boot and alarm-ring still use
    /// `refresh_full` explicitly. Callers re-render the whole canvas
    /// before refreshing, so the partial rect always shows fresh pixels.
    pub fn refresh_partial(&mut self, rect: Rect) -> Result<()> {
        check_epd("power on EPD", unsafe { zectrix_epd_power_on(self.handle) })?;
        let pixels = self.canvas.pack_rect(rect);
        let c_rect = zectrix_epd_rect_t {
            x: rect.x as i32,
            y: rect.y as i32,
            width: rect.width as i32,
            height: rect.height as i32,
        };
        let refresh = check_epd("refresh EPD partial", unsafe {
            zectrix_epd_refresh_partial_1bpp(self.handle, &c_rect, pixels.as_ptr(), pixels.len())
        });
        let power_off = check_epd("power off EPD", unsafe {
            zectrix_epd_power_off(self.handle)
        });
        refresh.and(power_off)
    }
}

impl Drop for EpdDisplay {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                zectrix_epd_del(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Largest scale in `1..=max_scale` at which `text` fits within
/// `max_width` pixels, falling back to 1 (never drawn narrower, just
/// possibly clipped in a pathological case) if even that doesn't fit.
fn fit_scale(text: &str, max_width: usize, max_scale: usize) -> usize {
    (1..=max_scale)
        .rev()
        .find(|&scale| Canvas::text_prop_width(text, scale) <= max_width)
        .unwrap_or(1)
}

/// Draws `text` at the largest scale (up to 3) that fits `max_width`,
/// centered within that width starting at `x`. A single-digit todo count
/// and an 11-character one-shot alarm date both use this same card slot;
/// left-pinning both at `x` made the short one look orphaned against the
/// long one's near-full-width line, so the value is centered in the
/// available column instead - a short value now reads as "a number
/// deliberately centered in its card", not "text that happened to be
/// short".
fn draw_value_centered(canvas: &mut Canvas, x: usize, y: usize, max_width: usize, text: &str) {
    let scale = fit_scale(text, max_width, 3);
    let width = Canvas::text_prop_width(text, scale);
    canvas.draw_text_prop(x + (max_width.saturating_sub(width)) / 2, y, scale, text);
}

fn check_epd(operation: &str, result: i32) -> Result<()> {
    if result != 0 {
        bail!("{operation} failed with ESP-IDF error 0x{result:04x}");
    }
    Ok(())
}
