// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The arithmetic of a pipe's beat (ADR 0703).
//!
//! A beat is the moment a new frame starts, and a change shows at one. A
//! card whose hardware tells it of each frame keeps its own. The cards
//! of release 1.1 have nothing to tell, so the kernel's clock keeps
//! their beat at the mode's rate: a [`Beat`] says when each beat of a
//! pipe is, and a thread of the card sleeps from one to the next while
//! anything waits for them.
//!
//! A mode's rate is in frames in 1000 seconds, and few rates divide a
//! second evenly: at 59.94 Hz a frame lasts 16683350.02 ns. So the time
//! of a beat is worked out from its number and the rate, never added up
//! from a rounded period, and the beat of a pipe does not drift from the
//! clock however long it runs.
//!
//! Beats are numbered, and a pipe's numbers only grow: a new mode starts
//! a new [`Beat`] one past the last number of the old ([`Beat::restart`]),
//! so a program that counts frames sees each number once.

/// The nanoseconds of the 1000 seconds a mode's rate counts frames in.
const SPAN_NS: u128 = 1_000_000_000_000;

/// The beat of a pipe in one mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beat {
    /// When beat `first` was.
    start_ns: u64,
    /// The number of the beat at `start_ns`.
    first: u64,
    /// Frames in 1000 seconds; never 0.
    refresh_mhz: u32,
}

impl Beat {
    /// A beat at `refresh_mhz` frames in 1000 seconds, whose beat
    /// `first` is at `now_ns`. `None` for a rate of 0.
    pub fn start(now_ns: u64, first: u64, refresh_mhz: u32) -> Option<Beat> {
        (refresh_mhz != 0).then_some(Beat { start_ns: now_ns, first, refresh_mhz })
    }

    /// The beat of the same pipe in a new mode set at `now_ns`: its
    /// first beat is then, with the number after the latest of this one.
    pub fn restart(&self, now_ns: u64, refresh_mhz: u32) -> Option<Beat> {
        Beat::start(now_ns, self.latest(now_ns).saturating_add(1), refresh_mhz)
    }

    /// The nanoseconds from one beat to the next, to the nearest.
    pub fn period_ns(&self) -> u64 {
        let rate = u128::from(self.refresh_mhz);
        ((SPAN_NS + rate / 2) / rate) as u64
    }

    /// When beat `count` is. A number before the first is the first.
    pub fn time_of(&self, count: u64) -> u64 {
        let beats = u128::from(count.saturating_sub(self.first));
        let after = beats * SPAN_NS / u128::from(self.refresh_mhz);
        u64::try_from(u128::from(self.start_ns) + after).unwrap_or(u64::MAX)
    }

    /// The number of the latest beat at or before `now_ns`. Before the
    /// first beat it is the first.
    pub fn latest(&self, now_ns: u64) -> u64 {
        let since = u128::from(now_ns.saturating_sub(self.start_ns));
        // The most beats whose time, rounded down as `time_of` rounds
        // it, is not past `since`.
        let beats = ((since + 1) * u128::from(self.refresh_mhz) - 1) / SPAN_NS;
        self.first.saturating_add(u64::try_from(beats).unwrap_or(u64::MAX))
    }

    /// The first beat after `now_ns`: its number and its time.
    pub fn next(&self, now_ns: u64) -> (u64, u64) {
        let count = self.latest(now_ns).saturating_add(1);
        (count, self.time_of(count))
    }

    /// The last moment a change can be asked for and still show at the
    /// beat after `count`, on a card that needs `latch_ns` before a beat
    /// to have a change ready for it.
    pub fn deadline(&self, count: u64, latch_ns: u64) -> u64 {
        self.time_of(count.saturating_add(1)).saturating_sub(latch_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rates that divide a second evenly and rates that do not, from one
    /// frame a second to a thousand.
    const RATES: [u32; 9] =
        [1_000, 23_976, 30_000, 59_940, 60_000, 75_025, 143_856, 240_000, 1_000_000];

    #[test]
    fn a_rate_of_nothing_has_no_beat() {
        assert_eq!(Beat::start(0, 0, 0), None);
        assert!(Beat::start(0, 0, 60_000).is_some());
    }

    #[test]
    fn the_period_is_the_rate_turned_over() {
        let period = |mhz| Beat::start(0, 0, mhz).unwrap().period_ns();
        assert_eq!(period(60_000), 16_666_667);
        assert_eq!(period(59_940), 16_683_350);
        assert_eq!(period(144_000), 6_944_444);
        assert_eq!(period(1_000), 1_000_000_000);
    }

    #[test]
    fn each_beat_is_found_at_its_time_and_not_a_moment_before() {
        for rate in RATES {
            let beat = Beat::start(5_000, 100, rate).unwrap();
            assert_eq!(beat.time_of(100), 5_000);
            assert_eq!(beat.latest(5_000), 100);
            for count in 101..3_000 {
                let time = beat.time_of(count);
                assert!(time > beat.time_of(count - 1), "{rate}: {count}");
                assert_eq!(beat.latest(time), count, "{rate}: {count}");
                assert_eq!(beat.latest(time - 1), count - 1, "{rate}: {count}");
                assert_eq!(beat.next(time - 1), (count, time), "{rate}: {count}");
                assert_eq!(beat.next(time).0, count + 1, "{rate}: {count}");
            }
        }
    }

    #[test]
    fn a_beat_does_not_drift_from_the_clock() {
        // An hour at 59.94 Hz is 215784 frames to the nanosecond, where
        // adding up the rounded period would be 4 microseconds out.
        let beat = Beat::start(0, 0, 59_940).unwrap();
        assert_eq!(beat.time_of(215_784), 3_600_000_000_000);
        assert_eq!(beat.latest(3_600_000_000_000), 215_784);
        // And a year at 60 Hz.
        let beat = Beat::start(0, 0, 60_000).unwrap();
        let year_ns = 365 * 24 * 3_600 * 1_000_000_000u64;
        assert_eq!(beat.latest(year_ns), 365 * 24 * 3_600 * 60);
        assert_eq!(beat.time_of(365 * 24 * 3_600 * 60), year_ns);
    }

    #[test]
    fn before_its_first_beat_a_pipe_is_at_its_first() {
        let beat = Beat::start(1_000_000, 7, 60_000).unwrap();
        assert_eq!(beat.latest(0), 7);
        assert_eq!(beat.time_of(3), 1_000_000);
        assert_eq!(beat.next(0), (8, 1_000_000 + 16_666_666));
    }

    #[test]
    fn a_new_mode_goes_on_counting() {
        let beat = Beat::start(0, 0, 60_000).unwrap();
        // Beat 6 is at 100 ms exactly, so the new mode's first is 7.
        let now = 100_000_000;
        assert_eq!(beat.latest(now), 6);
        let faster = beat.restart(now, 144_000).unwrap();
        assert_eq!(faster.latest(now), 7);
        assert_eq!(faster.time_of(7), now);
        assert_eq!(faster.next(now), (8, now + 6_944_444));
        assert_eq!(beat.restart(now, 0), None);
    }

    #[test]
    fn the_deadline_is_the_next_beat_less_what_the_card_needs() {
        let beat = Beat::start(0, 0, 60_000).unwrap();
        assert_eq!(beat.deadline(0, 0), 16_666_666);
        assert_eq!(beat.deadline(0, 2_000_000), 14_666_666);
        assert_eq!(beat.deadline(59, 0), 1_000_000_000);
        // A card that needs longer than there is has a deadline of 0,
        // not one that wrapped.
        assert_eq!(beat.deadline(0, u64::MAX), 0);
    }

    #[test]
    fn the_ends_of_the_clock_do_not_wrap() {
        let beat = Beat::start(u64::MAX - 10, u64::MAX - 1, 1_000_000).unwrap();
        assert_eq!(beat.latest(u64::MAX), u64::MAX - 1);
        assert_eq!(beat.time_of(u64::MAX), u64::MAX);
        assert_eq!(beat.next(u64::MAX), (u64::MAX, u64::MAX));
        let beat = Beat::start(0, 0, u32::MAX).unwrap();
        assert_eq!(beat.latest(u64::MAX), 79_228_162_495_817_593);
    }
}
