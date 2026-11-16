// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who waits on one network socket (ADR 0550): the threads blocked in its
//! calls, and those polling it.
//!
//! Each of a socket's smoltcp sockets has the socket's [`Waiters`] as its
//! waker, registered once when it is made, for receiving and sending.
//! smoltcp wakes it whenever what that socket can receive or send may have
//! changed: data or a datagram came, room came free, its state changed;
//! the kernel's patch keeps the waker registered after each wake, and has
//! the TCP calls that pass bytes straight across (ADR 0186) wake the other
//! end too. A wake moves the socket's own count of changes and wakes the
//! threads waiting on that socket alone, and its pollers. A change that no
//! socket sees, to an interface's addresses or link, wakes every socket's
//! waiters ([`super::changed`]).
//!
//! Until this, one count served all of networking: every packet woke
//! every thread blocked in any socket call, and every poller of any
//! socket. On a machine whose services wait on their sockets, as an
//! installed system's do, each round trip between two programs woke all
//! of them, and kept the other CPUs busy.
//!
//! No wake-up is lost between a waiter's look and its sleep, by the same
//! rule as `poll`'s ([`Polled`]): a waiter counts itself in before it reads
//! the count of changes and looks, and a wake moves the count before it
//! reads how many wait, each sequentially consistent, so either the
//! waiter sees the change or the wake sees the waiter.

use alloc::sync::Arc;
use core::ptr;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};

use radix_mm::{Flags, HEAP, Kmalloc};

use crate::sched::{self, wait};
use crate::user::posix::poll::Polled;

/// A network socket's waiters, and its count of changes.
pub struct Waiters {
    /// Moves on every change: a waiter sleeps until it does.
    changes: AtomicU64,
    /// How many threads are in [`Waiters::wait`].
    sleepers: AtomicUsize,
    /// Its pollers.
    polled: Polled,
}

impl Waiters {
    /// Waiters for a new socket, or `None` without memory for them.
    pub fn new() -> Option<Arc<Waiters, Kmalloc>> {
        let waiters = Waiters {
            changes: AtomicU64::new(0),
            sleepers: AtomicUsize::new(0),
            polled: Polled::new(),
        };
        Arc::try_new_in(waiters, HEAP.with(Flags::MAY_SLEEP)).ok()
    }

    /// What its pollers count themselves in.
    pub fn polled(&self) -> &Polled {
        &self.polled
    }

    /// What the socket can do may have changed: its waiters look again,
    /// and so do its pollers. Costs an atomic add and two loads while
    /// nobody waits or polls.
    pub fn changed(&self) {
        self.changes.fetch_add(1, Ordering::SeqCst);
        if self.sleepers.load(Ordering::SeqCst) > 0 {
            wait::wake_all(self.key());
        }
        self.polled.changed();
    }

    /// Counts the caller among its waiters until the guard drops: a
    /// thread does this before it first reads [`Waiters::count`].
    pub fn waiting(&self) -> Waiting<'_> {
        self.sleepers.fetch_add(1, Ordering::SeqCst);
        Waiting(self)
    }

    /// Its count of changes, which a waiter reads before it looks.
    pub fn count(&self) -> u64 {
        self.changes.load(Ordering::SeqCst)
    }

    /// Sleeps until the count of changes moves from `seen`, or the thread
    /// is interrupted, or `deadline_ns` (on the kernel's clock) passes if
    /// there is one. Returns false if the deadline passed first. The
    /// caller holds a [`Waiting`] guard.
    pub fn sleep(&self, reason: &'static str, seen: u64, deadline_ns: Option<u64>) -> bool {
        let moved = || self.changes.load(Ordering::Acquire) != seen || sched::interrupted();
        match deadline_ns {
            None => {
                wait::wait_until(self.key(), reason, moved);
                true
            }
            Some(deadline) => wait::wait_until_or_timeout(self.key(), reason, deadline, moved),
        }
    }

    /// The key its waiters sleep on.
    fn key(&self) -> usize {
        &raw const self.changes as usize
    }

    /// A waker that calls [`Waiters::changed`] on these waiters, for its
    /// socket's smoltcp sockets. It holds no reference: smoltcp's patch
    /// never drops a socket's wakers, whose drop glue would cost each frame
    /// that holds a socket a copy of it more (ADR 0058).
    ///
    /// # Safety
    ///
    /// Every smoltcp socket given the waker has it replaced
    /// ([`radix_net::Stack::set_waker`] with [`Waker::noop`]) or is taken
    /// out of its stack before these waiters go: the socket's own parts
    /// when it closes, an orphan when its interface adopts it.
    #[allow(unsafe_code)]
    pub unsafe fn waker(&self) -> Waker {
        let raw = RawWaker::new(ptr::from_ref(self).cast(), &VTABLE);
        // SAFETY: the vtable's functions keep RawWaker's contract while
        // the waiters are there, which the caller promises; `changed` may
        // be called from any thread.
        unsafe { Waker::from_raw(raw) }
    }
}

/// A thread counted among a socket's waiters, while it lives.
pub struct Waiting<'a>(&'a Waiters);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.sleepers.fetch_sub(1, Ordering::SeqCst);
    }
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop);

/// A copy of the waker: it holds no reference.
fn clone(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

/// # Safety
///
/// `data` is the waiters a waker was made for, which are still there.
#[allow(unsafe_code)]
unsafe fn wake(data: *const ()) {
    // SAFETY: as the function says.
    unsafe { &*data.cast::<Waiters>() }.changed();
}

/// Nothing to let go of.
fn drop(_: *const ()) {}
