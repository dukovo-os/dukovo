// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Futexes (Phase 8, radix-address-space(2)): threads waiting on 32-bit
//! words of an address space, what a C library's locks and condition
//! variables sleep on.
//!
//! A word is named by a [`Key`]: in memory only its address space maps, by
//! the space and its address; in memory several spaces may map, each at
//! its own address (a shared mapping, a frame, a ring), by what they map
//! and the word's place in it, so that a wake in one process reaches the
//! waiters of another (`SpaceObject::futex_key`). Its waiters are kept in
//! one of [`BUCKETS`] lists, chosen by hashing the two, in the order they
//! came. `futex_wait` checks the word with the bucket locked,
//! so a `futex_wake` that comes after the word changed (it takes the same
//! lock) always finds the waiter: no wake-up is lost between the check and
//! the sleep. A woken waiter is taken off the list by its waker and told
//! so through its own flag, both with the list locked; the waker wakes
//! its thread once the list is unlocked, so that the thread, which may
//! run at once on the waker's CPU, does not find the list still locked.
//! One that stops waiting for another reason (a timeout, a signal) takes
//! itself off, unless a waker did first, which then counts as its
//! wake-up. A waiter without a deadline sleeps on its own, outside the
//! scheduler's wait table, as nothing but its waker, a signal or a stop
//! wakes it ([`sched::wake`]). The TLA+ model `kernel/radix/tla/Futex.tla`
//! checks that no wake-up is lost this way.
//!
//! A waiter on shared memory holds what it waits on, so that its key
//! names nothing else while it waits, even if every space unmaps it.
//!
//! Since version 4 of the interface (ADR 0617) a waiter has a bit set, and
//! a wake passes over the waiters whose sets share no bit with its own
//! ([`wake`]); a requeue wakes some waiters of one word and moves others
//! to wait on another, where a wake of that word finds them, without
//! waking them ([`requeue`]); a wake-op changes one word in one atomic step
//! and wakes waiters of two ([`wake_op`]). The calls that work on two
//! words lock both lists, the one first in [`LISTS`] first, so that no two
//! of them wait for each other, and do all their work in one step: a
//! waiter that checked either word before cannot sleep through it. A
//! waiter's list is the one its thread's `futex_bucket` names, set when it
//! begins to wait and changed only by a requeue that holds both lists, so
//! a waiter that stops waiting for a timeout or a signal looks for itself
//! in the list it is in now ([`leave`]).

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use radix_abi::address_space::{FUTEX_ALL, FutexWaitFlags};
use radix_abi::errors::Kernel;
use radix_hal::{Timer, UserMode};
use radix_linux::futex::WakeOp;
use radix_mm::Kmalloc;
use radix_sync::{Mutex, MutexGuard};

use super::anon::SharedRef;
use super::{FrameRef, RingRef, SpaceObject, kmalloc};
use crate::Arch;
use crate::sched::{self, ThreadRef, wait};
use crate::user::posix::signal::{Restart, interrupted};
use crate::user::{Result, UserRef, error};

/// How many lists waiters are spread over.
const BUCKETS: usize = 64;

/// How many waiters a wake takes off their list at a time, before it
/// wakes them with the list unlocked.
const WAKE_BATCH: usize = 8;

/// What names a word.
pub struct Key {
    /// What it is in, by address: its address space, or the memory spaces
    /// share.
    pub(super) owner: usize,
    /// Its address in the space, or its place in the shared memory.
    pub(super) offset: u64,
    /// The shared memory, which a waiter holds.
    pub(super) hold: Option<Held>,
}

impl Key {
    /// Whether `waiter` waits on the word this names.
    fn names(&self, waiter: &Waiter) -> bool {
        waiter.owner == self.owner && waiter.offset == self.offset
    }
}

/// Memory several spaces may map, held by a waiter on a word of it: a
/// pointer each, so that a key stays small enough to move cheaply.
#[derive(Clone)]
#[expect(dead_code, reason = "held only to keep the memory alive")]
pub enum Held {
    /// A shared mapping's pages.
    Pages(SharedRef),
    /// A frame.
    Frame(FrameRef),
    /// A ring's pages.
    Ring(RingRef),
}

/// A thread waiting on a word.
struct Waiter {
    owner: usize,
    offset: u64,
    /// Its bit set: a wake whose set shares no bit with it passes it over.
    bits: u32,
    /// Held while it waits ([`Key::hold`]).
    hold: Option<Held>,
    thread: ThreadRef,
    /// Its program side, whose flag the waker sets.
    user: UserRef,
}

/// One list of waiters, made when first needed.
type Waiters = Option<Vec<Waiter, Kmalloc>>;

/// The lists of waiters.
static LISTS: [Mutex<Waiters>; BUCKETS] = [const { Mutex::new(None) }; BUCKETS];

/// The place in [`LISTS`] of the list for the word `key` names.
fn bucket(key: &Key) -> usize {
    let hash = (key.owner as u64 ^ key.offset.rotate_left(17)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash >> 58) as usize % BUCKETS
}

/// The lists of two words, locked: the one first in [`LISTS`] first, so
/// that two calls that each lock two lists cannot each hold one the other
/// waits for; one lock, and no second, when they share a list.
struct Pair {
    first: MutexGuard<'static, Waiters>,
    second: Option<MutexGuard<'static, Waiters>>,
    /// Whether the second word's list comes first in [`LISTS`].
    swapped: bool,
}

impl Pair {
    /// Locks the lists of the words `one` and `two` name.
    fn lock(one: &Key, two: &Key) -> Pair {
        let (a, b) = (bucket(one), bucket(two));
        if a == b {
            return Pair { first: LISTS[a].lock(), second: None, swapped: false };
        }
        let (low, high) = (a.min(b), a.max(b));
        let first = LISTS[low].lock();
        // Lock order: the list first in LISTS, then the other.
        let second = Some(LISTS[high].lock_nested(1));
        Pair { first, second, swapped: b < a }
    }

    /// The first word's list and the second's: the same list twice is
    /// given as one, with `None` for the second.
    fn lists(&mut self) -> (&mut Waiters, Option<&mut Waiters>) {
        match (&mut self.second, self.swapped) {
            (None, _) => (&mut *self.first, None),
            (Some(second), false) => (&mut *self.first, Some(&mut **second)),
            (Some(second), true) => (&mut **second, Some(&mut *self.first)),
        }
    }
}

/// Threads taken off their lists as woken, to be woken once the lists
/// are unlocked: [`WAKE_BATCH`] of them at most.
struct Woken {
    threads: [Option<ThreadRef>; WAKE_BATCH],
    count: usize,
}

impl Woken {
    fn new() -> Woken {
        Woken { threads: [const { None }; WAKE_BATCH], count: 0 }
    }

    /// Whether it holds as many threads as it can.
    fn is_full(&self) -> bool {
        self.count == WAKE_BATCH
    }

    /// Takes `waiter`, just taken off its list, as woken: its flag is set
    /// now, with the list locked, and its thread wakes in [`Woken::wake`].
    /// What its wait held goes here, with the list locked, and is never
    /// the last hold: the waker holds the key of the same word, until the
    /// lists are unlocked, so no object is freed under the lock.
    fn take(&mut self, waiter: Waiter) {
        debug_assert!(!self.is_full());
        waiter.user.futex_woken.store(true, Ordering::Release);
        self.threads[self.count] = Some(waiter.thread);
        self.count += 1;
    }

    /// Wakes the threads it holds, and holds none.
    fn wake(&mut self) {
        for thread in &mut self.threads[..self.count] {
            if let Some(thread) = thread.take() {
                sched::wake(&thread);
            }
        }
        self.count = 0;
    }
}

/// Takes the waiters on the word `key` names whose bit sets share a bit
/// with `bits` off `waiters` into `woken`, first come first, until `count`
/// are taken or `woken` is full; returns how many it took.
fn take(waiters: &mut Waiters, key: &Key, bits: u32, count: u32, woken: &mut Woken) -> u32 {
    let Some(waiters) = waiters.as_mut() else { return 0 };
    let mut taken = 0;
    let mut index = 0;
    while index < waiters.len() && taken < count && !woken.is_full() {
        let waiter = &waiters[index];
        if !key.names(waiter) || waiter.bits & bits == 0 {
            index += 1;
            continue;
        }
        woken.take(waiters.remove(index));
        taken += 1;
    }
    taken
}

/// [`take`], until `count` are taken or none is left, for a call that
/// does its work in one step: each time `woken` fills, its threads wake
/// with the lists still locked.
fn take_all(waiters: &mut Waiters, key: &Key, bits: u32, count: u32, woken: &mut Woken) -> u32 {
    let mut taken = take(waiters, key, bits, count, woken);
    while taken < count && woken.is_full() {
        woken.wake();
        let more = take(waiters, key, bits, count - taken, woken);
        if more == 0 {
            break;
        }
        taken += more;
    }
    taken
}

/// What a waiter with a deadline waits on in the scheduler's table: its
/// own flag.
fn key_of(user: &UserRef) -> usize {
    &raw const user.futex_woken as usize
}

/// When a wait of radix-address-space(2)'s `futex_wait_set` with
/// `timeout_ns` and `flags` ends, on the kernel's clock: `None` for no
/// end. A time on the wall clock is made a time on the kernel's clock
/// once, as the wait begins: a change of the wall clock while it waits
/// does not move its end, as it does Linux's (ADR 0617). `realtime`
/// without `absolute` is invalid_argument.
pub fn deadline(timeout_ns: u64, flags: FutexWaitFlags) -> Result<Option<u64>> {
    let absolute = flags.contains(FutexWaitFlags::ABSOLUTE);
    let realtime = flags.contains(FutexWaitFlags::REALTIME);
    if realtime && !absolute {
        return Err(Kernel::InvalidArgument.status(0));
    }
    if flags.contains(FutexWaitFlags::FOREVER) {
        return Ok(None);
    }
    let now = sched::now_ns();
    Ok(Some(match (absolute, realtime) {
        (false, _) => now.saturating_add(timeout_ns),
        (true, false) => timeout_ns,
        (true, true) => {
            let wall = <Arch as Timer>::wall_clock_ns().unwrap_or(0);
            now.saturating_add(timeout_ns.saturating_sub(wall))
        }
    }))
}

/// Waits while the word at `address` of `space`, the running thread's,
/// holds `expected`, until a wake whose bit set shares a bit with `bits`
/// (not 0), `deadline_ns` (on the kernel's clock) or a signal. `user` is
/// the running thread's program side.
pub fn wait(
    space: &SpaceObject,
    address: u64,
    expected: u32,
    bits: u32,
    deadline_ns: Option<u64>,
    user: &UserRef,
) -> Result<()> {
    if !address.is_multiple_of(4) || bits == 0 {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let me = sched::current().ok_or(error(Kernel::Gone))?;
    let key = space.futex_key(address);
    let index = bucket(&key);
    {
        let mut waiters = LISTS[index].lock();
        // Read with the list locked: a waker changes the word before it
        // takes the lock.
        let mut word = [0u8; 4];
        if !Arch::copy_from_user(&mut word, address) {
            return Err(error(Kernel::BadAddress));
        }
        if u32::from_le_bytes(word) != expected {
            return Err(error(Kernel::WouldBlock));
        }
        let waiters = waiters.get_or_insert_with(|| Vec::new_in(kmalloc()));
        waiters.try_reserve(1).map_err(|_| error(Kernel::NoMemory))?;
        user.futex_woken.store(false, Ordering::Relaxed);
        // Read and changed only with the list it names locked.
        user.futex_bucket.store(index, Ordering::Relaxed);
        // Room was reserved above.
        let waiter = Waiter {
            owner: key.owner,
            offset: key.offset,
            bits,
            hold: key.hold,
            thread: me.clone(),
            user: user.clone(),
        };
        let _ = waiters.push_within_capacity(waiter);
    }
    let done = || user.futex_woken.load(Ordering::Acquire) || sched::interrupted();
    let in_time = match deadline_ns {
        None => {
            // A wake that comes before the thread sleeps makes the sleep
            // return at once.
            while !done() {
                sched::block("futex");
            }
            true
        }
        Some(deadline) => wait::wait_until_or_timeout(key_of(user), "futex", deadline, done),
    };
    if user.futex_woken.load(Ordering::Acquire) {
        return Ok(());
    }
    // Off its list, unless a waker took it off just now: then the
    // wake-up is this wait's. What it held goes with the list unlocked.
    let Some(mine) = leave(user, &me) else { return Ok(()) };
    drop(mine);
    if !in_time {
        return Err(error(Kernel::TimedOut));
    }
    // As Linux's: a wait with a deadline runs again only if no handler
    // ran, so that a sleep a handler cuts short says so; one without runs
    // again unless the handler lacks SA_RESTART.
    let restart = if deadline_ns.is_some() { Restart::NoHandler } else { Restart::Always };
    Err(interrupted(restart))
}

/// Takes the running thread, `me`, whose program side is `user`, off the
/// list it waits in, unless a waker took it off first: then `None`. A
/// requeue may have moved it to another list since it began to wait; as
/// only a requeue that holds both lists changes `futex_bucket`, once the
/// list it names is locked, and it still names that list, the thread is
/// in that list or in none.
fn leave(user: &UserRef, me: &ThreadRef) -> Option<Waiter> {
    loop {
        let index = user.futex_bucket.load(Ordering::Relaxed);
        let mut waiters = LISTS[index].lock();
        if user.futex_bucket.load(Ordering::Relaxed) != index {
            continue;
        }
        let waiters = waiters.as_mut()?;
        let at = waiters.iter().position(|w| ThreadRef::ptr_eq(&w.thread, me))?;
        return Some(waiters.remove(at));
    }
}

/// Wakes up to `count` of the threads waiting on the word at `address` of
/// `space` whose bit sets share a bit with `bits` (not 0), first come
/// first woken; returns how many. The threads wake a batch at a time,
/// each once the list is unlocked.
pub fn wake(space: &SpaceObject, address: u64, count: u32, bits: u32) -> Result<u32> {
    if !address.is_multiple_of(4) || bits == 0 {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let key = space.futex_key(address);
    let list = &LISTS[bucket(&key)];
    let mut woken = 0;
    let mut batch = Woken::new();
    loop {
        let more = {
            let mut waiters = list.lock();
            woken += take(&mut waiters, &key, bits, count - woken, &mut batch);
            batch.is_full() && woken < count
        };
        batch.wake();
        if !more {
            return Ok(woken);
        }
    }
}

/// Wakes up to `count` of the threads waiting on the word at `address` of
/// `space`, and moves up to `moved` of the others on it to wait on the
/// word at `to` instead, as radix-address-space(2)'s `futex_requeue`;
/// with `expected`, first checks that the word at `address` holds it
/// (would_block if not), which needs `space` to be the running thread's.
/// Returns how many it woke and how many it moved. The two may be the
/// same word: then the moved threads stay where they are, and are counted.
/// A thread that cannot be moved for want of memory in the list of `to`
/// is woken instead, which a waiter allows for as it allows for any early
/// wake-up, and counted as moved: the move cannot fail halfway.
pub fn requeue(
    space: &SpaceObject,
    address: u64,
    count: u32,
    to: u64,
    moved: u32,
    expected: Option<u32>,
) -> Result<(u32, u32)> {
    if !address.is_multiple_of(4) || !to.is_multiple_of(4) {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let from = space.futex_key(address);
    let into = space.futex_key(to);
    let mut batch = Woken::new();
    let result = {
        let mut pair = Pair::lock(&from, &into);
        if let Some(expected) = expected {
            // Read with both lists locked: a waiter that saw another value
            // has been woken by whoever changed it, or will be.
            let mut word = [0u8; 4];
            if !Arch::copy_from_user(&mut word, address) {
                return Err(error(Kernel::BadAddress));
            }
            if u32::from_le_bytes(word) != expected {
                return Err(error(Kernel::WouldBlock));
            }
        }
        let (source, target) = pair.lists();
        let woken = take_all(source, &from, FUTEX_ALL, count, &mut batch);
        let moving = move_waiters(source, target, &from, &into, moved, &mut batch);
        (woken, moving)
    };
    batch.wake();
    Ok(result)
}

/// The move of [`requeue`]: up to `count` waiters on the word `from`
/// names in `source` now wait on the word `into` names, in `target`, or in
/// `source` if that is its list too (`None`); returns how many.
fn move_waiters(
    source: &mut Waiters,
    target: Option<&mut Waiters>,
    from: &Key,
    into: &Key,
    count: u32,
    woken: &mut Woken,
) -> u32 {
    let Some(waiters) = source.as_mut() else { return 0 };
    let Some(target) = target else {
        // One list: the waiters change their word where they are.
        let mut moved = 0;
        for waiter in waiters.iter_mut().filter(|w| from.names(w)).take(count as usize) {
            rename(waiter, into);
            moved += 1;
        }
        return moved;
    };
    // Room for all that move, or as many as there is memory for.
    let wanted = waiters.iter().filter(|w| from.names(w)).take(count as usize).count();
    let target = target.get_or_insert_with(|| Vec::new_in(kmalloc()));
    let mut room = (0..=wanted).rev().find(|&n| target.try_reserve(n).is_ok()).unwrap_or(0);
    let index = bucket(into);
    let mut moved = 0;
    let mut at = 0;
    while at < waiters.len() && moved < count {
        if !from.names(&waiters[at]) {
            at += 1;
            continue;
        }
        let mut waiter = waiters.remove(at);
        moved += 1;
        if room == 0 {
            // No memory to move it: it wakes instead.
            if woken.is_full() {
                woken.wake();
            }
            woken.take(waiter);
            continue;
        }
        rename(&mut waiter, into);
        // Its list changes with both lists locked (`leave`).
        waiter.user.futex_bucket.store(index, Ordering::Relaxed);
        // Room was reserved above.
        let _ = target.push_within_capacity(waiter);
        room -= 1;
    }
    moved
}

/// `waiter` now waits on the word `key` names, holding what it holds. What
/// it held goes with the lists locked, and is never the last hold, as
/// [`Woken::take`] says.
fn rename(waiter: &mut Waiter, key: &Key) {
    waiter.owner = key.owner;
    waiter.offset = key.offset;
    waiter.hold.clone_from(&key.hold);
}

/// Changes the word at `to` of `space`, the running thread's, by
/// `operation` in one atomic step, wakes up to `count` of the threads
/// waiting on the word at `address`, and, if `operation`'s comparison
/// holds of the changed word's old value, up to `count_to` of those
/// waiting on `to`, as radix-address-space(2)'s `futex_wake_op`; returns
/// how many it woke in all. The change is made with both lists locked, so
/// that a waiter on either word that read it before cannot miss the wake.
pub fn wake_op(
    space: &SpaceObject,
    address: u64,
    count: u32,
    to: u64,
    count_to: u32,
    operation: WakeOp,
) -> Result<u32> {
    if !address.is_multiple_of(4) || !to.is_multiple_of(4) {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let first = space.futex_key(address);
    let second = space.futex_key(to);
    let mut batch = Woken::new();
    let woken = {
        let mut pair = Pair::lock(&first, &second);
        let old = change(to, operation)?;
        let (one, two) = pair.lists();
        let mut woken = take_all(one, &first, FUTEX_ALL, count, &mut batch);
        if operation.holds(old) {
            let list = match two {
                Some(two) => two,
                None => one,
            };
            woken += take_all(list, &second, FUTEX_ALL, count_to, &mut batch);
        }
        woken
    };
    batch.wake();
    Ok(woken)
}

/// Changes the word at `address` of the running thread's memory by
/// `operation`, in one atomic step; returns its old value.
fn change(address: u64, operation: WakeOp) -> Result<u32> {
    let mut word = [0u8; 4];
    if !Arch::copy_from_user(&mut word, address) {
        return Err(error(Kernel::BadAddress));
    }
    let mut old = u32::from_le_bytes(word);
    loop {
        match Arch::user_compare_exchange(address, old, operation.apply(old)) {
            None => return Err(error(Kernel::BadAddress)),
            Some(seen) if seen == old => return Ok(old),
            // Changed since it was read: again, from what it holds now.
            Some(seen) => old = seen,
        }
    }
}
