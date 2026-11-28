// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! RCU grace periods, quiescent-state based.
//!
//! An RCU read section (`radix_sync::rcu`) only holds off preemption, so a
//! CPU that switches threads, goes idle, or takes a timer tick in code that
//! could have been preempted is not inside any read section that began
//! before: it has passed a *quiescent state*. A grace period starts with
//! the set of CPUs that run threads and ends once each has passed one;
//! then every read section that was running when it started has ended.
//!
//! A CPU that *rests*, halted in its idle loop (`CpuLocal::rest`), is not
//! waited for (ADR 0812): it is in no read section, and it passes no
//! quiescent state by itself once its tick is paused. A read section it
//! begins later, in the handler of the interrupt that ends its rest,
//! began after the grace period did, and sees what the grace period's
//! caller left. That holds on a machine where a write reaches other CPUs
//! late, because both sides pass a full barrier in between: the caller
//! after it took its item away and before the grace period looks which
//! CPUs rest ([`request`]), the resting CPU after it wrote that it rests
//! no longer and before its handler reads. Either the grace period sees
//! the rest ended, and waits for the CPU, or the handler reads after the
//! taking. A CPU about to rest writes that it does and then passes a
//! quiescent state, reading whether a grace period runs, while a grace
//! period writes that it runs and then reads which CPUs rest, all four
//! sequentially consistent: either the grace period sees the CPU rest,
//! or the CPU sees the grace period and leaves it.
//!
//! [`synchronize`] waits for the end of a grace period that starts after
//! it is called: the next one, or the one after if one is already under
//! way (a reader that started after the running one began might still be
//! inside). Grace periods run one at a time and are shared, so many
//! callers waiting at once cost one grace period, not one each.
//!
//! [`defer`] queues a call for the end of the grace period `synchronize`
//! would wait for, and returns; the `rcu` thread makes the calls whose
//! grace period has completed, in the order they came.

use core::ptr::NonNull;
use core::sync::atomic::{self, AtomicBool, AtomicU64, Ordering};

use radix_hal::CpuLocal;
use radix_sched::CpuSet;
use radix_sync::SpinLock;
use radix_sync::rcu::{Deferred, Queue};

use super::wait;
use crate::Arch;

struct GracePeriods {
    /// Grace periods completed.
    completed: u64,
    /// Whether one is under way.
    running: bool,
    /// CPUs it still waits for.
    waiting: CpuSet,
    /// The highest grace period someone waits for.
    wanted: u64,
    started_ns: u64,
    /// Deferred calls, for the `rcu` thread.
    deferred: Queue,
}

static STATE: SpinLock<GracePeriods> = SpinLock::new(GracePeriods {
    completed: 0,
    running: false,
    waiting: CpuSet::EMPTY,
    wanted: 0,
    started_ns: 0,
    deferred: Queue::new(),
});
/// Mirrors of the state, read without the lock.
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Total time grace periods took, for the self-test's report.
static TOTAL_NS: AtomicU64 = AtomicU64::new(0);
/// The grace period the first deferred call waits for; `u64::MAX` with
/// none. A mirror, for the `rcu` thread's wait.
static FIRST_TARGET: AtomicU64 = AtomicU64::new(u64::MAX);
/// Whether the `rcu` thread runs.
static THREAD: AtomicBool = AtomicBool::new(false);

fn key() -> usize {
    &raw const COMPLETED as usize
}

/// Starts a grace period, which waits for the CPUs that run threads and
/// do not rest. With every CPU at rest it has none to wait for and is
/// complete at once, and so is the next if one is wanted. Returns whether
/// any completed: the caller then wakes who waits for them, once it has
/// let go of the state.
fn start(state: &mut GracePeriods) -> bool {
    let mut completed = false;
    loop {
        state.running = true;
        state.started_ns = super::now_ns();
        // Before the look at who rests: a CPU that begins to rest after
        // the look reads this, and passes a quiescent state.
        RUNNING.store(true, Ordering::SeqCst);
        let online = super::online();
        state.waiting = online;
        for cpu in online.iter().filter(|&cpu| Arch::resting(cpu)) {
            state.waiting.remove(cpu);
        }
        if !state.waiting.is_empty() {
            return completed;
        }
        complete(state);
        completed = true;
        if state.wanted <= state.completed {
            return true;
        }
    }
}

/// Ends the grace period under way.
fn complete(state: &mut GracePeriods) {
    state.completed += 1;
    state.running = false;
    RUNNING.store(false, Ordering::SeqCst);
    TOTAL_NS.fetch_add(super::now_ns().saturating_sub(state.started_ns), Ordering::Relaxed);
    COMPLETED.store(state.completed, Ordering::Release);
}

/// Waits until every RCU read section running when it was called has
/// ended. Sleeps; call it where sleeping is allowed.
pub fn synchronize() {
    try_synchronize();
}

/// The grace period a caller that starts waiting now needs, which is
/// asked for and started if none runs, and whether grace periods
/// completed as it started ([`start`]).
fn request(state: &mut GracePeriods) -> (u64, bool) {
    // What the caller took away has reached every CPU before any grace
    // period that it waits for looks which CPUs rest.
    atomic::fence(Ordering::SeqCst);
    let target = state.completed + if state.running { 2 } else { 1 };
    state.wanted = state.wanted.max(target);
    let completed = !state.running && start(state);
    (target, completed)
}

/// Queues `item`'s call for the `rcu` thread, to run once every read
/// section running now has ended. With one CPU there is no other read
/// section: it runs now.
///
/// # Safety
///
/// As [`radix_sync::rcu::defer`].
#[allow(unsafe_code)]
pub unsafe fn defer(item: NonNull<Deferred>) {
    if super::online().len() <= 1 {
        // The caller is in no read section, and no other CPU runs.
        // SAFETY: the caller's guarantee.
        unsafe { Deferred::call(item) };
        return;
    }
    let mut state = STATE.lock_irq();
    let (target, completed) = request(&mut state);
    // SAFETY: the caller hands `item` over; targets only grow, as
    // `completed` does.
    unsafe { state.deferred.push(item, target) };
    if let Some(first) = state.deferred.first_target() {
        FIRST_TARGET.store(first, Ordering::Release);
    }
    drop(state);
    if completed {
        wake_waiters();
    }
}

/// Starts the `rcu` thread, which makes deferred calls.
pub fn start_thread() {
    let options = super::Options::new().policy(super::Policy::Fair { nice: -5 });
    match super::spawn("rcu", run, 0, options) {
        Ok(_) => THREAD.store(true, Ordering::Release),
        Err(err) => radix_klog::error!("RCU: could not start the thread for deferred calls: {err}"),
    }
}

/// The `rcu` thread: waits for the first deferred call's grace period,
/// then makes every call whose grace period has completed.
fn run(_: usize) {
    loop {
        wait::wait_until(key(), "rcu calls", || {
            FIRST_TARGET.load(Ordering::Acquire) <= COMPLETED.load(Ordering::Acquire)
        });
        loop {
            let ready = {
                let mut state = STATE.lock_irq();
                let completed = state.completed;
                let ready = state.deferred.pop_ready(completed);
                let first = state.deferred.first_target().unwrap_or(u64::MAX);
                FIRST_TARGET.store(first, Ordering::Release);
                ready
            };
            let Some(item) = ready else { break };
            // SAFETY: queued by `defer`, whose caller made it safe to call
            // once its grace period completed, as it now has.
            #[allow(unsafe_code)]
            unsafe {
                Deferred::call(item);
            }
        }
    }
}

/// [`synchronize`], saying whether it could: false where sleeping is not
/// allowed and other CPUs run, where it returns at once.
pub fn try_synchronize() -> bool {
    if super::online().len() <= 1 {
        // With one CPU, a thread that can call this is not in a read
        // section, and no other CPU could be.
        return true;
    }
    if !super::can_sleep() {
        // Nothing safe to do but return; the lock checker reports such
        // callers of `synchronize`.
        return false;
    }
    let (target, completed) = request(&mut STATE.lock_irq());
    if completed {
        wake_waiters();
    }
    wait::wait_until(key(), "rcu", || COMPLETED.load(Ordering::Acquire) >= target);
    true
}

/// CPU `cpu` is in a quiescent state. With interrupts off, on that CPU.
pub(super) fn quiescent(cpu: usize) {
    if pass(cpu) {
        wake_waiters();
    }
}

/// [`quiescent`] without its wake-up: returns whether grace periods
/// completed, whose waiters the caller then wakes ([`wake_waiters`]). For
/// a CPU that has said it rests, which until it has said otherwise may
/// read nothing a grace period guards, as a wake-up does: this takes the
/// grace periods' own lock and no more.
pub(super) fn pass(cpu: usize) -> bool {
    // Sequentially consistent, for a CPU that has just written that it
    // rests (`start`).
    if !RUNNING.load(Ordering::SeqCst) {
        return false;
    }
    let mut state = STATE.lock();
    if !state.running || !state.waiting.contains(cpu) {
        return false;
    }
    state.waiting.remove(cpu);
    // CPUs that stopped running threads meanwhile do not count.
    state.waiting = state.waiting.and(&super::online());
    if !state.waiting.is_empty() {
        return false;
    }
    complete(&mut state);
    if state.wanted > state.completed {
        start(&mut state);
    }
    true
}

/// Wakes who waits for a grace period, after one completed.
pub(super) fn wake_waiters() {
    wait::wake_all(key());
}

/// Grace periods completed.
pub fn completed() -> u64 {
    COMPLETED.load(Ordering::Relaxed)
}

/// The average length of a grace period so far, in nanoseconds.
pub fn average_ns() -> u64 {
    TOTAL_NS.load(Ordering::Relaxed) / completed().max(1)
}
