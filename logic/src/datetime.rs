//! Calendar/epoch math, moved verbatim out of `rust-firmware/src/rtc.rs`
//! (which re-exports `DateTime`/`is_leap` from here so nothing else has to
//! change). Kept separate from that file's PCF8563 I2C code specifically so
//! it has no hardware dependency and can be unit-tested on the host.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    /// 0=Sunday..6=Saturday.
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub voltage_low: bool,
}

impl DateTime {
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
        // 1970-01-01 was a Thursday (0=Sunday..6=Saturday convention - see
        // `docs/remaining-work.md` item 0: this constant was `3` for a long
        // time, which put every weekday-derived feature a day off).
        let weekday = ((epoch / 86400 + 4) % 7) as u8;
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

    pub fn to_unix(self) -> u64 {
        let mut days = 0u64;
        for year in 1970..self.year as i64 {
            days += if is_leap(year) { 366 } else { 365 };
        }
        let month_days = if is_leap(self.year as i64) {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        days += month_days
            .iter()
            .take(self.month.saturating_sub(1) as usize)
            .map(|days| *days as u64)
            .sum::<u64>();
        days += self.day.saturating_sub(1) as u64;
        days * 86_400 + self.hour as u64 * 3_600 + self.minute as u64 * 60 + self.second as u64
    }

    pub fn shifted_minutes(self, minutes: i32) -> Self {
        let shifted = (self.to_unix() as i64 + minutes as i64 * 60).max(0) as u64;
        Self::from_unix(shifted)
    }
}

pub fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent ground truth for weekday, deliberately *not* sharing any
    /// code with `DateTime::from_unix` - the item-0 bug (weekday off by one
    /// everywhere) would have passed a test that reused the same `+3`/`+4`
    /// formula under test. Zeller's congruence (Gregorian form); returns
    /// 0=Sunday..6=Saturday to match this codebase's convention.
    fn zeller_weekday(year: i64, month: i64, day: i64) -> u8 {
        let (y, m) = if month < 3 {
            (year - 1, month + 12)
        } else {
            (year, month)
        };
        let k = y.rem_euclid(100);
        let j = y.div_euclid(100);
        let h = (day + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
        // Zeller's h: 0=Saturday,1=Sunday,...,6=Friday - shift to 0=Sunday.
        ((h + 6) % 7) as u8
    }

    #[test]
    fn zeller_matches_known_anchors() {
        // Sanity-check the reference implementation itself against widely
        // known anchor dates before trusting it as ground truth below.
        assert_eq!(zeller_weekday(1970, 1, 1), 4, "1970-01-01 was a Thursday");
        assert_eq!(zeller_weekday(2000, 1, 1), 6, "2000-01-01 was a Saturday");
        assert_eq!(zeller_weekday(2024, 1, 1), 1, "2024-01-01 was a Monday");
    }

    #[test]
    fn from_unix_epoch_is_thursday() {
        assert_eq!(DateTime::from_unix(0).weekday, 4);
    }

    #[test]
    fn weekday_matches_zeller_across_a_wide_date_range() {
        // Sweep a spread of dates - including leap-year Februaries, month
        // and year boundaries, and a century mark - checking every 37 days
        // (coprime with 7) so the sampled weekdays cover all seven values
        // rather than always landing on the same one.
        let mut days: i64 = 0; // 1970-01-01
        let end_days: i64 = 60 * 365 + 15; // ~through 2030
        while days < end_days {
            let dt = DateTime::from_unix((days * 86_400) as u64);
            let expected = zeller_weekday(dt.year as i64, dt.month as i64, dt.day as i64);
            assert_eq!(
                dt.weekday, expected,
                "weekday mismatch at {:04}-{:02}-{:02} (day offset {days})",
                dt.year, dt.month, dt.day
            );
            days += 37;
        }
    }

    #[test]
    fn known_bug_report_date_is_saturday() {
        // 2026-08-22: the date `docs/remaining-work.md` records as the
        // physical-hardware reproduction of the weekday bug ("device showed
        // Friday on a Saturday"). Locks in the fix for that exact report.
        let epoch = DateTime {
            year: 2026,
            month: 8,
            day: 22,
            ..Default::default()
        }
        .to_unix();
        assert_eq!(DateTime::from_unix(epoch).weekday, 6, "2026-08-22 is a Saturday");
    }

    #[test]
    fn to_unix_from_unix_roundtrip() {
        for epoch in [0u64, 86_400, 1_700_000_000, 1_900_000_000] {
            let dt = DateTime::from_unix(epoch);
            assert_eq!(dt.to_unix(), epoch, "roundtrip failed for epoch {epoch}");
        }
    }

    #[test]
    fn leap_year_rules() {
        assert!(is_leap(2000)); // divisible by 400
        assert!(!is_leap(1900)); // divisible by 100, not 400
        assert!(is_leap(2024)); // divisible by 4, not 100
        assert!(!is_leap(2023));
    }

    #[test]
    fn shifted_minutes_crosses_midnight_and_year_boundary() {
        let new_years_eve_2359 = DateTime {
            year: 2025,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            ..Default::default()
        };
        let shifted = new_years_eve_2359.shifted_minutes(2);
        assert_eq!((shifted.year, shifted.month, shifted.day), (2026, 1, 1));
        assert_eq!((shifted.hour, shifted.minute), (0, 1));
    }

    #[test]
    fn shifted_minutes_never_underflows_before_epoch() {
        let near_epoch = DateTime::from_unix(30);
        let shifted = near_epoch.shifted_minutes(-10);
        assert_eq!(shifted.to_unix(), 0);
    }
}
