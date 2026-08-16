use anyhow::{anyhow, Result};
use esp_idf_svc::sys::TickType_t;

use crate::board::SharedI2c;

pub const PCF8563_ADDR: u8 = 0x51;
const I2C_TIMEOUT_TICKS: TickType_t = 100; // 100 ms RTOS ticks

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

impl DateTime {
    #[allow(dead_code)]
    pub fn from_unix(epoch: u64) -> Self {
        let secs = (epoch % 86400) as u32;
        let mut days = (epoch / 86400) as i64;
        let mut year: i64 = 1970;
        loop {
            let leap = is_leap(year);
            let dy = if leap { 366 } else { 365 };
            if days < dy {
                break;
            }
            days -= dy;
            year += 1;
        }
        let leap = is_leap(year);
        let month_days = if leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut month = 1;
        for &dm in &month_days {
            if days < dm {
                break;
            }
            days -= dm;
            month += 1;
        }
        let day = days as u8 + 1;
        let hour = (secs / 3600) as u8;
        let minute = ((secs % 3600) / 60) as u8;
        let second = (secs % 60) as u8;
        let weekday = ((epoch / 86400 + 3) % 7) as u8; // 1970-01-01 was Thursday
        Self {
            year: year as u16,
            month,
            day,
            weekday,
            hour,
            minute,
            second,
            voltage_low: false,
        }
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn bcd_to_bin(b: u8) -> u8 {
    ((b >> 4) & 0x0F) * 10 + (b & 0x0F)
}

#[allow(dead_code)]
fn bin_to_bcd(b: u8) -> u8 {
    ((b / 10) << 4) | (b % 10)
}

pub struct Pcf8563 {
    bus: SharedI2c,
    addr: u8,
}

impl Pcf8563 {
    pub fn new(bus: SharedI2c, addr: u8) -> Self {
        Self { bus, addr }
    }

    fn read_regs(&mut self, reg: u8, out: &mut [u8]) -> Result<()> {
        self.bus
            .borrow_mut()
            .write_read(self.addr, &[reg], out, I2C_TIMEOUT_TICKS)
            .map_err(|e| anyhow!("PCF8563 read regs 0x{reg:02x} failed: {e}"))
    }

    fn write_regs(&mut self, start_reg: u8, bytes: &[u8]) -> Result<()> {
        let mut buf = [0u8; 16];
        if bytes.len() + 1 > buf.len() {
            return Err(anyhow!(
                "PCF8563 write too long: {} bytes (max {})",
                bytes.len() + 1,
                buf.len()
            ));
        }
        buf[0] = start_reg;
        buf[1..=bytes.len()].copy_from_slice(bytes);
        self.bus
            .borrow_mut()
            .write(self.addr, &buf[..=bytes.len()], I2C_TIMEOUT_TICKS)
            .map_err(|e| anyhow!("PCF8563 write regs 0x{start_reg:02x} failed: {e}"))
    }

    pub fn probe(&mut self) -> Result<()> {
        let mut buf = [0u8; 1];
        self.read_regs(0x00, &mut buf)?;
        Ok(())
    }

    pub fn read_time(&mut self) -> Result<DateTime> {
        let mut buf = [0u8; 7];
        self.read_regs(0x02, &mut buf)?;
        let voltage_low = buf[0] & 0x80 != 0;
        Ok(DateTime {
            second: bcd_to_bin(buf[0] & 0x7F),
            minute: bcd_to_bin(buf[1] & 0x7F),
            hour: bcd_to_bin(buf[2] & 0x3F),
            day: bcd_to_bin(buf[3] & 0x3F),
            weekday: buf[4] & 0x07,
            month: bcd_to_bin(buf[5] & 0x1F),
            year: 2000 + bcd_to_bin(buf[6]) as u16,
            voltage_low,
        })
    }

    #[allow(dead_code)]
    pub fn write_time(&mut self, dt: &DateTime) -> Result<()> {
        let year_offset = (dt.year % 100) as u8;
        let payload = [
            bin_to_bcd(dt.second) & 0x7F,
            bin_to_bcd(dt.minute) & 0x7F,
            bin_to_bcd(dt.hour) & 0x3F,
            bin_to_bcd(dt.day) & 0x3F,
            dt.weekday & 0x07,
            bin_to_bcd(dt.month) & 0x1F,
            bin_to_bcd(year_offset),
        ];
        self.write_regs(0x02, &payload)?;
        let mut ctrl = [0u8; 1];
        self.read_regs(0x00, &mut ctrl)?;
        let cleared = ctrl[0] & !0x80;
        self.write_regs(0x00, &[cleared])?;
        Ok(())
    }

    pub fn clear_alarm(&mut self) -> Result<()> {
        self.write_regs(0x09, &[0x80, 0x80, 0x80, 0x80])?;
        let mut ctrl2 = [0u8; 1];
        self.read_regs(0x01, &mut ctrl2)?;
        self.write_regs(0x01, &[ctrl2[0] & !0x08])?;
        Ok(())
    }
}