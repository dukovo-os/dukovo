// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Metadata's expiry times, `YYYY-MM-DDTHH:MM:SSZ` in UTC as TUF writes
//! them, as seconds since 1970.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in seconds since 1970 (UTC).
#[must_use]
pub fn now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// The seconds since 1970 of `text`, a time in TUF's form; `None` if it
/// is not one.
#[must_use]
pub fn parse(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    let field = |from: usize, to: usize| -> Option<i64> {
        let digits = &text[from..to];
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day < 1 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar
/// (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse_to_seconds() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse("2000-03-01T00:00:01Z"), Some(951_868_801));
        assert_eq!(parse("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(parse("2038-01-19T03:14:08Z"), Some(1 << 31));
        assert_eq!(parse("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn other_forms_do_not() {
        for text in [
            "2023-02-29T00:00:00Z",
            "2024-13-01T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01 00:00:00Z",
            "2024-01-01T00:00:00",
            "2024-01-01T00:00:00+00:00",
            "2024-01-01T00:00:00.5Z",
            "+024-01-01T00:00:00Z",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        assert!(now() > parse("2026-01-01T00:00:00Z").unwrap());
    }
}
