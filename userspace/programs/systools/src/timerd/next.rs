// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The next time a calendar matches (ADR 0478): the times of day a
//! service's `[timer]` or a crontab line names, in a time zone.
//!
//! The calendar is of local times, which a time zone turns into moments:
//! a local time the clocks skip, as they go forward, runs at the first
//! minute after the skip, as cron runs it, so that a job set at 02:30 runs
//! on the night the clocks skip 02:00 to 03:00 too; one they repeat, as
//! they go back, runs once, the first time. The zone's rules come from
//! the tzdata files in `/system/resources/zoneinfo` (TZif, RFC 9636),
//! read by jiff, with the rule of each file's footer for the years after
//! its last change.

use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone};
use manifest::schedule::Calendar;

use crate::timedatectl::{LOCALTIME, ZONEINFO};

/// How far ahead a calendar is looked at: five years, so that a calendar
/// that names a day no year has (`0 0 31 2 *`) ends the search.
const YEARS_AHEAD: i16 = 5;

/// The first whole minute after `after` (seconds since 1970, UTC) that
/// `calendar` matches in `zone`, a tzdata name, or the system's when
/// `None`, as seconds since 1970; `None` when none comes within five
/// years, or the zone cannot be read. A local time the clocks skip runs
/// at the first minute after it; one they repeat runs once, the first
/// time.
pub fn next_time(calendar: &Calendar, after: i64, zone: Option<&str>) -> Option<i64> {
    next_in(calendar, after, &load_zone(zone).ok()?)
}

/// The time zone `name`, from the tzdata files, or the system's, which
/// `/etc/localtime` is, when `None`: UTC when the system has none set.
/// UTC itself needs no file.
///
/// # Errors
///
/// When `name` is no zone's, or its file cannot be read, in words.
pub fn load_zone(name: Option<&str>) -> Result<TimeZone, String> {
    let Some(name) = name else {
        return match std::fs::read(LOCALTIME) {
            Ok(data) => TimeZone::tzif("localtime", &data)
                .map_err(|err| format!("the system's time zone, {LOCALTIME}: {err}")),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TimeZone::UTC),
            Err(err) => Err(format!("the system's time zone, {LOCALTIME}: {err}")),
        };
    };
    if name == "UTC" {
        return Ok(TimeZone::UTC);
    }
    let plain = !name.is_empty()
        && name.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
    if !plain {
        return Err(format!("`{}` is no time zone's name", name.escape_debug()));
    }
    let path = format!("{ZONEINFO}/{name}");
    let data = std::fs::read(&path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            format!("there is no time zone {name} in {ZONEINFO}")
        } else {
            format!("{path}: {err}")
        }
    })?;
    TimeZone::tzif(name, &data).map_err(|err| format!("{path}: {err}"))
}

/// [`next_time`] in the time zone `zone`.
pub fn next_in(calendar: &Calendar, after: i64, zone: &TimeZone) -> Option<i64> {
    // The first whole minute after `after`.
    let from = after.div_euclid(60).checked_add(1)?.checked_mul(60)?;
    let first = zone.to_datetime(Timestamp::from_second(from).ok()?).date();
    let last = first.checked_add(jiff::Span::new().years(YEARS_AHEAD)).ok()?;
    let mut day = first;
    while day <= last {
        let weekday = u32::try_from(day.weekday().to_sunday_zero_offset()).ok()?;
        let (number, month) = (u32::try_from(day.day()).ok()?, u32::try_from(day.month()).ok()?);
        if calendar.day_matches(number, month, weekday)
            && let Some(at) = first_on(calendar, day, from, zone)
        {
            return Some(at);
        }
        day = day.tomorrow().ok()?;
    }
    None
}

/// The first moment, at or after `from`, of the times of `calendar` on
/// `day`, a day it matches.
fn first_on(calendar: &Calendar, day: Date, from: i64, zone: &TimeZone) -> Option<i64> {
    for hour in (0..24).filter(|hour| calendar.hours & (1 << hour) != 0) {
        for minute in (0..60).filter(|minute| calendar.minutes & (1 << minute) != 0) {
            let local = day.at(hour, minute, 0, 0);
            if let Some(at) = moment(local, zone).filter(|&at| at >= from) {
                return Some(at);
            }
        }
    }
    None
}

/// The moment local time `local` is in `zone`: the first, when the clocks
/// repeat it, and the first minute after the clocks skip to, when they
/// skip it. Local times are in order, and so are their moments, the
/// skipped ones all at the skip's end.
fn moment(local: DateTime, zone: &TimeZone) -> Option<i64> {
    // Seconds since 1970 of the local time, as if it were UTC.
    let wall = local.to_zoned(TimeZone::UTC).ok()?.timestamp().as_second();
    match zone.to_ambiguous_timestamp(local).offset() {
        AmbiguousOffset::Unambiguous { offset } => wall.checked_sub(seconds(offset)),
        // The earlier offset is the one before the clocks went back.
        AmbiguousOffset::Fold { before, .. } => wall.checked_sub(seconds(before)),
        AmbiguousOffset::Gap { before, after } => {
            skip_end(zone, wall.checked_sub(seconds(after))?, wall.checked_sub(seconds(before))?)
                .map(|end| end.div_euclid(60).saturating_add(i64::from(end % 60 != 0)) * 60)
        }
    }
}

/// The moment within `low..=high` at which `zone`'s offset changes to the
/// one it has at `high`: where the clocks skip. The skipped local time
/// read with the later offset is a moment before the change, and with
/// the earlier one a moment at or after it.
fn skip_end(zone: &TimeZone, mut low: i64, mut high: i64) -> Option<i64> {
    let offset_at = |second: i64| Timestamp::from_second(second).ok().map(|t| zone.to_offset(t));
    let target = offset_at(high)?;
    while low < high {
        let middle = low + (high - low) / 2;
        if offset_at(middle)? == target { high = middle } else { low = middle + 1 }
    }
    Some(high)
}

/// An offset's seconds east of UTC.
fn seconds(offset: Offset) -> i64 {
    i64::from(offset.seconds())
}

#[cfg(test)]
mod tests {
    use manifest::schedule;

    use super::*;

    fn calendar(line: &str) -> Calendar {
        schedule::read(line).unwrap_or_else(|(at, why)| panic!("{line}: {at}: {why}"))
    }

    /// Seconds since 1970 of `text`, `YYYY-MM-DDTHH:MM` in UTC.
    fn utc(text: &str) -> i64 {
        let local: DateTime = text.parse().unwrap();
        local.to_zoned(TimeZone::UTC).unwrap().timestamp().as_second()
    }

    /// The US's Mountain time and central Europe's, by their rules, which
    /// need no tzdata file.
    fn denver() -> TimeZone {
        TimeZone::posix("MST7MDT,M3.2.0,M11.1.0").unwrap()
    }

    fn berlin() -> TimeZone {
        TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap()
    }

    #[test]
    fn the_next_time_is_the_first_whole_minute_after() {
        let every = calendar("* * * * *");
        let at = utc("2026-11-14T01:00");
        assert_eq!(next_in(&every, at, &TimeZone::UTC), Some(at + 60));
        assert_eq!(next_in(&every, at + 59, &TimeZone::UTC), Some(at + 60));
        assert_eq!(next_in(&every, at - 1, &TimeZone::UTC), Some(at));
        let daily = calendar("daily");
        assert_eq!(next_in(&daily, at, &TimeZone::UTC), Some(utc("2026-11-15T00:00")));
        // 02:30 on weekdays, from a Friday's afternoon: Monday.
        let weekdays = calendar("30 2 * * 1-5");
        assert_eq!(
            next_in(&weekdays, utc("2026-11-13T15:00"), &TimeZone::UTC),
            Some(utc("2026-11-16T02:30"))
        );
        // Both day fields restricted: either matches, as cron has it; the
        // 13th is a Friday, then the 15th a Sunday.
        let either = calendar("0 9 13 * 0");
        assert_eq!(
            next_in(&either, utc("2026-11-12T00:00"), &TimeZone::UTC),
            Some(utc("2026-11-13T09:00"))
        );
        assert_eq!(
            next_in(&either, utc("2026-11-13T09:00"), &TimeZone::UTC),
            Some(utc("2026-11-15T09:00"))
        );
    }

    #[test]
    fn a_day_no_year_has_ends_the_search_and_a_leap_day_is_found() {
        assert_eq!(next_in(&calendar("0 0 31 2 *"), utc("2026-01-01T00:00"), &TimeZone::UTC), None);
        assert_eq!(
            next_in(&calendar("0 12 29 2 *"), utc("2026-03-01T00:00"), &TimeZone::UTC),
            Some(utc("2028-02-29T12:00"))
        );
    }

    #[test]
    fn local_times_are_the_zone_s() {
        // 02:30 in Denver in winter is 09:30 UTC, in summer 08:30.
        let early = calendar("30 2 * * *");
        assert_eq!(
            next_in(&early, utc("2026-11-14T00:00"), &denver()),
            Some(utc("2026-11-14T09:30"))
        );
        assert_eq!(
            next_in(&early, utc("2026-07-01T00:00"), &denver()),
            Some(utc("2026-07-01T08:30"))
        );
    }

    #[test]
    fn a_skipped_local_time_runs_at_the_first_minute_after_the_skip() {
        // 2027-03-14: Denver's clocks go from 02:00 MST to 03:00 MDT, at
        // 09:00 UTC. 02:30 runs then; 03:30 an hour later, as usual.
        let skipped = calendar("30 2 * * *");
        let night = utc("2027-03-14T06:00");
        assert_eq!(next_in(&skipped, night, &denver()), Some(utc("2027-03-14T09:00")));
        assert_eq!(
            next_in(&skipped, utc("2027-03-14T09:00"), &denver()),
            Some(utc("2027-03-15T08:30"))
        );
        // Every minute of the skipped hour comes to the same moment, once.
        let each = calendar("* 2 * * *");
        let at = next_in(&each, night, &denver()).unwrap();
        assert_eq!(at, utc("2027-03-14T09:00"));
        // The next day's 02:00 is summer time's, 08:00 UTC.
        assert_eq!(next_in(&each, at, &denver()), Some(utc("2027-03-15T08:00")));
        // Berlin skips 02:00 to 03:00 CEST at 01:00 UTC on 2027-03-28.
        assert_eq!(
            next_in(&calendar("15 2 * * *"), utc("2027-03-27T23:00"), &berlin()),
            Some(utc("2027-03-28T01:00"))
        );
    }

    #[test]
    fn a_repeated_local_time_runs_once_the_first_time() {
        // 2026-11-01: Denver's clocks go from 02:00 MDT back to 01:00 MST,
        // at 08:00 UTC; 01:30 comes at 07:30 UTC and again at 08:30.
        let repeated = calendar("30 1 * * *");
        let first = next_in(&repeated, utc("2026-11-01T06:00"), &denver());
        assert_eq!(first, Some(utc("2026-11-01T07:30")));
        // Not again in the repeated hour: the next is the next day's.
        assert_eq!(
            next_in(&repeated, utc("2026-11-01T07:30"), &denver()),
            Some(utc("2026-11-02T08:30"))
        );
        // From within the repeated hour, too.
        assert_eq!(
            next_in(&repeated, utc("2026-11-01T08:10"), &denver()),
            Some(utc("2026-11-02T08:30"))
        );
    }

    #[test]
    fn a_zone_s_name_is_a_path_beneath_zoneinfo_and_nothing_else() {
        assert_eq!(load_zone(Some("UTC")).unwrap(), TimeZone::UTC);
        for name in ["", "/etc/passwd", "../passwd", "America/../../x", "America//Denver", "."] {
            assert!(load_zone(Some(name)).unwrap_err().contains("no time zone's name"), "{name}");
        }
    }

    /// A small generator of numbers, so that the property below sees the
    /// same cases on each run.
    struct Numbers(u64);

    impl Numbers {
        fn below(&mut self, bound: u64) -> u64 {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % bound
        }
    }

    /// A calendar of random fields, each a `*`, a number, a range or a
    /// list, as people write them.
    fn random_calendar(numbers: &mut Numbers) -> Calendar {
        let mut field = |low: u64, high: u64| -> String {
            match numbers.below(5) {
                0 => "*".into(),
                1 => (low + numbers.below(high - low + 1)).to_string(),
                2 => {
                    let a = low + numbers.below(high - low + 1);
                    let b = a + numbers.below(high - a + 1);
                    format!("{a}-{b}")
                }
                3 => format!("*/{}", 1 + numbers.below(high.min(12))),
                _ => {
                    let a = low + numbers.below(high - low + 1);
                    let b = low + numbers.below(high - low + 1);
                    if a == b { a.to_string() } else { format!("{},{}", a.min(b), a.max(b)) }
                }
            }
        };
        let line = format!(
            "{} {} {} {} {}",
            field(0, 59),
            field(0, 23),
            field(1, 28),
            field(1, 12),
            field(0, 6)
        );
        calendar(&line)
    }

    /// Whether local time `local` is one of `calendar`'s.
    fn matches(calendar: &Calendar, local: DateTime) -> bool {
        let (minute, hour) = (local.minute() as u32, local.hour() as u32);
        let weekday = local.weekday().to_sunday_zero_offset() as u32;
        calendar.minutes & (1 << minute) != 0
            && calendar.hours & (1 << hour) != 0
            && calendar.day_matches(local.day() as u32, local.month() as u32, weekday)
    }

    #[test]
    fn the_next_time_is_the_first_minute_that_matches() {
        // For random calendars and moments: the time found is a whole
        // minute after the moment, it matches, and no minute between
        // does. In UTC, where no local time is skipped or repeated, minute
        // by minute over up to 40 days.
        let mut numbers = Numbers(0x9e37_79b9_7f4a_7c15);
        let start = utc("2026-01-01T00:00");
        for _ in 0..400 {
            let calendar = random_calendar(&mut numbers);
            let after = start + i64::try_from(numbers.below(3 * 366 * 86_400)).unwrap();
            let found = next_in(&calendar, after, &TimeZone::UTC);
            let limit = after + 40 * 86_400;
            let mut minute = (after / 60 + 1) * 60;
            let expected = loop {
                if minute > limit {
                    break None;
                }
                let local = TimeZone::UTC.to_datetime(Timestamp::from_second(minute).unwrap());
                if matches(&calendar, local) {
                    break Some(minute);
                }
                minute += 60;
            };
            match (found, expected) {
                (Some(found), Some(expected)) => assert_eq!(found, expected, "{calendar:?}"),
                // Further than the walk went: still after it, and matching.
                (Some(found), None) => {
                    assert!(found > limit, "{calendar:?}");
                    let local = TimeZone::UTC.to_datetime(Timestamp::from_second(found).unwrap());
                    assert!(matches(&calendar, local), "{calendar:?}");
                }
                (None, expected) => panic!("{calendar:?}: none found, {expected:?} matches"),
            }
        }
    }

    #[test]
    fn in_a_zone_with_daylight_saving_each_time_runs_once_in_order() {
        // Through a year of Denver's and Berlin's changes: each time found
        // is later than the one before, matches the calendar in local time
        // unless the clocks skipped it, and no local time runs twice.
        let mut numbers = Numbers(0x2545_f491_4f6c_dd1d);
        for zone in [denver(), berlin()] {
            for _ in 0..40 {
                let calendar = random_calendar(&mut numbers);
                let mut after = utc("2027-01-01T00:00");
                let mut seen: Vec<DateTime> = Vec::new();
                for _ in 0..60 {
                    let Some(found) = next_in(&calendar, after, &zone) else { break };
                    assert!(found > after && found % 60 == 0, "{calendar:?}");
                    let local = zone.to_datetime(Timestamp::from_second(found).unwrap());
                    let skipped = zone.to_offset(Timestamp::from_second(found - 60).unwrap())
                        != zone.to_offset(Timestamp::from_second(found).unwrap());
                    assert!(skipped || matches(&calendar, local), "{calendar:?} at {local}");
                    assert!(!seen.contains(&local), "{calendar:?}: {local} twice");
                    seen.push(local);
                    after = found;
                }
            }
        }
    }
}
