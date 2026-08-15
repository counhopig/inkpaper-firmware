use anyhow::{bail, Result};
use esp_idf_svc::sys::zectrix_epd::{
    zectrix_epd_config_t, zectrix_epd_del, zectrix_epd_get_default_config, zectrix_epd_handle_t,
    zectrix_epd_new, zectrix_epd_power_off, zectrix_epd_power_on, zectrix_epd_rect_t,
    zectrix_epd_refresh_full_1bpp, zectrix_epd_refresh_partial_1bpp,
};

use crate::canvas::Canvas;
use crate::rtc::DateTime;

pub use crate::canvas::Rect;

pub struct ButtonCounts {
    pub enter: u32,
    pub up: u32,
    pub down: u32,
}

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

    #[allow(dead_code)]
    pub fn render(&mut self, counts: &ButtonCounts) {
        self.render_with_time(counts, None);
    }

    /// Direct canvas access for screens that don't fit the fixed
    /// `render_with_time` layout, e.g. the Wi-Fi setup wizard.
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        &mut self.canvas
    }

    pub fn render_with_time(&mut self, counts: &ButtonCounts, clock: Option<&DateTime>) {
        self.canvas.clear();
        if let Some(dt) = clock {
            self.draw_clock(dt);
        }
        self.canvas.draw_text(52, 34, 4, "Hello world");
        self.canvas.fill_rect(32, 82, 336, 3, true);
        self.draw_count_row(108, "ENTER", counts.enter);
        self.draw_count_row(166, "UP", counts.up);
        self.draw_count_row(224, "DOWN", counts.down);
    }

    /// Draws one `LABEL ... N` row of the button-count list. Label and value
    /// share the fixed columns used throughout `render_with_time`.
    fn draw_count_row(&mut self, y: usize, label: &str, count: u32) {
        self.canvas.draw_text(36, y, 3, label);
        self.canvas.draw_text(255, y, 3, &count.to_string());
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
            zectrix_epd_refresh_partial_1bpp(
                self.handle,
                &c_rect,
                pixels.as_ptr(),
                pixels.len(),
            )
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
