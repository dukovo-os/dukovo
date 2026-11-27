// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Event queues (Phase 17, radix-event-queue(2), ADR 0353): the kernel's
//! half of kqueue(2).
//!
//! An [`EventQueue`] holds watches ([`Watch`]), each of a description for
//! reading or writing, or of an event of the program's own, by `ident`
//! and `filter` ([`Registry`]). A watch of a description counts itself in
//! the [`Polled`]s that `poll` waits on for it, so that a change there
//! queues the watch for a look ([`changed`]): it goes on its queue's ready
//! ring, at most once, and the queue's waiters wake. `wait` makes its
//! changes, then takes watches off the ring in the order they went on,
//! looks at what each one watches, and reports those that are ready, as
//! `radix_posix::watch` says; a level-triggered watch it reported goes
//! back on the ring for the next call. So a call costs what changed, not
//! what is watched.
//!
//! No change is lost between a look and a sleep: a look clears a watch's
//! queued mark before it looks at the description, and a change marks the
//! watch after it changes the description, each with a full barrier
//! between, as `poll`'s pollers and changes do; so either the look sees
//! the change, or the change queues the watch again. The queue's waiters
//! count themselves before they look at the ring, and a watch put on it
//! wakes them only if one is counted (`WaitCount`), so a queue nobody
//! waits on takes no lock but its ring's for a change. The TLA+ model
//! `kernel/radix/tla/EventQueue.tla` checks this, and that a deleted watch
//! is never reported.
//!
//! The `poll` filter is Linux's epoll on the same machinery (ADR 0612):
//! its watch asks for any of a description's events, a change sets them
//! anew, and an exclusive one (EPOLLEXCLUSIVE) is queued by a change only
//! until one is queued where a thread waits ([`Polled::changed`]). The
//! Linux table makes its changes and waits through [`change_given`] and
//! [`wait_given`], with the records in the kernel's memory, and with the
//! same checks as `wait`.
//!
//! A watch holds its description weakly, so that watching keeps nothing
//! open: once the description goes, its watches go at their next look, or
//! when their key is used again. A watch a change deletes is marked, never
//! queued again, and taken out of its description's `Polled`s; those of a
//! queue that has gone are only marked, with the store's locks held, and
//! go when the next watch of the same description is added.
//!
//! Locks, outermost first: the queue's `registry` (a mutex), which every
//! change and look holds, so that a change that deletes a watch is never
//! overtaken by a report of it; a watch's `held`; a `Polled`'s list of
//! watches; the queue's ready ring; the wait table. A description's change
//! queues its watches with only the last three, in whatever context it
//! runs, and allocates nothing: the ring has room for every watch that may
//! be on it. The posix boot test's `events` program covers this file
//! (userspace/posix/events.c).

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use radix_abi::capability::SlotFlags;
use radix_abi::errors::{File, Kernel};
use radix_abi::event_queue::{
    self as abi, CHANGES_MAX, QueueEvent, QueueFilter, QueueFlags, QueueWaitFlags, WaitArgs,
    WaitResults,
};
use radix_abi::process::{PipeFlags, PollEvents};
use radix_cap::{ObjectKind, Rights, SlotRef};
use radix_hal::UserMode;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_posix::watch::{self, Asks, State, Then};
use radix_sync::{Mutex, SpinLock, barrier};

use super::account::{Account, Charge};
use super::methods::{Answer, answer};
use super::notify::Notification;
use super::objects::{self, Found, Object};
use super::posix::poll::{self, Polled};
use super::posix::signal::{self, Restart, interrupted};
use super::{Result, UserRef, cap_error, error};
use crate::Arch;
use crate::caps;
use crate::fs::FileObject;
use crate::sched::{self, wait::WaitCount};

/// An event queue, shared.
pub type QueueRef = Arc<EventQueue, Kmalloc>;

/// A watch, shared by its queue and the `Polled`s of what it watches.
pub type WatchRef = Arc<Watch, Kmalloc>;

/// Where a queue's watches wait for a look, shared by the queue and its
/// watches.
type ReadyRef = Arc<Ready, Kmalloc>;

fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// A watch's mark: it is on its queue's ring, or about to go on it.
const QUEUED: u8 = 1;
/// A watch's mark: its queue has deleted it, or gone.
const DELETED: u8 = 2;

/// The most `Polled`s one description has: a Unix stream socket's own
/// and the two pipes of its connection.
const POLLED_MAX: usize = 3;

/// What a watch costs the account of the process that adds it: itself,
/// its counts, its place on the ring and its place in the registry.
const WATCH_BYTES: u64 = (size_of::<Watch>() + 5 * size_of::<usize>()) as u64;

/// How many changes `wait` copies in at a time, on the kernel stack.
const CHANGES_AT_ONCE: usize = 8;

/// Where a wait's events go, by their index: the program's array of
/// `struct kevent`, or the Linux table's of `struct epoll_event`. It
/// fails with bad_address if the program's memory cannot be written. A
/// generic, not a `dyn`, so that each event costs no indirect call.
pub trait Sink: FnMut(u32, &QueueEvent) -> Result<()> {}

impl<F: FnMut(u32, &QueueEvent) -> Result<()>> Sink for F {}

/// An event queue.
pub struct EventQueue {
    /// Its watches, held while changes are made and watches looked at.
    registry: Mutex<Registry>,
    /// Its ready ring.
    ready: ReadyRef,
}

/// A queue's ready ring, and what waits for it.
struct Ready {
    ring: SpinLock<Ring>,
    /// How many watches are on the ring, for the waiters, which read it
    /// without its lock.
    queued: AtomicUsize,
    /// Set once the queue has gone: nothing goes on the ring after.
    closed: AtomicBool,
    /// Its waiters (`EventQueue::wait_ready`), whom a watch put on the
    /// ring wakes.
    waits: WaitCount,
    /// The pollers of the queue itself, for which it is readable while a
    /// watch is on its ring.
    polled: Polled,
}

impl Ready {
    /// What the queue's waiters wait on.
    fn key(&self) -> usize {
        core::ptr::from_ref(&self.queued) as usize
    }

    /// A watch has gone on the ring: its waiters and pollers look again.
    fn wake(&self) {
        self.waits.wake_all(self.key());
        // `wake_all` took the full barrier that `Polled::changed` would.
        // This may take the queue's own `Polled`'s lock under a watched
        // object's: it holds pollers but no watches, as no queue watches
        // another (`Watched::of`), so nothing under it takes another
        // `Polled`'s lock.
        self.polled.changed_by_rmw();
    }

    /// Takes the first watch off the ring.
    fn pop(&self) -> Option<WatchRef> {
        let mut ring = self.ring.lock_irq();
        let watch = ring.pop();
        self.queued.store(ring.len, Ordering::Release);
        watch
    }
}

/// Watches waiting for a look, first in first out, in slots made before
/// they are needed: one for each watch that may be on it at once.
struct Ring {
    slots: Vec<Option<WatchRef>, Kmalloc>,
    head: usize,
    len: usize,
}

impl Ring {
    const fn new() -> Ring {
        Ring { slots: Vec::new_in(HEAP.with(Flags::MAY_SLEEP)), head: 0, len: 0 }
    }

    /// Puts `watch` last. There is room: a slot was made for it when it
    /// was added ([`EventQueue::make_room`]), and it is on the ring at most
    /// once.
    fn push(&mut self, watch: WatchRef) {
        debug_assert!(self.len < self.slots.len(), "an event queue's ring is full");
        let at = self.wrap(self.head + self.len);
        self.slots[at] = Some(watch);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<WatchRef> {
        if self.len == 0 {
            return None;
        }
        let watch = self.slots[self.head].take();
        self.head = self.wrap(self.head + 1);
        self.len -= 1;
        watch
    }

    /// `at`, less than one lap past the end of the slots, as the head and
    /// the length make it, brought back into them: what `%` would give,
    /// without its division, which costs some 26 cycles on the lab's
    /// Skylake, on each watch put on the ring and each taken off.
    fn wrap(&self, at: usize) -> usize {
        let size = self.slots.len();
        debug_assert!(at < 2 * size, "an index more than a lap past the ring's end");
        if at >= size { at - size } else { at }
    }

    /// Takes `slots`, empty and at least as many as its own, in place of
    /// its own, keeping its watches in order; returns its own.
    fn regrow(
        &mut self,
        mut slots: Vec<Option<WatchRef>, Kmalloc>,
    ) -> Vec<Option<WatchRef>, Kmalloc> {
        let size = self.slots.len();
        for (index, slot) in slots.iter_mut().take(self.len).enumerate() {
            *slot = self.slots[(self.head + index) % size].take();
        }
        self.head = 0;
        core::mem::replace(&mut self.slots, slots)
    }
}

/// What a watch watches.
enum Watched {
    File(Weak<FileObject, Kmalloc>),
    Notification(Weak<Notification, Kmalloc>),
    /// An event of the program's own.
    User,
}

impl Watched {
    /// What a watch of `object` holds of it.
    ///
    /// # Errors
    ///
    /// invalid_argument for an event queue, which watching would let wait
    /// for itself, and not_permitted for what has no `Polled`: files and
    /// directories, always ready, and other kinds, never ready, as Linux's
    /// `epoll_ctl` refuses what does not poll.
    fn of(object: &Object) -> Result<Watched> {
        let mut polled = 0;
        poll::polled_by(object, &mut |_| polled += 1);
        match object {
            Object::EventQueue(_) => Err(error(Kernel::InvalidArgument)),
            Object::File(file) if polled > 0 => Ok(Watched::File(Arc::downgrade(file))),
            Object::Notification(notification) => {
                Ok(Watched::Notification(Arc::downgrade(notification)))
            }
            _ => Err(error(Kernel::NotPermitted)),
        }
    }

    /// The description, unless it has gone or this is a user event.
    fn upgrade(&self) -> Option<Object> {
        match self {
            Watched::File(file) => file.upgrade().map(Object::File),
            Watched::Notification(notification) => notification.upgrade().map(Object::Notification),
            Watched::User => None,
        }
    }

    /// Whether it is `object`.
    fn is(&self, object: &Object) -> bool {
        match (self, object) {
            (Watched::File(file), Object::File(other)) => {
                core::ptr::eq(file.as_ptr(), Arc::as_ptr(other))
            }
            (Watched::Notification(notification), Object::Notification(other)) => {
                core::ptr::eq(notification.as_ptr(), Arc::as_ptr(other))
            }
            _ => false,
        }
    }

    /// Whether its description has gone.
    fn has_gone(&self) -> bool {
        match self {
            Watched::File(file) => file.strong_count() == 0,
            Watched::Notification(notification) => notification.strong_count() == 0,
            Watched::User => false,
        }
    }
}

/// One watch of an event queue.
pub struct Watch {
    ident: u64,
    filter: QueueFilter,
    /// Whether a change queues it only if no other exclusive watch of its
    /// description was queued where a thread waits (`exclusive`): fixed
    /// when it is added.
    exclusive: bool,
    watched: Watched,
    /// Its queue's ring.
    ready: ReadyRef,
    /// [`QUEUED`] and [`DELETED`].
    marks: AtomicU8,
    /// What it keeps between looks, changed only with its queue's
    /// `registry` held.
    held: SpinLock<Held>,
    /// What it costs the account of the process that added it, given back
    /// when it goes.
    _charge: Charge,
}

/// What a watch keeps between looks.
struct Held {
    /// kqueue's state of it.
    state: State,
    /// The addresses of the `Polled`s it counts itself in; 0 for none.
    polled: [usize; POLLED_MAX],
}

impl Watch {
    /// Whether its queue has deleted it, or gone, so that what it watches
    /// may let go of it.
    pub fn is_deleted(&self) -> bool {
        self.marks.load(Ordering::Relaxed) & DELETED != 0
    }

    /// Whether it is exclusive (epoll's EPOLLEXCLUSIVE).
    pub fn is_exclusive(&self) -> bool {
        self.exclusive
    }

    /// Whether it watches for reading, which a change that only empties
    /// what it watches cannot make ready (ADR 0777). A watch of the poll
    /// filter (epoll's, ADR 0612) may watch for writing too, so it is not
    /// one.
    pub fn watches_reads(&self) -> bool {
        self.filter == QueueFilter::Read
    }
}

/// What a change to a watched description does to `watch`: puts it on its
/// queue's ring, unless it is on it already or deleted, and wakes the
/// queue's waiters and pollers. Called with any lock held, interrupts
/// masked or not, so it takes only the ring's lock and the wait table's.
///
/// Returns whether the watch is on the ring, by this call or before it,
/// and a thread was counted waiting on its queue then: whether the change
/// reached a waiter, for an exclusive watch ([`Polled::changed`]).
pub fn changed(watch: &WatchRef) -> bool {
    let ready = &watch.ready;
    let marked = watch
        .marks
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |marks| (marks == 0).then_some(QUEUED));
    match marked {
        Ok(_) => {}
        // Already on the ring: a waiter there will take it.
        Err(marks) if marks & DELETED == 0 => return ready.waits.waiting(),
        Err(_) => return false,
    }
    {
        let mut ring = ready.ring.lock_irq();
        if ready.closed.load(Ordering::Relaxed) {
            return false;
        }
        ring.push(watch.clone());
        ready.queued.store(ring.len, Ordering::Release);
    }
    ready.wake();
    ready.waits.waiting()
}

/// A queue's watches, by key.
struct Registry {
    /// Descriptors' watches, by descriptor: `read`'s, `write`'s and
    /// `poll`'s.
    descriptors: Vec<[Option<WatchRef>; 3], Kmalloc>,
    /// User events' watches, in order of `ident`.
    users: Vec<WatchRef, Kmalloc>,
    /// How many watches may be on the ring at once: those held, and those
    /// deleted while on it, until a look takes them off.
    on_ring_max: usize,
}

/// A descriptor watch's place in its descriptor's watches.
fn pair_index(filter: QueueFilter) -> usize {
    match filter {
        QueueFilter::Write => 1,
        QueueFilter::Poll => 2,
        QueueFilter::Read | QueueFilter::User => 0,
    }
}

impl Registry {
    fn get(&self, ident: u64, filter: QueueFilter) -> Option<&WatchRef> {
        if filter == QueueFilter::User {
            let at = self.users.binary_search_by_key(&ident, |watch| watch.ident).ok()?;
            return self.users.get(at);
        }
        let pair = self.descriptors.get(usize::try_from(ident).ok()?)?;
        pair[pair_index(filter)].as_ref()
    }

    fn take(&mut self, ident: u64, filter: QueueFilter) -> Option<WatchRef> {
        if filter == QueueFilter::User {
            let at = self.users.binary_search_by_key(&ident, |watch| watch.ident).ok()?;
            return Some(self.users.remove(at));
        }
        let pair = self.descriptors.get_mut(usize::try_from(ident).ok()?)?;
        pair[pair_index(filter)].take()
    }

    /// Makes room for a watch of `ident` and `filter`, so that [`put`]
    /// cannot fail.
    ///
    /// [`put`]: Registry::put
    fn make_room(&mut self, ident: u64, filter: QueueFilter) -> Result<()> {
        let no_memory = |_| error(Kernel::NoMemory);
        if filter == QueueFilter::User {
            return self.users.try_reserve(1).map_err(no_memory);
        }
        // The descriptor was open, so its number is a slot's.
        let index = usize::try_from(ident).map_err(|_| error(Kernel::NoSuchSlot))?;
        if index >= self.descriptors.len() {
            self.descriptors.try_reserve(index + 1 - self.descriptors.len()).map_err(no_memory)?;
            while self.descriptors.len() <= index {
                // Room was reserved above.
                let _ = self.descriptors.push_within_capacity([None, None, None]);
            }
        }
        Ok(())
    }

    /// Holds `watch`, in room [`Registry::make_room`] made; returns the
    /// watch it takes the place of.
    fn put(&mut self, watch: WatchRef) -> Option<WatchRef> {
        if watch.filter == QueueFilter::User {
            return match self.users.binary_search_by_key(&watch.ident, |held| held.ident) {
                Ok(at) => Some(core::mem::replace(&mut self.users[at], watch)),
                Err(at) => {
                    // Room was made for it.
                    let _ = self.users.push_within_capacity(watch);
                    self.users[at..].rotate_right(1);
                    None
                }
            };
        }
        // The pair was made for it.
        let index = usize::try_from(watch.ident).unwrap_or(usize::MAX);
        let pair = &mut self.descriptors[index];
        pair[pair_index(watch.filter)].replace(watch)
    }
    /// The watch of `ident` and `filter`, unless its description has gone,
    /// when it goes too.
    fn live(&mut self, ident: u64, filter: QueueFilter) -> Option<WatchRef> {
        let watch = self.get(ident, filter)?.clone();
        if watch.watched.has_gone() {
            self.take(ident, filter);
            self.delete(&watch);
            return None;
        }
        Some(watch)
    }

    /// Deletes `watch`, which it no longer holds: it is never queued
    /// again, and what it watches lets go of it. Its slot on the ring is
    /// given back now if it is not on the ring, or else by the look that
    /// takes it off.
    fn delete(&mut self, watch: &WatchRef) {
        if let Some(object) = watch.watched.upgrade() {
            poll::polled_by(&object, &mut |polled| polled.unwatch(watch));
        }
        if watch.marks.fetch_or(DELETED, Ordering::SeqCst) & QUEUED == 0 {
            self.on_ring_max -= 1;
        }
    }

    /// Holds `watch`, new, in room made for it, in place of any of its key,
    /// and queues it for a first look if it is enabled.
    fn hold(&mut self, watch: &WatchRef) {
        if let Some(replaced) = self.put(watch.clone()) {
            self.delete(&replaced);
        }
        if !watch.held.lock().state.enabled() {
            return;
        }
        // A new watch goes on the ring for the next wait to look at, but an
        // exclusive one is looked at now, as Linux's epoll looks at a new
        // registration: left on the ring, it would be reported for a change
        // that woke another queue's waiter in its place.
        if !watch.exclusive || ready_now(watch) {
            changed(watch);
        }
    }

    /// Looks at what `watch` watches: its event, if it is ready, and what
    /// becomes of it then.
    fn look_at(&mut self, watch: &WatchRef) -> Option<(QueueEvent, Then)> {
        let events = if let Watched::User = watch.watched {
            PollEvents::NONE
        } else {
            let Some(object) = watch.watched.upgrade() else {
                self.take(watch.ident, watch.filter);
                self.delete(watch);
                return None;
            };
            // Only a Unix socket gains `Polled`s, once it connects or
            // listens; nothing else is looked at for new ones.
            if gains_polled(&object) && attach(watch, &object).is_err() {
                // Its new parts would not queue the watch, so it is looked
                // at again by every call until there is memory to count it
                // in them.
                changed(watch);
            }
            PollEvents(u32::from(poll::events_of(&object)))
        };
        watch.held.lock().state.report(watch.ident, watch.filter, events)
    }
}

/// The status of a change of a watch the queue does not hold.
fn not_found() -> radix_abi::Status {
    File::NotFound.status(0)
}

impl EventQueue {
    /// A new queue, watching nothing.
    pub fn new() -> Option<QueueRef> {
        let ready = Ready {
            ring: SpinLock::new(Ring::new()),
            queued: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            waits: WaitCount::new(),
            polled: Polled::new(),
        };
        let registry = Registry {
            descriptors: Vec::new_in(kmalloc()),
            users: Vec::new_in(kmalloc()),
            on_ring_max: 0,
        };
        let ready = Arc::try_new_in(ready, kmalloc()).ok()?;
        Arc::try_new_in(EventQueue { registry: Mutex::new(registry), ready }, kmalloc()).ok()
    }

    /// What its pollers count themselves in.
    pub fn polled(&self) -> &Polled {
        &self.ready.polled
    }

    /// Whether a watch waits for a look, which makes it readable to
    /// `poll`.
    pub fn has_queued(&self) -> bool {
        self.ready.queued.load(Ordering::Acquire) > 0
    }

    /// `wait`, for `user`'s thread: its changes, then its events.
    fn wait(&self, user: &UserRef, a: &WaitArgs) -> Result<u32> {
        if a.count > CHANGES_MAX {
            return Err(Kernel::InvalidArgument.status(1));
        }
        let changes = u64::from(a.count) * QueueEvent::SIZE as u64;
        let events = u64::from(a.capacity) * QueueEvent::SIZE as u64;
        if !radix_ipc::range::user_bytes(a.changes, changes)
            || !radix_ipc::range::user_bytes(a.events, events)
        {
            return Err(error(Kernel::BadAddress));
        }
        let reported = self.make_changes(user, a)?;
        if reported > 0 || a.capacity == 0 {
            return Ok(reported);
        }
        let mut sink = |index: u32, event: &QueueEvent| {
            let at = a.events + u64::from(index) * QueueEvent::SIZE as u64;
            if Arch::copy_to_user(at, &watch::write(event)) {
                Ok(())
            } else {
                Err(error(Kernel::BadAddress))
            }
        };
        let forever = a.flags.contains(QueueWaitFlags::FOREVER);
        let timeout = (!forever).then_some(a.timeout_ns);
        let mask = a.flags.contains(QueueWaitFlags::MASK).then_some(a.mask);
        // The changes were made: a call restarted would make them again.
        let restart = if a.count == 0 { Restart::NoHandler } else { Restart::Never };
        self.wait_for(user, &mut sink, a.capacity, timeout, mask, restart)
    }

    /// Writes the events of the watches that are ready, at most `capacity`
    /// of them, through `sink`, waiting for one for at most `timeout`
    /// nanoseconds (`None`: as long as it takes; 0: not at all), with
    /// `mask` as the thread's signal mask while it waits, if one is given;
    /// returns how many it wrote. A signal ends the wait with interrupted,
    /// which `restart` says the call may be run again after. Always
    /// inlined: `wait` and `epoll_wait` each pay for no call more.
    #[inline(always)]
    fn wait_for(
        &self,
        user: &UserRef,
        sink: &mut impl Sink,
        capacity: u32,
        timeout: Option<u64>,
        mask: Option<u64>,
        restart: Restart,
    ) -> Result<u32> {
        if timeout == Some(0) && mask.is_none() {
            return self.look(sink, capacity);
        }
        self.wait_ready(user, sink, capacity, timeout, mask, restart)
    }

    /// Makes `a`'s changes, in order; returns how many of them it wrote
    /// back as events at `a.events`, because they failed or asked for a
    /// receipt, as kqueue's `kevent` does.
    fn make_changes(&self, user: &UserRef, a: &WaitArgs) -> Result<u32> {
        if a.count == 0 {
            // A wait alone, as Go's poller makes: the registry's lock is
            // not taken for it.
            return Ok(0);
        }
        let mut reported = 0;
        let mut bytes = [0u8; CHANGES_AT_ONCE * QueueEvent::SIZE];
        let mut registry = self.registry.lock();
        for first in (0..a.count as usize).step_by(CHANGES_AT_ONCE) {
            let count = (a.count as usize - first).min(CHANGES_AT_ONCE);
            let bytes = &mut bytes[..count * QueueEvent::SIZE];
            if !Arch::copy_from_user(bytes, a.changes + (first * QueueEvent::SIZE) as u64) {
                return Err(error(Kernel::BadAddress));
            }
            for change in bytes.as_chunks::<{ QueueEvent::SIZE }>().0 {
                let change = watch::read(change);
                let made = match watch::asks(&change) {
                    Ok(None) => continue,
                    Ok(Some((filter, asks))) => {
                        self.make(user, &mut registry, a.base, &change, filter, asks)
                    }
                    Err(watch::Invalid) => Err(error(Kernel::InvalidArgument)),
                };
                let receipt = QueueFlags(u32::from(change.flags)).contains(QueueFlags::RECEIPT);
                if made.is_ok() && !receipt {
                    continue;
                }
                if reported == a.capacity {
                    return made.map(|()| reported);
                }
                let status = made.err().map_or(0, |status| status.0);
                let event = watch::write(&watch::error_event(&change, status));
                let at = a.events + u64::from(reported) * QueueEvent::SIZE as u64;
                if !Arch::copy_to_user(at, &event) {
                    return Err(error(Kernel::BadAddress));
                }
                reported += 1;
            }
        }
        Ok(reported)
    }

    /// Makes one change, `asks` of the watch of `change.ident` and
    /// `filter`.
    fn make(
        &self,
        user: &UserRef,
        registry: &mut Registry,
        base: u32,
        change: &QueueEvent,
        filter: QueueFilter,
        asks: Asks,
    ) -> Result<()> {
        match asks {
            Asks::Add if filter == QueueFilter::User => self.add(user, registry, change, filter),
            Asks::Add => self.add_descriptor(user, registry, base, change, filter),
            Asks::Delete => {
                if filter == QueueFilter::Poll {
                    let watch = registry.get(change.ident, filter).ok_or_else(not_found)?;
                    if !names_watched(user, base, change.ident, watch)? {
                        return Err(not_found());
                    }
                }
                let watch = registry.take(change.ident, filter).ok_or_else(not_found)?;
                let gone = watch.watched.has_gone();
                registry.delete(&watch);
                // A watch went with its description, though the queue had
                // yet to look.
                if gone { Err(not_found()) } else { Ok(()) }
            }
            Asks::Change => {
                let watch = registry.live(change.ident, filter).ok_or_else(not_found)?;
                if filter == QueueFilter::Poll {
                    if !names_watched(user, base, change.ident, &watch)? {
                        return Err(not_found());
                    }
                    // epoll_ctl's EPOLL_CTL_MOD of an exclusive watch.
                    if watch.exclusive {
                        return Err(error(Kernel::InvalidArgument));
                    }
                }
                let enabled = {
                    let mut held = watch.held.lock();
                    held.state.change(change, filter, false);
                    held.state.enabled()
                };
                if enabled {
                    changed(&watch);
                }
                Ok(())
            }
        }
    }

    /// Adds a watch of the description in the slot `change.ident` names,
    /// or changes the one held, if it watches that description; one of
    /// another description goes.
    fn add_descriptor(
        &self,
        user: &UserRef,
        registry: &mut Registry,
        base: u32,
        change: &QueueEvent,
        filter: QueueFilter,
    ) -> Result<()> {
        let slot = u32::try_from(change.ident)
            .ok()
            .and_then(|fd| base.checked_add(fd))
            .ok_or(error(Kernel::NoSuchSlot))?;
        let object =
            objects::find_object(user.process.cspace(), slot).ok_or(error(Kernel::EmptySlot))?;
        let watched = Watched::of(&object)?;
        if let Some(held) = registry.live(change.ident, filter)
            && held.watched.is(&object)
        {
            // epoll_ctl's EPOLL_CTL_ADD of what it watches already.
            if watch::creates(change) {
                return Err(File::Exists.status(0));
            }
            // An exclusive watch is changed by no add.
            if held.exclusive || watch::exclusive(change) {
                return Err(error(Kernel::InvalidArgument));
            }
            let enabled = {
                let mut held = held.held.lock();
                held.state.change(change, filter, true);
                held.state.enabled()
            };
            if enabled {
                changed(&held);
            }
            return Ok(());
        }
        self.make_room(registry, change.ident, filter)?;
        let watch = match new_watch(user, &self.ready, change, filter, watched) {
            Ok(watch) => watch,
            Err(status) => {
                registry.on_ring_max -= 1;
                return Err(status);
            }
        };
        if let Err(status) = attach(&watch, &object) {
            registry.delete(&watch);
            return Err(status);
        }
        registry.hold(&watch);
        Ok(())
    }

    /// Adds a user event's watch, or changes the one held.
    fn add(
        &self,
        user: &UserRef,
        registry: &mut Registry,
        change: &QueueEvent,
        filter: QueueFilter,
    ) -> Result<()> {
        if let Some(held) = registry.get(change.ident, filter) {
            let held = held.clone();
            let enabled = {
                let mut held = held.held.lock();
                held.state.change(change, filter, true);
                held.state.enabled()
            };
            if enabled {
                changed(&held);
            }
            return Ok(());
        }
        self.make_room(registry, change.ident, filter)?;
        match new_watch(user, &self.ready, change, filter, Watched::User) {
            Ok(watch) => {
                registry.hold(&watch);
                Ok(())
            }
            Err(status) => {
                registry.on_ring_max -= 1;
                Err(status)
            }
        }
    }

    /// Makes room for one more watch, of `ident` and `filter`: in the
    /// registry, and a slot on the ring, which is made with the ring's
    /// lock let go and its watches moved over with it held.
    fn make_room(&self, registry: &mut Registry, ident: u64, filter: QueueFilter) -> Result<()> {
        registry.make_room(ident, filter)?;
        let want = registry.on_ring_max + 1;
        let size = self.ready.ring.lock_irq().slots.len();
        if want > size {
            let size = want.max(size.saturating_mul(2));
            let mut slots = Vec::new_in(kmalloc());
            slots.try_reserve_exact(size).map_err(|_| error(Kernel::NoMemory))?;
            while slots.len() < size {
                // Room was reserved above.
                let _ = slots.push_within_capacity(None);
            }
            let old = self.ready.ring.lock_irq().regrow(slots);
            drop(old);
        }
        registry.on_ring_max = want;
        Ok(())
    }

    /// Takes watches off the ring, as many as are on it, and writes the
    /// events of those ready, at most `capacity`, through `sink`; returns
    /// how many it wrote. A watch it puts back on the ring waits for the
    /// next call.
    fn look(&self, sink: &mut impl Sink, capacity: u32) -> Result<u32> {
        if !self.has_queued() {
            // Nothing to look at, and no lock taken: a watch put on the
            // ring after this load wakes a waiter counted before it.
            return Ok(0);
        }
        let mut registry = self.registry.lock();
        let mut left = self.ready.queued.load(Ordering::Acquire);
        let mut found = 0;
        while found < capacity && left > 0 {
            let Some(watch) = self.ready.pop() else { break };
            left -= 1;
            // Cleared before the look, with a barrier between, so that a
            // change the look misses queues the watch again.
            let marks = watch.marks.fetch_and(!QUEUED, Ordering::SeqCst);
            barrier::after_rmw();
            if marks & DELETED != 0 {
                registry.on_ring_max -= 1;
                continue;
            }
            let Some((event, then)) = registry.look_at(&watch) else { continue };
            match then {
                Then::LookAgain => {
                    changed(&watch);
                }
                Then::AwaitChange => {}
                Then::Delete => {
                    registry.take(watch.ident, watch.filter);
                    registry.delete(&watch);
                }
            }
            sink(found, &event)?;
            found += 1;
        }
        Ok(found)
    }

    /// Waits, as [`EventQueue::wait_for`] says, until a watch is ready,
    /// and writes the events of those that are; returns how many, 0 if the
    /// time ran out.
    fn wait_ready(
        &self,
        user: &UserRef,
        sink: &mut impl Sink,
        capacity: u32,
        timeout: Option<u64>,
        mask: Option<u64>,
        restart: Restart,
    ) -> Result<u32> {
        let masked = mask.is_some();
        if let Some(mask) = mask {
            signal::begin_masked_wait(user, mask);
        }
        let deadline = timeout.map(|timeout| sched::now_ns().saturating_add(timeout));
        let mut timed_out = false;
        let found = loop {
            let found = match self.look(sink, capacity) {
                Ok(found) => found,
                Err(status) => {
                    if masked {
                        signal::end_masked_wait(user);
                    }
                    return Err(status);
                }
            };
            if found > 0 || timed_out {
                break found;
            }
            if sched::interrupted() {
                // The mask stays until the handler has run.
                return Err(interrupted(restart));
            }
            let ready = || self.has_queued() || sched::interrupted();
            let waits = &self.ready.waits;
            match deadline {
                None => waits.wait_until(self.ready.key(), "event queue", ready),
                Some(deadline) => {
                    // A last look, as `poll` takes one, when the time runs out.
                    timed_out = !waits.wait_until_or_timeout(
                        self.ready.key(),
                        "event queue",
                        deadline,
                        ready,
                    );
                }
            }
        };
        if masked {
            signal::end_masked_wait(user);
        }
        Ok(found)
    }
}

impl Drop for EventQueue {
    /// Its watches are marked deleted, for the `Polled`s that hold them to
    /// let go of, and its ring is emptied, which breaks the cycle of the
    /// watches on it, which hold the ring. It may go with the store's
    /// locks held, so it takes only spin locks, and frees only memory.
    fn drop(&mut self) {
        let emptied = {
            let mut ring = self.ready.ring.lock_irq();
            self.ready.closed.store(true, Ordering::Relaxed);
            core::mem::replace(&mut *ring, Ring::new())
        };
        drop(emptied);
        let registry = self.registry.get_mut();
        let pairs = registry.descriptors.iter().flat_map(|pair| pair.iter().flatten());
        for watch in pairs.chain(registry.users.iter()) {
            watch.marks.fetch_or(DELETED, Ordering::SeqCst);
        }
    }
}

/// A new watch, as `change` adds it, of `filter` and `watched`, on the
/// ring of `ready`, charged to `user`'s process.
fn new_watch(
    user: &UserRef,
    ready: &ReadyRef,
    change: &QueueEvent,
    filter: QueueFilter,
    watched: Watched,
) -> Result<WatchRef> {
    let charge = Account::charge_from(user.process.account(), &user.stock, WATCH_BYTES)?;
    let watch = Watch {
        ident: change.ident,
        filter,
        exclusive: watch::exclusive(change),
        watched,
        ready: ready.clone(),
        marks: AtomicU8::new(0),
        held: SpinLock::new(Held { state: State::added(change, filter), polled: [0; POLLED_MAX] }),
        _charge: charge,
    };
    Arc::try_new_in(watch, kmalloc()).map_err(|_| error(Kernel::NoMemory))
}

/// Whether the descriptor `ident`, the slot `base + ident` of `user`'s
/// process, names the description `watch` watches: a `poll` watch is
/// found by both, as Linux's epoll finds a registration by descriptor and
/// file.
///
/// # Errors
///
/// no_such_slot and empty_slot for a descriptor that is not open.
fn names_watched(user: &UserRef, base: u32, ident: u64, watch: &WatchRef) -> Result<bool> {
    let slot = u32::try_from(ident)
        .ok()
        .and_then(|fd| base.checked_add(fd))
        .ok_or(error(Kernel::NoSuchSlot))?;
    let object =
        objects::find_object(user.process.cspace(), slot).ok_or(error(Kernel::EmptySlot))?;
    Ok(watch.watched.is(&object))
}

/// Whether `object` may gain `Polled`s after a watch is added: a Unix
/// socket's connection or listener.
fn gains_polled(object: &Object) -> bool {
    matches!(object, Object::File(file) if file.socket().is_some())
}

/// Counts `watch` in each `Polled` of `object` it is not counted in yet:
/// every one when it is added, and later those a Unix socket gains when it
/// connects or listens. Its `held` lock is let go while it is counted,
/// which may allocate.
///
/// # Errors
///
/// no_memory if a `Polled` has no room for it.
fn attach(watch: &WatchRef, object: &Object) -> Result<()> {
    let mut attached = Ok(());
    poll::polled_by(object, &mut |polled| {
        let at = core::ptr::from_ref(polled) as usize;
        let free = {
            let held = watch.held.lock();
            if attached.is_err() || held.polled.contains(&at) {
                return;
            }
            held.polled.iter().position(|&address| address == 0)
        };
        let Some(free) = free else {
            debug_assert!(false, "a description with more than {POLLED_MAX} Polleds");
            return;
        };
        attached = polled.watch(watch);
        if attached.is_ok() {
            watch.held.lock().polled[free] = at;
        }
    });
    attached
}

/// Whether `watch`, just added and counted in its description's
/// `Polled`s, is ready now; also when that takes a look to tell, because
/// the description has gone or its new parts could not count the watch.
/// A change made before the watch was counted is seen here, through the
/// barrier, and one made after queues the watch.
fn ready_now(watch: &WatchRef) -> bool {
    let Some(object) = watch.watched.upgrade() else { return true };
    if gains_polled(&object) && attach(watch, &object).is_err() {
        return true;
    }
    barrier::full();
    let events = PollEvents(u32::from(poll::events_of(&object)));
    watch.held.lock().state.ready(watch.filter, events)
}

/// Calls the method `label` on the queue `found` holds, for `user`.
#[inline(never)]
pub fn method(user: &UserRef, found: &Found, label: u32, args: &[u64; 8]) -> Result<Answer> {
    match label {
        abi::WAIT => {
            let a = WaitArgs::decode(args)?;
            let mut rights = Rights::NONE;
            if a.count > 0 {
                rights = rights.union(Rights::WRITE);
            }
            if a.capacity > 0 {
                rights = rights.union(Rights::READ);
            }
            found.cap.check(ObjectKind::EventQueue, rights).map_err(cap_error)?;
            let Some(Object::EventQueue(queue)) = &found.object else {
                return Err(error(Kernel::Gone));
            };
            let found = queue.wait(user, &a)?;
            answer(label, WaitResults::WORDS, WaitResults { found }.encode())
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}

/// The queue `found` holds, if its capability has `rights`, as `wait`
/// checks it.
fn queue_of(found: &Found, rights: Rights) -> Result<&QueueRef> {
    found.cap.check(ObjectKind::EventQueue, rights).map_err(cap_error)?;
    let Some(Object::EventQueue(queue)) = &found.object else {
        return Err(error(Kernel::Gone));
    };
    Ok(queue)
}

/// The Linux table's `epoll_ctl` (ADR 0612): makes `change`, in the
/// kernel's memory, on the queue `found` holds, for `user`, as `wait`
/// makes one of its changes, with the right it needs (write). Returns the
/// change's status rather than an event.
pub fn change_given(user: &UserRef, found: &Found, change: &QueueEvent, base: u32) -> Result<()> {
    let queue = queue_of(found, Rights::WRITE)?;
    let asked = watch::asks(change).map_err(|watch::Invalid| error(Kernel::InvalidArgument))?;
    let Some((filter, asks)) = asked else { return Ok(()) };
    let mut registry = queue.registry.lock();
    queue.make(user, &mut registry, base, change, filter, asks)
}

/// The Linux table's `epoll_wait` (ADR 0612): waits on the queue `found`
/// holds, as `wait` with no changes does, with the right it needs (read),
/// giving `sink` the events. A signal ends the wait for good, as Linux's
/// epoll_wait is never run again (signal(7)).
pub fn wait_given(
    user: &UserRef,
    found: &Found,
    sink: &mut impl Sink,
    capacity: u32,
    timeout: Option<u64>,
    mask: Option<u64>,
) -> Result<u32> {
    let queue = queue_of(found, Rights::READ)?;
    queue.wait_for(user, sink, capacity, timeout, mask, Restart::Never)
}

/// radix-process(2)'s `event_queue`: a new queue, charged to `user`'s
/// process, in its slot `to`, closed by `exec` if `flags` say so, placed
/// and flagged under one hold of the space's lock, as a file is
/// (`install_file`), so that no `exec` finds it without its flag.
///
/// # Errors
///
/// invalid_argument for non_blocking, which a queue has no use for;
/// occupied for a slot that holds something; no_memory.
pub fn make_in(user: &UserRef, to: u32, flags: PipeFlags) -> Result<()> {
    if flags.contains(PipeFlags::NON_BLOCKING) {
        return Err(Kernel::InvalidArgument.status(2));
    }
    // From the thread's own stock of its process's charges (ADR 0405).
    let bytes = size_of::<EventQueue>() as u64;
    let charge = Account::charge_from(user.process.account(), &user.stock, bytes)?;
    let queue = EventQueue::new().ok_or(error(Kernel::NoMemory))?;
    let mut object = Some(Object::EventQueue(queue));
    let close_on_exec = flags.contains(PipeFlags::CLOSE_ON_EXEC);
    let slot_flags = if close_on_exec { SlotFlags::CLOSE_ON_EXEC.0 as u8 } else { 0 };
    let kind = ObjectKind::EventQueue;
    let record = |_, kept: &mut _| objects::record(kept, &mut object, Some(charge));
    let at = SlotRef::new(user.process.cspace(), to);
    let made =
        caps::create_in(at, kind, kind.rights(), slot_flags, record).map(drop).map_err(cap_error);
    // Not made: it goes now, with the store's locks gone.
    drop(object);
    made
}
