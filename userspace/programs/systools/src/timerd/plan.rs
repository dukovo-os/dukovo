// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! When each timer runs next (ADR 0478): the arithmetic of calendar
//! times, `every`, `after_boot`, `spread` and catching up, and the check
//! of the clock against the monotonic clock. It touches no file and asks
//! no program, so that its tests run on a clock they set themselves; the
//! calendar arithmetic itself is [`super::next::next_time`].

use manifest::schedule::Calendar;

use super::next::next_time;

/// How long after timerd starts a time missed while the machine was off
/// runs, in seconds: time for the services the run needs to be up.
pub const CATCH_UP_DELAY: i64 = 20;

/// The most the clock may differ from the monotonic clock's account of
/// the time that passed, in seconds, before it counts as moved: the
/// waits of a loop that sleeps up to a minute are never exact.
pub const STEP_TOLERANCE: i64 = 5;

/// When one timer runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timing {
    /// The times of day, each read.
    pub calendars: Vec<Calendar>,
    /// Seconds from the last start to the next.
    pub every: Option<u64>,
    /// Seconds from the machine's start to the first start by `every`.
    pub after_boot: Option<u64>,
    /// The seconds a time of day is put off by: [`spread_delay`], worked
    /// out once for the timer.
    pub delay: i64,
    /// Whether a time missed while the machine was off runs once.
    pub catch_up: bool,
    /// The tzdata name the calendars are read in; the system's if none.
    pub zone: Option<String>,
}

/// The seconds from 0 to `spread` that timer `name` is put off by on the
/// machine whose seed is `seed`: the same every day and every start, and
/// different between machines, so that a fleet does not reach a server
/// at once. FNV-1a over the seed and the name, which unlike the standard
/// library's hashers never changes between releases.
#[must_use]
pub fn spread_delay(seed: &str, name: &str, spread: u64) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.bytes().chain(std::iter::once(0)).chain(name.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    i64::try_from(hash % (spread + 1)).unwrap_or(0)
}

impl Timing {
    /// The first time after `now` that the calendars match, put off by
    /// the delay, counting no time before `fence`, the time of the last
    /// run: a time already run does not run again, even when the clock
    /// has gone back past it.
    fn calendar_next(&self, now: i64, fence: Option<i64>) -> Option<i64> {
        let after = fence.map_or(now, |fence| fence.max(now)) - self.delay;
        self.calendars
            .iter()
            .filter_map(|calendar| next_time(calendar, after, self.zone.as_deref()))
            .min()
            .map(|time| time + self.delay)
    }

    /// Whether a calendar matched after `last` and by `now`: a time that
    /// passed while the machine was off.
    fn calendar_missed(&self, last: i64, now: i64) -> bool {
        self.calendars
            .iter()
            .filter_map(|calendar| next_time(calendar, last, self.zone.as_deref()))
            .any(|time| time <= now)
    }

    /// The time of the first start by `every` after the machine started
    /// at `boot`.
    fn first_every(&self, every: u64, boot: i64) -> i64 {
        boot.saturating_add(i64::try_from(self.after_boot.unwrap_or(every)).unwrap_or(i64::MAX))
    }

    /// Where a missed time is made up: soon after `now`, and not before
    /// `after_boot` when the file names one.
    fn soon(&self, now: i64, boot: i64) -> i64 {
        let boot_delay = i64::try_from(self.after_boot.unwrap_or(0)).unwrap_or(0);
        (now + CATCH_UP_DELAY).max(boot.saturating_add(boot_delay))
    }

    /// The time of the next run, when timerd has just started (`loading`)
    /// or at any other time; `last` is when it last ran, and `boot` when
    /// the machine started.
    #[must_use]
    pub fn next(&self, last: Option<i64>, now: i64, boot: i64, loading: bool) -> Option<i64> {
        let mut candidates = Vec::new();
        if !self.calendars.is_empty() {
            let missed =
                loading && self.catch_up && last.is_some_and(|l| self.calendar_missed(l, now));
            candidates.push(if missed {
                Some(self.soon(now, boot))
            } else {
                self.calendar_next(now, last)
            });
        }
        if let Some(every) = self.every {
            candidates.push(Some(self.every_next(every, last, now, boot, loading)));
        }
        candidates.into_iter().flatten().min()
    }

    /// The next start by `every`.
    fn every_next(&self, every: u64, last: Option<i64>, now: i64, boot: i64, loading: bool) -> i64 {
        let seconds = i64::try_from(every).unwrap_or(i64::MAX);
        let Some(last) = last else { return self.first_every(every, boot) };
        if !loading {
            // A clock that went back leaves the last start in the future.
            return last.min(now).saturating_add(seconds);
        }
        let due = last.saturating_add(seconds);
        if due > now {
            return self.after_boot.map_or(due, |_| due.max(self.first_every(every, boot)));
        }
        if self.catch_up { self.soon(now, boot) } else { self.first_every(every, boot) }
    }
}

/// The clock's account of itself, against the monotonic clock's.
#[derive(Clone, Copy, Debug)]
pub struct Watch {
    wall: i64,
    monotonic: i64,
}

impl Watch {
    /// A watch started at wall-clock second `wall` and monotonic second
    /// `monotonic`.
    #[must_use]
    pub fn new(wall: i64, monotonic: i64) -> Watch {
        Watch { wall, monotonic }
    }

    /// Sets the watch to the clocks' readings and says how far the wall
    /// clock moved beyond the time that passed: positive when it jumped
    /// forward, negative when it went back, `None` when it kept time.
    pub fn check(&mut self, wall: i64, monotonic: i64) -> Option<i64> {
        let passed = monotonic - self.monotonic;
        let moved = wall - self.wall - passed;
        *self = Watch { wall, monotonic };
        (moved.abs() > STEP_TOLERANCE).then_some(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every(seconds: u64) -> Timing {
        Timing {
            calendars: Vec::new(),
            every: Some(seconds),
            after_boot: None,
            delay: 0,
            catch_up: true,
            zone: None,
        }
    }

    #[test]
    fn the_spread_is_the_same_each_time_and_within_its_limit() {
        let first = spread_delay("seed-a", "backup", 3600);
        assert_eq!(first, spread_delay("seed-a", "backup", 3600));
        assert!((0..=3600).contains(&first));
        assert_eq!(spread_delay("seed-a", "backup", 0), 0);
        // Machines and timers differ: with 1000 draws from 3601 values,
        // some must.
        let delays: std::collections::BTreeSet<i64> =
            (0..1000).map(|n| spread_delay(&format!("machine-{n}"), "backup", 3600)).collect();
        assert!(delays.len() > 500);
        assert_ne!(
            spread_delay("seed-a", "backup", 1 << 30),
            spread_delay("seed-a", "other", 1 << 30)
        );
    }

    #[test]
    fn every_counts_from_the_last_start() {
        let timing = every(600);
        assert_eq!(timing.next(Some(1000), 1100, 0, false), Some(1600));
        // After a run of its own, at the time it ran.
        assert_eq!(timing.next(Some(1600), 1601, 0, false), Some(2200));
    }

    #[test]
    fn the_first_start_by_every_is_after_boot_or_one_every_after_boot() {
        assert_eq!(every(600).next(None, 5000, 4000, true), Some(4600));
        let mut timing = every(600);
        timing.after_boot = Some(60);
        assert_eq!(timing.next(None, 5000, 4000, true), Some(4060));
        assert_eq!(timing.next(None, 5000, 4000, false), Some(4060));
    }

    #[test]
    fn a_start_missed_while_off_runs_soon_once_started() {
        let timing = every(600);
        // Last ran at 1000, the machine was off, and is on at 9000.
        assert_eq!(timing.next(Some(1000), 9000, 8990, true), Some(9000 + CATCH_UP_DELAY));
        let mut never = every(600);
        never.catch_up = false;
        assert_eq!(never.next(Some(1000), 9000, 8990, true), Some(8990 + 600));
        // Not missed: its time comes as it did.
        assert_eq!(timing.next(Some(8800), 9000, 8990, true), Some(9400));
        let mut after = every(600);
        after.after_boot = Some(120);
        assert_eq!(after.next(Some(1000), 9000, 8990, true), Some(9110));
    }

    #[test]
    fn a_clock_that_went_back_does_not_run_a_start_again() {
        let timing = every(600);
        // It ran at 10000; the clock is now at 7000: the next start is
        // 600 after now, not 3600 after it.
        assert_eq!(timing.next(Some(10_000), 7000, 0, false), Some(7600));
    }

    #[test]
    fn a_watch_notices_the_clock_moving_either_way() {
        let mut watch = Watch::new(1_000_000, 500);
        assert_eq!(watch.check(1_000_030, 530), None);
        // A late wake is no move.
        assert_eq!(watch.check(1_000_093, 593), None);
        assert_eq!(watch.check(1_007_293, 600), Some(7193));
        // Back by an hour.
        assert_eq!(watch.check(1_003_660, 660), Some(-3693));
        assert_eq!(watch.check(1_003_720, 720), None);
    }

    fn calendar(fields: &str) -> Timing {
        Timing {
            calendars: vec![manifest::schedule::parse(fields).unwrap()],
            every: None,
            after_boot: None,
            delay: 0,
            catch_up: true,
            zone: Some("UTC".into()),
        }
    }

    // 2026-11-14 12:00:00 UTC.
    const NOON: i64 = 1_794_657_600;

    #[test]
    fn a_calendar_runs_at_its_next_match_put_off_by_the_delay() {
        let mut timing = calendar("30 13 * * *");
        assert_eq!(timing.next(None, NOON, NOON - 3600, false), Some(NOON + 5400));
        timing.delay = 90;
        assert_eq!(timing.next(None, NOON, NOON - 3600, false), Some(NOON + 5490));
        // Between the match and the delayed run, the run is still ahead.
        assert_eq!(timing.next(None, NOON + 5450, NOON - 3600, false), Some(NOON + 5490));
        // After it ran, the next is the day after.
        assert_eq!(
            timing.next(Some(NOON + 5490), NOON + 5491, NOON - 3600, false),
            Some(NOON + 5490 + 86_400)
        );
    }

    #[test]
    fn a_time_of_day_missed_while_off_runs_once_soon() {
        let timing = calendar("0 * * * *");
        let last = NOON - 3 * 3600 + 30;
        assert_eq!(
            timing.next(Some(last), NOON + 100, NOON + 80, true),
            Some(NOON + 100 + CATCH_UP_DELAY)
        );
        let mut no_catch_up = timing.clone();
        no_catch_up.catch_up = false;
        assert_eq!(no_catch_up.next(Some(last), NOON + 100, NOON + 80, true), Some(NOON + 3600));
        // Never run: nothing was missed.
        assert_eq!(timing.next(None, NOON + 100, NOON + 80, true), Some(NOON + 3600));
    }

    #[test]
    fn a_clock_moved_back_does_not_run_a_time_again() {
        let timing = calendar("0 * * * *");
        // It ran at 12:00; the clock went back to 10:30.
        assert_eq!(timing.next(Some(NOON), NOON - 5400, NOON - 9000, false), Some(NOON + 3600));
    }

    #[test]
    fn a_clock_moved_forward_leaves_the_times_it_passed_due_once() {
        let timing = calendar("0 * * * *");
        let due = timing.next(Some(NOON), NOON + 10, NOON - 9000, false).unwrap();
        assert_eq!(due, NOON + 3600);
        // Two hours on, it is due; once run, the next is ahead.
        assert!(due <= NOON + 7300);
        assert_eq!(
            timing.next(Some(NOON + 7300), NOON + 7301, NOON - 9000, false),
            Some(NOON + 10_800)
        );
    }
}
