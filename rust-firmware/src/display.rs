use anyhow::{bail, Result};
use esp_idf_svc::sys::zectrix_epd::{
    zectrix_epd_config_t, zectrix_epd_del, zectrix_epd_get_default_config, zectrix_epd_handle_t,
    zectrix_epd_new, zectrix_epd_power_off, zectrix_epd_power_on, zectrix_epd_rect_t,
    zectrix_epd_refresh_full_1bpp, zectrix_epd_refresh_partial_1bpp,
};

use crate::rtc::DateTime;

pub const WIDTH: usize = 400;
pub const HEIGHT: usize = 300;
const BYTES_PER_ROW: usize = WIDTH / 8;
const FRAME_SIZE: usize = BYTES_PER_ROW * HEIGHT;

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub struct ButtonCounts {
    pub enter: u32,
    pub up: u32,
    pub down: u32,
}

pub struct EpdDisplay {
    handle: zectrix_epd_handle_t,
    frame: Vec<u8>,
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
            frame: vec![0xFF; FRAME_SIZE],
        })
    }

    #[allow(dead_code)]
    pub fn render(&mut self, counts: &ButtonCounts) {
        self.render_with_time(counts, None);
    }

    pub fn render_with_time(&mut self, counts: &ButtonCounts, clock: Option<&DateTime>) {
        self.clear();
        if let Some(dt) = clock {
            self.draw_clock(dt);
        }
        self.draw_text(52, 34, 4, "Hello world");
        self.fill_rect(32, 82, 336, 3, true);
        self.draw_text(36, 108, 3, "ENTER");
        self.draw_text(255, 108, 3, &counts.enter.to_string());
        self.draw_text(36, 166, 3, "UP");
        self.draw_text(255, 166, 3, &counts.up.to_string());
        self.draw_text(36, 224, 3, "DOWN");
        self.draw_text(255, 224, 3, &counts.down.to_string());
    }

    #[allow(dead_code)]
    pub fn render_clock(&mut self, clock: &DateTime) {
        self.clear();
        self.draw_clock(clock);
    }

    fn draw_clock(&mut self, dt: &DateTime) {
        let date = format!(
            "{:04}-{:02}-{:02}",
            dt.year,
            dt.month,
            dt.day
        );
        let time = format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second);
        self.draw_text(20, 4, 1, &date);
        self.draw_text(20, 14, 2, &time);
        let mut status = String::from("OK");
        if dt.voltage_low {
            status = "LOW!".to_string();
        }
        self.draw_text(260, 8, 1, &format!("RTC {}", status));
    }

    pub fn refresh_full(&mut self) -> Result<()> {
        check_epd("power on EPD", unsafe { zectrix_epd_power_on(self.handle) })?;
        let refresh = check_epd("refresh EPD", unsafe {
            zectrix_epd_refresh_full_1bpp(self.handle, self.frame.as_ptr(), self.frame.len())
        });
        let power_off = check_epd("power off EPD", unsafe {
            zectrix_epd_power_off(self.handle)
        });
        refresh.and(power_off)
    }

    pub fn refresh_partial(&mut self, rect: Rect) -> Result<()> {
        check_epd("power on EPD", unsafe { zectrix_epd_power_on(self.handle) })?;
        let pixels = self.pack_rect(rect);
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

    fn pixel_is_white(&self, x: usize, y: usize) -> bool {
        let index = y * BYTES_PER_ROW + x / 8;
        let mask = 1 << (7 - (x & 7));
        self.frame[index] & mask != 0
    }

    fn pack_rect(&self, rect: Rect) -> Vec<u8> {
        let row_bytes = (rect.width as usize + 7) / 8;
        let mut packed = vec![0u8; row_bytes * rect.height as usize];
        for (row, y) in (rect.y..rect.y + rect.height).enumerate() {
            for (column, x) in (rect.x..rect.x + rect.width).enumerate() {
                if self.pixel_is_white(x as usize, y as usize) {
                    packed[row * row_bytes + column / 8] |= 1 << (7 - (column & 7));
                }
            }
        }
        packed
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

    fn draw_text(&mut self, x: usize, y: usize, scale: usize, text: &str) {
        let mut cursor = x;
        for character in text.chars() {
            let glyph = glyph(character);
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) != 0 {
                        self.fill_rect(
                            cursor + column * scale,
                            y + row * scale,
                            scale,
                            scale,
                            true,
                        );
                    }
                }
            }
            cursor += 6 * scale;
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

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        ':' => [0x00, 0x00, 0x0A, 0x00, 0x0A, 0x00, 0x00],
        '/' => [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x00],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        ' ' => [0; 7],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x14, 0x04, 0x04, 0x04, 0x1F],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x10, 0x1E, 0x01, 0x01, 0x1E],
        '6' => [0x0E, 0x10, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],
        _ => [0; 7],
    }
}
