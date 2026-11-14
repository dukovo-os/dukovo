// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The times `at` takes (at(1), ADR 0478): POSIX's `HH:MM`, `HHMM`, `H`
//! with `am` or `pm`, `now`, `today`, `tomorrow` and `now + 2 hours`, and
//! Linux's `noon`, `midnight`, `teatime`, `next week`, month and day
//! names, and the date forms `MM/DD/YY`, `DD.MM.YY` and `YYYY-MM-DD`.
//!
//! The time is read against the clock and the time zone it is given, so
//! that the tests run on a fixed clock in UTC. A time of day without a
//! date is today's, or tomorrow's when today's has passed; a date that
//! has passed is refused. A time that comes from `now`, with or without
//! `+ N unit`, is cut to the minute, since jobs start at the minute.

/// A date and a time of day on the wall clock of a time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// 0 for Sunday to 6.
    pub weekday: u32,
}

/// A time zone: how to read seconds since 1970 on its wall clock, and back.
pub trait Zone {
    /// The wall clock at `epoch`.
    fn civil(&self, epoch: i64) -> Civil;
    /// The seconds since 1970 of the wall-clock time `civil` says; a time
    /// the clocks skip is the first minute after the gap.
    fn epoch(&self, civil: &Civil) -> i64;
}

/// The days from 1970-01-01 to the date, by the proleptic Gregorian
/// calendar.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month_from_march = i64::from((month + 9) % 12);
    let doy = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The days in `month` of `year`.
fn month_days(year: i64, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// UTC, which the tests use.
#[cfg(test)]
pub struct Utc;

#[cfg(test)]
impl Zone for Utc {
    fn civil(&self, epoch: i64) -> Civil {
        let days = epoch.div_euclid(86_400);
        let seconds = epoch.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Civil {
            year,
            month,
            day,
            hour: (seconds / 3600) as u32,
            minute: (seconds % 3600 / 60) as u32,
            weekday: (days + 4).rem_euclid(7) as u32,
        }
    }

    fn epoch(&self, civil: &Civil) -> i64 {
        days_from_civil(civil.year, civil.month, civil.day) * 86_400
            + i64::from(civil.hour) * 3600
            + i64::from(civil.minute) * 60
    }
}

/// The system's time zone, through the C library.
pub struct Local;

impl Zone for Local {
    fn civil(&self, epoch: i64) -> Civil {
        let time = epoch as libc::time_t;
        // SAFETY: an all-zero tm is valid; localtime_r fills it.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers are valid for the call.
        unsafe { libc::localtime_r(&raw const time, &raw mut tm) };
        Civil {
            year: i64::from(tm.tm_year) + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            weekday: tm.tm_wday as u32,
        }
    }

    fn epoch(&self, civil: &Civil) -> i64 {
        // SAFETY: an all-zero tm is valid; the fields mktime reads are set.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = (civil.year - 1900) as libc::c_int;
        tm.tm_mon = civil.month as libc::c_int - 1;
        tm.tm_mday = civil.day as libc::c_int;
        tm.tm_hour = civil.hour as libc::c_int;
        tm.tm_min = civil.minute as libc::c_int;
        tm.tm_isdst = -1;
        // SAFETY: `tm` is valid for the call.
        // `time_t` is `i64` here and may be narrower elsewhere.
        #[allow(clippy::useless_conversion)]
        i64::from(unsafe { libc::mktime(&raw mut tm) })
    }
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const WEEKDAYS: [&str; 7] =
    ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];

/// The index of the word in `names`, written in full or by its first
/// three letters.
fn named(word: &str, names: &[&str]) -> Option<usize> {
    names.iter().position(|name| *name == word || (word.len() == 3 && name.starts_with(word)))
}

/// A unit of time that `+ N unit` takes.
#[derive(Clone, Copy)]
enum Unit {
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

fn unit(word: &str) -> Option<Unit> {
    let word = word.strip_suffix('s').unwrap_or(word);
    Some(match word {
        "minute" | "min" => Unit::Minutes,
        "hour" => Unit::Hours,
        "day" => Unit::Days,
        "week" => Unit::Weeks,
        "month" => Unit::Months,
        "year" => Unit::Years,
        _ => return None,
    })
}

/// What is read from the words so far.
#[derive(Default)]
struct Read {
    time: Option<(u32, u32)>,
    date: Option<(Option<i64>, u32, u32)>,
    weekday: Option<u32>,
    now: bool,
    today: bool,
    tomorrow: bool,
    increments: Vec<(i64, Unit)>,
}

/// The seconds since 1970 that the words `words` say, `now` being the
/// time it is, in `zone`.
///
/// # Errors
///
/// What is wrong with the words, in plain words.
pub fn parse(words: &[String], now: i64, zone: &dyn Zone) -> Result<i64, String> {
    let text = words.join(" ").to_lowercase().replace('+', " + ").replace(',', " ");
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("give a time, such as `now + 1 hour`, `14:30` or `noon tomorrow`".into());
    }
    let mut read = Read::default();
    let mut at = 0;
    while at < tokens.len() {
        at += word(&tokens, at, &mut read)?;
    }
    resolve(&read, now, zone)
}

/// Reads the words from `at` on: what one part of the time says. How many
/// words it took.
fn word(tokens: &[&str], at: usize, read: &mut Read) -> Result<usize, String> {
    let token = tokens[at];
    let next = tokens.get(at + 1).copied();
    match token {
        "now" => read.now = true,
        "today" => read.today = true,
        "tomorrow" => read.tomorrow = true,
        "noon" => read.time = Some((12, 0)),
        "midnight" => read.time = Some((0, 0)),
        "teatime" => read.time = Some((16, 0)),
        "+" => {
            let number = next.and_then(|n| n.parse::<i64>().ok());
            let unit = tokens.get(at + 2).and_then(|u| unit(u));
            let (Some(number), Some(unit)) = (number, unit) else {
                return Err("after `+` give a number and a unit, such as `+ 2 hours`".into());
            };
            read.increments.push((number, unit));
            return Ok(3);
        }
        "next" => {
            let Some(unit) = next.and_then(unit) else {
                return Err("after `next` give a unit, such as `next week`".into());
            };
            read.increments.push((1, unit));
            return Ok(2);
        }
        _ => {
            if let Some(month) = named(token, &MONTHS) {
                return month_and_day(tokens, at, month as u32 + 1, read);
            }
            if let Some(day) = named(token, &WEEKDAYS) {
                read.weekday = Some(day as u32);
            } else if let Some(used) = time_of_day(token, next, read)? {
                return Ok(used);
            } else if !date_form(token, read)? {
                return Err(format!("`{token}` is not part of a time"));
            }
        }
    }
    Ok(1)
}

/// `Jan 5` and `January 5 2027`: the month's name, the day, and a year
/// that may follow.
fn month_and_day(tokens: &[&str], at: usize, month: u32, read: &mut Read) -> Result<usize, String> {
    let day = tokens
        .get(at + 1)
        .and_then(|d| d.parse::<u32>().ok())
        .filter(|d| (1..=31).contains(d))
        .ok_or_else(|| format!("give the day of the month after {}", tokens[at]))?;
    let year = tokens.get(at + 2).filter(|y| y.len() == 4).and_then(|y| y.parse::<i64>().ok());
    read.date = Some((year, month, day));
    Ok(if year.is_some() { 3 } else { 2 })
}

/// `14:30`, `1430`, `3pm`, `3:30 pm` and `5`: a time of day, and how many
/// words it took; none when `token` is not one.
fn time_of_day(token: &str, next: Option<&str>, read: &mut Read) -> Result<Option<usize>, String> {
    let (digits, suffix) = match token.find(|c: char| c.is_ascii_alphabetic()) {
        Some(at) => token.split_at(at),
        None => (token, ""),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return Ok(None);
    }
    let (suffix, used) = match (suffix, next) {
        ("", Some(next @ ("am" | "pm"))) => (next, 2),
        (suffix, _) => (suffix, 1),
    };
    if !matches!(suffix, "" | "am" | "pm") {
        return Ok(None);
    }
    let (hour, minute) = if let Some((hour, minute)) = digits.split_once(':') {
        (hour, minute)
    } else if digits.len() <= 2 {
        (digits, "0")
    } else if digits.len() <= 4 {
        digits.split_at(digits.len() - 2)
    } else {
        return Ok(None);
    };
    let (Ok(mut hour), Ok(minute)) = (hour.parse::<u32>(), minute.parse::<u32>()) else {
        return Ok(None);
    };
    if minute > 59 {
        return Err(format!("`{token}` has more than 59 minutes"));
    }
    match suffix {
        "am" | "pm" if !(1..=12).contains(&hour) => {
            return Err(format!("`{token}`: with am or pm the hour is from 1 to 12"));
        }
        "am" if hour == 12 => hour = 0,
        "pm" if hour != 12 => hour += 12,
        _ => {}
    }
    if hour > 23 || (hour == 24 && minute > 0) {
        return Err(format!("`{token}` has more than 23 hours"));
    }
    read.time = Some((hour, minute));
    Ok(Some(used))
}

/// `MM/DD/YY`, `DD.MM.YY` and `YYYY-MM-DD`, and their longer years:
/// whether `token` is one.
fn date_form(token: &str, read: &mut Read) -> Result<bool, String> {
    let (separator, order) = if token.contains('/') {
        ('/', [0, 1, 2])
    } else if token.contains('.') {
        ('.', [1, 0, 2])
    } else if token.contains('-') {
        ('-', [1, 2, 0])
    } else {
        return Ok(false);
    };
    let parts: Vec<&str> = token.split(separator).collect();
    let numbers: Option<Vec<i64>> = parts.iter().map(|p| p.parse().ok()).collect();
    let (Some(numbers), true) = (numbers, parts.len() == 3) else {
        return Err(format!("`{token}` is not a date"));
    };
    // The first number of each form is the one named by `order`: month,
    // day and year, in the form's order.
    let [month, day, year] = [numbers[order[0]], numbers[order[1]], numbers[order[2]]];
    let year = match parts[order[2]].len() {
        4 => year,
        2 if year >= 70 => 1900 + year,
        2 => 2000 + year,
        _ => return Err(format!("`{token}`: write the year with two or four digits")),
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(format!("`{token}` is not a date"));
    }
    read.date = Some((Some(year), month as u32, day as u32));
    Ok(true)
}

/// The seconds since 1970 that what was read says.
fn resolve(read: &Read, now: i64, zone: &dyn Zone) -> Result<i64, String> {
    let here = zone.civil(now);
    let explicit_date = read.date.is_some() || read.weekday.is_some() || read.tomorrow;
    if read.time.is_none() && explicit_date && read.increments.is_empty() {
        return Err("give a time of day with the date, such as `10am tomorrow`".into());
    }
    if read.time.is_none() && !read.now && read.increments.is_empty() && !read.today {
        return Err("give a time, such as `now + 1 hour` or `14:30`".into());
    }
    let (hour, minute) = read.time.unwrap_or((here.hour, here.minute));
    let mut civil = Civil { hour, minute, ..here };
    let mut when;
    if let Some((year, month, day)) = read.date {
        civil.month = month;
        civil.day = day;
        civil.year = year.unwrap_or(here.year);
        if civil.day > month_days(civil.year, month) {
            return Err(format!("month {month} has no day {day} in {}", civil.year));
        }
        when = zone.epoch(&civil);
        if year.is_none() && when <= now {
            civil.year += 1;
            when = zone.epoch(&civil);
        } else if when <= now {
            return Err("that time has passed".into());
        }
    } else {
        when = zone.epoch(&civil);
        if read.tomorrow {
            when = add_days(&civil, 1, zone);
        } else if let Some(weekday) = read.weekday {
            let ahead = i64::from((weekday + 7 - here.weekday) % 7);
            let mut days = ahead;
            if days == 0 && when <= now {
                days = 7;
            }
            when = add_days(&civil, days, zone);
        } else if read.time.is_some() && !read.today && when <= now && read.increments.is_empty() {
            when = add_days(&civil, 1, zone);
        } else if read.time.is_some() && read.today && when <= now {
            return Err("that time has passed".into());
        }
    }
    for (number, unit) in &read.increments {
        when = add(when, *number, *unit, zone);
    }
    if read.time.is_none() {
        // From now, to the minute: a job starts at the start of one.
        when = when.div_euclid(60) * 60;
        if read.increments.is_empty() {
            when = now;
        }
    }
    Ok(when)
}

/// `civil` with `days` more days, on the same wall clock.
fn add_days(civil: &Civil, days: i64, zone: &dyn Zone) -> i64 {
    let (year, month, day) =
        civil_from_days(days_from_civil(civil.year, civil.month, civil.day) + days);
    zone.epoch(&Civil { year, month, day, ..*civil })
}

/// `when` with `number` more `unit`s.
fn add(when: i64, number: i64, unit: Unit, zone: &dyn Zone) -> i64 {
    match unit {
        Unit::Minutes => when + number * 60,
        Unit::Hours => when + number * 3600,
        Unit::Days | Unit::Weeks => {
            let days = if matches!(unit, Unit::Weeks) { number * 7 } else { number };
            add_days(&zone.civil(when), days, zone)
        }
        Unit::Months | Unit::Years => {
            let civil = zone.civil(when);
            let months = if matches!(unit, Unit::Years) { number * 12 } else { number };
            let total = civil.year * 12 + i64::from(civil.month) - 1 + months;
            let (year, month) = (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32);
            let day = civil.day.min(month_days(year, month));
            zone.epoch(&Civil { year, month, day, ..civil })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saturday 2026-11-14 12:34:56 UTC.
    const NOW: i64 = 1_794_659_696;

    fn at(text: &str) -> Result<i64, String> {
        let words: Vec<String> = text.split(' ').map(str::to_string).collect();
        parse(&words, NOW, &Utc)
    }

    fn when(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.epoch(&Civil { year, month, day, hour, minute, weekday: 0 })
    }

    #[test]
    fn the_fixed_clock_is_what_the_tests_say() {
        let civil = Utc.civil(NOW);
        assert_eq!(
            (civil.year, civil.month, civil.day, civil.hour, civil.minute),
            (2026, 11, 14, 12, 34)
        );
        assert_eq!(civil.weekday, 6);
        assert_eq!(when(2026, 11, 14, 12, 34) + 56, NOW);
    }

    #[test]
    fn now_is_now_and_an_increment_is_cut_to_the_minute() {
        assert_eq!(at("now"), Ok(NOW));
        assert_eq!(at("now + 1 minute"), Ok(when(2026, 11, 14, 12, 35)));
        assert_eq!(at("now + 2 hours"), Ok(when(2026, 11, 14, 14, 34)));
        assert_eq!(at("now + 3 days"), Ok(when(2026, 11, 17, 12, 34)));
        assert_eq!(at("now + 1 week"), Ok(when(2026, 11, 21, 12, 34)));
        assert_eq!(at("now + 1 month"), Ok(when(2026, 12, 14, 12, 34)));
        assert_eq!(at("now + 1 year"), Ok(when(2027, 11, 14, 12, 34)));
        assert_eq!(at("now+90 minutes"), Ok(when(2026, 11, 14, 14, 4)));
        assert_eq!(at("now + 1 hour + 30 minutes"), Ok(when(2026, 11, 14, 14, 4)));
        assert_eq!(at("next week"), Ok(when(2026, 11, 21, 12, 34)));
        assert_eq!(at("next day"), Ok(when(2026, 11, 15, 12, 34)));
    }

    #[test]
    fn a_month_increment_keeps_to_the_end_of_a_short_month() {
        let words: Vec<String> = ["now", "+", "1", "month"].map(String::from).to_vec();
        let jan_31 = when(2027, 1, 31, 10, 0);
        assert_eq!(parse(&words, jan_31, &Utc), Ok(when(2027, 2, 28, 10, 0)));
    }

    #[test]
    fn a_time_of_day_is_today_or_tomorrow_when_it_has_passed() {
        assert_eq!(at("14:30"), Ok(when(2026, 11, 14, 14, 30)));
        assert_eq!(at("1430"), Ok(when(2026, 11, 14, 14, 30)));
        assert_eq!(at("12:00"), Ok(when(2026, 11, 15, 12, 0)));
        assert_eq!(at("9"), Ok(when(2026, 11, 15, 9, 0)));
        assert_eq!(at("noon"), Ok(when(2026, 11, 15, 12, 0)));
        assert_eq!(at("midnight"), Ok(when(2026, 11, 15, 0, 0)));
        assert_eq!(at("teatime"), Ok(when(2026, 11, 14, 16, 0)));
        assert_eq!(at("today 16:00"), Ok(when(2026, 11, 14, 16, 0)));
        assert_eq!(at("today 9:00"), Err("that time has passed".into()));
    }

    #[test]
    fn am_and_pm() {
        assert_eq!(at("3pm"), Ok(when(2026, 11, 14, 15, 0)));
        assert_eq!(at("3 pm"), Ok(when(2026, 11, 14, 15, 0)));
        assert_eq!(at("3:45PM"), Ok(when(2026, 11, 14, 15, 45)));
        assert_eq!(at("12am"), Ok(when(2026, 11, 15, 0, 0)));
        assert_eq!(at("12pm"), Ok(when(2026, 11, 15, 12, 0)));
        assert_eq!(at("1am"), Ok(when(2026, 11, 15, 1, 0)));
        assert!(at("13pm").is_err() && at("0am").is_err());
    }

    #[test]
    fn dates_name_a_day() {
        assert_eq!(at("10:00 tomorrow"), Ok(when(2026, 11, 15, 10, 0)));
        assert_eq!(at("tomorrow 10:00"), Ok(when(2026, 11, 15, 10, 0)));
        assert_eq!(at("noon tomorrow"), Ok(when(2026, 11, 15, 12, 0)));
        assert_eq!(at("10:00 Dec 25"), Ok(when(2026, 12, 25, 10, 0)));
        assert_eq!(at("10:00 december 25 2027"), Ok(when(2027, 12, 25, 10, 0)));
        assert_eq!(at("10:00 Jan 5"), Ok(when(2027, 1, 5, 10, 0)));
        assert_eq!(at("10:00 12/25/26"), Ok(when(2026, 12, 25, 10, 0)));
        assert_eq!(at("10:00 25.12.2026"), Ok(when(2026, 12, 25, 10, 0)));
        assert_eq!(at("10:00 2026-12-25"), Ok(when(2026, 12, 25, 10, 0)));
        assert_eq!(at("10:00 2026-12-25 + 1 day"), Ok(when(2026, 12, 26, 10, 0)));
        assert_eq!(at("23:59 2026-11-14"), Ok(when(2026, 11, 14, 23, 59)));
    }

    #[test]
    fn a_weekday_is_its_next_day() {
        assert_eq!(at("10:00 monday"), Ok(when(2026, 11, 16, 10, 0)));
        assert_eq!(at("10:00 Sun"), Ok(when(2026, 11, 15, 10, 0)));
        assert_eq!(at("10:00 saturday"), Ok(when(2026, 11, 21, 10, 0)));
        assert_eq!(at("14:00 saturday"), Ok(when(2026, 11, 14, 14, 0)));
    }

    #[test]
    fn a_time_with_an_increment_counts_from_that_time() {
        // Today's noon has passed: the day is added to it, as written.
        assert_eq!(at("noon + 1 day"), Ok(when(2026, 11, 15, 12, 0)));
        assert_eq!(at("14:00 + 2 hours"), Ok(when(2026, 11, 14, 16, 0)));
    }

    #[test]
    fn a_wrong_time_is_refused_with_why() {
        for text in [
            "",
            "later",
            "25:00",
            "10:61",
            "tomorrow",
            "Jan",
            "Jan 32",
            "Feb 30 2027",
            "10:00 2026-13-01",
            "10:00 2026-11-13",
            "+ 1",
            "now + x hours",
            "now + 1 fortnight",
            "10:00 1/2",
            "10:00 1/2/345",
            "next",
            "10:00 5/5/5/5",
        ] {
            assert!(at(text).is_err(), "{text:?}");
        }
        assert!(at("10:00 2026-11-13").unwrap_err().contains("passed"));
    }

    #[test]
    fn the_calendar_arithmetic_agrees_with_known_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(month_days(2024, 2), 29);
        assert_eq!(month_days(2100, 2), 28);
        assert_eq!(month_days(2000, 2), 29);
        assert_eq!(Utc.civil(0).weekday, 4);
    }
}
