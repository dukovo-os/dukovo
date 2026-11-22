// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who waits on one network socket (ADR 0550): the threads blocked in its
//! calls, and those polling it.
//!
//! Each of a socket's smoltcp sockets has the socket's two wakers
//! ([`Wakers`]), registered once when it is made: one for receiving, one
//! for sending. smoltcp wakes the first when what the socket can receive
//! may have changed (data or a datagram came, the other end closed) and
//! the second when what it can send may have (room came free), and both
//! when its state changes; the kernel's patch keeps the wakers registered
//! after each wake, and has the TCP calls that pass bytes straight across
//! (ADR 0186) wake the other end too. A wake moves that side's own count
//! of changes and wakes the threads waiting on that side of that socket
//! alone, and its pollers. A thread waiting to receive does not wake when
//! room comes free for sending, as each byte a TCP connection passes
//! frees, nor one waiting to send when data comes. A change that no
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
//!
//! A listening socket's threads wait in `accept`, and one connection is
//! for one of them: its receive side wakes one waiter for each change
//! ([`Waiters::accepts`]), as Linux's exclusive waits in `accept` do, and
//! its smoltcp sockets have no wakers of their own. Its interface tells it
//! of each connection made, once, after the poll that made it (ADR 0651).
//! A waiter woken alone that leaves without looking again, at its
//! deadline or for a signal, looks once more as it goes, so that a
//! connection it was woken for is not left with the others asleep
//! ([`Waiters::wakes_one`]).

use alloc::sync::Arc;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};

use radix_mm::{Flags, HEAP, Kmalloc};

use crate::sched::{self, wait};
use crate::user::posix::poll::Polled;

/// Which way a socket call goes, and which of a socket's changes it waits
/// for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Data, a datagram, a connection to accept, or the end of them.
    Receive,
    /// Room to send, or a connection made.
    Send,
}

/// One side of a socket's waiters: its count of changes, and how many
/// threads wait for it to move.
struct Side {
    /// Moves on every change: a waiter sleeps until it does.
    changes: AtomicU64,
    /// How many threads wait on this side.
    sleepers: AtomicUsize,
    /// Set once its socket listens: a change wakes one waiter, not all.
    one: AtomicBool,
}

impl Side {
    const fn new() -> Side {
        Side {
            changes: AtomicU64::new(0),
            sleepers: AtomicUsize::new(0),
            one: AtomicBool::new(false),
        }
    }

    /// Moves the count, and wakes the waiters if there are any: one of
    /// them on a side that wakes one, unless `all`, and lazily for a waker
    /// that holds its stack's lock ([`sched::lazily`]).
    fn changed(&self, lazily: bool, all: bool) {
        self.changes.fetch_add(1, Ordering::SeqCst);
        if self.sleepers.load(Ordering::SeqCst) == 0 {
            return;
        }
        let key = self.key();
        match (lazily, !all && self.one.load(Ordering::Relaxed)) {
            (false, false) => wait::wake_all(key),
            (true, false) => wait::wake_all_lazily(key),
            (false, true) => {
                wait::wake_one(key);
            }
            (true, true) => wait::wake_one_lazily(key),
        }
    }

    /// The key its waiters sleep on.
    fn key(&self) -> usize {
        &raw const self.changes as usize
    }
}

/// A network socket's waiters, each way, and its pollers.
pub struct Waiters {
    receive: Side,
    send: Side,
    polled: Polled,
}

impl Waiters {
    /// Waiters for a new socket, or `None` without memory for them.
    pub fn new() -> Option<Arc<Waiters, Kmalloc>> {
        let waiters = Waiters { receive: Side::new(), send: Side::new(), polled: Polled::new() };
        Arc::try_new_in(waiters, HEAP.with(Flags::MAY_SLEEP)).ok()
    }

    fn side(&self, way: Way) -> &Side {
        match way {
            Way::Receive => &self.receive,
            Way::Send => &self.send,
        }
    }

    /// What its pollers count themselves in.
    pub fn polled(&self) -> &Polled {
        &self.polled
    }

    /// What the socket can do either way may have changed: all its
    /// waiters look again, and so do its pollers.
    pub fn changed(&self) {
        self.receive.changed(false, true);
        self.send.changed(false, true);
        self.polled.changed();
    }

    /// What the socket can do `way` may have changed, as smoltcp found
    /// with its stack's lock held: those waiting that way look again, once
    /// the lock is let go if they would run on this CPU ([`StackGuard`]),
    /// and so do its pollers. Costs an atomic add and two loads while
    /// nobody waits or polls.
    ///
    /// [`StackGuard`]: super::StackGuard
    pub fn changed_way(&self, way: Way) {
        self.side(way).changed(true, false);
        self.polled.changed();
    }

    /// Its socket listens: from now on a change to what it can receive,
    /// a connection made, wakes one of the threads waiting in `accept`.
    /// [`Waiters::changed`] still wakes them all.
    pub fn accepts(&self) {
        self.receive.one.store(true, Ordering::Relaxed);
    }

    /// Whether a change `way` wakes one waiter, who then looks once more
    /// if it leaves without a look after its wake.
    pub fn wakes_one(&self, way: Way) -> bool {
        self.side(way).one.load(Ordering::Relaxed)
    }

    /// Counts the caller among those waiting `way` until the guard drops:
    /// a thread does this before it first reads [`Waiters::count`].
    pub fn waiting(&self, way: Way) -> Waiting<'_> {
        let side = self.side(way);
        side.sleepers.fetch_add(1, Ordering::SeqCst);
        Waiting(side)
    }

    /// The count of changes `way`, which a waiter reads before it looks.
    pub fn count(&self, way: Way) -> u64 {
        self.side(way).changes.load(Ordering::SeqCst)
    }

    /// Sleeps until the count of changes `way` moves from `seen`, or the
    /// thread is interrupted, or `deadline_ns` (on the kernel's clock)
    /// passes if there is one. Returns false if the deadline passed first.
    /// The caller holds a [`Waiting`] guard for `way`.
    pub fn sleep(
        &self,
        way: Way,
        reason: &'static str,
        seen: u64,
        deadline_ns: Option<u64>,
    ) -> bool {
        let side = self.side(way);
        let moved = || side.changes.load(Ordering::Acquire) != seen || sched::interrupted();
        match deadline_ns {
            None => {
                wait::wait_until(side.key(), reason, moved);
                true
            }
            Some(deadline) => wait::wait_until_or_timeout(side.key(), reason, deadline, moved),
        }
    }

    /// The wakers that call [`Waiters::changed_way`] on these waiters, for
    /// its socket's smoltcp sockets. They hold no reference: smoltcp's
    /// patch never drops a socket's wakers, whose drop glue would cost each
    /// frame that holds a socket a copy of it more (ADR 0058).
    ///
    /// # Safety
    ///
    /// Every smoltcp socket given the wakers has them replaced
    /// ([`radix_net::Stack::set_wakers`] with [`Waker::noop`]) or is taken
    /// out of its stack before these waiters go: the socket's own parts
    /// when it closes, an orphan when its interface adopts it.
    #[allow(unsafe_code)]
    pub unsafe fn wakers(&self) -> Wakers {
        let data = ptr::from_ref(self).cast::<()>();
        // SAFETY: the vtables' functions keep RawWaker's contract while the
        // waiters are there, which the caller promises; `changed_way` may
        // be called from any thread.
        unsafe {
            Wakers {
                receive: Waker::from_raw(RawWaker::new(data, &RECEIVE)),
                send: Waker::from_raw(RawWaker::new(data, &SEND)),
            }
        }
    }
}

/// A socket's two wakers, for receiving and for sending.
#[derive(Clone)]
pub struct Wakers {
    pub receive: Waker,
    pub send: Waker,
}

/// A thread counted among one side of a socket's waiters, while it lives.
pub struct Waiting<'a>(&'a Side);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.sleepers.fetch_sub(1, Ordering::SeqCst);
    }
}

static RECEIVE: RawWakerVTable =
    RawWakerVTable::new(clone_receive, wake_receive, wake_receive, drop);
static SEND: RawWakerVTable = RawWakerVTable::new(clone_send, wake_send, wake_send, drop);

/// A copy of a receive waker: it holds no reference.
fn clone_receive(data: *const ()) -> RawWaker {
    RawWaker::new(data, &RECEIVE)
}

/// A copy of a send waker: it holds no reference.
fn clone_send(data: *const ()) -> RawWaker {
    RawWaker::new(data, &SEND)
}

/// # Safety
///
/// `data` is the waiters a waker was made for, which are still there.
#[allow(unsafe_code)]
unsafe fn wake_receive(data: *const ()) {
    // SAFETY: as the function says.
    unsafe { &*data.cast::<Waiters>() }.changed_way(Way::Receive);
}

/// # Safety
///
/// As [`wake_receive`].
#[allow(unsafe_code)]
unsafe fn wake_send(data: *const ()) {
    // SAFETY: as the function says.
    unsafe { &*data.cast::<Waiters>() }.changed_way(Way::Send);
}

/// Nothing to let go of.
fn drop(_: *const ()) {}
