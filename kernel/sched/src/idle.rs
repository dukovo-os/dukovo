// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which idle state an idle CPU waits in (ADR 0795).
//!
//! A deeper state saves more power but takes longer to leave, and pays
//! only if the CPU stays in it long enough: its *target residency*. So
//! the deepest state worth entering is the deepest whose target residency
//! the coming idle period will reach and whose exit latency the system
//! allows. The period cannot outlast the CPU's next timer interrupt, which
//! the kernel knows; what it cannot know is whether another interrupt or
//! another CPU's wake-up ends it sooner.
//!
//! [`Governor`] learns that from the CPU's recent idle periods. It sorts
//! each period into the bin of the deepest state it would have paid for,
//! and counts it as a *timeout* if it lasted until the timer it was
//! chosen for, or an *early wake* if something else ended it. The counts
//! decay, an eighth at each period, so they tell of the last few dozen
//! periods. To choose, it starts from the deepest state the timer allows;
//! if most recent periods ended early, before that state's residency, it
//! steps down to the bin where the median of those early wakes lies, so
//! that a CPU woken every 50 µs by a network card or a partner thread
//! waits in a state it can leave in time, and one that sleeps until its
//! tick goes deep.

/// What the governor knows of an idle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// How long leaving it takes.
    pub exit_ns: u64,
    /// How long the CPU must stay in it for it to save power.
    pub residency_ns: u64,
}

/// The most idle states the governor tells apart.
pub const MAX_STATES: usize = 8;

/// How much a new period weighs: counts are in units of this, and decay
/// by an eighth at each period, so none passes about eight times it.
const ONE: u32 = 1 << 10;

/// How close to its timer a period must last to count as a timeout:
/// wake-up and the time it takes to read the clock make it end a little
/// early.
const TIMER_SLACK_NS: u64 = 20_000;

/// One CPU's idle governor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Governor {
    /// Decayed counts of periods that lasted until their timer, by bin.
    timeouts: [u32; MAX_STATES],
    /// Decayed counts of periods that something else ended, by bin.
    early: [u32; MAX_STATES],
}

impl Governor {
    /// A governor that knows no periods yet.
    pub const fn new() -> Governor {
        Governor { timeouts: [0; MAX_STATES], early: [0; MAX_STATES] }
    }

    /// The state to enter for an idle period that the CPU's timer ends
    /// `sleep_ns` from now if nothing else does: an index into `states`,
    /// which are shallowest first and of which the first, which costs
    /// nothing to leave, is always allowed. `allowed` says which of the
    /// others may be used, and `max_exit_ns` caps their exit latency.
    pub fn choose(
        &self,
        states: &[State],
        allowed: impl Fn(usize) -> bool,
        max_exit_ns: u64,
        sleep_ns: u64,
    ) -> usize {
        let states = &states[..states.len().min(MAX_STATES)];
        let usable = |i: usize| i == 0 || (allowed(i) && states[i].exit_ns <= max_exit_ns);
        let deepest_up_to = |last: usize| (0..=last).rev().find(|&i| usable(i)).unwrap_or(0);
        let Some(by_timer) =
            (0..states.len()).rev().find(|&i| usable(i) && states[i].residency_ns <= sleep_ns)
        else {
            return 0;
        };
        if by_timer == 0 {
            return 0;
        }
        let total: u64 = self.timeouts.iter().chain(&self.early).map(|&n| u64::from(n)).sum();
        let early_below: u64 = self.early[..by_timer].iter().map(|&n| u64::from(n)).sum();
        if 2 * early_below <= total {
            return by_timer;
        }
        // Most periods ended before this state's residency: the bin that
        // holds the median of those early wakes.
        let mut seen = 0;
        for bin in (0..by_timer).rev() {
            seen += u64::from(self.early[bin]);
            if 2 * seen >= early_below {
                return deepest_up_to(bin);
            }
        }
        0
    }

    /// Learns from an idle period that lasted `idle_ns`, for which the
    /// timer was due `sleep_ns` after it began, among `states`.
    pub fn record(&mut self, states: &[State], idle_ns: u64, sleep_ns: u64) {
        let states = &states[..states.len().min(MAX_STATES)];
        for count in self.timeouts.iter_mut().chain(&mut self.early) {
            *count -= *count / 8;
        }
        if states.is_empty() {
            return;
        }
        let bin = (0..states.len()).rev().find(|&i| states[i].residency_ns <= idle_ns).unwrap_or(0);
        if idle_ns.saturating_add(TIMER_SLACK_NS) >= sleep_ns {
            self.timeouts[bin] += ONE;
        } else {
            self.early[bin] += ONE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C1, C1E, C3 and C6 of a desktop Skylake, as its firmware's _CST
    /// gives them with residencies of three times the exit latency.
    const SKYLAKE: [State; 4] = [
        State { exit_ns: 1_000, residency_ns: 0 },
        State { exit_ns: 10_000, residency_ns: 20_000 },
        State { exit_ns: 70_000, residency_ns: 210_000 },
        State { exit_ns: 85_000, residency_ns: 255_000 },
    ];

    fn all(_: usize) -> bool {
        true
    }

    #[test]
    fn a_new_governor_goes_as_deep_as_the_timer_allows() {
        let governor = Governor::new();
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 10_000_000), 3);
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 220_000), 2);
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 25_000), 1);
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 5_000), 0);
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 0), 0);
    }

    #[test]
    fn states_that_are_not_allowed_or_too_slow_to_leave_are_skipped() {
        let governor = Governor::new();
        assert_eq!(governor.choose(&SKYLAKE, |i| i != 3, u64::MAX, 10_000_000), 2);
        assert_eq!(governor.choose(&SKYLAKE, all, 50_000, 10_000_000), 1);
        assert_eq!(governor.choose(&SKYLAKE, |_| false, u64::MAX, 10_000_000), 0);
        // The first state is always there.
        assert_eq!(governor.choose(&SKYLAKE, all, 0, 10_000_000), 0);
        assert_eq!(governor.choose(&[], all, u64::MAX, 10_000_000), 0);
    }

    #[test]
    fn a_cpu_woken_every_50_us_stays_shallow() {
        let mut governor = Governor::new();
        for _ in 0..32 {
            let state = governor.choose(&SKYLAKE, all, u64::MAX, 10_000_000);
            assert!(state <= 3);
            governor.record(&SKYLAKE, 50_000, 10_000_000);
        }
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 10_000_000), 1);
    }

    #[test]
    fn a_cpu_that_sleeps_until_its_tick_goes_deep() {
        let mut governor = Governor::new();
        for _ in 0..32 {
            governor.record(&SKYLAKE, 50_000, 10_000_000);
        }
        // The load stops: from then on each period lasts until the tick.
        let mut chosen = Vec::new();
        for _ in 0..16 {
            chosen.push(governor.choose(&SKYLAKE, all, u64::MAX, 10_000_000));
            governor.record(&SKYLAKE, 10_000_000, 10_000_000);
        }
        assert_eq!(chosen[0], 1);
        assert_eq!(chosen.last(), Some(&3));
        // Within a dozen periods.
        assert!(chosen[12..].iter().all(|&state| state == 3), "{chosen:?}");
    }

    #[test]
    fn early_wakes_that_are_a_minority_do_not_hold_it_back() {
        let mut governor = Governor::new();
        for round in 0..64 {
            let idle = if round % 3 == 0 { 30_000 } else { 10_000_000 };
            governor.record(&SKYLAKE, idle, 10_000_000);
        }
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 10_000_000), 3);
    }

    #[test]
    fn the_median_early_wake_picks_the_bin() {
        let mut governor = Governor::new();
        // Early wakes at 30 µs and at 230 µs, more of the latter.
        for round in 0..64 {
            let idle = if round % 3 == 0 { 30_000 } else { 230_000 };
            governor.record(&SKYLAKE, idle, 10_000_000);
        }
        assert_eq!(governor.choose(&SKYLAKE, all, u64::MAX, 10_000_000), 2);
        // If C3 is not allowed, the next shallower state.
        assert_eq!(governor.choose(&SKYLAKE, |i| i != 2, u64::MAX, 10_000_000), 1);
    }

    #[test]
    fn a_period_that_lasted_until_its_timer_is_a_timeout() {
        let mut governor = Governor::new();
        governor.record(&SKYLAKE, 9_990_000, 10_000_000);
        assert_eq!(governor.timeouts[3], ONE);
        governor.record(&SKYLAKE, 100_000, 10_000_000);
        assert_eq!(governor.early[1], ONE);
        assert_eq!(governor.timeouts[3], ONE - ONE / 8);
        // A short timer that fired is a timeout too.
        governor.record(&SKYLAKE, 15_000, 15_000);
        assert_eq!(governor.timeouts[0], ONE);
    }

    #[test]
    fn counts_stay_bounded() {
        let mut governor = Governor::new();
        for _ in 0..10_000 {
            governor.record(&SKYLAKE, 10_000_000, 10_000_000);
        }
        assert!(governor.timeouts[3] <= 8 * ONE + 8);
    }
}
