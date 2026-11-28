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
//! so through its own flag, which is on its thread's scheduler side, on
//! the line the wake-up writes anyway, both with the list locked; the
//! waker wakes its thread once the list is unlocked, so that the thread,
//! which may run at once on the waker's CPU, does not find the list still
//! locked. The waiter's reference to its thread goes with the wake-up to
//! the queue the thread joins ([`sched::wake_given`]).
//!
//! Each list has a cache line of its own, which holds its lock and its
//! oldest waiter; the rest wait in a second list beside it, which the
//! first's lock covers too. A word one thread waits on, the usual case, is
//! then waited on and woken in that one line: the waker takes it from the
//! waiter's CPU once, with the lock.
//! One that stops waiting for another reason (a timeout, a signal) takes
//! itself off, unless a waker did first, which then counts as its
//! wake-up. A waiter without a deadline sleeps on its own, outside the
//! scheduler's wait table, as nothing but its waker, a signal or a stop
//! wakes it ([`sched::wake`]). The TLA+ model `kernel/radix/tla/Futex.tla`
//! checks that no wake-up is lost this way.
//!
//! A waiter on shared memory holds what it waits on until it is off its
//! list, so that its key names nothing else while it waits, even if every
//! space unmaps it.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use radix_abi::errors::Kernel;
use radix_hal::UserMode;
use radix_mm::Kmalloc;
use radix_sync::Mutex;

use super::anon::SharedRef;
use super::{FrameRef, RingRef, SpaceObject, kmalloc};
use crate::Arch;
use crate::sched::{self, ThinRef, ThreadRef, wait};
use crate::user::posix::signal::{Restart, interrupted};
use crate::user::{Result, error};

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

/// Memory several spaces may map, held by a waiter on a word of it: a
/// pointer each, so that a key stays small enough to move cheaply.
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
    /// Its thread, whose flag the waker sets, in one word.
    thread: ThinRef,
}

/// One list of waiters, with its lock and its oldest waiter in one cache
/// line.
#[repr(align(64))]
struct List(Mutex<Waiters>);

const _: () = assert!(size_of::<List>() == 64, "a list's lock and oldest waiter share a line");

/// The waiters of one list, in the order they came: the oldest here, the
/// rest in the list's part of [`MORE`].
struct Waiters {
    first: Option<Waiter>,
    /// How many wait in [`MORE`]: none while `first` is empty.
    more: usize,
}

/// The lists of waiters.
static LISTS: [List; BUCKETS] =
    [const { List(Mutex::new(Waiters { first: None, more: 0 })) }; BUCKETS];

/// Each list's waiters after its oldest, made when first needed, and
/// locked only with that list locked.
static MORE: [Mutex<Option<Vec<Waiter, Kmalloc>>>; BUCKETS] = [const { Mutex::new(None) }; BUCKETS];

/// The list for the word a key names by `owner` and `offset`, by index.
fn list(owner: usize, offset: u64) -> usize {
    let hash = (owner as u64 ^ offset.rotate_left(17)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash >> 58) as usize % BUCKETS
}

impl Waiters {
    /// Adds `waiter` after the others of list `index`; gives it back if
    /// there is no memory for it.
    fn push(&mut self, index: usize, waiter: Waiter) -> core::result::Result<(), Waiter> {
        if self.first.is_none() {
            self.first = Some(waiter);
            return Ok(());
        }
        let mut more = MORE[index].lock();
        let rest = more.get_or_insert_with(|| Vec::new_in(kmalloc()));
        if rest.try_reserve(1).is_err() {
            return Err(waiter);
        }
        // Room was reserved above.
        let _ = rest.push_within_capacity(waiter);
        self.more += 1;
        Ok(())
    }

    /// Takes the oldest waiter of list `index` that `matches`, keeping the
    /// others in order.
    fn take(&mut self, index: usize, matches: impl Fn(&Waiter) -> bool) -> Option<Waiter> {
        if self.first.as_ref().is_some_and(&matches) {
            let taken = self.first.take();
            if self.more > 0 {
                self.more -= 1;
                self.first = MORE[index].lock().as_mut().map(|rest| rest.remove(0));
            }
            return taken;
        }
        if self.more == 0 {
            return None;
        }
        let mut more = MORE[index].lock();
        let rest = more.as_mut()?;
        let at = rest.iter().position(matches)?;
        self.more -= 1;
        Some(rest.remove(at))
    }
}

/// What a waiter with a deadline waits on in the scheduler's table: its
/// own flag.
fn key_of(thread: &ThreadRef) -> usize {
    &raw const thread.futex_woken as usize
}

/// Waits while the word at `address` of `space`, the running thread's,
/// holds `expected`, until a wake, `deadline_ns` (on the kernel's clock)
/// or a signal.
pub fn wait(
    space: &SpaceObject,
    address: u64,
    expected: u32,
    deadline_ns: Option<u64>,
) -> Result<()> {
    if !address.is_multiple_of(4) {
        return Err(Kernel::InvalidArgument.status(0));
    }
    // Borrowed: the waiter's reference is the one counted.
    let me = sched::running().ok_or(error(Kernel::Gone))?;
    // What the key holds stays held here until the waiter is off its list,
    // by its waker or by itself, both before this returns.
    let Key { owner, offset, hold: _held } = space.futex_key(address);
    let index = list(owner, offset);
    {
        let mut waiters = LISTS[index].0.lock();
        // Read with the list locked: a waker changes the word before it
        // takes the lock.
        let mut word = [0u8; 4];
        if !Arch::copy_from_user(&mut word, address) {
            return Err(error(Kernel::BadAddress));
        }
        if u32::from_le_bytes(word) != expected {
            return Err(error(Kernel::WouldBlock));
        }
        me.futex_woken.store(false, Ordering::Relaxed);
        let waiter = Waiter { owner, offset, thread: ThinRef::new(ThreadRef::clone(&me)) };
        if waiters.push(index, waiter).is_err() {
            return Err(error(Kernel::NoMemory));
        }
    }
    let done = || me.futex_woken.load(Ordering::Acquire) || sched::interrupted();
    let in_time = match deadline_ns {
        None => {
            // A wake that comes before the thread sleeps makes the sleep
            // return at once.
            while !done() {
                sched::block("futex");
            }
            true
        }
        Some(deadline) => wait::wait_until_or_timeout(key_of(&me), "futex", deadline, done),
    };
    if me.futex_woken.load(Ordering::Acquire) {
        return Ok(());
    }
    // Off the list, unless a waker took it off just now: then the wake-up
    // is this wait's. Its reference goes with the list unlocked.
    let mine =
        LISTS[index].0.lock().take(index, |w| core::ptr::eq(&raw const *w.thread, &raw const **me));
    if mine.is_none() {
        return Ok(());
    }
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

/// Wakes up to `count` of the threads waiting on the word at `address` of
/// `space`, first come first woken; returns how many.
pub fn wake(space: &SpaceObject, address: u64, count: u32) -> Result<u32> {
    if !address.is_multiple_of(4) {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let key = space.futex_key(address);
    let index = list(key.owner, key.offset);
    let mut woken = 0;
    loop {
        let mut batch: [Option<ThinRef>; WAKE_BATCH] = [const { None }; WAKE_BATCH];
        let mut taken = 0;
        let more = {
            let mut waiters = LISTS[index].0.lock();
            while woken < count && taken < WAKE_BATCH {
                let on_word = |w: &Waiter| w.owner == key.owner && w.offset == key.offset;
                let Some(waiter) = waiters.take(index, on_word) else { break };
                waiter.thread.futex_woken.store(true, Ordering::Release);
                batch[taken] = Some(waiter.thread);
                taken += 1;
                woken += 1;
            }
            taken == WAKE_BATCH && woken < count
        };
        for thread in batch.into_iter().flatten() {
            sched::wake_given(thread.into_ref());
        }
        if !more {
            return Ok(woken);
        }
    }
}
