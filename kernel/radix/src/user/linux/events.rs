// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! epoll (ADR 0612): Linux's calls on the event queue's `poll` filter.
//!
//! An epoll instance is an event queue, made by the process's native
//! `event_queue` method; `epoll_ctl` makes one change of the queue's `poll`
//! watch of a descriptor (`radix_linux::epoll::change`), and the
//! `epoll_wait` calls wait on it as the queue's `wait` with no changes
//! does, writing each event as a `struct epoll_event` straight into the
//! program's array. The queue's work is done by `queue::change_given` and
//! `queue::wait_given`, with the change in the kernel's memory and the
//! same checks of the capability as its `wait` method makes.
//!
//! What Linux checks first comes first: the event's record, then both
//! descriptors (EBADF), then whether the first is an epoll instance and
//! not the second (EINVAL); the queue says the rest (ENOENT, EEXIST,
//! EPERM for a file or directory, which is always ready). An epoll
//! instance cannot watch another, which Linux allows: EINVAL, as the queue
//! refuses to watch a queue (docs/deferred.md).

use radix_abi::errors::Kernel;
use radix_abi::event_queue::QueueEvent;
use radix_abi::process;
use radix_abi::startup::{SLOT_FD_BASE, SLOT_PROCESS};
use radix_cap::ObjectKind;
use radix_hal::UserMode;
use radix_linux::epoll::{self, EPOLL_CTL_DEL, EPOLL_EVENT_SIZE, EpollEvent, MAX_EVENTS};
use radix_linux::errno::{EFAULT, EINVAL};
use radix_linux::nr;

use super::files::{descriptor, into_free, kind_of, on_object};
use super::poll::{read_mask, read_wait};
use super::{Args, Error, Result, copy_in, fail, method, paths};
use crate::Arch;
use crate::sched::ThreadRef;
use crate::user::{UserRef, error, queue};

/// `epoll_create1`: a new queue in the lowest free descriptor, closed by
/// `exec` with EPOLL_CLOEXEC; EINVAL for any other flag.
pub fn epoll_create1(me: &ThreadRef, user: &UserRef, how: u64) -> Result<u64> {
    let flags = epoll::create_flags(how as u32).map_err(Error::Errno)?;
    let slot = into_free(user, 0, |slot| {
        let args = process::EventQueueArgs { to: slot, flags };
        method(me, user, SLOT_PROCESS, process::EVENT_QUEUE, args.encode()).map(|_| slot)
    })?;
    Ok(descriptor(slot))
}

/// `epoll_create`: `epoll_create1` with no flags, once `size`, a hint
/// Linux no longer uses, is found positive (EINVAL otherwise).
pub fn epoll_create(me: &ThreadRef, user: &UserRef, size: u64) -> Result<u64> {
    if size as i32 <= 0 {
        return fail(EINVAL);
    }
    epoll_create1(me, user, 0)
}

/// `epoll_ctl(epfd, op, fd, event)`: the change `op` asks for, of the
/// queue's `poll` watch of `fd`.
pub fn epoll_ctl(user: &UserRef, a: &Args) -> Result<u64> {
    let (epfd, op, fd, address) = (a[0], a[1] as u32, a[2], a[3]);
    // Linux reads the record for every operation but EPOLL_CTL_DEL, an
    // unknown one too.
    let mut event = EpollEvent::default();
    if op != EPOLL_CTL_DEL {
        let mut bytes = [0u8; EPOLL_EVENT_SIZE];
        copy_in(address, &mut bytes)?;
        event = EpollEvent::read(&bytes);
    }
    let queue_slot = paths::slot_of(epfd)?;
    let target = paths::slot_of(fd)?;
    let kind = kind_of(user, queue_slot)?;
    kind_of(user, target)?;
    if queue_slot == target || kind != ObjectKind::EventQueue {
        return fail(EINVAL);
    }
    // The descriptor was found open, so it is a slot's number.
    let change = epoll::change(op, target - SLOT_FD_BASE, event).map_err(Error::Errno)?;
    on_object(user, queue_slot, |found| queue::change_given(user, found, &change, SLOT_FD_BASE))?;
    Ok(0)
}

/// `epoll_wait(epfd, events, maxevents, timeout)` and, by `number`,
/// `epoll_pwait`, which adds a signal mask, and `epoll_pwait2`, which
/// takes a `timespec` for the timeout in milliseconds. A negative
/// timeout, or no `timespec`, waits as long as it takes. A signal ends
/// the wait with EINTR, never a restart, as on Linux (signal(7)).
pub fn epoll_wait(user: &UserRef, a: &Args, number: u64) -> Result<u64> {
    let (epfd, events, maxevents) = (a[0], a[1], a[2] as i32);
    let (timeout, mask) = match number {
        nr::EPOLL_WAIT => (milliseconds(a[3]), None),
        nr::EPOLL_PWAIT => (milliseconds(a[3]), read_mask(a[4], a[5])?),
        _ => (read_wait(a[3], false)?, read_mask(a[4], a[5])?),
    };
    let capacity = match u32::try_from(maxevents) {
        Ok(capacity) if capacity > 0 && capacity <= MAX_EVENTS => capacity,
        _ => return fail(EINVAL),
    };
    if !radix_ipc::range::user_bytes(events, u64::from(capacity) * EPOLL_EVENT_SIZE as u64) {
        return fail(EFAULT);
    }
    let slot = paths::slot_of(epfd)?;
    if kind_of(user, slot)? != ObjectKind::EventQueue {
        return fail(EINVAL);
    }
    let mut sink = |index: u32, event: &QueueEvent| {
        let at = events + u64::from(index) * EPOLL_EVENT_SIZE as u64;
        if Arch::copy_to_user(at, &EpollEvent::of(event).bytes()) {
            Ok(())
        } else {
            Err(error(Kernel::BadAddress))
        }
    };
    let found = on_object(user, slot, |found| {
        queue::wait_given(user, found, &mut sink, capacity, timeout, mask)
    })?;
    Ok(u64::from(found))
}

/// A timeout of `epoll_wait` and `epoll_pwait`, an `int` of milliseconds,
/// in nanoseconds; `None` for a negative one, which waits as long as it
/// takes.
fn milliseconds(timeout: u64) -> Option<u64> {
    u64::try_from(timeout as i32).ok().map(|ms| ms * 1_000_000)
}
