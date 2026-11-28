// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How long an idle CPU polls before it halts.
//!
//! A virtual CPU that halts exits to the hypervisor, and waking it takes
//! an interprocessor interrupt, which is another exit for the CPU that
//! sends it. Together they cost microseconds, far more than most waits for
//! a reply. So an idle virtual CPU first polls for a while, and a waker
//! that sees it polling writes a word instead of sending an interrupt.
//!
//! How long it polls adapts to how long its recent idle periods were, the
//! way Linux's cpuidle-haltpoll governor does it, with the same defaults
//! (its documentation is `Documentation/virt/guest-halt-polling.rst`):
//!
//! - the limit starts at 0, so a CPU that is idle for long never polls;
//! - an idle period that another CPU's wake-up ended after the limit but
//!   within the maximum ([`VIRTUAL`]'s 200 µs) means polling longer would
//!   have caught it, so the limit doubles (to at least the first step, at
//!   most the maximum);
//! - an idle period longer than the maximum means polling only wasted
//!   time, so the limit halves.
//!
//! Unlike Linux's governor, it learns only from wake-ups that other CPUs
//! send. One that comes from this CPU's own interrupt, a disk's or the
//! timer's, needs that interrupt whether the CPU polls or halts, and the
//! hypervisor delivers it sooner to a halted CPU, which it watches, than
//! to a running one, which it must first stop. So an idle period such an
//! interrupt ended halves the limit too, and a CPU that waits only for
//! its devices halts at once, as Linux's do (ADR 0116).
//!
//! A CPU that waits in hardware, on bare metal or with MWAIT, wakes in a
//! microsecond or two rather than several, but a thread that answers
//! another across CPUs in a few microseconds still waits that long twice
//! a round trip. So it polls too, by the same rule, within the bounds of
//! [`HARDWARE`]: a few microseconds, as Linux polls before C1 when its
//! governor expects the CPU to be woken that soon (ADR 0551).
//!
//! A wake-up from another CPU comes from a thread that runs there. So a
//! CPU polls only while some other CPU has work, a thread to run or one
//! on its way to it, and halts once none has ([`Others`], ADR 0813): what
//! would end its idle period then is an interrupt, its own or another
//! CPU's, and polling for one burns the limit, in a virtual machine the
//! host's processor, each time. A program and the one that answers it
//! keep each other polling as before; two that take turns with a timer,
//! as a window's program and the desktop do every frame, no longer poll
//! through the wait for the timer.

/// How far a CPU's limit may grow, and the first step it grows to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// The longest the CPU polls, and the idle period beyond which polling
    /// does not pay.
    pub max_ns: u64,
    /// What the limit grows to first.
    pub grow_ns: u64,
}

/// A virtual CPU that halts: Linux's cpuidle-haltpoll defaults.
pub const VIRTUAL: Bounds = Bounds { max_ns: MAX_POLL_NS, grow_ns: GROW_START_NS };

/// A CPU that waits in hardware: a few times the microsecond or two that
/// waking from MWAIT's or HLT's lightest state takes.
pub const HARDWARE: Bounds = Bounds { max_ns: 8_000, grow_ns: 2_000 };

/// The longest a virtual CPU polls, and the idle period beyond which
/// polling does not pay.
pub const MAX_POLL_NS: u64 = 200_000;

/// What a virtual CPU's limit grows to first.
pub const GROW_START_NS: u64 = 50_000;

/// One CPU's polling limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HaltPoll {
    limit_ns: u64,
    bounds: Bounds,
}

impl Default for HaltPoll {
    fn default() -> HaltPoll {
        HaltPoll::new()
    }
}

impl HaltPoll {
    /// A limit of 0 within [`VIRTUAL`]'s bounds: no polling until an idle
    /// period shows it would help.
    pub const fn new() -> HaltPoll {
        HaltPoll::within(VIRTUAL)
    }

    /// A limit of 0 within `bounds`.
    pub const fn within(bounds: Bounds) -> HaltPoll {
        HaltPoll { limit_ns: 0, bounds }
    }

    /// A limit learnt before, kept elsewhere, within [`VIRTUAL`]'s bounds:
    /// a disk's lane keeps its own in an atomic, as its waits on any CPU
    /// teach it.
    pub const fn with_limit(limit_ns: u64) -> HaltPoll {
        let limit_ns = if limit_ns > MAX_POLL_NS { MAX_POLL_NS } else { limit_ns };
        HaltPoll { limit_ns, bounds: VIRTUAL }
    }

    /// How long to poll before halting.
    pub const fn limit_ns(&self) -> u64 {
        self.limit_ns
    }

    /// Learns from an idle period that lasted `idle_ns`, polling and halted
    /// together, and that a wake-up from another CPU ended if `called`.
    pub fn record(&mut self, idle_ns: u64, called: bool) {
        let Bounds { max_ns, grow_ns } = self.bounds;
        if idle_ns > max_ns || !called {
            self.limit_ns /= 2;
        } else if idle_ns > self.limit_ns {
            self.limit_ns = (self.limit_ns * 2).clamp(grow_ns, max_ns);
        }
    }
}

/// The other CPUs, as an idle CPU looks them over for work while it
/// polls: one CPU a look, so that a poll's turn costs the same however
/// many CPUs there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Others {
    own: usize,
    count: usize,
    /// The CPU the next look is at.
    next: usize,
    /// The looks in a row that found no work.
    quiet: usize,
}

impl Others {
    /// The other CPUs of `count`, for CPU `own`.
    pub const fn of(own: usize, count: usize) -> Others {
        Others { own, count, next: own, quiet: 0 }
    }

    /// Looks at the next of the other CPUs, which `has_work` says of, and
    /// returns whether polling may still pay: `false` once every other
    /// CPU, one after another, had no work, and at once where there is no
    /// other CPU.
    pub fn may_wake(&mut self, has_work: impl FnOnce(usize) -> bool) -> bool {
        if self.count < 2 || self.own >= self.count {
            return false;
        }
        let after = |cpu: usize| if cpu + 1 >= self.count { 0 } else { cpu + 1 };
        let cpu = if self.next == self.own { after(self.next) } else { self.next };
        self.next = after(cpu);
        self.quiet = if has_work(cpu) { 0 } else { self.quiet + 1 };
        self.quiet < self.count - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cpu_alone_never_polls() {
        for count in 0..2 {
            let mut others = Others::of(0, count);
            assert!(!others.may_wake(|_| panic!("no other CPU to look at")));
        }
        // A CPU the count does not hold looks at none either.
        assert!(!Others::of(4, 4).may_wake(|_| panic!("no CPU to look at")));
    }

    #[test]
    fn two_cpus_poll_while_the_other_has_work() {
        let mut others = Others::of(1, 2);
        for _ in 0..1000 {
            assert!(others.may_wake(|cpu| {
                assert_eq!(cpu, 0);
                true
            }));
        }
        assert!(!others.may_wake(|_| false));
        // And again once it has.
        assert!(others.may_wake(|_| true));
    }

    #[test]
    fn polling_ends_once_every_other_cpu_had_no_work() {
        const COUNT: usize = 6;
        for own in 0..COUNT {
            let mut others = Others::of(own, COUNT);
            let mut seen = [0u32; COUNT];
            // Four quiet looks leave one CPU unseen: go on.
            for _ in 0..COUNT - 2 {
                assert!(others.may_wake(|cpu| {
                    seen[cpu] += 1;
                    false
                }));
            }
            // Work on the fifth starts the round again.
            assert!(others.may_wake(|cpu| {
                seen[cpu] += 1;
                true
            }));
            for _ in 0..COUNT - 2 {
                assert!(others.may_wake(|cpu| {
                    seen[cpu] += 1;
                    false
                }));
            }
            assert!(!others.may_wake(|cpu| {
                seen[cpu] += 1;
                false
            }));
            // Every other CPU was looked at twice, in turn, and its own never.
            for (cpu, looks) in seen.iter().enumerate() {
                assert_eq!(*looks, if cpu == own { 0 } else { 2 }, "CPU {cpu} from {own}");
            }
        }
    }

    #[test]
    fn work_anywhere_keeps_a_cpu_polling() {
        const COUNT: usize = 64;
        let mut others = Others::of(17, COUNT);
        // One CPU of 63 has work: every round finds it before it ends.
        for _ in 0..10 * COUNT {
            assert!(others.may_wake(|cpu| cpu == 40));
        }
        // From its last look at work, a round of the other 63 without any
        // ends the poll.
        assert!(others.may_wake(|_| true));
        for _ in 0..COUNT - 2 {
            assert!(others.may_wake(|_| false));
        }
        assert!(!others.may_wake(|_| false));
    }

    #[test]
    fn a_limit_kept_elsewhere_goes_on_learning() {
        assert_eq!(HaltPoll::with_limit(u64::MAX).limit_ns(), MAX_POLL_NS);
        let mut poll = HaltPoll::with_limit(GROW_START_NS);
        poll.record(GROW_START_NS + 1, true);
        assert_eq!(poll.limit_ns(), 2 * GROW_START_NS);
    }

    #[test]
    fn short_waits_grow_the_limit_and_long_ones_shrink_it() {
        let mut poll = HaltPoll::new();
        assert_eq!(poll.limit_ns(), 0);
        // Woken soon after halting: poll next time.
        poll.record(10_000, true);
        assert_eq!(poll.limit_ns(), GROW_START_NS);
        // Caught while polling: nothing to learn.
        poll.record(GROW_START_NS - 1, true);
        assert_eq!(poll.limit_ns(), GROW_START_NS);
        // Woken after the limit, within the maximum: double it.
        poll.record(GROW_START_NS + 1, true);
        assert_eq!(poll.limit_ns(), 2 * GROW_START_NS);
        poll.record(MAX_POLL_NS, true);
        assert_eq!(poll.limit_ns(), MAX_POLL_NS);
        poll.record(MAX_POLL_NS, true);
        assert_eq!(poll.limit_ns(), MAX_POLL_NS);
        // Idle for long: polling was wasted, so halve it.
        poll.record(MAX_POLL_NS + 1, true);
        assert_eq!(poll.limit_ns(), MAX_POLL_NS / 2);
        poll.record(u64::MAX, true);
        assert_eq!(poll.limit_ns(), MAX_POLL_NS / 4);
    }

    #[test]
    fn the_limit_stays_within_bounds() {
        let mut poll = HaltPoll::new();
        let mut seed = 0x1234_5678_9abc_def0u64;
        for _ in 0..100_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let before = poll.limit_ns();
            let idle = seed % (3 * MAX_POLL_NS);
            let called = seed & (1 << 40) != 0;
            poll.record(idle, called);
            let after = poll.limit_ns();
            assert!(after <= MAX_POLL_NS);
            if idle > MAX_POLL_NS || !called {
                assert_eq!(after, before / 2);
            } else if idle > before {
                assert!(after > before || after == MAX_POLL_NS);
            } else {
                assert_eq!(after, before);
            }
        }
    }

    #[test]
    fn a_cpu_that_waits_in_hardware_polls_a_few_microseconds() {
        let mut poll = HaltPoll::within(HARDWARE);
        assert_eq!(poll.limit_ns(), 0);
        // Woken soon after halting: poll next time.
        poll.record(1_000, true);
        assert_eq!(poll.limit_ns(), HARDWARE.grow_ns);
        assert_eq!(poll.limit_ns(), 2_000);
        // Caught while polling: nothing to learn.
        poll.record(1_999, true);
        assert_eq!(poll.limit_ns(), 2_000);
        // Woken after the limit, within the maximum: double it.
        poll.record(2_001, true);
        assert_eq!(poll.limit_ns(), 4_000);
        poll.record(4_001, true);
        assert_eq!(poll.limit_ns(), 8_000);
        poll.record(HARDWARE.max_ns, true);
        assert_eq!(poll.limit_ns(), 8_000);
        // Idle for longer than the maximum: polling was wasted.
        poll.record(8_001, true);
        assert_eq!(poll.limit_ns(), 4_000);
        poll.record(u64::MAX, true);
        assert_eq!(poll.limit_ns(), 2_000);
        for _ in 0..20 {
            poll.record(u64::MAX, true);
        }
        assert_eq!(poll.limit_ns(), 0);
    }

    #[test]
    fn a_hardware_cpu_its_own_interrupts_wake_stops_polling() {
        let mut poll = HaltPoll::within(HARDWARE);
        poll.record(3_000, true);
        poll.record(3_000, true);
        assert_eq!(poll.limit_ns(), 4_000);
        poll.record(3_000, false);
        assert_eq!(poll.limit_ns(), 2_000);
        poll.record(500, false);
        assert_eq!(poll.limit_ns(), 1_000);
        for _ in 0..20 {
            poll.record(500, false);
        }
        assert_eq!(poll.limit_ns(), 0);
        // Another CPU's wake-up starts it again.
        poll.record(500, true);
        assert_eq!(poll.limit_ns(), HARDWARE.grow_ns);
    }

    #[test]
    fn the_hardware_limit_stays_within_its_bounds() {
        let mut poll = HaltPoll::within(HARDWARE);
        let mut seed = 0x0fed_cba9_8765_4321u64;
        for _ in 0..100_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let before = poll.limit_ns();
            let idle = seed % (3 * HARDWARE.max_ns);
            let called = seed & (1 << 40) != 0;
            poll.record(idle, called);
            let after = poll.limit_ns();
            assert!(after <= HARDWARE.max_ns);
            if idle > HARDWARE.max_ns || !called {
                assert_eq!(after, before / 2);
            } else if idle > before {
                assert!(after > before || after == HARDWARE.max_ns);
                assert!(after >= HARDWARE.grow_ns);
            } else {
                assert_eq!(after, before);
            }
        }
    }

    #[test]
    fn the_bounds_are_the_ones_given() {
        assert_eq!(HaltPoll::new(), HaltPoll::within(VIRTUAL));
        assert_eq!(HaltPoll::default(), HaltPoll::within(VIRTUAL));
        assert_eq!(HaltPoll::within(HARDWARE).limit_ns(), 0);
        assert_eq!(VIRTUAL, Bounds { max_ns: 200_000, grow_ns: 50_000 });
        assert_eq!(HARDWARE, Bounds { max_ns: 8_000, grow_ns: 2_000 });
        // A limit kept elsewhere stays virtual: it grows by the virtual
        // step, not the hardware one.
        let mut poll = HaltPoll::with_limit(0);
        poll.record(1_000, true);
        assert_eq!(poll.limit_ns(), VIRTUAL.grow_ns);
        assert_eq!(HaltPoll::with_limit(HARDWARE.max_ns + 1).limit_ns(), HARDWARE.max_ns + 1);
    }

    #[test]
    fn a_cpu_idle_for_long_stops_polling() {
        let mut poll = HaltPoll::new();
        for _ in 0..3 {
            poll.record(MAX_POLL_NS, true);
        }
        assert_eq!(poll.limit_ns(), MAX_POLL_NS);
        for _ in 0..20 {
            poll.record(10_000_000, true);
        }
        assert_eq!(poll.limit_ns(), 0);
    }

    #[test]
    fn a_cpu_its_own_interrupts_wake_stops_polling() {
        let mut poll = HaltPoll::new();
        for _ in 0..3 {
            poll.record(MAX_POLL_NS, true);
        }
        assert_eq!(poll.limit_ns(), MAX_POLL_NS);
        // A disk's interrupt ends each wait, soon after it began: polling
        // would not have spared it, so the CPU learns to halt.
        poll.record(30_000, false);
        assert_eq!(poll.limit_ns(), MAX_POLL_NS / 2);
        for _ in 0..20 {
            poll.record(30_000, false);
        }
        assert_eq!(poll.limit_ns(), 0);
        // Halting, it stays at 0; another CPU's wake-up starts it again.
        poll.record(10_000, false);
        assert_eq!(poll.limit_ns(), 0);
        poll.record(10_000, true);
        assert_eq!(poll.limit_ns(), GROW_START_NS);
    }
}
