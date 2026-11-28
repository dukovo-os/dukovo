// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Whether an idle CPU stops its tick as it halts (ADR 0812).
//!
//! A CPU's tick does nothing for it while it is idle, and each one wakes
//! it: a hundred times a second for a machine at rest, in a virtual
//! machine each time an exit to the host and back. So an idle CPU pauses
//! its tick. The kernel can pause it two ways:
//!
//! - *at once*: it writes the timer for its next sleeper alone as it
//!   halts, and again for the tick as it leaves idle. No tick wakes the
//!   CPU, at the price of the two writes, each an exit to the host in a
//!   virtual machine.
//! - *lazily*: it writes nothing. The tick that is armed still comes, once,
//!   and is not armed again; a CPU that leaves idle before it has paid
//!   nothing.
//!
//! Pausing at once pays for an idle period long enough that a tick would
//! have fallen in it, and costs a short one two writes for nothing: a
//! thread that waits 50 µs for a disk, or for another's answer, would pay
//! them at every wait. Which kind the coming period is, the CPU's recent
//! ones tell. They come in patterns: a machine at rest has long periods
//! only; a program that answers another has short ones only; a window's
//! program and the desktop take turns in short periods and then wait
//! long for the screen, frame after frame.
//!
//! [`TickPause`] guesses the way a processor guesses a branch from its
//! recent history: it remembers whether each of the last [`HISTORY`]
//! periods was long, and for each such pattern keeps a small count of
//! whether the period that followed it was long. A pattern that repeats
//! is guessed right once it has come round three times. A wrong guess
//! costs what the other way of pausing costs, no more.

/// How many of the latest idle periods make the pattern the next is
/// guessed from.
pub const HISTORY: u32 = 3;

/// The patterns of [`HISTORY`] periods.
const PATTERNS: usize = 1 << HISTORY;

/// The highest a pattern's count goes: from here two short periods in a
/// row are needed to guess short again.
const SURE: u8 = 3;

/// The count from which the period after a pattern is guessed long.
const LIKELY: u8 = 2;

/// An idle period at least this fraction of a tick long is *long*: about
/// where the chance that a tick falls in it outweighs two writes of the
/// timer, at a virtual machine's price for each.
pub const LONG_SHARE: u64 = 4;

/// One CPU's guess of whether its next idle period is long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickPause {
    /// The latest periods, one bit each, set for long: the newest lowest.
    recent: u8,
    /// For each pattern of recent periods, how surely the next was long.
    counts: [u8; PATTERNS],
}

impl TickPause {
    /// A guess that knows no periods yet: short, so nothing is written
    /// until long periods have been seen.
    pub const fn new() -> TickPause {
        TickPause { recent: 0, counts: [0; PATTERNS] }
    }

    /// Whether an idle period of `idle_ns` is long beside a tick of
    /// `tick_ns`.
    pub const fn is_long(idle_ns: u64, tick_ns: u64) -> bool {
        idle_ns >= tick_ns / LONG_SHARE
    }

    /// Whether the coming idle period is guessed long: the tick is then
    /// worth pausing at once.
    pub const fn at_once(&self) -> bool {
        self.counts[self.recent as usize] >= LIKELY
    }

    /// Learns from an idle period that halted, and was long or not.
    pub fn record(&mut self, long: bool) {
        let count = &mut self.counts[self.recent as usize];
        *count = if long { (*count + 1).min(SURE) } else { count.saturating_sub(1) };
        self.recent = ((self.recent << 1) | u8::from(long)) & (PATTERNS as u8 - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many of `periods`, taken round and round `rounds` times, a
    /// new guess gets wrong, after a first `warm` rounds that do not
    /// count.
    fn misses(periods: &[bool], warm: usize, rounds: usize) -> usize {
        let mut guess = TickPause::new();
        let mut wrong = 0;
        for round in 0..warm + rounds {
            for &long in periods {
                if round >= warm && guess.at_once() != long {
                    wrong += 1;
                }
                guess.record(long);
            }
        }
        wrong
    }

    #[test]
    fn a_period_is_long_from_a_quarter_of_a_tick() {
        assert!(!TickPause::is_long(0, 10_000_000));
        assert!(!TickPause::is_long(2_499_999, 10_000_000));
        assert!(TickPause::is_long(2_500_000, 10_000_000));
        assert!(TickPause::is_long(u64::MAX, 10_000_000));
    }

    #[test]
    fn a_new_guess_writes_nothing() {
        let guess = TickPause::new();
        assert!(!guess.at_once());
        assert_eq!(guess, TickPause::default());
    }

    #[test]
    fn a_machine_at_rest_pauses_at_once_from_its_sixth_period() {
        let mut guess = TickPause::new();
        // The first long periods each end a pattern not seen before:
        // none, one, two, then three long ones before it.
        let mut guessed = [false; 8];
        for at_once in &mut guessed {
            *at_once = guess.at_once();
            guess.record(true);
        }
        assert_eq!(guessed, [false, false, false, false, false, true, true, true]);
        assert_eq!(misses(&[true], 5, 100), 0);
    }

    #[test]
    fn short_waits_alone_never_pause_at_once() {
        assert_eq!(misses(&[false], 0, 1000), 0);
    }

    #[test]
    fn a_pattern_that_repeats_is_guessed_after_three_rounds() {
        // Two programs that take turns and then wait for a timer.
        assert_eq!(misses(&[false, true], 3, 100), 0);
        assert_eq!(misses(&[false, false, true], 3, 100), 0);
        assert_eq!(misses(&[false, true, true], 3, 100), 0);
        // A frame: three short waits and a long one.
        assert_eq!(misses(&[false, false, false, true], 3, 100), 0);
        assert_eq!(misses(&[false, true, false, false, true, true], 3, 100), 0);
    }

    #[test]
    fn a_pattern_longer_than_the_history_is_guessed_short_where_it_is_unsure() {
        // Four short waits and then a long one: the pattern before the
        // long period is the one before the fourth short one too, so one
        // of the two is guessed wrong each round, never more.
        let periods = [false, false, false, false, true];
        assert_eq!(misses(&periods, 4, 100), 100);
    }

    #[test]
    fn one_odd_period_does_not_unlearn_a_pattern() {
        let mut guess = TickPause::new();
        for _ in 0..16 {
            guess.record(true);
        }
        assert!(guess.at_once());
        // A short period among long ones: the next few patterns hold it
        // and are new, then the old one is back, and still sure.
        guess.record(false);
        for _ in 0..HISTORY {
            guess.record(true);
        }
        assert!(guess.at_once());
    }

    #[test]
    fn a_change_of_load_is_learnt_within_a_few_periods() {
        let mut guess = TickPause::new();
        for _ in 0..64 {
            guess.record(true);
        }
        let mut wrong = 0;
        for _ in 0..64 {
            wrong += usize::from(guess.at_once());
            guess.record(false);
        }
        // The patterns on the way from all long to all short, each new.
        assert!(wrong <= HISTORY as usize + usize::from(SURE), "{wrong} wrong");
        assert!(!guess.at_once());
    }

    #[test]
    fn counts_and_patterns_stay_in_range() {
        let mut guess = TickPause::new();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..100_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            guess.record(seed & (1 << 33) != 0);
            assert!((guess.recent as usize) < PATTERNS);
            assert!(guess.counts.iter().all(|&count| count <= SURE));
        }
    }
}
