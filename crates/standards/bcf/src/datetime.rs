//! `xs:dateTime` for BCF: formatting "now" and checking the lexical form of dates read from files.
//!
//! BCF dates are kept as the text they were written with (`2026-03-02T08:15:30+01:00` stays as is),
//! so nothing here parses a date into an instant; [`is_xs_datetime`] only decides whether a string
//! is a valid `xs:dateTime`, which is what the BCF schemas demand of every date.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time as `YYYY-MM-DDThh:mm:ssZ` (UTC); the Unix epoch if the clock is before it.
pub fn now_utc() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_utc(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// `unix_secs` (seconds since 1970-01-01T00:00:00Z) as `YYYY-MM-DDThh:mm:ssZ`; times outside the
/// years 0001 to 9999 are clamped to them.
pub fn format_utc(unix_secs: i64) -> String {
    const MIN: i64 = -62_135_596_800; // 0001-01-01T00:00:00Z
    const MAX: i64 = 253_402_300_799; // 9999-12-31T23:59:59Z
    let secs = unix_secs.clamp(MIN, MAX);
    let days = secs.div_euclid(86_400);
    let second_of_day = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        second_of_day % 3600 / 60,
        second_of_day % 60
    )
}

/// A cursor over the bytes of a date string.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    /// Exactly `n` ASCII digits as a number.
    fn digits(&mut self, n: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            let digit = self.peek().filter(u8::is_ascii_digit)?;
            value = value * 10 + u32::from(digit - b'0');
            self.at += 1;
        }
        Some(value)
    }
}

fn days_in_month(year: u64, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Whether `s` (not trimmed: surrounding whitespace makes it invalid) is a valid `xs:dateTime`:
/// `[-]YYYY[Y…]-MM-DDThh:mm:ss[.s+][Z|(+|-)hh:mm]`, with a real calendar date, hour 00-23 (or
/// `24:00:00`), minute and second below 60 and a time zone offset of at most 14:00. Year 0000 does
/// not exist.
pub fn is_xs_datetime(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut cur = Cursor { bytes, at: 0 };
    cur.eat(b'-');
    let year_start = cur.at;
    while cur.peek().is_some_and(|b| b.is_ascii_digit()) {
        cur.at += 1;
    }
    let Some(year_digits) = bytes.get(year_start..cur.at) else { return false };
    if year_digits.len() < 4 || year_digits.len() > 18 {
        return false;
    }
    if year_digits.len() > 4 && year_digits.first() == Some(&b'0') {
        return false;
    }
    let year = year_digits.iter().fold(0u64, |acc, d| acc * 10 + u64::from(d - b'0'));
    if year == 0 {
        return false;
    }
    let ok = (|| {
        if !cur.eat(b'-') {
            return None;
        }
        let month = cur.digits(2).filter(|m| (1..=12).contains(m))?;
        if !cur.eat(b'-') {
            return None;
        }
        cur.digits(2).filter(|d| (1..=days_in_month(year, month)).contains(d))?;
        if !cur.eat(b'T') {
            return None;
        }
        let hour = cur.digits(2).filter(|h| *h <= 24)?;
        if !cur.eat(b':') {
            return None;
        }
        let minute = cur.digits(2).filter(|m| *m < 60)?;
        if !cur.eat(b':') {
            return None;
        }
        let second = cur.digits(2).filter(|s| *s < 60)?;
        let mut fraction_is_zero = true;
        if cur.eat(b'.') {
            let start = cur.at;
            while let Some(d) = cur.peek().filter(u8::is_ascii_digit) {
                fraction_is_zero &= d == b'0';
                cur.at += 1;
            }
            if cur.at == start {
                return None;
            }
        }
        if hour == 24 && !(minute == 0 && second == 0 && fraction_is_zero) {
            return None;
        }
        if cur.eat(b'Z') {
        } else if cur.peek().is_some_and(|b| b == b'+' || b == b'-') {
            cur.at += 1;
            let offset_hours = cur.digits(2).filter(|h| *h <= 14)?;
            if !cur.eat(b':') {
                return None;
            }
            let offset_minutes = cur.digits(2).filter(|m| *m < 60)?;
            if offset_hours == 14 && offset_minutes != 0 {
                return None;
            }
        }
        Some(())
    })();
    ok.is_some() && cur.at == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_709_210_096), "2024-02-29T12:34:56Z");
        assert_eq!(format_utc(-1), "1969-12-31T23:59:59Z");
        assert_eq!(format_utc(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(format_utc(i64::MAX), "9999-12-31T23:59:59Z");
        assert_eq!(format_utc(i64::MIN), "0001-01-01T00:00:00Z");
    }

    #[test]
    fn now_is_a_valid_datetime() {
        let now = now_utc();
        assert!(is_xs_datetime(&now), "{now}");
        assert!(now.starts_with("20") && now.ends_with('Z'), "{now}");
    }

    #[test]
    fn accepts_valid_lexical_forms() {
        for ok in [
            "2026-03-01T09:30:00+00:00",
            "2026-03-01T09:30:00Z",
            "2026-03-02T08:15:30.123+01:00",
            "2026-03-02T08:15:30",
            "2026-03-02T08:15:30.5",
            "2024-02-29T23:59:59-14:00",
            "2000-02-29T00:00:00Z",
            "2026-12-31T24:00:00Z",
            "2026-12-31T24:00:00.000Z",
            "12345-01-01T00:00:00Z",
            "-0044-03-15T12:00:00Z",
            "2026-03-01T09:30:00+14:00",
        ] {
            assert!(is_xs_datetime(ok), "{ok}");
        }
    }

    #[test]
    fn rejects_invalid_lexical_forms() {
        for bad in [
            "",
            "T",
            "2026-02-30T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2026-1-01T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-00-01T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "2026-01-32T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-03-01 09:30:00Z",
            "2026-03-01",
            "2026-03-01T09:30Z",
            "2026-03-01T24:00:01",
            "2026-03-01T24:01:00",
            "2026-03-01T25:00:00",
            "2026-03-01T09:60:00",
            "2026-03-01T09:30:60",
            "2026-03-01T09:30:00.",
            "2026-03-01T09:30:00+15:00",
            "2026-03-01T09:30:00+14:30",
            "2026-03-01T09:30:00+01",
            "2026-03-01T09:30:00+01:60",
            "2026-03-01T09:30:00z",
            "2026-03-01T09:30:00Zjunk",
            " 2026-03-01T09:30:00Z",
            "2026-03-01T09:30:00Z ",
            "0000-01-01T00:00:00Z",
            "02026-01-01T00:00:00Z",
            "202-01-01T00:00:00Z",
            "2026-03-01T09:30:00,5Z",
            "２０２６-03-01T09:30:00Z",
            "2026-03-01T09:30:00+0100",
        ] {
            assert!(!is_xs_datetime(bad), "{bad:?}");
        }
    }
}
