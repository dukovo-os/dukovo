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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_sched::CpuSet;
use radix_sync::SpinLock;
use radix_sync::rcu::{Deferred, Queue};

use super::wait;

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

fn start(state: &mut GracePeriods) {
    state.running = true;
    state.waiting = super::online();
    state.started_ns = super::now_ns();
    RUNNING.store(true, Ordering::Release);
}

/// Waits until every RCU read section running when it was called has
/// ended. Sleeps; call it where sleeping is allowed.
pub fn synchronize() {
    try_synchronize();
}

/// The grace period a caller that starts waiting now needs, which is
/// asked for and started if none runs.
fn request(state: &mut GracePeriods) -> u64 {
    let target = state.completed + if state.running { 2 } else { 1 };
    state.wanted = state.wanted.max(target);
    if !state.running {
        start(state);
    }
    target
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
    let target = request(&mut state);
    // SAFETY: the caller hands `item` over; targets only grow, as
    // `completed` does.
    unsafe { state.deferred.push(item, target) };
    if let Some(first) = state.deferred.first_target() {
        FIRST_TARGET.store(first, Ordering::Release);
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
    let target = request(&mut STATE.lock_irq());
    wait::wait_until(key(), "rcu", || COMPLETED.load(Ordering::Acquire) >= target);
    true
}

/// CPU `cpu` is in a quiescent state. With interrupts off, on that CPU.
pub(super) fn quiescent(cpu: usize) {
    if !RUNNING.load(Ordering::Acquire) {
        return;
    }
    let done = {
        let mut state = STATE.lock();
        if !state.running || !state.waiting.contains(cpu) {
            return;
        }
        state.waiting.remove(cpu);
        // CPUs that stopped running threads meanwhile do not count.
        state.waiting = state.waiting.and(&super::online());
        if !state.waiting.is_empty() {
            return;
        }
        state.completed += 1;
        state.running = false;
        RUNNING.store(false, Ordering::Release);
        TOTAL_NS.fetch_add(super::now_ns().saturating_sub(state.started_ns), Ordering::Relaxed);
        COMPLETED.store(state.completed, Ordering::Release);
        if state.wanted > state.completed {
            start(&mut state);
        }
        true
    };
    if done {
        wait::wake_all(key());
    }
}

/// Grace periods completed.
pub fn completed() -> u64 {
    COMPLETED.load(Ordering::Relaxed)
}

/// The average length of a grace period so far, in nanoseconds.
pub fn average_ns() -> u64 {
    TOTAL_NS.load(Ordering::Relaxed) / completed().max(1)
}
