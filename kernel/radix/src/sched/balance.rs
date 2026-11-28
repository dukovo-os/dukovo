// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Load balancing: CPUs with less to do pull waiting threads from busier
//! ones.
//!
//! Each CPU looks at the levels of the topology from the closest out
//! (`radix_sched::Topology`): the hardware threads of its core, its
//! package, its NUMA node, the machine. At each level it compares its
//! group of CPUs with the others there and, if one is busier by enough
//! that moving a thread makes things more even (`should_pull`), takes a
//! waiting thread from that group's busiest CPU. A busy CPU tries every
//! [`interval_ticks`] ticks of a level, so farther levels are balanced less
//! often; an idle CPU tries every level before it sleeps, if any CPU has a
//! thread waiting ([`WAITING`]). Threads whose caches are probably still
//! warm are left where they are, unless attempts at that level have
//! failed several times in a row.
//!
//! An idle CPU's tick is paused (ADR 0812), so once it sleeps it tries no
//! more by itself. A CPU with a thread waiting ticks, and at each tick
//! wakes one sleeping CPU that the thread may run on to try again
//! ([`offer`]), as Linux's busy CPUs kick one idle CPU to balance for the
//! idle ones.
//!
//! On a processor whose cores differ in speed, as Intel's performance and
//! efficient cores do (ADR 0740), each CPU has a rank ([`rank_cores`]):
//! a performance core's first hardware thread, then an efficient core,
//! then a performance core's other threads, which share their core's
//! execution units with a first thread that is busy whenever they are
//! chosen after it. A waking thread goes to the fastest idle CPU it may
//! use (`radix_sched::Topology::select_cpu`), and a thread that has run
//! alone on a CPU for [`MOVE_UP_NS`] moves to a faster idle CPU at its
//! next switch ([`move_up`]): balancing alone never moves a thread that
//! is alone on its CPU. Linux places threads on these processors much
//! the same way, from the ranks Intel's ITMT gives its CPUs. The kernel
//! flag `hybrid=off` leaves the ranks unset, and the CPUs placed alike.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use radix_hal::{CoreKind, CpuLocal, MAX_CPUS, Smp};
use radix_klog::info;
use radix_sched::{Level, interval_ticks};

use super::{CPUS, LiveLoads, TOPOLOGY, ThreadRef, adopt, borrow, online};
use crate::Arch;

/// The rank of a performance core's first hardware thread, the fastest.
pub(super) const FASTEST: u8 = 3;
/// The rank of an efficient core.
const EFFICIENT: u8 = 2;
/// The rank of a performance core's other hardware threads.
const PERFORMANCE_SIBLING: u8 = 1;

/// How long a thread runs alone on a slower CPU before it moves to a
/// faster idle one, checked at each tick. A thread that has run for less
/// was placed as it woke, against the CPUs idle then, and may block again
/// before a move pays for the caches it leaves; one that has run this long
/// is work that a faster core finishes sooner.
pub(super) const MOVE_UP_NS: u64 = 4_000_000;

/// A [`CoreKind`] as a byte, for a CPU's slot.
pub(super) const fn kind_code(kind: CoreKind) -> u8 {
    match kind {
        CoreKind::Only => 0,
        CoreKind::Performance => 1,
        CoreKind::Efficient => 2,
    }
}

/// Ranks the online CPUs by the kinds of core they said they are, if
/// they are of more than one kind and `off` (`hybrid=off`) is not set.
/// Call once, when the CPUs are all up.
pub fn rank_cores(off: bool) {
    let cpus = online();
    let count = |kind: CoreKind| {
        cpus.iter()
            .filter(|&cpu| CPUS.get(cpu).kind.load(Ordering::Relaxed) == kind_code(kind))
            .count()
    };
    let (performance, efficient) = (count(CoreKind::Performance), count(CoreKind::Efficient));
    let Some(topology) = TOPOLOGY.get() else { return };
    if performance == 0 || efficient == 0 {
        return;
    }
    if off {
        info!(
            "Scheduler: {performance} CPUs on performance cores and {efficient} on efficient              cores, placed alike (hybrid=off)"
        );
        return;
    }
    for cpu in cpus.iter() {
        let slot = CPUS.get(cpu);
        let first = Arch::cpu_place(cpu).is_none_or(|place| place.thread == 0);
        let rank = match slot.kind.load(Ordering::Relaxed) {
            code if code == kind_code(CoreKind::Performance) && first => FASTEST,
            code if code == kind_code(CoreKind::Performance) => PERFORMANCE_SIBLING,
            _ => EFFICIENT,
        };
        slot.rank.store(rank, Ordering::Relaxed);
        if rank == FASTEST {
            FASTEST_CPUS.0[cpu / 64].fetch_or(1 << (cpu % 64), Ordering::Relaxed);
        }
    }
    topology.set_ranked(true);
    info!(
        "Scheduler: {performance} CPUs on performance cores and {efficient} on efficient cores;          threads go to idle performance cores first"
    );
}

/// The CPUs [`rank_cores`] ranked fastest, a bit each, in a line of its
/// own that nothing writes once they are ranked: [`fastest`] reads it
/// rather than a CPU's own line, which its waker would then take again.
static FASTEST_CPUS: FastestCpus = FastestCpus([const { AtomicU64::new(0) }; MAX_CPUS / 64]);

#[repr(C, align(64))]
struct FastestCpus([AtomicU64; MAX_CPUS / 64]);

/// Whether a thread woken for idle CPU `cpu` would go nowhere else if
/// `cpu` may run it (`choose_cpu`): the CPUs are not ranked, or `cpu` is
/// among the fastest. False before the topology is known, when
/// `choose_cpu` takes the first CPU allowed.
pub(super) fn fastest(cpu: usize) -> bool {
    let Some(topology) = TOPOLOGY.get() else { return false };
    !topology.is_ranked()
        || FASTEST_CPUS.0[cpu / 64].load(Ordering::Relaxed) & (1 << (cpu % 64)) != 0
}

/// From the timer tick on CPU `cpu`, busy, with interrupts off: when the
/// thread running there is alone and has run for [`MOVE_UP_NS`], and a
/// faster CPU it may use is idle, asks for a switch that moves it there.
pub(super) fn move_up(cpu: usize, now: u64) {
    let Some(topology) = TOPOLOGY.get().filter(|topology| topology.is_ranked()) else { return };
    let slot = CPUS.get(cpu);
    if slot.rank.load(Ordering::Relaxed) == FASTEST || slot.running.load(Ordering::Relaxed) != 1 {
        return;
    }
    let Some(running) = borrow(slot.current.load(Ordering::Relaxed)) else { return };
    let since = running.stats.last_ran_ns.load(Ordering::Relaxed);
    if now.saturating_sub(since) < MOVE_UP_NS {
        return;
    }
    if let Some(to) = topology.faster_idle(cpu, &running.allowed(), &LiveLoads) {
        slot.move_to.store(to + 1, Ordering::Relaxed);
        slot.resched.request();
    }
}

/// Whether `thread`, running alone on this CPU, may still move to `to`,
/// which [`move_up`] chose: `to` runs threads, is idle, and the thread
/// may use it.
pub(super) fn may_move_up(thread: &ThreadRef, to: usize) -> bool {
    let target = CPUS.get(to);
    thread.allowed().contains(to)
        && target.online.load(Ordering::Relaxed)
        && target.running.load(Ordering::Relaxed) == 0
}

/// Failed attempts at a level after which a cache-hot thread may move.
const FORCE_AFTER: u32 = 4;

fn level_index(level: Level) -> usize {
    match level {
        Level::Core => 0,
        Level::Package => 1,
        Level::Node => 2,
        Level::Machine => 3,
    }
}

/// From the timer tick on CPU `cpu`, with interrupts off.
pub(super) fn tick(cpu: usize, now: u64) {
    let Some(topology) = TOPOLOGY.get() else { return };
    let ticks = CPUS.get(cpu).ticks.load(Ordering::Relaxed);
    for level in topology.levels(cpu) {
        if ticks.is_multiple_of(interval_ticks(level)) && pull(cpu, level, now) {
            return;
        }
    }
}

/// How many ticks apart a CPU with a thread waiting offers it to another
/// resting CPU than the nearest ([`offer`]): the nearest then has had
/// more tries in a row than force a warm thread over
/// ([`FORCE_AFTER`]).
const OFFER_ROUND: u64 = 8;

/// From the timer tick on CPU `cpu`, with interrupts off: if a thread
/// waits here, wakes one CPU that rests in its idle loop and may run it,
/// which then tries to pull as it does before it sleeps ([`idle`]). Its
/// own tick would have woken it for that before idle CPUs paused theirs
/// (ADR 0812).
///
/// The CPU woken is the next one up from this, most often of the same
/// package; every [`OFFER_ROUND`]th tick it is the next one up from a
/// place that moves on by one CPU each such tick. So the same CPU is
/// asked tick after tick, as often as it takes to pull a thread whose
/// caches are warm, and one whose own rules keep it from pulling (its
/// group is as busy as this one's) does not keep the others from being
/// asked.
pub(super) fn offer(cpu: usize) {
    let slot = CPUS.get(cpu);
    if !slot.waiting.load(Ordering::Relaxed) {
        return;
    }
    let count = Arch::cpu_count().min(MAX_CPUS);
    let ticks = slot.ticks.load(Ordering::Relaxed);
    let turn = if ticks.is_multiple_of(OFFER_ROUND) { ticks / OFFER_ROUND } else { 0 };
    let from = cpu + 1 + (turn % count as u64) as usize;
    let rests = {
        let guard = slot.rq.lock();
        let Some(rq) = guard.as_ref() else { return };
        (0..count)
            .map(|step| (from + step) % count)
            .find(|&other| other != cpu && Arch::resting(other) && rq.waits_for(other))
    };
    if let Some(other) = rests {
        super::kick(other);
    }
}

/// How many CPUs have a thread waiting for them, as they last published
/// their loads ([`waiting_changed`]). A CPU pulls only from one that has
/// (`Topology::find_busiest`), so an idle CPU reads the others' loads
/// only while this says there is one, as Linux's looks only while its
/// root domain is overloaded: two threads that take turns on two CPUs, as
/// a lock's do, leave it at 0. In a line of its own, which a CPU writes
/// only as it starts or stops having a thread wait.
static WAITING: Waiting = Waiting(AtomicU32::new(0));

#[repr(C, align(64))]
struct Waiting(AtomicU32);

/// A CPU's run queue, locked, now has a thread waiting (`waiting`) or no
/// longer has, as the CPU's published load says from now on.
pub(super) fn waiting_changed(waiting: bool) {
    if waiting {
        WAITING.0.fetch_add(1, Ordering::Relaxed);
    } else {
        WAITING.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// From the idle loop of CPU `cpu`, with interrupts off: tries every
/// level, closest first, until a thread comes.
pub(super) fn idle(cpu: usize, now: u64) -> bool {
    if WAITING.0.load(Ordering::Relaxed) == 0 {
        return false;
    }
    let Some(topology) = TOPOLOGY.get() else { return false };
    topology.levels(cpu).any(|level| pull(cpu, level, now))
}

/// Tries to pull one thread to `cpu` across `level`.
fn pull(cpu: usize, level: Level, now: u64) -> bool {
    let Some(topology) = TOPOLOGY.get() else { return false };
    let Some(busiest) = topology.find_busiest(cpu, level, &LiveLoads) else { return false };
    if busiest == cpu {
        return false;
    }
    let failures = &CPUS.get(cpu).balance_failures[level_index(level)];
    let force = failures.load(Ordering::Relaxed) >= FORCE_AFTER;
    // Both queues, in CPU order.
    let (first, second) = (cpu.min(busiest), cpu.max(busiest));
    let mut low = CPUS.get(first).rq.lock();
    let mut high = CPUS.get(second).rq.lock_nested(1);
    let (Some(low_rq), Some(high_rq)) = (low.as_mut(), high.as_mut()) else { return false };
    let (here, there) = if first == cpu { (low_rq, high_rq) } else { (high_rq, low_rq) };
    match there.take_for(cpu, now, level, force) {
        Some((thread, class)) => {
            adopt(here, cpu, thread, class);
            here.pulled[level_index(level)] += 1;
            CPUS.get(cpu).publish(here);
            CPUS.get(busiest).publish(there);
            failures.store(0, Ordering::Relaxed);
            let preempt = here.wants_preempt();
            drop(high);
            drop(low);
            if preempt {
                CPUS.get(cpu).resched.request();
            }
            true
        }
        None => {
            failures.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}
