// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `poll` (Phase 8, radix-process(2)): waits until one of many
//! capabilities is ready, as POSIX's `poll` and `ppoll`.
//!
//! The entries are copied in (onto the kernel stack, for up to
//! [`ON_STACK`] of them), then each slot is looked at: a pipe's end
//! or a terminal's side says what it is ready for
//! ([`crate::fs::pipe::Pipe::events`], [`crate::fs::tty::Handle::events`]),
//! a notification is readable while bits are set, other files and
//! directories are always ready, and an empty slot is invalid. Up to as
//! many slots as a CPU's capability cache holds are found through it
//! ([`objects::find_object`]); more would miss it and take their space's
//! lock one by one, so they are looked at with that lock taken once for
//! them all, and each object borrowed from it ([`look_space`], ADR 0354).
//! An object whose look may wait for a lock of its own, as a network
//! socket's does, is held and looked at with the space's lock let go. If none is
//! ready, and the poll may wait, the thread holds the objects its entries
//! designate, as Linux's `poll` holds its files (a slot closed meanwhile
//! does not change what it waits for), adds itself to the pollers of each
//! ([`Polled`]), and sleeps until one of them changes: whatever a poll
//! looks at calls [`Polled::changed`] after it changes, which tells each
//! thread polling it to look again and wakes it. A woken poller does not
//! know which of its objects changed, so it looks at them all again, as
//! Linux's does. A change wakes only the threads that poll what changed,
//! and a change to what nobody polls, such as a pipe that is only read and
//! written, wakes nobody (ADR 0558).
//!
//! No wake-up is lost between a poller's look and its sleep: a poller
//! adds itself to each object's [`Polled`], then clears its flag
//! ([`crate::user::UserThread`]'s `poll_changed`), before it looks, and
//! [`Polled::changed`] reads the object's count of pollers after the
//! change, each with a full barrier between, so either the poller sees the
//! change or the change sees the poller, sets its flag and wakes it; a
//! wake that comes before the sleep makes the sleep return at once. A
//! notification, whose word changes by a read-modify-write that is already
//! such a barrier, skips the extra one ([`Polled::changed_by_rmw`]), as do
//! a pipe and an event queue, which have just taken one to wake their own
//! waiters. The TLA+ model `kernel/radix/tla/Poll.tla` checks this.
//!
//! A [`Polled`] also holds the event queues' watches of its object
//! (radix-event-queue(2), `crate::user::queue`): a change queues each of
//! them for a look by its queue, with the same barrier, so that a queue
//! looks only at what changed, and nothing else wakes.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering, fence};

use radix_abi::errors::Kernel;
use radix_abi::process::{PollArgs, PollEntry, PollEvents, PollFlags};
use radix_cap::{SlotRef, SpaceId};
use radix_hal::UserMode;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::SpinLock;

use super::kmalloc;
use super::signal::{self, Restart, interrupted};
use crate::fs::FileObject;
use crate::sched::{self, ThreadRef};
use crate::user::notify::Notification;
use crate::user::objects::{self, Object, Peeked};
use crate::user::queue::EventQueue;
use crate::user::queue::{self, WatchRef};
use crate::user::{Result, UserRef, error};
use crate::{Arch, caps};

/// There is data to read, or the end of a stream.
pub const READABLE: u16 = PollEvents::READABLE.0 as u16;
/// A write would not wait.
pub const WRITABLE: u16 = PollEvents::WRITABLE.0 as u16;
/// An error: a pipe no end reads, a terminal that hung up.
pub const ERROR: u16 = PollEvents::ERROR.0 as u16;
/// The other end has gone.
pub const HANG_UP: u16 = PollEvents::HANG_UP.0 as u16;
/// The slot holds no capability.
pub const INVALID: u16 = PollEvents::INVALID.0 as u16;

/// The events reported whether asked for or not.
const ALWAYS: u16 = ERROR | HANG_UP | INVALID;

/// The most entries one call takes: as many as Linux's default limit of
/// open files allows.
pub const ENTRIES_MAX: u32 = 4096;

/// What a poll may look at keeps one of these: the threads polling it,
/// which its changes wake, and the event queues' watches of it, which its
/// changes queue.
pub struct Polled {
    /// How many threads `lists` holds, so that a change to what nobody
    /// polls takes no lock.
    pollers: AtomicUsize,
    /// How many watches `lists` holds, likewise.
    watched: AtomicUsize,
    /// Taken with interrupts masked: the keyboard's and the mouse's
    /// change in their interrupt handlers.
    lists: SpinLock<Lists>,
}

/// Whom a change to a [`Polled`]'s object reaches.
struct Lists {
    /// The threads waiting in `poll` for it to change, once for each time
    /// a poll names it.
    pollers: Vec<ThreadRef, Kmalloc>,
    /// The watches, with those whose queue has deleted them until the
    /// next one is added.
    watches: Vec<WatchRef, Kmalloc>,
}

impl Polled {
    /// Nobody polls or watches it yet.
    pub const fn new() -> Polled {
        Polled {
            pollers: AtomicUsize::new(0),
            watched: AtomicUsize::new(0),
            lists: SpinLock::new(Lists {
                pollers: Vec::new_in(HEAP.with(Flags::MAY_SLEEP)),
                watches: Vec::new_in(HEAP.with(Flags::MAY_SLEEP)),
            }),
        }
    }

    /// It has changed: the threads polling it look again, and the queues
    /// watching it look at it. Costs a barrier and two loads while nobody
    /// polls or watches it.
    pub fn changed(&self) {
        fence(Ordering::SeqCst);
        self.changed_by_rmw();
    }

    /// As [`Polled::changed`], for a change already ordered before what
    /// it reads as the barrier would order it: one made by a sequentially
    /// consistent read-modify-write that pollers read with sequentially
    /// consistent loads (a notification's word), or one its caller took a
    /// full barrier after (`WaitCount::wake_all`'s, for a pipe and an
    /// event queue). Only two loads while nobody polls or watches it.
    pub fn changed_by_rmw(&self) {
        if self.pollers.load(Ordering::SeqCst) == 0 && self.watched.load(Ordering::SeqCst) == 0 {
            return;
        }
        let lists = self.lists.lock_irq();
        for thread in &lists.pollers {
            wake_poller(thread);
        }
        for watch in &lists.watches {
            queue::changed(watch);
        }
    }

    /// Adds `watch` to its watches, which its changes queue from then on,
    /// and lets go of those whose queue deleted them: a watch's last
    /// reference frees memory and gives back its charge, nothing more, so
    /// it may go with the lock held.
    ///
    /// # Errors
    ///
    /// no_memory when there is no room for one more.
    pub fn watch(&self, watch: &WatchRef) -> Result<()> {
        let tidy = |watches: &mut Vec<WatchRef, Kmalloc>| watches.retain(|held| !held.is_deleted());
        self.push(|lists| &mut lists.watches, &self.watched, watch.clone(), tidy)
    }

    /// Takes `watch` out of its watches, if it is there: its queue has
    /// deleted it.
    pub fn unwatch(&self, watch: &WatchRef) {
        let taken = {
            let mut lists = self.lists.lock_irq();
            let at = lists.watches.iter().position(|held| WatchRef::ptr_eq(held, watch));
            let taken = at.map(|at| lists.watches.swap_remove(at));
            self.watched.store(lists.watches.len(), Ordering::SeqCst);
            taken
        };
        drop(taken);
    }

    /// Adds `thread`, about to wait in `poll`, to its pollers, which its
    /// changes wake from then on.
    ///
    /// # Errors
    ///
    /// no_memory when there is no room for one more.
    fn add_poller(&self, thread: &ThreadRef) -> Result<()> {
        self.push(|lists| &mut lists.pollers, &self.pollers, thread.clone(), |_| {})
    }

    /// Takes `thread` off its pollers, once: its poll has looked for the
    /// last time.
    fn remove_poller(&self, thread: &ThreadRef) {
        let taken = {
            let mut lists = self.lists.lock_irq();
            let at = lists.pollers.iter().position(|held| ThreadRef::ptr_eq(held, thread));
            let taken = at.map(|at| lists.pollers.swap_remove(at));
            // No change needs to reach the poll any more: a count left
            // high a while longer costs a look at the list.
            self.pollers.store(lists.pollers.len(), Ordering::Relaxed);
            taken
        };
        drop(taken);
    }

    /// Pushes `item` onto the list that `list` picks, once `tidy` has let
    /// go of what that list no longer needs, and stores the list's new
    /// length in `count`, all with the lock held. Room is made with the
    /// lock let go, as memory is never allocated under a spin lock, and
    /// kept: a list that has grown for its object's pollers or watches
    /// does not allocate again for as many.
    ///
    /// # Errors
    ///
    /// no_memory when there is no room for one more.
    fn push<T>(
        &self,
        list: fn(&mut Lists) -> &mut Vec<T, Kmalloc>,
        count: &AtomicUsize,
        item: T,
        tidy: fn(&mut Vec<T, Kmalloc>),
    ) -> Result<()> {
        let mut room: Vec<T, Kmalloc> = Vec::new_in(kmalloc());
        loop {
            let mut lists = self.lists.lock_irq();
            let held = list(&mut lists);
            tidy(held);
            if held.len() == held.capacity() && room.capacity() > held.len() {
                while let Some(kept) = held.pop() {
                    // `room` holds more than `held` did.
                    let _ = room.push_within_capacity(kept);
                }
                core::mem::swap(held, &mut room);
            }
            if held.len() < held.capacity() {
                // Checked just above.
                let _ = held.push_within_capacity(item);
                count.store(held.len(), Ordering::SeqCst);
                return Ok(());
            }
            let want = held.len().saturating_mul(2).max(2);
            drop(lists);
            room = Vec::new_in(kmalloc());
            room.try_reserve_exact(want).map_err(|_| error(Kernel::NoMemory))?;
        }
    }
}

/// Tells `thread`, which polls an object that has changed, to look again,
/// and wakes it, unless a change has told it already since its last look.
fn wake_poller(thread: &ThreadRef) {
    if let Some(user) = thread.user()
        && !user.poll_changed.swap(true, Ordering::SeqCst)
    {
        sched::wake(thread);
    }
}

/// Calls `each` with what `object` keeps for its pollers: a pipe end's
/// pipe, a Unix socket's own and both pipes of its connection or its
/// listener, its own for a network socket, a terminal, the kernel log or
/// the keyboard, a notification, or an event queue.
pub(crate) fn polled_by(object: &Object, each: &mut dyn FnMut(&Polled)) {
    match object {
        Object::File(file) => {
            if let Some(end) = file.pipe_end() {
                each(end.pipe().polled());
            } else if let Some(socket) = file.socket() {
                socket.polled(each);
            } else if let Some(socket) = file.inet() {
                socket.polled(each);
            } else if let Some(handle) = file.tty_handle() {
                each(handle.polled());
            } else if file.is_kmsg() {
                each(&crate::fs::kmsg::POLLED);
            } else if file.is_keyboard() {
                each(&crate::fs::keyboard::POLLED);
            } else if file.is_mouse() {
                each(&crate::fs::mouse::POLLED);
            } else if file.fuse_channel().is_some() {
                each(&crate::fs::fuse::POLLED);
            } else if file.is_tunnel() {
                each(&crate::fs::tunnel::POLLED);
            } else if file.is_capture() {
                each(&crate::fs::capture::POLLED);
            } else if file.is_audio() {
                each(&crate::audio::POLLED);
            } else if file.is_crashes() {
                each(&crate::fs::crashes::POLLED);
            }
        }
        Object::Notification(notification) => each(notification.polled()),
        Object::EventQueue(queue) => each(queue.polled()),
        // Always ready, or never.
        _ => {}
    }
}

/// Keeps a thread among the pollers of `objects` while it lives: in each
/// [`Polled`] that [`polled_by`] names for them.
struct Polling<'a> {
    objects: &'a [Option<Object>],
    thread: ThreadRef,
    /// How many of those [`Polled`]s, in [`polled_by`]'s order, hold it.
    added: usize,
}

impl<'a> Polling<'a> {
    /// Adds `thread` to the pollers of each of `objects`.
    ///
    /// # Errors
    ///
    /// no_memory when the pollers of one have no room for one more; the
    /// thread is then taken off those it was added to.
    fn new(objects: &'a [Option<Object>], thread: ThreadRef) -> Result<Polling<'a>> {
        let mut polling = Polling { objects, thread, added: 0 };
        let mut failed = None;
        for object in objects.iter().flatten() {
            polled_by(object, &mut |polled| {
                if failed.is_none() {
                    match polled.add_poller(&polling.thread) {
                        Ok(()) => polling.added += 1,
                        Err(status) => failed = Some(status),
                    }
                }
            });
            if let Some(status) = failed {
                return Err(status);
            }
        }
        Ok(polling)
    }
}

impl Drop for Polling<'_> {
    fn drop(&mut self) {
        let mut left = self.added;
        for object in self.objects.iter().flatten() {
            if left == 0 {
                break;
            }
            polled_by(object, &mut |polled| {
                if left > 0 {
                    left -= 1;
                    polled.remove_poller(&self.thread);
                }
            });
        }
    }
}

/// An entry's descriptor, and the events it asks for.
fn entry_of(entry: [u8; PollEntry::SIZE]) -> (i32, u16) {
    let fd = i32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
    let events =
        u16::from_le_bytes([entry[PollEntry::OFFSET_EVENTS], entry[PollEntry::OFFSET_EVENTS + 1]]);
    (fd, events)
}

/// The events an entry held found when it was copied in.
fn found_of(entry: [u8; PollEntry::SIZE]) -> u16 {
    u16::from_le_bytes([entry[PollEntry::OFFSET_FOUND], entry[PollEntry::OFFSET_FOUND + 1]])
}

fn entries_of(entries: &[u8]) -> core::slice::Iter<'_, [u8; PollEntry::SIZE]> {
    entries.as_chunks::<{ PollEntry::SIZE }>().0.iter()
}

/// The slot of descriptor `fd`, which is not negative, past `base`; `None`
/// past the last slot there can be.
fn slot_of(base: u32, fd: i32) -> Option<u32> {
    base.checked_add(fd as u32)
}

/// What slot `slot` of `cspace` is ready for, among `events` and those
/// always reported.
fn slot_events(cspace: SpaceId, slot: u32, events: u16) -> u16 {
    let found = match objects::find_object(cspace, slot) {
        Some(object) => events_of(&object),
        // An empty slot, no kernel object (a memory capability), or
        // released since the capability was found.
        None => INVALID,
    };
    found & (events | ALWAYS)
}

/// What `object` is ready for.
pub(crate) fn events_of(object: &Object) -> u16 {
    match object {
        Object::File(file) => file_events(file),
        Object::Directory(_) => READABLE | WRITABLE,
        Object::Notification(notification) => notification_events(notification),
        Object::EventQueue(queue) => queue_events(queue),
        _ => 0,
    }
}

/// What `file` is ready for.
fn file_events(file: &FileObject) -> u16 {
    if let Some(events) = file_events_now(file) {
        events
    } else if let Some(socket) = file.inet() {
        socket.events()
    } else if file.is_kmsg() {
        crate::fs::kmsg::events(file) | WRITABLE
    } else if file.is_keyboard() {
        crate::fs::keyboard::events(file)
    } else if file.is_mouse() {
        crate::fs::mouse::events(file)
    } else if file.fuse_channel().is_some() {
        crate::fs::fuse::events(file)
    } else if file.is_tunnel() {
        crate::fs::tunnel::events(file)
    } else if file.is_capture() {
        crate::fs::capture::events(file)
    } else if file.is_audio() {
        crate::audio::events(file)
    } else if file.is_crashes() {
        crate::fs::crashes::events(file)
    } else {
        READABLE | WRITABLE
    }
}

/// What `file` is ready for, if a look finds it without waiting for a
/// lock: a pipe's end, a Unix socket and a terminal's side keep it in
/// atomics and behind spin locks ([`FileObject::polls_now`]), and a file
/// or directory of a filesystem is always ready. `None` for every other
/// kind, whose look may wait for a lock (a network socket's, an audio
/// output's) and so must not be done with a capability space locked
/// ([`look_space`]).
fn file_events_now(file: &FileObject) -> Option<u16> {
    if file.is_special() { file.at.node.poll_events() } else { Some(READABLE | WRITABLE) }
}

/// What `notification` is ready for: readable while bits are set.
fn notification_events(notification: &Notification) -> u16 {
    let (set, closed) = notification.peek();
    (if set { READABLE } else { 0 }) | (if closed { HANG_UP } else { 0 })
}

/// What `queue` is ready for: readable while a watch waits for a look,
/// though the look may find it no longer ready.
fn queue_events(queue: &EventQueue) -> u16 {
    if queue.has_queued() { READABLE } else { 0 }
}

/// What an object borrowed from a locked space is ready for, if a look
/// finds it without waiting for a lock ([`file_events_now`]).
fn peeked_events(peeked: Peeked<'_>) -> Option<u16> {
    Some(match peeked {
        Peeked::Empty => INVALID,
        Peeked::Ready => READABLE | WRITABLE,
        Peeked::File(file) => return file.at.node.poll_events(),
        Peeked::Notification(notification) => notification_events(notification),
        Peeked::EventQueue(queue) => queue_events(queue),
        Peeked::Later => return None,
        Peeked::Other => 0,
    })
}

/// Looks at every entry's slot, writing the events found in `found`;
/// returns how many found any.
fn look(cspace: SpaceId, base: u32, entries: &[u8], found: &mut [u16]) -> u32 {
    if found.len() > objects::CACHED {
        return look_space(cspace, base, entries, found);
    }
    let mut ready = 0;
    for (entry, found) in entries_of(entries).zip(found.iter_mut()) {
        let (fd, events) = entry_of(*entry);
        *found = if fd < 0 {
            0
        } else {
            match slot_of(base, fd) {
                Some(slot) => slot_events(cspace, slot, events),
                None => INVALID,
            }
        };
        ready += u32::from(*found != 0);
    }
    ready
}

/// How many objects whose look waits for a lock [`look_space`] holds at a
/// time, to look at once the space's lock is let go.
const LATER: usize = 8;

/// [`look`], for more entries than a CPU's capability cache holds, which
/// would miss on most of them and take the space's lock for each (ADR
/// 0354). The lock is taken once, and each entry's object borrowed from
/// it ([`objects::peek`]), with neither a lock nor a reference of its own.
/// An object whose look may wait for a lock ([`file_events_now`]) is held
/// instead, up to [`LATER`] of them; then the space's lock goes while they
/// are looked at, and is taken again for the entries after them.
fn look_space(cspace: SpaceId, base: u32, entries: &[u8], found: &mut [u16]) -> u32 {
    let entries = entries_of(entries).as_slice();
    let mut next = 0;
    while next < found.len() {
        let mut later: [Option<(usize, Object)>; LATER] = [const { None }; LATER];
        let looked = caps::in_space(cspace, |space| {
            let mut held = 0;
            while next < found.len() && held < LATER {
                let (fd, events) = entry_of(entries[next]);
                found[next] = if fd < 0 {
                    0
                } else if let Some(slot) = slot_of(base, fd) {
                    let at = SlotRef::new(cspace, slot);
                    match peeked_events(objects::peek(space, at)) {
                        Some(found) => found & (events | ALWAYS),
                        None => match objects::find_locked(space, at) {
                            Some(object) => {
                                later[held] = Some((next, object));
                                held += 1;
                                0
                            }
                            None => INVALID,
                        },
                    }
                } else {
                    INVALID
                };
                next += 1;
            }
        });
        if looked.is_err() {
            // The space has gone with its process: no slot holds anything.
            for (entry, found) in entries[next..].iter().zip(&mut found[next..]) {
                *found = if entry_of(*entry).0 < 0 { 0 } else { INVALID };
            }
            break;
        }
        for (at, object) in later.iter().flatten() {
            let (_, events) = entry_of(entries[*at]);
            found[*at] = events_of(object) & (events | ALWAYS);
        }
    }
    found.iter().map(|&found| u32::from(found != 0)).sum()
}

/// Holds in `held` the object each entry's slot names, for a poll that
/// waits, writing INVALID in `found` for each slot emptied since the first
/// look; returns how many were. More entries than a CPU's capability cache
/// holds are found with their space's lock taken once ([`look_space`]).
fn hold(
    cspace: SpaceId,
    base: u32,
    entries: &[u8],
    held: &mut [Option<Object>],
    found: &mut [u16],
) -> u32 {
    let many = found.len() > objects::CACHED;
    let mut each = |find: &mut dyn FnMut(u32) -> Option<Object>| {
        let mut emptied = 0;
        for ((entry, held), found) in entries_of(entries).zip(held.iter_mut()).zip(found.iter_mut())
        {
            let (fd, _) = entry_of(*entry);
            if fd < 0 {
                continue;
            }
            *held = slot_of(base, fd).and_then(&mut *find);
            if held.is_none() {
                *found = INVALID;
                emptied += 1;
            }
        }
        emptied
    };
    if !many {
        return each(&mut |slot| objects::find_object(cspace, slot));
    }
    caps::in_space(cspace, |space| {
        each(&mut |slot| objects::find_locked(space, SlotRef::new(cspace, slot)))
    })
    // The space has gone with its process: no slot holds anything.
    .unwrap_or_else(|_| each(&mut |_| None))
}

/// As [`look`], at the objects `held` for the entries (none for one left
/// out).
fn look_held(held: &[Option<Object>], entries: &[u8], found: &mut [u16]) -> u32 {
    let mut ready = 0;
    for ((entry, object), found) in entries_of(entries).zip(held).zip(found.iter_mut()) {
        let (_, events) = entry_of(*entry);
        *found = object.as_ref().map_or(0, |object| events_of(object) & (events | ALWAYS));
        ready += u32::from(*found != 0);
    }
    ready
}

/// The most entries a call keeps on its kernel stack; a call with more
/// copies them into the heap.
const ON_STACK: usize = 8;

/// `poll`, for `user`'s thread.
#[inline(never)]
pub fn poll(user: &UserRef, a: PollArgs) -> Result<u32> {
    if a.count > ENTRIES_MAX {
        return Err(Kernel::InvalidArgument.status(1));
    }
    let count = a.count as usize;
    let len = count * PollEntry::SIZE;
    if !radix_ipc::range::user_bytes(a.entries, len as u64) {
        return Err(error(Kernel::BadAddress));
    }
    let mut entries_here = [0u8; ON_STACK * PollEntry::SIZE];
    let mut found_here = [0u16; ON_STACK];
    let mut entries_heap: Vec<u8, Kmalloc>;
    let mut found_heap: Vec<u16, Kmalloc> = Vec::new_in(kmalloc());
    let (entries, found): (&mut [u8], &mut [u16]) = if count <= ON_STACK {
        (&mut entries_here[..len], &mut found_here[..count])
    } else {
        let no_memory = |_| error(Kernel::NoMemory);
        entries_heap = radix_mm::bytes::zeroed(len, kmalloc()).map_err(no_memory)?;
        found_heap.try_reserve_exact(count).map_err(no_memory)?;
        while found_heap.len() < count {
            // Room was reserved above.
            let _ = found_heap.push_within_capacity(0);
        }
        (&mut entries_heap, &mut found_heap)
    };
    if !Arch::copy_from_user(entries, a.entries) {
        return Err(error(Kernel::BadAddress));
    }
    let cspace = user.process.cspace();
    // A first look, without counting as a poller: what is ready, or a
    // poll that does not wait, returns at once. One with a signal mask
    // goes on to take it, so that a signal the mask lets through
    // interrupts it, as on Linux.
    let mut ready = look(cspace, a.base, entries, found);
    let waits = a.flags.contains(PollFlags::FOREVER) || a.timeout_ns > 0;
    if ready == 0 && (waits || a.flags.contains(PollFlags::MASK)) {
        ready = wait_ready(user, &a, cspace, entries, found)?;
    }
    // An array polled again mostly holds what it found last time, as an
    // entry left as ready or as idle as it was: only events that differ
    // from what an entry held are written, and nothing at all, not even
    // to a page shared until written, when none do.
    for (i, (entry, found)) in entries_of(entries).zip(found.iter()).enumerate() {
        if found_of(*entry) == *found {
            continue;
        }
        let at = a.entries + (i * PollEntry::SIZE + PollEntry::OFFSET_FOUND) as u64;
        if !Arch::copy_to_user(at, &found.to_le_bytes()) {
            return Err(error(Kernel::BadAddress));
        }
    }
    Ok(ready)
}

/// Waits, as `a` says, until one of `entries` is ready, writing what
/// each one is ready for in `found` (all 0, from a first look); returns
/// how many are, 0 if the time ran out.
fn wait_ready(
    user: &UserRef,
    a: &PollArgs,
    cspace: SpaceId,
    entries: &[u8],
    found: &mut [u16],
) -> Result<u32> {
    let mut held_here: [Option<Object>; ON_STACK] = [const { None }; ON_STACK];
    let mut held_heap: Vec<Option<Object>, Kmalloc> = Vec::new_in(kmalloc());
    let held: &mut [Option<Object>] = if found.len() <= ON_STACK {
        &mut held_here[..found.len()]
    } else {
        held_heap.try_reserve_exact(found.len()).map_err(|_| error(Kernel::NoMemory))?;
        while held_heap.len() < found.len() {
            // Room was reserved above.
            let _ = held_heap.push_within_capacity(None);
        }
        &mut held_heap
    };
    let emptied = hold(cspace, a.base, entries, held, found);
    if emptied > 0 {
        return Ok(emptied);
    }
    let me = sched::current().ok_or(error(Kernel::Gone))?;
    let polling = Polling::new(held, me)?;
    let masked = a.flags.contains(PollFlags::MASK);
    if masked {
        signal::begin_masked_wait(user, a.mask);
    }
    let deadline = (!a.flags.contains(PollFlags::FOREVER))
        .then(|| sched::now_ns().saturating_add(a.timeout_ns));
    let ready = loop {
        // Cleared, and the barrier taken, before the look: a change the
        // look misses sets it again (the module's doc says why).
        user.poll_changed.store(false, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        let ready = look_held(held, entries, found);
        if ready > 0 {
            break ready;
        }
        if sched::interrupted() {
            // The mask stays until the handler has run.
            return Err(interrupted(Restart::NoHandler));
        }
        if !sleep_until_changed(user, deadline) {
            break look_held(held, entries, found);
        }
    };
    drop(polling);
    if masked {
        signal::end_masked_wait(user);
    }
    Ok(ready)
}

/// Sleeps until something the running thread polls has changed, it has a
/// signal to take or a stop to make, or the kernel's clock reaches
/// `deadline`; returns false only for the deadline. A poller sleeps
/// outside the scheduler's wait table: nothing but a change
/// ([`wake_poller`]), a signal or a stop ([`sched::interrupt`]) and its
/// deadline wakes it, and a wake that comes before the sleep makes the
/// sleep return at once.
fn sleep_until_changed(user: &UserRef, deadline: Option<u64>) -> bool {
    loop {
        if user.poll_changed.load(Ordering::Acquire) || sched::interrupted() {
            return true;
        }
        match deadline {
            None => sched::block("poll"),
            Some(deadline) => {
                if sched::now_ns() >= deadline {
                    return false;
                }
                sched::block_until(deadline, "poll");
            }
        }
    }
}
