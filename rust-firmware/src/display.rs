use anyhow::{bail, Result};
use esp_idf_svc::sys::zectrix_epd::{
    zectrix_epd_config_t, zectrix_epd_del, zectrix_epd_get_default_config, zectrix_epd_handle_t,
    zectrix_epd_new, zectrix_epd_power_off, zectrix_epd_power_on, zectrix_epd_rect_t,
    zectrix_epd_refresh_full_1bpp, zectrix_epd_refresh_partial_1bpp,
};

use crate::canvas::Canvas;
use crate::rtc::DateTime;

pub use crate::canvas::Rect;

/// Consecutive partial refreshes allowed before `refresh_partial` silently
/// promotes to a full refresh instead. Matches the policy in the upstream
/// ZECTRIX demo's `docs/UI_FLOW.md` ("After eight UI partial refreshes, the
/// next update is promoted to full refresh") - this repo's screens do a lot
/// more partial-refresh churn (clock ticking every ~1.2s) than the original
/// counter-demo baseline did, so ghosting control matters more here.
const PARTIAL_REFRESH_PROMOTE_LIMIT: u32 = 8;

pub struct EpdDisplay {
    handle: zectrix_epd_handle_t,
    canvas: Canvas,
    partial_refresh_count: u32,
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
            partial_refresh_count: 0,
        })
    }

    /// Direct canvas access for screens that don't fit the fixed
    /// `render_home` layout, e.g. the Wi-Fi setup wizard and `screens.rs`.
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        &mut self.canvas
    }

    /// The idle/background screen: clock, next-alarm summary, pending-todo
    /// count. `main.rs` redraws this after returning from any modal screen
    /// (`screens::open_menu`, the Wi-Fi wizard, an alarm ring).
    pub fn render_home(
        &mut self,
        clock: Option<&DateTime>,
        next_alarm: Option<&str>,
        todo_pending: usize,
    ) {
        self.canvas.clear();
        if let Some(dt) = clock {
            self.draw_clock(dt);
        }
        self.canvas.draw_text_prop(20, 60, 3, "INKPAPER");
        let alarm_line = match next_alarm {
            Some(label) => format!("NEXT ALARM: {label}"),
            None => "NEXT ALARM: NONE".to_string(),
        };
        self.canvas.draw_text_prop(20, 120, 1, &alarm_line);
        self.canvas
            .draw_text_prop(20, 140, 1, &format!("TODOS PENDING: {todo_pending}"));
        self.canvas
            .draw_text_prop(20, 280, 1, "ENTER=MENU  HOLD UP=SETUP  HOLD DOWN=SLEEP");
    }

    #[allow(dead_code)]
    pub fn render_clock(&mut self, clock: &DateTime) {
        self.canvas.clear();
        self.draw_clock(clock);
    }

    fn draw_clock(&mut self, dt: &DateTime) {
        let date = format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day);
        let time = format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second);
        self.canvas.draw_text(20, 4, 1, &date);
        self.canvas.draw_text(20, 14, 2, &time);
        let status = if dt.voltage_low { "LOW!" } else { "OK" };
        self.canvas.draw_text(260, 8, 1, &format!("RTC {status}"));
    }

    pub fn refresh_full(&mut self) -> Result<()> {
        self.partial_refresh_count = 0;
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

    /// Silently promotes to a full refresh after
    /// `PARTIAL_REFRESH_PROMOTE_LIMIT` consecutive partial ones, to bound
    /// ghosting. Safe to call as often as `refresh_full` itself: every
    /// caller in this codebase already re-renders the whole canvas (see
    /// e.g. `main.rs::render_home_now`) before calling either refresh
    /// method, so promoting mid-call still draws the correct full screen,
    /// not a stale one. If a single loop iteration ever calls this for
    /// several rects at once, promotion on an early rect will trigger a
    /// full refresh per remaining rect too (redundant, not incorrect) -
    /// not a concern today since no caller passes more than one dirty rect
    /// per iteration, but worth knowing if that changes.
    pub fn refresh_partial(&mut self, rect: Rect) -> Result<()> {
        if self.partial_refresh_count >= PARTIAL_REFRESH_PROMOTE_LIMIT {
            return self.refresh_full();
        }
        self.partial_refresh_count += 1;
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

fn check_epd(operation: &str, result: i32) -> Result<()> {
    if result != 0 {
        bail!("{operation} failed with ESP-IDF error 0x{result:04x}");
    }
    Ok(())
}
