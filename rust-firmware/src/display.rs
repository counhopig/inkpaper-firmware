use anyhow::{bail, Result};
use esp_idf_svc::sys::zectrix_epd::{
    zectrix_epd_config_t, zectrix_epd_del, zectrix_epd_get_default_config, zectrix_epd_handle_t,
    zectrix_epd_new, zectrix_epd_power_off, zectrix_epd_power_on, zectrix_epd_rect_t,
    zectrix_epd_refresh_full_1bpp, zectrix_epd_refresh_partial_1bpp,
};

use crate::canvas::Canvas;
use crate::canvas::WIDTH;
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

    /// The idle/background screen: clock, next-alarm summary, pending-todo
    /// count. `main.rs` redraws this after returning from any modal screen
    /// (the navigation drawer, settings menu, alarm ring).
    pub fn render_home(
        &mut self,
        clock: Option<&DateTime>,
        next_alarm: Option<&str>,
        todo_pending: usize,
    ) {
        self.canvas.clear();
        self.canvas.draw_text_prop(16, 8, 1, "INKPAPER");
        self.canvas.draw_text_prop(316, 8, 1, "NOTE 4");
        self.canvas.fill_rect(16, 29, 368, 1, true);
        if let Some(dt) = clock {
            self.draw_clock(dt);
        } else {
            let dash_w = Canvas::text_prop_width("--:--", 3);
            let dash_x = (WIDTH - dash_w) / 2;
            self.canvas.draw_text_prop(dash_x, 52, 3, "--:--");
        }

        self.canvas.stroke_rect(16, 139, 176, 94, 2);
        self.canvas.fill_rect(16, 139, 5, 94, true);
        self.canvas.draw_text_prop(32, 153, 1, "NEXT ALARM");
        self.canvas
            .draw_text_prop(32, 181, 2, next_alarm.unwrap_or("NONE"));

        self.canvas.stroke_rect(208, 139, 176, 94, 2);
        self.canvas.fill_rect(208, 139, 5, 94, true);
        self.canvas.draw_text_prop(224, 153, 1, "OPEN TODOS");
        self.canvas
            .draw_text_prop(224, 181, 2, &todo_pending.to_string());
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

fn check_epd(operation: &str, result: i32) -> Result<()> {
    if result != 0 {
        bail!("{operation} failed with ESP-IDF error 0x{result:04x}");
    }
    Ok(())
}
