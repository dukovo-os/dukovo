// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A watch file (ADR 0615): what radix-process(2)'s `watch_file` puts in a
//! slot, as Linux's `inotify_init1` gives a descriptor. radix-file(2)'s
//! `watch_add` and `watch_remove` add and take away its watches of files
//! and directories (`super::notify`), and its reads give what became of
//! them, as Linux's `struct inotify_event` records.
//!
//! A read gives as many whole records as fit, waiting for the first
//! unless the description is non-blocking, and fails with invalid_argument
//! if the first does not fit. A poll finds it readable while it holds an
//! event; it is never writable. The queue holds [`WATCH_QUEUE_MAX`]
//! events, and then one `IN_Q_OVERFLOW` for those that did not fit, as
//! on Linux; an event equal to the last one queued is not queued again.
//! `control`'s FIONREAD gives the bytes the records waiting take.
//!
//! A watch file is a node of the pipe filesystem, as a tunnel's file is. Its
//! watches are in `super::notify`'s table too, which tells its [`Inbox`]
//! of a change: the inbox is what the watches know of the file, and the
//! file's own memory goes when the last description of it does, taking
//! its watches with it.
//!
//! When a program gets no events it expects, its `watch_add` of the same
//! file again returns the watch's number, and a `read` in non-blocking
//! mode shows whether anything is queued.

use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::file::WATCH_QUEUE_MAX;
use radix_abi::process::PipeFlags;
use radix_cap::{ObjectKind, Rights, SlotRef};
use radix_linux::notify::{
    EVENT_SIZE, IN_ALL_EVENTS, IN_EXCL_UNLINK, IN_IGNORED, IN_MASK_ADD, IN_MASK_CREATE, IN_ONESHOT,
    IN_Q_OVERFLOW,
};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, SpinLock};
use radix_vfs::mount::Location;
use radix_vfs::{Access, In, KArc, Kind, Node, NodeRef, Out, Stat, mem, next_id};

use super::notify::{self, Watch};
use super::{Description, FileObject, pipe};
use crate::sched::{self, wait::WaitCount};
use crate::user::account::Account;
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};
use crate::user::{Fifo, UserRef};

/// The name of an event's entry: shared by the events of every watch
/// file a change tells.
pub type Name = KArc<Vec<u8, Kmalloc>>;

/// The longest a record is: its fixed part, and a name of 255 bytes with
/// its NUL, padded to a multiple of 16.
const RECORD_MAX: usize = EVENT_SIZE + 256;

/// One event a program has yet to read.
struct Event {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: Option<Name>,
}

impl Event {
    /// The event that stands for those the queue had no room for.
    const OVERFLOW: Event = Event { wd: -1, mask: IN_Q_OVERFLOW, cookie: 0, name: None };

    fn name(&self) -> &[u8] {
        self.name.as_deref().map_or(&[], |name| &name[..])
    }

    /// How many bytes its record takes.
    fn record_len(&self) -> usize {
        radix_linux::notify::record_len(self.name().len())
    }

    /// Whether the queue need not hold it after `last`: the same event of
    /// the same watch for the same name, as Linux merges them; a watch
    /// that ended is always told.
    fn repeats(&self, last: &Event) -> bool {
        last.mask & IN_IGNORED == 0
            && last.mask == self.mask
            && last.wd == self.wd
            && last.name() == self.name()
    }
}

/// What a watch file holds unread, behind its lock.
struct Events {
    queue: Fifo<Event>,
    /// How many bytes the records of the queue take.
    bytes: usize,
    /// Whether an `IN_Q_OVERFLOW` event is queued.
    overflow_queued: bool,
    /// Whether events were lost for want of memory to queue an overflow
    /// event: one is owed, and read once the queue is empty.
    overflow_owed: bool,
}

/// What a look at the first record found.
enum Next {
    Event(Event),
    /// The queue is empty.
    Empty,
    /// The first record is longer than the room.
    TooSmall,
}

impl Events {
    /// Queues `event`, unless the queue is full, when one overflow event
    /// stands for it, or it repeats the last.
    fn enqueue(&mut self, event: Event) {
        if self.queue.len() >= WATCH_QUEUE_MAX as usize {
            self.overflow();
            return;
        }
        if self.queue.back().is_some_and(|last| event.repeats(last)) {
            return;
        }
        let size = event.record_len();
        if self.queue.push_back(event).is_ok() {
            self.bytes += size;
        } else {
            self.overflow();
        }
    }

    /// Notes that events were lost: queues an overflow event, once.
    fn overflow(&mut self) {
        if self.overflow_queued || self.overflow_owed {
            return;
        }
        if self.queue.push_back(Event::OVERFLOW).is_ok() {
            self.overflow_queued = true;
            self.bytes += EVENT_SIZE;
        } else {
            self.overflow_owed = true;
        }
    }

    /// Takes the first event if its record fits in `room` bytes.
    fn next(&mut self, room: usize) -> Next {
        let Some(first) = self.queue.front() else {
            if !self.overflow_owed {
                return Next::Empty;
            }
            if EVENT_SIZE > room {
                return Next::TooSmall;
            }
            self.overflow_owed = false;
            return Next::Event(Event::OVERFLOW);
        };
        let size = first.record_len();
        if size > room {
            return Next::TooSmall;
        }
        self.bytes -= size;
        match self.queue.pop_front() {
            Some(event) => {
                self.overflow_queued &= event.mask != IN_Q_OVERFLOW;
                Next::Event(event)
            }
            None => Next::Empty,
        }
    }

    /// How many records a read finds, and how many bytes they take.
    fn waiting(&self) -> (usize, usize) {
        let owed = usize::from(self.overflow_owed);
        (self.queue.len() + owed, self.bytes + owed * EVENT_SIZE)
    }
}

/// What a watch file's watches know of it: the events it holds, and the
/// waits and polls on it. A watch holds it by a reference that does not
/// keep it, so that the file goes with its last description.
pub struct Inbox {
    events: SpinLock<Events>,
    /// How many records are waiting, and the bytes they take, as the
    /// queue says, for the waits, polls and FIONREAD.
    waiting: AtomicUsize,
    bytes: AtomicUsize,
    polled: Polled,
    waits: WaitCount,
}

impl Inbox {
    fn new() -> Inbox {
        let events =
            Events { queue: Fifo::new(), bytes: 0, overflow_queued: false, overflow_owed: false };
        Inbox {
            events: SpinLock::new(events),
            waiting: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            polled: Polled::new(),
            waits: WaitCount::new(),
        }
    }

    fn key(&self) -> usize {
        core::ptr::from_ref::<Inbox>(self) as usize
    }

    /// Queues an event of watch `number`, which the caller then follows
    /// with [`Inbox::changed`], once for all it queued.
    pub fn queue(&self, number: u32, mask: u32, cookie: u32, name: Option<Name>) {
        // A watch's number is at most i32::MAX (`WatchFile::add`).
        let event = Event { wd: number as i32, mask, cookie, name };
        let mut events = self.events.lock();
        events.enqueue(event);
        self.publish(&events);
    }

    /// Lets the waits and polls see what the queue holds.
    fn publish(&self, events: &Events) {
        let (waiting, bytes) = events.waiting();
        self.waiting.store(waiting, Ordering::Release);
        self.bytes.store(bytes, Ordering::Release);
    }

    /// Something was queued: the waits look again, and so do the polls.
    pub fn changed(&self) {
        self.waits.wake_all(self.key());
        // `wake_all` took the full barrier that `Polled::changed` would.
        self.polled.changed_by_rmw();
    }
}

/// The watches a watch file holds, by number.
struct Held {
    /// In order of number, which only grows; the ones the table took out
    /// (`gone`) stay until the next add or remove sweeps them.
    watches: Vec<KArc<Watch>, Kmalloc>,
    /// The number the next watch gets, from 1, as Linux numbers them.
    next: u32,
}

/// A watch file, as a node.
pub struct WatchFile {
    id: u64,
    inbox: KArc<Inbox>,
    held: Mutex<Held>,
}

impl Drop for WatchFile {
    fn drop(&mut self) {
        let mut held = self.held.lock();
        while let Some(watch) = held.watches.pop() {
            // Each watch lets go of its node here, with no table lock.
            drop(notify::remove(&watch));
        }
    }
}

impl Node for WatchFile {
    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::CharDevice,
            size: 0,
            allocated: 0,
            id: self.id,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::CharDevice
    }

    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o600 }
    }

    // The kernel's own reads and writes through the node find nothing:
    // the descriptions' are `read` below.
    fn read(&self, _offset: u64, _out: &mut dyn Out) -> radix_vfs::Result<usize> {
        Ok(0)
    }

    fn write(&self, _offset: u64, data: &dyn In) -> radix_vfs::Result<usize> {
        Ok(data.len())
    }

    fn poll_events(&self) -> Option<u16> {
        Some(self.events())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl WatchFile {
    /// What a poll finds: readable while an event waits.
    pub fn events(&self) -> u16 {
        if self.inbox.waiting.load(Ordering::Acquire) > 0 { poll::READABLE } else { 0 }
    }

    /// What its pollers count themselves in.
    pub fn polled(&self) -> &Polled {
        &self.inbox.polled
    }

    /// How many bytes the records waiting take.
    pub fn pending(&self) -> usize {
        self.inbox.bytes.load(Ordering::Acquire)
    }
}

/// The watch file `node` is, if it is one.
pub fn watch_file_of(node: &NodeRef) -> Option<&WatchFile> {
    radix_vfs::downcast::<WatchFile>(&**node)
}

/// Makes a watch file for `user`'s process in slot `to` with `flags`
/// (radix-process(2)'s `watch_file`), which can be read but not written.
///
/// # Errors
///
/// no_memory, and the slot's status if the file cannot go there.
pub fn make(user: &UserRef, to: u32, flags: PipeFlags) -> Result<(), Status> {
    let no_memory = |_| Kernel::NoMemory.status(0);
    let inbox = mem::arc(Inbox::new()).map_err(no_memory)?;
    let held = Held { watches: Vec::new_in(HEAP.with(Flags::MAY_SLEEP)), next: 1 };
    let node: NodeRef =
        mem::arc(WatchFile { id: next_id(), inbox, held: Mutex::new(held) }).map_err(no_memory)?;
    let mount = pipe::mount().map_err(super::status)?;
    let description = Description {
        non_blocking: flags.contains(PipeFlags::NON_BLOCKING),
        ..Description::default()
    };
    let file = FileObject::with(Location::new(mount, node), description, user.process.identity())
        .ok_or(Kernel::NoMemory.status(0))?;
    crate::user::files::install_file(
        user,
        SlotRef::new(user.process.cspace(), to),
        ObjectKind::File,
        Rights::COPY.union(Rights::TRANSFER).union(Rights::READ),
        file,
        flags.contains(PipeFlags::CLOSE_ON_EXEC),
    )
}

/// Adds a watch of `node`, which the caller has found and checked, to the
/// watch file `file`, for the events and flags of `mask` (Linux's bits,
/// checked), and returns its number: the file's own if `file` watches
/// `node` already, which then has the events `mask` says, or those and the
/// ones it had with `IN_MASK_ADD`.
///
/// # Errors
///
/// invalid_argument if `file` is not a watch file, exists with
/// `IN_MASK_CREATE` for a node it watches, no_room past the account's
/// share of watches or the numbers a file has, and no_memory.
pub fn add(user: &UserRef, file: &FileObject, node: &NodeRef, mask: u32) -> Result<u32, Status> {
    let this = watch_file_of(&file.at.node).ok_or(Kernel::InvalidArgument.status(0))?;
    let inbox = KArc::downgrade(&this.inbox);
    let kept = mask & (IN_ALL_EVENTS | IN_ONESHOT | IN_EXCL_UNLINK);
    // Taken before the table's lock, which no other lock is taken under.
    let mut held = this.held.lock();
    held.watches.retain(|watch| !watch.is_gone());
    if let Some(watch) = notify::find(node, &inbox) {
        if mask & IN_MASK_CREATE != 0 {
            return Err(FileError::Exists.status(0));
        }
        watch.set_mask(if mask & IN_MASK_ADD != 0 { watch.mask() | kept } else { kept });
        return Ok(watch.number);
    }
    let number = held.next;
    let after = number.checked_add(1).filter(|after| i32::try_from(*after).is_ok());
    let after = after.ok_or(Kernel::NoRoom.status(0))?;
    let account = user.process.account();
    let slot = Account::take_watch(account)?;
    let charge = Account::charge_from(account, &user.stock, notify::Entry::BYTES)?;
    held.watches.try_reserve(1).map_err(|_| Kernel::NoMemory.status(0))?;
    let watch = notify::add(inbox, number, kept, node, charge, slot)?;
    // Room was reserved above.
    let _ = held.watches.push_within_capacity(watch);
    held.next = after;
    Ok(number)
}

/// Takes watch `number` out of the watch file `file`, which then reads an
/// `IN_IGNORED` event for it.
///
/// # Errors
///
/// invalid_argument if `file` is not a watch file or holds no such watch.
pub fn remove(file: &FileObject, number: u32) -> Result<(), Status> {
    let invalid = Kernel::InvalidArgument.status(0);
    let this = watch_file_of(&file.at.node).ok_or(invalid)?;
    let watch = {
        let mut held = this.held.lock();
        held.watches.retain(|watch| !watch.is_gone());
        let at = held.watches.binary_search_by_key(&number, |watch| watch.number);
        held.watches.remove(at.map_err(|_| invalid)?)
    };
    // The table ended it already if an event did, which told the file.
    let entry = notify::remove(&watch).ok_or(invalid)?;
    // Let go of the node with no lock held: it may be the last reference.
    drop(entry);
    this.inbox.queue(number, IN_IGNORED, 0, None);
    this.inbox.changed();
    Ok(())
}

/// The program reads the records of the events of `file`, a watch file,
/// into `out`: as many whole ones as fit, waiting for the first unless
/// `non_blocking`; the bytes they take. A record that cannot be copied is
/// lost, as on Linux.
///
/// # Errors
///
/// invalid_argument if the first record does not fit, would_block if
/// there is none and the read does not wait, and interrupted if a signal
/// ends the wait.
pub fn read(file: &FileObject, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
    let this = watch_file_of(&file.at.node).ok_or(Kernel::InvalidArgument.status(0))?;
    let inbox = &*this.inbox;
    let mut done = 0;
    loop {
        let next = {
            let mut events = inbox.events.lock();
            let next = events.next(out.len() - done);
            inbox.publish(&events);
            next
        };
        match next {
            Next::Event(event) => {
                let mut record = [0u8; RECORD_MAX];
                let size = radix_linux::notify::write_event(
                    event.wd,
                    event.mask,
                    event.cookie,
                    event.name(),
                    &mut record,
                );
                match out.put(done, &record[..size]) {
                    Ok(()) => done += size,
                    Err(_) if done > 0 => return Ok(done),
                    Err(err) => return Err(super::status(err)),
                }
            }
            Next::TooSmall if done > 0 => return Ok(done),
            Next::TooSmall => return Err(Kernel::InvalidArgument.status(0)),
            Next::Empty if done > 0 => return Ok(done),
            Next::Empty if non_blocking => return Err(Kernel::WouldBlock.status(0)),
            Next::Empty => {
                inbox.waits.wait_until(inbox.key(), "watch read", || {
                    inbox.waiting.load(Ordering::Acquire) > 0 || sched::interrupted()
                });
                if inbox.waiting.load(Ordering::Acquire) == 0 && sched::interrupted() {
                    return Err(interrupted(Restart::Always));
                }
            }
        }
    }
}
