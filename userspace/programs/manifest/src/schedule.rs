// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Times of day as a crontab line's five fields have them (crontab(5),
//! ADR 0478), for a service file's `[timer]` and a person's crontab: one
//! syntax for both.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A crontab line's five fields, read: the minutes, hours, days of the
/// month, months and days of the week a time matches, each a set of bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calendar {
    /// Bit m for minute m, 0 to 59.
    pub minutes: u64,
    /// Bit h for hour h, 0 to 23.
    pub hours: u32,
    /// Bit d for day d of the month, 1 to 31.
    pub days: u32,
    /// Bit m for month m, 1 to 12.
    pub months: u16,
    /// Bit d for day d of the week, 0 (Sunday) to 6; a 7 in a line is
    /// Sunday too.
    pub weekdays: u8,
    /// Whether the day-of-the-month field was `*` (or a `*` with a step),
    /// which decides how the two day fields combine: when neither was,
    /// a day matches either; otherwise it must match both, as cron has
    /// it.
    pub days_any: bool,
    /// Whether the day-of-the-week field was `*` (or a `*` with a step).
    pub weekdays_any: bool,
}

impl Calendar {
    /// Whether the day `day` of the month, in month `month`, which is
    /// day `weekday` of the week, is one of its days.
    #[must_use]
    pub fn day_matches(&self, day: u32, month: u32, weekday: u32) -> bool {
        if self.months & (1 << month) == 0 {
            return false;
        }
        let by_day = self.days & (1 << day) != 0;
        let by_weekday = self.weekdays & (1 << weekday) != 0;
        if self.days_any || self.weekdays_any { by_day && by_weekday } else { by_day || by_weekday }
    }
}

/// The longest a duration is: 366 days. A timer that runs less often than
/// a year is written as a time of year with `at`, which keeps its date
/// when the machine is off; and every duration then fits where the next
/// time is counted in seconds.
pub const DURATION_MAX: u64 = 366 * 86_400;

/// The seconds a duration such as `"20s"`, `"6h"`, `"1h30m"`, `"2d"` or
/// `"1w"` says: one or more numbers, each with its unit (`w`eeks, `d`ays,
/// `h`ours, `m`inutes, `s`econds), from the largest unit to the smallest,
/// each unit once. It is at least a second and at most [`DURATION_MAX`].
///
/// # Errors
///
/// What is wrong, in plain words.
pub fn duration(text: &str) -> Result<u64, String> {
    const UNITS: [(char, u64); 5] =
        [('w', 604_800), ('d', 86_400), ('h', 3600), ('m', 60), ('s', 1)];
    let wrong = || {
        format!(
            "`{}` is not a duration: write numbers with their units, such as \"20s\", \"6h\", \
             \"1h30m\", \"2d\" or \"1w\"",
            text.escape_debug()
        )
    };
    let mut rest = text;
    let mut total: u64 = 0;
    let mut from = 0;
    if rest.is_empty() {
        return Err(wrong());
    }
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).ok_or_else(wrong)?;
        let number: u64 = rest[..digits].parse().map_err(|_| wrong())?;
        let unit = rest[digits..].chars().next().ok_or_else(wrong)?;
        let at = UNITS.iter().position(|(u, _)| *u == unit).ok_or_else(wrong)?;
        if at < from {
            return Err(format!(
                "`{}` is not a duration: give the largest unit first, and each unit once",
                text.escape_debug()
            ));
        }
        from = at + 1;
        total = number
            .checked_mul(UNITS[at].1)
            .and_then(|seconds| total.checked_add(seconds))
            .filter(|total| *total <= DURATION_MAX)
            .ok_or_else(|| format!("`{text}` is more than 366 days, the longest a duration is"))?;
        rest = &rest[digits + unit.len_utf8()..];
    }
    if total == 0 {
        return Err(format!("`{text}` is no time: a duration is at least 1 second"));
    }
    Ok(total)
}

/// The calendar of a word of `@hourly`'s kind, without the `@`: `hourly`,
/// `daily` (or `midnight`), `weekly`, `monthly` and `yearly` (or
/// `annually`), as cronie defines them.
#[must_use]
pub fn word(name: &str) -> Option<Calendar> {
    let fields = match name {
        "hourly" => "0 * * * *",
        "daily" | "midnight" => "0 0 * * *",
        "weekly" => "0 0 * * 0",
        "monthly" => "0 0 1 * *",
        "yearly" | "annually" => "0 0 1 1 *",
        _ => return None,
    };
    parse(fields).ok()
}

/// The calendar a service file's `at` item says: a word, or a crontab
/// line's five fields.
///
/// # Errors
///
/// The column of the mistake in `text`, and what it is.
pub fn read(text: &str) -> Result<Calendar, (usize, String)> {
    match word(text) {
        Some(calendar) => Ok(calendar),
        None => parse(text),
    }
}

/// One of the five fields' limits and names.
struct Field {
    name: &'static str,
    low: u32,
    high: u32,
    names: &'static [&'static str],
    /// The number of the first of `names`.
    first_name: u32,
}

const MINUTE: Field = Field { name: "minute", low: 0, high: 59, names: &[], first_name: 0 };
const HOUR: Field = Field { name: "hour", low: 0, high: 23, names: &[], first_name: 0 };
const DAY: Field = Field { name: "day of the month", low: 1, high: 31, names: &[], first_name: 0 };
const MONTH: Field = Field {
    name: "month",
    low: 1,
    high: 12,
    names: &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"],
    first_name: 1,
};
/// The day of the week runs to 7, which is Sunday as 0 is.
const WEEKDAY: Field = Field {
    name: "day of the week",
    low: 0,
    high: 7,
    names: &["sun", "mon", "tue", "wed", "thu", "fri", "sat"],
    first_name: 0,
};

/// The number a value of `field` says: digits, or a name of any case.
fn value(field: &Field, text: &str, column: usize) -> Result<u32, (usize, String)> {
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        let n: u32 = text.parse().unwrap_or(u32::MAX);
        if (field.low..=field.high).contains(&n) {
            return Ok(n);
        }
        return Err((
            column,
            format!("the {} is from {} to {}, not {text}", field.name, field.low, field.high),
        ));
    }
    let lower = text.to_ascii_lowercase();
    match field.names.iter().position(|name| *name == lower) {
        Some(at) => Ok(field.first_name + at as u32),
        None => Err((column, format!("`{text}` is no {}", field.name))),
    }
}

/// The bits a field's text sets, from bit 0, and whether it starts with
/// `*`. `column` is where the field starts.
fn field(field: &Field, text: &str, column: usize) -> Result<(u64, bool), (usize, String)> {
    let mut bits = 0u64;
    let mut at = column;
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (range, Some((step, at + range.len() + 1))),
            None => (item, None),
        };
        let (low, high) = if range == "*" {
            (field.low, if field.high == 7 { 6 } else { field.high })
        } else if let Some((a, b)) = range.split_once('-') {
            let low = value(field, a, at)?;
            let high = value(field, b, at + a.len() + 1)?;
            if low > high {
                return Err((at, format!("{a}-{b} runs backward")));
            }
            (low, high)
        } else {
            let n = value(field, range, at)?;
            if step.is_some() {
                return Err((
                    at + range.len() + 1,
                    "a step goes after `*` or a range, as in */5 or 10-30/5".to_string(),
                ));
            }
            (n, n)
        };
        let by = match step {
            None => 1,
            Some((step, column)) => match step.parse::<u32>() {
                Ok(by @ 1..) if step.bytes().all(|b| b.is_ascii_digit()) => by,
                _ => {
                    return Err((
                        column,
                        format!("a step is a whole number above 0, not `{step}`"),
                    ));
                }
            },
        };
        let mut n = low;
        while n <= high {
            bits |= 1 << n;
            n += by;
        }
        at += item.len() + 1;
    }
    Ok((bits, text.starts_with('*')))
}

/// A crontab line's five fields, or a service file's `at` line, read:
/// numbers, ranges `a-b`, steps `*/n` and `a-b/n`, lists, month names
/// `jan` to `dec` and day names `sun` to `sat` in any case, and 7 for
/// Sunday.
///
/// # Errors
///
/// The column of the mistake, from 0 in `fields`, and what it is.
pub fn parse(fields: &str) -> Result<Calendar, (usize, String)> {
    let mut parts: Vec<(usize, &str)> = Vec::new();
    let mut start = None;
    for (at, c) in fields.char_indices() {
        if c == ' ' || c == '\t' {
            if let Some(from) = start.take() {
                parts.push((from, &fields[from..at]));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        parts.push((from, &fields[from..]));
    }
    if parts.len() < 5 {
        return Err((
            fields.len(),
            format!(
                "a time has five fields, minute, hour, day of the month, month and day of the \
                 week, and this has {}",
                parts.len()
            ),
        ));
    }
    if parts.len() > 5 {
        return Err((parts[5].0, "a time has five fields, and this has more".to_string()));
    }
    let specs = [&MINUTE, &HOUR, &DAY, &MONTH, &WEEKDAY];
    let mut bits = [0u64; 5];
    let mut any = [false; 5];
    for (i, ((column, text), spec)) in parts.iter().zip(specs).enumerate() {
        (bits[i], any[i]) = field(spec, text, *column)?;
    }
    // Sunday is 0 or 7.
    let weekdays = (bits[4] | (bits[4] >> 7)) & 0x7f;
    Ok(Calendar {
        minutes: bits[0],
        hours: bits[1] as u32,
        days: bits[2] as u32,
        months: bits[3] as u16,
        weekdays: weekdays as u8,
        days_any: any[2],
        weekdays_any: any[4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(list: &[u32]) -> u64 {
        list.iter().fold(0, |all, n| all | 1 << n)
    }

    #[test]
    fn five_stars_match_everything() {
        let c = parse("* * * * *").unwrap();
        assert_eq!(c.minutes, bits(&(0..60).collect::<Vec<_>>()));
        assert_eq!(c.hours, (1 << 24) - 1);
        assert_eq!(c.days, ((1u64 << 32) - 2) as u32);
        assert_eq!(c.months, 0x1ffe);
        assert_eq!(c.weekdays, 0x7f);
        assert!(c.days_any && c.weekdays_any);
    }

    #[test]
    fn numbers_ranges_lists_and_steps() {
        let c = parse("0,30 9-17 1,15 */3 1-5").unwrap();
        assert_eq!(c.minutes, bits(&[0, 30]));
        assert_eq!(c.hours, (9..=17).fold(0, |a, h| a | 1 << h));
        assert_eq!(c.days, 1 << 1 | 1 << 15);
        assert_eq!(c.months, 1 << 1 | 1 << 4 | 1 << 7 | 1 << 10);
        assert_eq!(c.weekdays, 0b0111110);
        assert!(!c.days_any && !c.weekdays_any);
        assert_eq!(parse("*/20 * * * *").unwrap().minutes, bits(&[0, 20, 40]));
        assert_eq!(parse("10-40/15 * * * *").unwrap().minutes, bits(&[10, 25, 40]));
        assert_eq!(parse("5,10-12,*/30 * * * *").unwrap().minutes, bits(&[0, 5, 10, 11, 12, 30]));
        assert_eq!(parse("0 */7 * * *").unwrap().hours, 1 | 1 << 7 | 1 << 14 | 1 << 21);
    }

    #[test]
    fn names_of_months_and_days_take_any_case() {
        let c = parse("0 0 * Jan,MAR-may mon-FRI").unwrap();
        assert_eq!(c.months, 1 << 1 | 1 << 3 | 1 << 4 | 1 << 5);
        assert_eq!(c.weekdays, 0b0111110);
        assert_eq!(parse("0 0 * dec sun").unwrap().months, 1 << 12);
        assert_eq!(parse("0 0 * * Sat").unwrap().weekdays, 1 << 6);
    }

    #[test]
    fn seven_is_sunday() {
        assert_eq!(parse("0 0 * * 7").unwrap().weekdays, 1);
        assert_eq!(parse("0 0 * * 5-7").unwrap().weekdays, 1 << 5 | 1 << 6 | 1);
        assert_eq!(parse("0 0 * * 0,7").unwrap().weekdays, 1);
        assert_eq!(parse("0 0 * * */2").unwrap().weekdays, 1 | 1 << 2 | 1 << 4 | 1 << 6);
        assert_eq!(parse("0 0 * * 0-7/7").unwrap().weekdays, 1);
    }

    #[test]
    fn a_star_decides_how_the_day_fields_combine() {
        let both = parse("0 0 13 * 5").unwrap();
        assert!(!both.days_any && !both.weekdays_any);
        // Friday the 13th, and any other 13th or Friday.
        assert!(both.day_matches(13, 3, 2));
        assert!(both.day_matches(14, 3, 5));
        assert!(!both.day_matches(14, 3, 2));
        let weekday = parse("0 0 * * 5").unwrap();
        assert!(weekday.days_any);
        assert!(weekday.day_matches(14, 3, 5) && !weekday.day_matches(14, 3, 4));
        let day = parse("0 0 13 * *").unwrap();
        assert!(day.weekdays_any);
        assert!(day.day_matches(13, 3, 0) && !day.day_matches(12, 3, 0));
        // A star with a step is a star too.
        assert!(parse("0 0 */2 * 1").unwrap().days_any);
        assert!(parse("0 0 1 * */2").unwrap().weekdays_any);
        // A month the line leaves out never matches.
        assert!(!parse("0 0 * 6 *").unwrap().day_matches(1, 5, 1));
    }

    #[test]
    fn fields_are_split_by_spaces_and_tabs() {
        assert_eq!(parse("  1\t2   3 4  5 ").unwrap(), parse("1 2 3 4 5").unwrap());
    }

    #[test]
    fn a_mistake_is_found_at_its_column() {
        let at = |text: &str| parse(text).unwrap_err().0;
        assert_eq!(at("60 * * * *"), 0);
        assert_eq!(at("* 24 * * *"), 2);
        assert_eq!(at("* * 0 * *"), 4);
        assert_eq!(at("* * 32 * *"), 4);
        assert_eq!(at("* * * 13 *"), 6);
        assert_eq!(at("* * * 0 *"), 6);
        assert_eq!(at("* * * * 8"), 8);
        assert_eq!(at("1,61 * * * *"), 2);
        assert_eq!(at("* 5-3 * * *"), 2);
        assert_eq!(at("*/0 * * * *"), 2);
        assert_eq!(at("*/x * * * *"), 2);
        assert_eq!(at("5/2 * * * *"), 2);
        assert_eq!(at("* * * foo *"), 6);
        assert_eq!(at("* * * * mon-xyz"), 12);
        assert_eq!(at(",1 * * * *"), 0);
        assert_eq!(at("1,,2 * * * *"), 2);
        assert_eq!(at("* * * *"), 7);
        assert_eq!(at(""), 0);
        assert_eq!(at("* * * * * *"), 10);
        assert_eq!(at("-1 * * * *"), 0);
        assert_eq!(at("1-2-3 * * * *"), 2);
    }

    #[test]
    fn the_words_are_cronies() {
        assert_eq!(word("hourly"), parse("0 * * * *").ok());
        assert_eq!(word("daily"), parse("0 0 * * *").ok());
        assert_eq!(word("midnight"), word("daily"));
        assert_eq!(word("weekly"), parse("0 0 * * 0").ok());
        assert_eq!(word("monthly"), parse("0 0 1 * *").ok());
        assert_eq!(word("yearly"), parse("0 0 1 1 *").ok());
        assert_eq!(word("annually"), word("yearly"));
        assert_eq!(word("reboot"), None);
        assert_eq!(word("Daily"), None);
        assert_eq!(word(""), None);
        assert_eq!(read("weekly"), Ok(word("weekly").unwrap()));
        assert_eq!(read("30 2 * * 1-5"), parse("30 2 * * 1-5"));
        assert!(read("30 2").is_err());
    }

    #[test]
    fn durations_add_their_units() {
        assert_eq!(duration("20s"), Ok(20));
        assert_eq!(duration("6h"), Ok(21_600));
        assert_eq!(duration("1h30m"), Ok(5400));
        assert_eq!(duration("2d"), Ok(172_800));
        assert_eq!(duration("1w"), Ok(604_800));
        assert_eq!(duration("1w2d3h4m5s"), Ok(604_800 + 172_800 + 10_800 + 240 + 5));
        assert_eq!(duration("90m"), Ok(5400));
        assert_eq!(duration("1s"), Ok(1));
        assert_eq!(duration("366d"), Ok(DURATION_MAX));
    }

    #[test]
    fn a_wrong_duration_is_refused() {
        for text in [
            "",
            "0s",
            "0",
            "20",
            "s",
            "1x",
            "1.5h",
            "-1h",
            " 1h",
            "1h ",
            "30m1h",
            "1h1h",
            "1H",
            "367d",
            "53w",
            "99999999999999999999s",
            "0d0s",
        ] {
            assert!(duration(text).is_err(), "{text:?}");
        }
    }
}
