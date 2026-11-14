// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/devices/crashes` (ADR 0530, radix-crashes(4)): the records of the
//! programs that crashed, which reportd(8) reads and turns into stack
//! traces.
//!
//! The records wait in a queue of at most RECORDS_KEPT, oldest first, made
//! by [`crate::user::crash`] in the thread that crashed. A read takes the
//! oldest whole, so that each is read once, by whichever reader comes
//! first; a crash that finds the queue full is counted, and the next record
//! read says how many were not kept. Nothing else holds a record: once
//! read, it is gone from the kernel.

use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::vec::Vec;

use radix_abi::Status;
use radix_abi::crash::{Header, RECORDS_KEPT};
use radix_abi::errors::Kernel;
use radix_mm::Kmalloc;
use radix_sync::SpinLock;
use radix_vfs::{Access, In, Kind, Node, NodeRef, Out, Stat};

use super::FileObject;
use crate::sched::{self, wait};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

const KEPT: usize = RECORDS_KEPT as usize;

/// A record, as [`crate::user::crash`] made it.
pub type Record = Vec<u8, Kmalloc>;

/// The records waiting, oldest first, and how many crashes found no room.
struct Queue {
    records: [Option<Record>; KEPT],
    /// Where the oldest is, and how many there are.
    first: usize,
    len: usize,
    dropped: u32,
}

static QUEUE: SpinLock<Queue> =
    SpinLock::new(Queue { records: [const { None }; KEPT], first: 0, len: 0, dropped: 0 });
pub static POLLED: Polled = Polled::new();

/// How many records wait, as [`Queue::len`], set with the queue's lock held.
/// A wait's condition runs under the wait table's lock, which interrupt
/// handlers take, so it reads this and never the queue's lock, which is
/// taken with interrupts on.
static WAITING: AtomicUsize = AtomicUsize::new(0);

/// The node, numbered as its id.
pub struct Crashes(pub u64);

impl Node for Crashes {
    // Root's alone, and only to read: a record holds part of a program's
    // memory.
    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o400 }
    }

    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::CharDevice,
            size: 0,
            allocated: 0,
            id: self.0,
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

    // Reads come through [`read`], with the description.
    fn read(&self, _offset: u64, _out: &mut dyn Out) -> radix_vfs::Result<usize> {
        Ok(0)
    }

    fn write(&self, _offset: u64, _data: &dyn In) -> radix_vfs::Result<usize> {
        Err(radix_vfs::Error::NotSupported)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Whether `node` is `/devices/crashes`.
pub fn is_crashes(node: &NodeRef) -> bool {
    radix_vfs::downcast::<Crashes>(&**node).is_some()
}

/// What readers wait on.
fn key() -> usize {
    core::ptr::addr_of!(QUEUE).addr()
}

/// Queues `record` for reportd, unless RECORDS_KEPT wait already, when the
/// crash is counted instead; the record is freed then, with no lock held.
pub fn keep(record: Record) {
    let refused = {
        let mut queue = QUEUE.lock();
        if queue.len == KEPT {
            queue.dropped = queue.dropped.saturating_add(1);
            Some(record)
        } else {
            let at = (queue.first + queue.len) % KEPT;
            queue.records[at] = Some(record);
            queue.len += 1;
            WAITING.store(queue.len, Ordering::Release);
            None
        }
    };
    if refused.is_some() {
        return;
    }
    wait::wake_all(key());
    POLLED.changed();
}

/// The count of crashes not kept that `record` carries.
fn dropped_in(record: &Record) -> u32 {
    record
        .get(Header::OFFSET_DROPPED..Header::OFFSET_DROPPED + 4)
        .and_then(|field| field.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

/// Takes the oldest record, with how many crashes were not kept before it
/// added to the count it carries, which is 0 unless a read put it back.
fn take() -> Option<Record> {
    let mut queue = QUEUE.lock();
    if queue.len == 0 {
        return None;
    }
    let first = queue.first;
    let mut record = queue.records[first].take()?;
    queue.first = (first + 1) % KEPT;
    queue.len -= 1;
    WAITING.store(queue.len, Ordering::Release);
    let dropped = dropped_in(&record).saturating_add(core::mem::take(&mut queue.dropped));
    if let Some(field) = record.get_mut(Header::OFFSET_DROPPED..Header::OFFSET_DROPPED + 4) {
        field.copy_from_slice(&dropped.to_le_bytes());
    }
    Some(record)
}

/// Puts back `record`, which a read could not hand over, as the oldest,
/// with the count it carries, and wakes the readers that wait.
fn put_back(record: Record) {
    let refused = {
        let mut queue = QUEUE.lock();
        if queue.len == KEPT {
            // A crash took its place meanwhile: the newer is the one kept,
            // and the queue counts this one and those it counted.
            let dropped = dropped_in(&record).saturating_add(1);
            queue.dropped = queue.dropped.saturating_add(dropped);
            Some(record)
        } else {
            queue.first = (queue.first + KEPT - 1) % KEPT;
            let first = queue.first;
            queue.records[first] = Some(record);
            queue.len += 1;
            WAITING.store(queue.len, Ordering::Release);
            None
        }
    };
    if refused.is_none() {
        wait::wake_all(key());
        POLLED.changed();
    }
}

/// Reads the oldest record into `out`, waiting for one unless
/// `non_blocking`. Fails with invalid_argument, keeping the record, if
/// `out` cannot hold it.
pub fn read(_file: &FileObject, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
    loop {
        if let Some(record) = take() {
            if out.len() < record.len() {
                put_back(record);
                return Err(Kernel::InvalidArgument.status(0));
            }
            // Copied with the queue let go of: the copy may wait for a page.
            return match out.put(0, &record) {
                Ok(()) => Ok(record.len()),
                Err(error) => {
                    put_back(record);
                    Err(super::status(error))
                }
            };
        }
        if non_blocking {
            return Err(Kernel::WouldBlock.status(0));
        }
        wait::wait_until(key(), "crash records", || waiting() || sched::interrupted());
        if sched::interrupted() && !waiting() {
            return Err(interrupted(Restart::Always));
        }
    }
}

/// Whether a record waits.
fn waiting() -> bool {
    WAITING.load(Ordering::Acquire) > 0
}

/// What `poll` finds on `file`: readable while a record waits.
pub fn events(_file: &FileObject) -> u16 {
    if waiting() { poll::READABLE } else { 0 }
}
