// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Waiting: the wait table, mutexes, and priority inheritance.
//!
//! One table holds every thread that waits for something, keyed by the
//! address of what it waits for. A thread waits at most once at a time,
//! so the table has room for every thread ([`super::make_room`]) and never
//! allocates. Two kinds of wait use it:
//!
//! - [`wait_until`] sleeps until a condition holds; whoever makes it true
//!   calls [`wake_all`] with the same key. The condition is checked with
//!   the table locked, and the waker takes the lock after changing it, so
//!   no wake-up is lost.
//! - A contended [`radix_sync::Mutex`] queues its waiters here, most
//!   urgent first, through the hooks ([`mutex_wait`], [`mutex_wake`]).
//!   A waiter first spins while the owner runs on a CPU, and queues only
//!   when it does not, or after a bounded spin. The owner lets
//!   go by handing the lock straight to a real-time waiter, or to one
//!   that has lost it too often; otherwise it lets the lock go and wakes
//!   the first waiter to try again, so that a running thread may take it
//!   meanwhile (ADR 0368).
//!
//! A key may be both: an object whose first field is a mutex has that
//! mutex's address, and objects wait on their own address. So the two
//! kinds of waiter are told apart by their `owner`, which only a mutex
//! waiter has: a hand-off goes only to a mutex waiter, and [`wake_all`]
//! wakes only the others (Phase 8: a pipe's mutex handed to a reader that
//! was waiting for data left both ends waiting for good).
//!
//! # Priority inheritance
//!
//! A thread waiting for a mutex lends its real-time priority to the owner,
//! and through it along the chain of mutexes the owner itself waits for:
//! a thread's lent priority (`boost`) is the highest among the waiters on
//! the mutexes it holds. When it lets a mutex go, it keeps only what the
//! waiters on its other mutexes lend it, and the new owner inherits the
//! remaining waiters. So a low-priority thread holding a lock that a
//! real-time thread needs runs at that thread's priority until it lets go,
//! and a thread of middle priority cannot keep both waiting (the classic
//! priority inversion). Fair waiters lend nothing: among fair threads
//! the fair class already bounds how long anyone waits for the CPU.
//!
//! When threads queue for a mutex far more than expected, `/processes/stat`
//! shows it: `mutex_contended` counts the times a thread found one taken,
//! `mutex_waits` the sleeps, `mutex_handoffs` and `mutex_wakes` the two
//! ways of letting go; `kdb`'s scheduler report
//! gives the handoffs, and the `sched-mutex` and `sched-pi` boot tests
//! cover both ways.
//!
//! Lock order: this table's lock, then the registry's or a thread's, then a
//! run queue's.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use radix_hal::{CpuLocal, MAX_CPUS};
use radix_mm::Kmalloc;
use radix_sync::{RawMutex, SpinLock};

use super::thread::ThreadRef;
use super::{current, registry_find, set_boost};
use crate::Arch;
use crate::percpu::{PerCpu, Slot};

/// The longest chain of mutex owners a priority is lent along.
const MAX_CHAIN: usize = 16;

/// The owner queued mutex waiters record while their mutex has none: it
/// was let go to them at large ([`mutex_wake`]), so there is nobody to
/// lend to. Odd, so never a thread's token, and not 0, which marks a
/// condition waiter.
const NO_OWNER: usize = 1;

/// How many times a fair waiter may wake to find its mutex taken by a
/// thread that never slept before the mutex is handed to it outright.
/// Each such loss is one critical section of another thread, so no
/// waiter waits for ever.
const LOSSES_BEFORE_HANDOFF: u8 = 4;

/// While its owner runs, a mutex waiter spins instead of sleeping: it
/// checks the owner this many times, with [`SPIN_PAUSES`] pause
/// instructions between checks, before it queues anyway: 4,096 pauses,
/// from some microseconds to a fifth of a millisecond as the CPU's
/// `pause` is quick or slow. The kernel is preemptible outside spin
/// locks, so a spinner still gives way to anything more urgent at a tick.
const SPIN_CHECKS: u32 = 512;
const SPIN_PAUSES: u32 = 8;

/// A waiting thread.
struct Waiter {
    thread: ThreadRef,
    key: usize,
    /// The real-time priority it lends (mutex waits only).
    prio: u8,
    /// Arrival order, for first come first served among equals.
    seq: u64,
    /// The owner's token, for mutex waits ([`NO_OWNER`] while the mutex
    /// is let go to its waiters at large); 0 otherwise.
    owner: usize,
    /// A fair mutex waiter that has woken to find its mutex taken too
    /// often: the mutex is handed to it outright next time.
    starved: bool,
}

impl Waiter {
    /// Whether it waits for `key` as a mutex.
    fn for_mutex(&self, key: usize) -> bool {
        self.key == key && self.owner != 0
    }
}

struct Table {
    waiters: Vec<Waiter, Kmalloc>,
    seq: u64,
}

impl Table {
    fn push(&mut self, waiter: Waiter) {
        if self.waiters.push_within_capacity(waiter).is_err() {
            // Room for every thread is made before the thread exists.
            panic!("Scheduler: the wait table had no room");
        }
    }

    fn remove(&mut self, thread: &ThreadRef, key: usize) -> bool {
        match self.waiters.iter().position(|w| w.key == key && ThreadRef::ptr_eq(&w.thread, thread))
        {
            Some(index) => {
                self.waiters.swap_remove(index);
                true
            }
            None => false,
        }
    }

    /// The most urgent waiter for the mutex at `key`: highest priority,
    /// then the first to come.
    fn best(&self, key: usize) -> Option<usize> {
        self.waiters
            .iter()
            .enumerate()
            .filter(|(_, w)| w.for_mutex(key))
            .max_by_key(|(_, w)| (w.prio, core::cmp::Reverse(w.seq)))
            .map(|(index, _)| index)
    }

    /// Whether a thread waits for the mutex at `key`.
    fn has(&self, key: usize) -> bool {
        self.waiters.iter().any(|w| w.for_mutex(key))
    }

    /// The priority the waiters on mutexes owned by `owner` lend it.
    fn lent_to(&self, owner: usize) -> u8 {
        self.waiters.iter().filter(|w| w.owner == owner).map(|w| w.prio).max().unwrap_or(0)
    }
}

static TABLE: SpinLock<Option<Table>> = SpinLock::new(None);

/// What the table counts, on each CPU, for the debugger and
/// `/processes/stat`.
///
/// Each CPU counts on its own line, and the counts are added up when read
/// (ADR 0405): threads of one process, contending for its capability
/// space's lock on several CPUs, would otherwise pass one line between
/// them on every open and close. A thread moved to another CPU between
/// reading its number and counting adds to the first CPU's line, which
/// the atomic add keeps right.
struct Counts {
    wakeups: AtomicU64,
    handoffs: AtomicU64,
    mutex_contended: AtomicU64,
    mutex_waits: AtomicU64,
    mutex_wakes: AtomicU64,
    boosts: AtomicU64,
}

impl Counts {
    const fn new() -> Counts {
        Counts {
            wakeups: AtomicU64::new(0),
            handoffs: AtomicU64::new(0),
            mutex_contended: AtomicU64::new(0),
            mutex_waits: AtomicU64::new(0),
            mutex_wakes: AtomicU64::new(0),
            boosts: AtomicU64::new(0),
        }
    }
}

static COUNTS: PerCpu<Counts> = PerCpu::new([const { Slot::new(Counts::new()) }; MAX_CPUS]);

/// Counts one more of what `count` picks, on the running CPU.
#[inline(always)]
fn count(count: impl FnOnce(&Counts) -> &AtomicU64) {
    count(COUNTS.get(Arch::current_cpu())).fetch_add(1, Ordering::Relaxed);
}

/// What `count` picks, added up over every CPU.
fn total(count: impl Fn(&Counts) -> &AtomicU64) -> u64 {
    (0..MAX_CPUS).map(|cpu| count(COUNTS.get(cpu)).load(Ordering::Relaxed)).sum()
}

/// Sets the table up with room for `room` waiters.
pub(super) fn init(room: usize, alloc: Kmalloc) -> bool {
    let mut waiters = Vec::new_in(alloc);
    if waiters.try_reserve_exact(room).is_err() {
        return false;
    }
    *TABLE.lock_irq() = Some(Table { waiters, seq: 0 });
    true
}

/// Gives the table room for `room` waiters; allocates outside the lock.
pub(super) fn grow(room: usize, alloc: Kmalloc) -> bool {
    let mut fresh = Vec::new_in(alloc);
    if fresh.try_reserve_exact(room).is_err() {
        return false;
    }
    let old = {
        let mut table = TABLE.lock_irq();
        let Some(table) = table.as_mut() else { return false };
        if table.waiters.capacity() >= room {
            return true;
        }
        while let Some(waiter) = table.waiters.pop() {
            let _ = fresh.push_within_capacity(waiter);
        }
        core::mem::replace(&mut table.waiters, fresh)
    };
    drop(old);
    true
}

/// Sleeps until `condition` holds; someone who makes it true calls
/// [`wake_all`] with `key`. `condition` runs with the table locked and
/// interrupts off, so it must only read (atomics, typically).
pub fn wait_until(key: usize, reason: &'static str, condition: impl Fn() -> bool) {
    let Some(me) = current() else {
        // Before the scheduler runs there is only the boot thread.
        while !condition() {
            core::hint::spin_loop();
        }
        return;
    };
    loop {
        {
            let mut table = TABLE.lock_irq();
            let Some(table) = table.as_mut() else { return };
            if condition() {
                return;
            }
            table.seq += 1;
            let seq = table.seq;
            table.push(Waiter { thread: me.clone(), key, prio: 0, seq, owner: 0, starved: false });
        }
        super::block(reason);
        // Still queued after a spurious wake-up: leave, check, queue again.
        if let Some(table) = TABLE.lock_irq().as_mut() {
            table.remove(&me, key);
        }
    }
}

/// Like [`wait_until`], but gives up at `deadline_ns` (on the kernel's
/// clock). Returns whether the condition held.
pub fn wait_until_or_timeout(
    key: usize,
    reason: &'static str,
    deadline_ns: u64,
    condition: impl Fn() -> bool,
) -> bool {
    let Some(me) = current() else { return condition() };
    loop {
        {
            let mut table = TABLE.lock_irq();
            let Some(table) = table.as_mut() else { return condition() };
            if condition() {
                return true;
            }
            if super::now_ns() >= deadline_ns {
                return false;
            }
            table.seq += 1;
            let seq = table.seq;
            table.push(Waiter { thread: me.clone(), key, prio: 0, seq, owner: 0, starved: false });
        }
        super::sleep_until_inner(&me, deadline_ns, reason);
        if let Some(table) = TABLE.lock_irq().as_mut() {
            table.remove(&me, key);
        }
    }
}

/// Wakes every thread waiting on `key`.
pub fn wake_all(key: usize) {
    let mut table = TABLE.lock_irq();
    let Some(table) = table.as_mut() else { return };
    let mut index = 0;
    while index < table.waiters.len() {
        if table.waiters[index].key == key && table.waiters[index].owner == 0 {
            let waiter = table.waiters.swap_remove(index);
            count(|c| &c.wakeups);
            super::wake(&waiter.thread);
        } else {
            index += 1;
        }
    }
}

/// The hook behind a contended [`radix_sync::Mutex`]: spins while the
/// owner runs, since it will let go soon, then queues the caller, lends
/// its priority along the chain of owners, and sleeps until the lock is
/// handed to it, or let go to the waiters, when it tries again.
pub(super) fn mutex_wait(mutex: &RawMutex) {
    // Counted here, off the fast path: taking a free mutex costs nothing
    // more for it.
    count(|c| &c.mutex_contended);
    let Some(me) = current() else { return spin(mutex, super::NO_TOKEN) };
    let token = me.token();
    // The waiter keeps its place in the queue each time it tries again.
    let mut seq = None;
    let mut losses: u8 = 0;
    loop {
        if spin_while_owner_runs(mutex, token) {
            return;
        }
        match queue(mutex, &me, &mut seq, losses >= LOSSES_BEFORE_HANDOFF) {
            Queued::Took => return,
            Queued::NoTable => return spin(mutex, token),
            Queued::Waiting => {}
        }
        // `mutex_wake` writes the word before it clears `blocked_on`, so
        // a waiter that sees `blocked_on` clear sees whether the mutex is
        // its own. Still set: the wake-up had no cause.
        while me.blocked_on.load(Ordering::Acquire) != 0 {
            super::block("mutex");
        }
        if mutex.owner() == token {
            return;
        }
        losses = losses.saturating_add(1);
    }
}

/// What [`queue`] did.
enum Queued {
    /// The mutex was let go meanwhile, and the caller took it.
    Took,
    /// The caller is queued.
    Waiting,
    /// The table is not set up: no threads sleep yet.
    NoTable,
}

/// Queues `me` for `mutex` with the table locked, taking the mutex instead
/// if it is free. `seq` is the caller's place in the queue, given the
/// first time; `starved` asks for the mutex to be handed to it outright.
fn queue(mutex: &RawMutex, me: &ThreadRef, seq: &mut Option<u64>, starved: bool) -> Queued {
    let token = me.token();
    let key = mutex.key();
    let word = mutex.word();
    let mut table = TABLE.lock_irq();
    let Some(table) = table.as_mut() else { return Queued::NoTable };
    let mut seen = word.load(Ordering::Relaxed);
    let owner = loop {
        let owner = seen & !RawMutex::WAITERS;
        if owner == 0 {
            // Let go meanwhile: take it, keeping the waiters bit for
            // whoever else is queued.
            let bit = if table.has(key) { RawMutex::WAITERS } else { 0 };
            match word.compare_exchange(seen, token | bit, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => return Queued::Took,
                Err(now) => seen = now,
            }
            continue;
        }
        match word.compare_exchange(
            seen,
            seen | RawMutex::WAITERS,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break owner,
            Err(now) => seen = now,
        }
    };
    // With the waiters bit set, the owner can only let go through
    // `mutex_wake`, which needs the table: it is still the owner.
    let prio = super::rt_prio_of(me);
    let seq = *seq.get_or_insert_with(|| {
        table.seq += 1;
        table.seq
    });
    table.push(Waiter { thread: me.clone(), key, prio, seq, owner, starved });
    me.blocked_on.store(key, Ordering::Relaxed);
    count(|c| &c.mutex_waits);
    lend(table, owner, prio);
    Queued::Waiting
}

/// Spins while the owner of `mutex` runs on a CPU, taking the mutex if it
/// is let go meanwhile. Returns whether the caller took it. A critical
/// section is usually far shorter than putting a thread to sleep and
/// waking it, which costs interrupts and, in a virtual machine, exits to
/// the host; an owner that does not run (preempted, or asleep in its
/// critical section) will not let go soon, so the caller stops and queues.
fn spin_while_owner_runs(mutex: &RawMutex, token: usize) -> bool {
    let word = mutex.word();
    for _ in 0..SPIN_CHECKS {
        let seen = word.load(Ordering::Relaxed);
        let owner = seen & !RawMutex::WAITERS;
        if owner == 0 {
            // Free, though perhaps with sleepers queued: the bit stays for
            // them, and this thread wakes the next one when it lets go.
            let mine = token | (seen & RawMutex::WAITERS);
            if word.compare_exchange(seen, mine, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return true;
            }
            continue;
        }
        if !super::is_running(owner) {
            return false;
        }
        for _ in 0..SPIN_PAUSES {
            core::hint::spin_loop();
        }
    }
    false
}

/// Lends `prio` to the thread owning `token` and along the chain of owners
/// it waits for.
fn lend(table: &mut Table, mut owner: usize, mut prio: u8) {
    if prio == 0 {
        return;
    }
    for _ in 0..MAX_CHAIN {
        let Some(thread) = registry_find(owner) else { return };
        let lent = table.lent_to(owner).max(prio);
        if !set_boost(&thread, lent) {
            return;
        }
        count(|c| &c.boosts);
        let key = thread.blocked_on.load(Ordering::Relaxed);
        if key == 0 {
            return;
        }
        // It waits for another mutex: what it lends there goes up too.
        let effective = super::rt_prio_of(&thread);
        let Some(entry) = table
            .waiters
            .iter_mut()
            .find(|w| w.key == key && ThreadRef::ptr_eq(&w.thread, &thread))
        else {
            return;
        };
        entry.prio = effective;
        owner = entry.owner;
        prio = effective;
    }
}

/// The hook behind letting go of a contended mutex. A real-time waiter,
/// or one that has lost too often, gets the mutex handed straight to it
/// ([`hand_over`]); otherwise the mutex is let go and the most urgent
/// waiter woken to try again ([`let_go_to_waiters`]). Then the old owner
/// drops what it was lent through the mutex.
pub(super) fn mutex_wake(mutex: &RawMutex) {
    let me = current();
    let key = mutex.key();
    let mut guard = TABLE.lock_irq();
    let Some(table) = guard.as_mut() else {
        mutex.word().store(0, Ordering::Release);
        return;
    };
    match table.best(key) {
        None => mutex.word().store(0, Ordering::Release),
        Some(index) if table.waiters[index].prio > 0 || table.waiters[index].starved => {
            hand_over(table, mutex, index);
        }
        Some(index) => let_go_to_waiters(table, mutex, index),
    }
    if let Some(me) = me {
        let kept = table.lent_to(me.token());
        set_boost(&me, kept);
    }
}

/// Makes the waiter at `index` the owner of `mutex`, and of its remaining
/// waiters, with the loans they make, and wakes it. Nobody can take the
/// mutex in between, so a real-time waiter waits only for the critical
/// sections ahead of it (`docs/design/scheduler.md`, property 8).
fn hand_over(table: &mut Table, mutex: &RawMutex, index: usize) {
    let key = mutex.key();
    let next = table.waiters.swap_remove(index);
    let token = next.thread.token();
    let more = table.has(key);
    for waiter in table.waiters.iter_mut().filter(|w| w.for_mutex(key)) {
        waiter.owner = token;
    }
    mutex.word().store(token | if more { RawMutex::WAITERS } else { 0 }, Ordering::Release);
    next.thread.blocked_on.store(0, Ordering::Release);
    count(|c| &c.handoffs);
    let inherited = table.lent_to(token);
    set_boost(&next.thread, inherited);
    super::wake(&next.thread);
}

/// Lets `mutex` go with fair waiters only queued, and wakes the one at
/// `index` to try again: a thread already running, spinning for it, may
/// take it first, so the lock does not wait for a sleeper to wake up
/// (a convoy, where every critical section costs a wake-up). The waiter
/// keeps its place, and after [`LOSSES_BEFORE_HANDOFF`] losses gets the
/// mutex handed over. Fair waiters lend nothing, so no loan moves.
fn let_go_to_waiters(table: &mut Table, mutex: &RawMutex, index: usize) {
    let key = mutex.key();
    let next = table.waiters.swap_remove(index);
    let more = table.has(key);
    for waiter in table.waiters.iter_mut().filter(|w| w.for_mutex(key)) {
        waiter.owner = NO_OWNER;
    }
    mutex.word().store(if more { RawMutex::WAITERS } else { 0 }, Ordering::Release);
    next.thread.blocked_on.store(0, Ordering::Release);
    count(|c| &c.mutex_wakes);
    super::wake(&next.thread);
}

/// A thread's own priority changed (`sched::set_policy`): what it lends
/// as a mutex waiter changes with it. Call with no locks held.
pub(super) fn priority_changed(thread: &ThreadRef) {
    let key = thread.blocked_on.load(Ordering::Relaxed);
    if key == 0 {
        return;
    }
    let mut table = TABLE.lock_irq();
    let Some(table) = table.as_mut() else { return };
    let prio = super::rt_prio_of(thread);
    let Some(entry) =
        table.waiters.iter_mut().find(|w| w.key == key && ThreadRef::ptr_eq(&w.thread, thread))
    else {
        return;
    };
    entry.prio = prio;
    let owner = entry.owner;
    if owner == NO_OWNER {
        // The mutex was let go to its waiters, so it has no owner on
        // record to lend to. A waiter now real-time tries again, and
        // queues behind whoever took it, lending to them. If it was the
        // last one queued, the waiters bit stays set with nobody queued
        // until the next owner lets go through `mutex_wake`, which finds
        // nobody and clears it: one slow let-go, no lost wake-up.
        if prio > 0
            && let Some(index) = table
                .waiters
                .iter()
                .position(|w| w.key == key && ThreadRef::ptr_eq(&w.thread, thread))
        {
            let waiter = table.waiters.swap_remove(index);
            waiter.thread.blocked_on.store(0, Ordering::Release);
            super::wake(&waiter.thread);
        }
        return;
    }
    // Recompute the owner's loan from scratch: it may have gone down.
    if let Some(owner_thread) = registry_find(owner) {
        set_boost(&owner_thread, table.lent_to(owner));
    }
    lend(table, owner, prio);
}

/// Waits for a mutex without the scheduler: before threads run.
fn spin(mutex: &RawMutex, token: usize) {
    while mutex.word().compare_exchange(0, token, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
}

/// Counts for the debugger: wake-ups through the table, mutex handoffs,
/// and priority loans.
pub fn stats() -> (u64, u64, u64) {
    (total(|c| &c.wakeups), total(|c| &c.handoffs), total(|c| &c.boosts))
}

/// How contended the kernel's mutexes have been since boot.
/// `/processes/stat` gives these, so that a benchmark can see a lock its
/// threads queue for.
#[derive(Clone, Copy, Debug, Default)]
pub struct MutexCounts {
    /// Times a thread found a mutex taken and went into [`mutex_wait`].
    pub contended: u64,
    /// Times a thread went to sleep waiting for a mutex.
    pub waits: u64,
    /// Times a mutex was handed to a sleeping waiter.
    pub handoffs: u64,
    /// Times a mutex was let go with a sleeping waiter woken to try again.
    pub wakes: u64,
}

/// The mutex counts now.
pub fn mutex_counts() -> MutexCounts {
    MutexCounts {
        contended: total(|c| &c.mutex_contended),
        waits: total(|c| &c.mutex_waits),
        handoffs: total(|c| &c.handoffs),
        wakes: total(|c| &c.mutex_wakes),
    }
}

/// How many threads wait in the table now, or `None` if it is locked.
pub fn waiting() -> Option<usize> {
    TABLE.try_lock_irq().map(|t| t.as_ref().map_or(0, |t| t.waiters.len()))
}
