use anyhow::{bail, Result};
use esp_idf_svc::sys::zectrix_epd::{
    zectrix_epd_config_t, zectrix_epd_del, zectrix_epd_get_default_config, zectrix_epd_handle_t,
    zectrix_epd_new, zectrix_epd_power_off, zectrix_epd_power_on, zectrix_epd_rect_t,
    zectrix_epd_refresh_full_1bpp, zectrix_epd_refresh_partial_1bpp,
};

use crate::board::ChargeSnapshot;
use crate::canvas::Canvas;
use crate::home;
use crate::rtc::DateTime;

pub use crate::canvas::Rect;

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
    /// summary (time, countdown), and a todos summary (open count,
    /// due-today count). `main.rs` redraws this after returning from any
    /// modal screen (the navigation drawer, settings menu, alarm ring).
    /// Layout lives in `home::render` so the same pixels can be previewed
    /// on a PC; this only hands it the canvas.
    #[allow(clippy::too_many_arguments)]
    pub fn render_home(
        &mut self,
        clock: Option<&DateTime>,
        next_alarm_time: Option<&str>,
        next_alarm_date: Option<&str>,
        next_alarm_days_left: Option<i64>,
        todo_pending: usize,
        todo_due_today: usize,
        unread_inbox: usize,
        wifi_configured: bool,
        battery_percent: Option<u8>,
        charge: ChargeSnapshot,
    ) {
        home::render(
            &mut self.canvas,
            clock,
            next_alarm_time,
            next_alarm_date,
            next_alarm_days_left,
            todo_pending,
            todo_due_today,
            unread_inbox,
            wifi_configured,
            battery_percent,
            charge,
        );
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
    /// The RTC is sampled frequently but the visible home clock refreshes
    /// only when its displayed minute changes; boot and alarm-ring still use
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

    /// UI screens are best-effort callers: a display fault must not silently
    /// disappear, but it also must not stop buttons, alarms, or the watchdog.
    pub fn refresh_full_best_effort(&mut self) {
        if let Err(err) = self.refresh_full() {
            log::error!("EPD full refresh failed: {err}");
        }
    }

    /// Logs a partial-refresh failure and attempts one full refresh using the
    /// already-rendered canvas. This is the common recovery path for a panel
    /// that lost partial-update state without turning every UI loop into an
    /// error-propagation tree.
    pub fn refresh_partial_best_effort(&mut self, rect: Rect) {
        if let Err(err) = self.refresh_partial(rect) {
            log::warn!("EPD partial refresh failed; trying full refresh: {err}");
            if let Err(full_err) = self.refresh_full() {
                log::error!("EPD recovery full refresh failed: {full_err}");
            }
        }
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
