// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Event counters (ADR 0613, radix-process(2)'s `event_counter`): a
//! count that 8-byte writes add to and 8-byte reads take, as Linux's
//! eventfd(2).
//!
//! A read takes the whole count, or with `semaphore` one from it, and
//! waits while the count is 0 unless the description is non-blocking. A
//! write adds a `u64` and waits while that would take the count past
//! `u64::MAX - 1`. A poll finds it readable while the count is not 0,
//! writable while it is below `u64::MAX - 1`, and in error at `u64::MAX`,
//! which no write reaches.
//!
//! The count is one atomic, changed by compare-and-exchange, so that a
//! look at it, a poll's or a wait's condition, takes no lock and nothing
//! is ever copied to or from a program's memory with one held. A read
//! whose buffer cannot be written puts what it took back. Waits are the
//! pipe's (`pipe`): a [`WaitCount`] on the counter's address, woken by
//! every change that someone waits for, and each change calls
//! [`Polled::changed`]. When it misbehaves, the boot test `posix` runs
//! `kernelfiles.c`, which covers each rule above.

use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::process::{CounterFlags, PipeFlags};
use radix_vfs::{Access, In, Node, NodeRef, Out, Stat, mem, next_id};

use super::own_file;
use crate::sched::{self, wait::WaitCount};
use crate::user::UserRef;
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// The most a count holds: one less than `u64::MAX`, which a read could
/// not tell from a full one.
const MAX: u64 = u64::MAX - 1;

/// An event counter's node.
pub struct Counter {
    id: u64,
    /// The count.
    value: AtomicU64,
    /// Whether a read takes 1 instead of the whole count.
    semaphore: bool,
    /// The threads waiting to read or to write.
    waits: WaitCount,
    /// Its pollers.
    polled: Polled,
}

impl Node for Counter {
    fn stat(&self) -> Stat {
        own_file::stat(self.id)
    }

    fn access(&self) -> Access {
        own_file::access()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The counter `node` is, if it is one.
pub fn counter_of(node: &NodeRef) -> Option<&Counter> {
    radix_vfs::downcast::<Counter>(&**node)
}

/// Makes an event counter holding `initial` for `user`'s process, in slot
/// `to` with `flags` (radix-process(2)'s `event_counter`).
///
/// # Errors
///
/// no_memory, and the slot's status if the counter cannot go there.
pub fn make(user: &UserRef, to: u32, initial: u32, flags: CounterFlags) -> Result<(), Status> {
    let counter = Counter {
        id: next_id(),
        value: AtomicU64::new(u64::from(initial)),
        semaphore: flags.contains(CounterFlags::SEMAPHORE),
        waits: WaitCount::new(),
        polled: Polled::new(),
    };
    let node: NodeRef = mem::arc(counter).map_err(|_| Kernel::NoMemory.status(0))?;
    let mut how = PipeFlags::NONE;
    if flags.contains(CounterFlags::NON_BLOCKING) {
        how = how.union(PipeFlags::NON_BLOCKING);
    }
    if flags.contains(CounterFlags::CLOSE_ON_EXEC) {
        how = how.union(PipeFlags::CLOSE_ON_EXEC);
    }
    own_file::install(user, node, to, how)
}

impl Counter {
    fn key(&self) -> usize {
        core::ptr::from_ref::<Counter>(self) as usize
    }

    /// What its pollers count themselves in.
    pub fn polled(&self) -> &Polled {
        &self.polled
    }

    /// The count changed: the waits look again, and so do its pollers.
    fn changed(&self) {
        self.waits.wake_all(self.key());
        // `wake_all` took the full barrier that `Polled::changed` would.
        self.polled.changed_by_rmw();
    }

    /// What `poll` finds: readable while the count is not 0, writable
    /// while a write of 1 would fit, and in error at `u64::MAX`.
    pub fn events(&self) -> u16 {
        let count = self.value.load(Ordering::Acquire);
        let mut events = 0;
        if count > 0 {
            events |= poll::READABLE;
        }
        if count < MAX {
            events |= poll::WRITABLE;
        }
        if count == u64::MAX {
            events |= poll::ERROR;
        }
        events
    }

    /// Takes what a read takes, if the count is not 0: the whole count,
    /// or with `semaphore` 1.
    fn take(&self) -> Option<u64> {
        let semaphore = self.semaphore;
        let step = |count: u64| match count {
            0 => None,
            _ if semaphore => Some(count - 1),
            _ => Some(0),
        };
        let before = self.value.try_update(Ordering::AcqRel, Ordering::Acquire, step).ok()?;
        Some(if semaphore { 1 } else { before })
    }

    /// Puts back `taken`, from a read that could not give it to the
    /// program, as far as it still fits, and has the waits and pollers
    /// look again: a reader that came while the count was taken waits for
    /// it.
    fn give_back(&self, taken: u64) {
        let _ = self.value.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            Some(count.saturating_add(taken).min(MAX))
        });
        self.changed();
    }

    /// Adds `amount`, if the count stays at or below `u64::MAX - 1`.
    fn add(&self, amount: u64) -> bool {
        let step = |count: u64| count.checked_add(amount).filter(|&sum| sum <= MAX);
        self.value.try_update(Ordering::AcqRel, Ordering::Acquire, step).is_ok()
    }

    /// Reads the count into `out`, as 8 bytes, waiting while it is 0
    /// unless `non_blocking`.
    ///
    /// # Errors
    ///
    /// invalid_argument for an `out` of fewer than 8 bytes, would_block
    /// for a count of 0 when `non_blocking`, and interrupted (restarting)
    /// for a wait a signal cut short.
    pub fn read(&self, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
        if out.len() < 8 {
            return Err(Kernel::InvalidArgument.status(0));
        }
        loop {
            if let Some(taken) = self.take() {
                if let Err(err) = out.put(0, &taken.to_le_bytes()) {
                    self.give_back(taken);
                    return Err(super::status(err));
                }
                self.changed();
                return Ok(8);
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            self.waits.wait_until(self.key(), "event counter read", || {
                self.value.load(Ordering::Acquire) > 0 || sched::interrupted()
            });
            if sched::interrupted() && self.value.load(Ordering::Acquire) == 0 {
                return Err(interrupted(Restart::Always));
            }
        }
    }

    /// Adds the 8-byte number in `data` to the count, waiting while the
    /// sum would pass `u64::MAX - 1` unless `non_blocking`.
    ///
    /// # Errors
    ///
    /// invalid_argument for a `data` of fewer than 8 bytes or the number
    /// `u64::MAX`, would_block for a sum that does not fit when
    /// `non_blocking`, and interrupted (restarting) for a wait a signal
    /// cut short.
    pub fn write(&self, data: &dyn In, non_blocking: bool) -> Result<usize, Status> {
        if data.len() < 8 {
            return Err(Kernel::InvalidArgument.status(0));
        }
        let mut bytes = [0u8; 8];
        data.get(0, &mut bytes).map_err(super::status)?;
        let amount = u64::from_le_bytes(bytes);
        if amount == u64::MAX {
            return Err(Kernel::InvalidArgument.status(0));
        }
        loop {
            if self.add(amount) {
                self.changed();
                return Ok(8);
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            // `amount` is at most MAX, so this does not wrap.
            let room = MAX - amount;
            self.waits.wait_until(self.key(), "event counter write", || {
                self.value.load(Ordering::Acquire) <= room || sched::interrupted()
            });
            if sched::interrupted() && self.value.load(Ordering::Acquire) > room {
                return Err(interrupted(Restart::Always));
            }
        }
    }
}
