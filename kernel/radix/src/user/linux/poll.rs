// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's calls that wait for descriptors (ADR 0610): `poll`, `ppoll`,
//! `select` and `pselect6`, each ending in the kernel's `poll` of the
//! process (`crate::user::posix::poll`), as Dukovo's C library makes the
//! same calls through the process's native `poll` method
//! (`ports/mlibc/sysdeps/radix/generic/poll.cpp`).
//!
//! The native entries are Linux's `struct pollfd` records, with the slot of
//! descriptor n at `base + n`. A `poll` reads the program's array once into
//! the kernel's memory, with each entry asking for the events the kernel
//! knows (`POLLRDNORM` and the other band events are the plain ones), polls
//! those, and writes the events found back as the program asked for them,
//! only where they differ from what its array held. A `select` makes the
//! same entries from its sets, with none for a descriptor that is in no
//! set, so a `select` of no descriptors is a sleep. No call maps memory in
//! the program's address space.
//!
//! A signal mask (`ppoll`, `pselect6`) is the native `mask` flag's: the
//! kernel's `poll` puts it in place when it goes on to wait, and the old one
//! back when it returns, or once the handler a signal ran has returned. A call
//! that finds a descriptor ready at once never applies it; a signal that
//! comes between the call's entry and the wait is a signal the old mask
//! lets through, where Linux's mask is in place from the entry.
//!
//! The waits that Linux gives back the time they have left (`select`,
//! `pselect6`, `ppoll`) write it before returning, interrupted or not, so
//! that the call a signal makes run again goes on with what is left.

use alloc::vec::Vec;

use radix_abi::process::{self as abi, PollFlags};
use radix_abi::startup::SLOT_FD_BASE;
use radix_linux::errno::{EBADF, EINVAL, ENOMEM};
use radix_linux::poll::{
    self as lx, FD_SETSIZE, POLLFD_REVENTS, POLLFD_SIZE, POLLNVAL, PollFd, in_set,
};
use radix_linux::records::{self, TIMESPEC_SIZE, Time};
use radix_linux::signals::SIGSET_SIZE;
use radix_mm::Kmalloc;

use super::files::descriptor_limit;
use super::sockets::{read_bytes, zeroed};
use super::{Args, Error, Result, copy_in, copy_out, fail, kmalloc};
use crate::sched;
use crate::user::UserRef;
use crate::user::posix::poll::{ENTRIES_MAX, ON_STACK, poll_entries};

/// How long a call waits and under what signal mask.
struct Wait {
    /// How many nanoseconds, or for ever.
    ns: Option<u64>,
    /// The signal mask while it waits, if the program gave one.
    mask: Option<u64>,
}

/// `len` found events, all zero, in the kernel's memory, or ENOMEM.
fn no_events(len: usize) -> Result<Vec<u16, Kmalloc>> {
    radix_mm::bytes::filled(0u16, len, kmalloc()).map_err(|_| Error::Errno(ENOMEM))
}

/// The kernel's `poll` of `entries`, a `struct pollfd` for each descriptor
/// to look at, for descriptors from 0, in the kernel's memory: how many are
/// ready, with what each is ready for in `found`, which is zero for each to
/// begin with.
fn native_poll(user: &UserRef, entries: &[u8], found: &mut [u16], wait: &Wait) -> Result<u32> {
    let mut flags = PollFlags::NONE;
    if wait.ns.is_none() {
        flags = flags.union(PollFlags::FOREVER);
    }
    if wait.mask.is_some() {
        flags = flags.union(PollFlags::MASK);
    }
    // The entries are given as bytes, so the call reads no address for them.
    // There are at most ENTRIES_MAX, which the callers check.
    let args = abi::PollArgs {
        entries: 0,
        count: found.len() as u32,
        base: SLOT_FD_BASE,
        timeout_ns: wait.ns.unwrap_or(0),
        mask: wait.mask.unwrap_or(0),
        flags,
    };
    Ok(poll_entries(user, &args, entries, found)?)
}

/// Polls the `nfds` entries at `fds`, on a copy that asks for the events
/// the kernel knows, with the events found put back as the program asked
/// for them where they differ from what its array held. The count of
/// entries with events. EINVAL for more entries than the process may have
/// descriptors, as on Linux, or than the kernel takes. A few entries, as
/// most calls have, stay on the kernel stack, as the native `poll`'s do.
fn poll_array(user: &UserRef, fds: u64, nfds: u64, wait: &Wait) -> Result<u64> {
    if nfds > u64::from(descriptor_limit(user)) || nfds > u64::from(ENTRIES_MAX) {
        return fail(EINVAL);
    }
    let count = nfds as usize;
    let len = count * POLLFD_SIZE;
    let mut asked_here = [0u8; ON_STACK * POLLFD_SIZE];
    let mut native_here = [0u8; ON_STACK * POLLFD_SIZE];
    let mut found_here = [0u16; ON_STACK];
    let mut asked_heap: Vec<u8, Kmalloc>;
    let mut native_heap: Vec<u8, Kmalloc>;
    let mut found_heap: Vec<u16, Kmalloc>;
    let (asked, native, found): (&mut [u8], &mut [u8], &mut [u16]) = if count <= ON_STACK {
        (&mut asked_here[..len], &mut native_here[..len], &mut found_here[..count])
    } else {
        asked_heap = zeroed(len)?;
        native_heap = zeroed(len)?;
        found_heap = no_events(count)?;
        (&mut asked_heap, &mut native_heap, &mut found_heap)
    };
    copy_in(fds, asked)?;
    let entries = asked.as_chunks::<POLLFD_SIZE>().0;
    for (out, entry) in native.as_chunks_mut::<POLLFD_SIZE>().0.iter_mut().zip(entries) {
        let entry = PollFd::read(entry);
        PollFd { fd: entry.fd, events: lx::native_events(entry.events), revents: 0 }.put(out);
    }
    let ready = native_poll(user, native, found, wait)?;
    // An array polled again mostly holds what it found last time, so only
    // the events that differ are written, and nothing at all, not even to a
    // page shared until written, when none do.
    for (i, (entry, found)) in entries.iter().zip(found.iter()).enumerate() {
        let entry = PollFd::read(entry);
        let revents = lx::found_events(entry.events, *found);
        if revents != entry.revents {
            copy_out(fds + (i * POLLFD_SIZE + POLLFD_REVENTS) as u64, &revents.to_le_bytes())?;
        }
    }
    Ok(u64::from(ready))
}

/// The wait of the `timespec`, or with `micros` `timeval`, at `address`:
/// for ever if there is none, or it is longer than a wait goes on.
pub(super) fn read_wait(address: u64, micros: bool) -> Result<Option<u64>> {
    if address == 0 {
        return Ok(None);
    }
    let mut bytes = [0u8; TIMESPEC_SIZE];
    copy_in(address, &mut bytes)?;
    let time = if micros { records::timeval(&bytes) } else { records::timespec(&bytes) };
    Ok(lx::wait_for(Some(time.map_err(Error::Errno)?)))
}

/// The signal mask at `address`, if the program gave one: its size,
/// `size`, must be the kernel's, as `ppoll` and `pselect6` check it only
/// when there is a mask.
pub(super) fn read_mask(address: u64, size: u64) -> Result<Option<u64>> {
    if address == 0 {
        return Ok(None);
    }
    if size != SIGSET_SIZE {
        return fail(EINVAL);
    }
    let mut bytes = [0u8; 8];
    copy_in(address, &mut bytes)?;
    Ok(Some(u64::from_le_bytes(bytes)))
}

/// Writes the time a wait that began now and was to last `ns` has left in
/// the `timespec`, or with `micros` `timeval`, at `address`, as Linux
/// does, and gives `done`: the call's own answer, unless it succeeded and
/// the time cannot be written. A wait for ever has none left to write.
fn put_left(address: u64, micros: bool, end: Option<u64>, done: Result<u64>) -> Result<u64> {
    let Some(end) = end.filter(|_| address != 0) else { return done };
    let left = Time::from_ns(end.saturating_sub(sched::now_ns()));
    let mut bytes = [0u8; TIMESPEC_SIZE];
    if micros {
        records::put_timeval(left, &mut bytes);
    } else {
        records::put_timespec(left, &mut bytes);
    }
    let written = copy_out(address, &bytes);
    done.and_then(|count| written.map(|()| count))
}

/// Where a wait of `ns` that begins now ends on the kernel's clock.
fn end_of(ns: Option<u64>) -> Option<u64> {
    ns.map(|ns| sched::now_ns().saturating_add(ns))
}

/// `poll(fds, nfds, timeout)`: the native `poll`, with a timeout in
/// milliseconds, for ever if negative. A call a signal interrupts that is
/// to run again waits the whole timeout again, where Linux's waits what
/// was left (docs/deferred.md).
pub fn poll(user: &UserRef, fds: u64, nfds: u64, timeout: u64) -> Result<u64> {
    let wait = Wait { ns: lx::poll_wait(timeout as i32), mask: None };
    poll_array(user, fds, u64::from(nfds as u32), &wait)
}

/// `ppoll(fds, nfds, tmo_p, sigmask, sigsetsize)`: the native `poll` with
/// the mask and a `timespec` timeout, which it updates with the time left.
pub fn ppoll(user: &UserRef, a: &Args) -> Result<u64> {
    let wait = Wait { ns: read_wait(a[2], false)?, mask: read_mask(a[3], a[4])? };
    let end = end_of(wait.ns);
    let done = poll_array(user, a[0], u64::from(a[1] as u32), &wait);
    put_left(a[2], false, end, done)
}

/// The sets of `select` as the program gave them: `nfds` descriptors of
/// each of `read`, `write` and `except` that it passed, in the kernel's
/// memory, the others empty.
struct Sets {
    nfds: usize,
    /// The addresses in the program's memory, 0 for a set it left out.
    addresses: [u64; 3],
    /// What each held.
    bits: [Vec<u8, Kmalloc>; 3],
}

impl Sets {
    /// Reads the sets at `addresses` for `nfds` descriptors: EINVAL for a
    /// negative number, or one above `FD_SETSIZE`, where Linux cuts it to
    /// its table's size (docs/deferred.md).
    fn read(nfds: u64, addresses: [u64; 3]) -> Result<Sets> {
        let Ok(nfds) = usize::try_from(nfds as i32) else { return fail(EINVAL) };
        if nfds > FD_SETSIZE {
            return fail(EINVAL);
        }
        let len = lx::set_bytes(nfds);
        let read = |address: u64| if address == 0 { zeroed(0) } else { read_bytes(address, len) };
        let bits = [read(addresses[0])?, read(addresses[1])?, read(addresses[2])?];
        Ok(Sets { nfds, addresses, bits })
    }

    /// The events `select` asks for descriptor `fd`.
    fn events(&self, fd: usize) -> u16 {
        lx::select_events(
            in_set(&self.bits[0], fd),
            in_set(&self.bits[1], fd),
            in_set(&self.bits[2], fd),
        )
    }

    /// Writes the sets of what was found: each set the program passed,
    /// with only the descriptors that are ready for what they were in it
    /// for, and returns how many bits are set in all.
    fn put(&mut self, found: &[PollFd]) -> Result<u64> {
        for bits in &mut self.bits {
            bits.fill(0);
        }
        let mut total = 0;
        for entry in found {
            for (bits, ready) in
                self.bits.iter_mut().zip(lx::select_ready(entry.events, entry.revents))
            {
                if ready {
                    lx::add_to_set(bits, entry.fd as usize);
                    total += 1;
                }
            }
        }
        for (address, bits) in self.addresses.iter().zip(&self.bits) {
            if *address != 0 {
                copy_out(*address, bits)?;
            }
        }
        Ok(total)
    }
}

/// The `select` of the sets at `addresses`, `nfds` descriptors of them,
/// waiting as `wait` says: the kernel's `poll` of one entry for each
/// descriptor that is in a set, as the C library makes it. EBADF for a
/// descriptor that is not open, with the sets left as they were; else the
/// sets hold what is ready, and the call returns how many bits that is.
fn select_sets(user: &UserRef, nfds: u64, addresses: [u64; 3], wait: &Wait) -> Result<u64> {
    let mut sets = Sets::read(nfds, addresses)?;
    let mut asked: Vec<PollFd, Kmalloc> = Vec::new_in(kmalloc());
    for fd in 0..sets.nfds {
        let events = sets.events(fd);
        if events != 0 {
            asked.try_reserve(1).map_err(|_| Error::Errno(ENOMEM))?;
            // Room was reserved.
            let _ = asked.push_within_capacity(PollFd { fd: fd as i32, events, revents: 0 });
        }
    }
    let mut entries = zeroed(asked.len() * POLLFD_SIZE)?;
    for (out, entry) in entries.as_chunks_mut::<POLLFD_SIZE>().0.iter_mut().zip(&asked) {
        entry.put(out);
    }
    let mut found = no_events(asked.len())?;
    native_poll(user, &entries, &mut found, wait)?;
    for (entry, found) in asked.iter_mut().zip(&found) {
        entry.revents = *found;
    }
    if asked.iter().any(|entry| entry.revents & POLLNVAL != 0) {
        return fail(EBADF);
    }
    sets.put(&asked)
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)`: the native
/// `poll`, with a `timeval` timeout it updates with the time left.
pub fn select(user: &UserRef, a: &Args) -> Result<u64> {
    let wait = Wait { ns: read_wait(a[4], true)?, mask: None };
    let end = end_of(wait.ns);
    let done = select_sets(user, a[0], [a[1], a[2], a[3]], &wait);
    put_left(a[4], true, end, done)
}

/// `pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask)`: the
/// native `poll` with the mask and a `timespec` timeout it updates with
/// the time left. The sixth argument points at the mask's address and
/// size, as Linux's does.
pub fn pselect6(user: &UserRef, a: &Args) -> Result<u64> {
    let wait_ns = read_wait(a[4], false)?;
    let mask = if a[5] == 0 {
        None
    } else {
        let (mut address, mut size) = ([0u8; 8], [0u8; 8]);
        copy_in(a[5], &mut address)?;
        copy_in(a[5].saturating_add(8), &mut size)?;
        read_mask(u64::from_le_bytes(address), u64::from_le_bytes(size))?
    };
    let wait = Wait { ns: wait_ns, mask };
    let end = end_of(wait.ns);
    let done = select_sets(user, a[0], [a[1], a[2], a[3]], &wait);
    put_left(a[4], false, end, done)
}
