// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! snap(8)'s names and retention (ADR 0461): a snapshot is named by the
//! local time it was made, `YYYY-MM-DD-HHMM` with `.2` and on for another
//! in the same minute, or by a name it was given. Of those named by
//! their time, a place keeps the newest of each of the `daily` latest
//! days that have one, the newest of each of the `weekly` latest weeks
//! (Monday to Sunday) that have one, and the newest of all, as restic's
//! `--keep-daily` and `--keep-weekly` count them; the rest go.

/// Why a snapshot named by its time is kept; none of them, and it goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Kept {
    /// The newest of all.
    pub newest: bool,
    /// The newest of one of the latest days.
    pub daily: bool,
    /// The newest of one of the latest weeks.
    pub weekly: bool,
}

impl Kept {
    /// Whether it is kept.
    pub fn any(self) -> bool {
        self.newest || self.daily || self.weekly
    }

    /// Its reasons, as `snap list` writes them: `newest, daily, weekly`.
    pub fn words(self) -> String {
        let words: Vec<&str> =
            [(self.newest, "newest"), (self.daily, "daily"), (self.weekly, "weekly")]
                .into_iter()
                .filter_map(|(on, word)| on.then_some(word))
                .collect();
        if words.is_empty() { "goes at the next prune".into() } else { words.join(", ") }
    }
}

/// Which of the snapshots made on local days `days`, newest first, are
/// kept, and why: one for each.
pub(crate) fn keep(days: &[i64], daily: usize, weekly: usize) -> Vec<Kept> {
    let mut kept = vec![Kept::default(); days.len()];
    if let Some(first) = kept.first_mut() {
        first.newest = true;
    }
    let (mut seen_days, mut seen_weeks) = (Vec::new(), Vec::new());
    for (i, &day) in days.iter().enumerate() {
        // Newest first, so the first of a day or week is its newest.
        if seen_days.len() < daily && !seen_days.contains(&day) {
            seen_days.push(day);
            kept[i].daily = true;
        }
        let week = week_of(day);
        if seen_weeks.len() < weekly && !seen_weeks.contains(&week) {
            seen_weeks.push(week);
            kept[i].weekly = true;
        }
    }
    kept
}

/// The week, Monday to Sunday, of day `day` since 1970-01-01, a
/// Thursday: weeks counted from the Monday before it.
pub(crate) fn week_of(day: i64) -> i64 {
    (day + 3).div_euclid(7)
}

/// The day since 1970-01-01 of the date `year`-`month`-`day`, by Howard
/// Hinnant's `days_from_civil`.
pub(crate) fn day_number(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The name of a snapshot made at a local time.
pub(crate) fn timed_name(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}")
}

/// Whether `name` is a time a snapshot is named by: `YYYY-MM-DD-HHMM`,
/// or that with `.N` for a number from 2.
pub(crate) fn is_timed(name: &str) -> bool {
    let (time, count) = match name.split_once('.') {
        Some((time, count)) => (time, Some(count)),
        None => (name, None),
    };
    let shape = time.len() == 15
        && time.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 | 10 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    let counted = count.is_none_or(|count| {
        !count.is_empty()
            && !count.starts_with('0')
            && count.bytes().all(|b| b.is_ascii_digit())
            && count != "1"
    });
    shape && counted
}

/// Whether `name` may be given to a snapshot: a file name that is not
/// hidden and not a time, so that retention never takes it.
pub(crate) fn may_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\0')
        && !is_timed(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The local day of `2030-MM-DD`.
    fn day(month: u32, day: u32) -> i64 {
        day_number(2030, month, day)
    }

    #[test]
    fn days_and_weeks_are_counted_from_1970() {
        assert_eq!(day_number(1970, 1, 1), 0);
        assert_eq!(day_number(2000, 3, 1), 11_017);
        assert_eq!(day_number(2030, 1, 1), 21_915);
        assert_eq!(day_number(1969, 12, 31), -1);
        // 1970-01-01 was a Thursday, in the week from Monday 1969-12-29.
        assert_eq!(week_of(0), week_of(-3));
        assert_ne!(week_of(0), week_of(-4));
        assert_eq!(week_of(4), 1);
        // 2030-01-07 is a Monday, 2030-01-13 the Sunday after it.
        assert_eq!(week_of(day(1, 7)), week_of(day(1, 13)));
        assert_ne!(week_of(day(1, 6)), week_of(day(1, 7)));
    }

    #[test]
    fn forty_days_keep_seven_days_and_four_weeks() {
        // One a day from 2030-01-01 to 2030-02-09, a Saturday, newest
        // first: the boot test's days.
        let days: Vec<i64> = (0..40).rev().map(|n| day(1, 1) + n).collect();
        assert_eq!(days[0], day(2, 9));
        let kept = keep(&days, 7, 4);
        let named = |i: usize| {
            let d = days[i];
            let (month, date) =
                if d >= day(2, 1) { (2, d - day(2, 1) + 1) } else { (1, d - day(1, 1) + 1) };
            format!("{month:02}-{date:02}")
        };
        let left: Vec<String> = (0..40).filter(|&i| kept[i].any()).map(named).collect();
        // The seven latest days; then the Sundays that end the weeks
        // before them: 02-03 is a daily one already, so two more.
        assert_eq!(
            left,
            ["02-09", "02-08", "02-07", "02-06", "02-05", "02-04", "02-03", "01-27", "01-20"]
        );
        assert_eq!(kept[0], Kept { newest: true, daily: true, weekly: true });
        assert_eq!(kept[6], Kept { newest: false, daily: true, weekly: true });
        assert_eq!(kept[1], Kept { newest: false, daily: true, weekly: false });
        assert_eq!(kept[13].words(), "weekly");
        assert_eq!(kept[39].words(), "goes at the next prune");
    }

    #[test]
    fn the_newest_of_a_day_is_kept_and_gaps_do_not_count() {
        // Two on one day, then a gap of a month: days that have none are
        // not among the seven.
        let days = [day(3, 10), day(3, 10), day(2, 1), day(1, 30), day(1, 2)];
        let kept = keep(&days, 7, 4);
        assert_eq!(
            kept.iter().map(|k| k.daily).collect::<Vec<_>>(),
            [true, false, true, true, true]
        );
        assert!(!kept[1].any());
        // 01-30 and 02-01 are in one week: the newer, 02-01, is its own.
        assert_eq!(
            kept.iter().map(|k| k.weekly).collect::<Vec<_>>(),
            [true, false, true, false, true]
        );
    }

    #[test]
    fn nothing_kept_but_the_newest_when_retention_is_zero() {
        let kept = keep(&[day(1, 3), day(1, 2), day(1, 1)], 0, 0);
        assert_eq!(kept.iter().map(|k| k.any()).collect::<Vec<_>>(), [true, false, false]);
        assert!(keep(&[], 7, 4).is_empty());
    }

    #[test]
    fn a_second_prune_removes_nothing_more() {
        let days: Vec<i64> = (0..60).rev().map(|n| day(1, 1) + n * 2).collect();
        let kept = keep(&days, 7, 4);
        let left: Vec<i64> =
            days.iter().zip(&kept).filter(|(_, k)| k.any()).map(|(d, _)| *d).collect();
        let again = keep(&left, 7, 4);
        assert!(again.iter().all(|k| k.any()));
    }

    #[test]
    fn names_by_time_and_names_given() {
        assert_eq!(timed_name(2030, 2, 9, 12, 0), "2030-02-09-1200");
        assert!(is_timed("2030-02-09-1200"));
        assert!(is_timed("2030-02-09-1200.2"));
        assert!(is_timed("2030-02-09-1200.13"));
        for name in [
            "2030-02-09-120",
            "2030-02-09-1200.",
            "2030-02-09-1200.1",
            "2030-02-09-1200.02",
            "2030/02/09-1200",
            "before-update",
            "2030-02-09-1200.x",
        ] {
            assert!(!is_timed(name), "{name}");
        }
        assert!(may_name("before-update"));
        for name in ["", ".hidden", "a/b", "2030-02-09-1200", "2030-02-09-1200.2"] {
            assert!(!may_name(name), "{name:?}");
        }
    }
}
