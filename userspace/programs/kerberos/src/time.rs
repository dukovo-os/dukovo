// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Time as Kerberos keeps it: seconds since 1970 in UTC, written as
//! GeneralizedTime (`YYYYMMDDHHMMSSZ`), and read from lifetimes such as
//! `10h` or `7d` in krb5.conf and on kinit's command line.
//!
//! A KDC refuses a request whose time is more than five minutes from its
//! own (KRB_AP_ERR_SKEW), so a wrong clock shows up here first; timed(8)
//! keeps the clock right.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now: seconds since 1970 and the microseconds past them.
pub fn now() -> (i64, u32) {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (since.as_secs() as i64, since.subsec_micros())
}

/// The days since 1970-01-01 of a date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of a day since 1970-01-01: year, month and day.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `seconds` since 1970 as `YYYYMMDDHHMMSSZ`.
pub fn generalized(seconds: i64) -> String {
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let in_day = seconds.rem_euclid(86_400);
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        in_day / 3600,
        in_day / 60 % 60,
        in_day % 60
    )
}

/// `YYYYMMDDHHMMSSZ` as seconds since 1970.
pub fn parse_generalized(text: &str) -> Option<i64> {
    let digits = text.strip_suffix('Z')?;
    if digits.len() != 14 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |from: usize, to: usize| digits[from..to].parse::<u32>().ok();
    let (month, day) = (part(4, 6)?, part(6, 8)?);
    let (hour, minute, second) = (part(8, 10)?, part(10, 12)?, part(12, 14)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(i64::from(part(0, 4)?), month, day);
    Some(days * 86_400 + i64::from(hour * 3600 + minute * 60 + second))
}

/// `seconds` since 1970 as a local reader sees it in klist's listing:
/// `YYYY-MM-DD HH:MM:SS` in UTC, which says so.
pub fn shown(seconds: i64) -> String {
    let g = generalized(seconds);
    format!(
        "{}-{}-{} {}:{}:{} UTC",
        &g[0..4],
        &g[4..6],
        &g[6..8],
        &g[8..10],
        &g[10..12],
        &g[12..14]
    )
}

/// A lifetime as krb5.conf and kinit write it, in seconds: a number of
/// seconds, numbers each followed by `d`, `h`, `m` or `s` (`1d12h`), or
/// `HH:MM[:SS]`.
pub fn parse_duration(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(seconds) = text.parse::<i64>() {
        return (seconds >= 0).then_some(seconds);
    }
    if text.contains(':') {
        let parts: Vec<i64> = text
            .split(':')
            .map(|p| p.parse().ok().filter(|&n: &i64| n >= 0))
            .collect::<Option<_>>()?;
        return match parts[..] {
            [h, m] if m < 60 => Some(h * 3600 + m * 60),
            [h, m, s] if m < 60 && s < 60 => Some(h * 3600 + m * 60 + s),
            _ => None,
        };
    }
    let mut total = 0i64;
    let mut number = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let unit = match c {
            'd' => 86_400,
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        total = total.checked_add(number.parse::<i64>().ok()?.checked_mul(unit)?)?;
        number.clear();
    }
    number.is_empty().then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generalized_time_reads_back() {
        assert_eq!(generalized(0), "19700101000000Z");
        assert_eq!(generalized(951_782_400), "20000229000000Z");
        assert_eq!(generalized(4_102_444_799), "20991231235959Z");
        for t in [0, 951_782_400, 1_700_000_000, 4_102_444_799, 1_234_567_890] {
            assert_eq!(parse_generalized(&generalized(t)), Some(t));
        }
        assert_eq!(parse_generalized("20261106083000"), None);
        assert_eq!(parse_generalized("20261306083000Z"), None);
    }

    #[test]
    fn lifetimes_read_as_krb5_conf_writes_them() {
        assert_eq!(parse_duration("36000"), Some(36_000));
        assert_eq!(parse_duration("10h"), Some(36_000));
        assert_eq!(parse_duration("1d12h"), Some(129_600));
        assert_eq!(parse_duration("7d"), Some(604_800));
        assert_eq!(parse_duration("90m"), Some(5400));
        assert_eq!(parse_duration("10:30"), Some(37_800));
        assert_eq!(parse_duration("1:00:30"), Some(3630));
        assert_eq!(parse_duration("10x"), None);
        assert_eq!(parse_duration("h"), None);
        assert_eq!(parse_duration("10h3"), None);
    }

    #[test]
    fn shown_times_say_utc() {
        assert_eq!(shown(1_000_000_000), "2001-09-09 01:46:40 UTC");
    }
}
