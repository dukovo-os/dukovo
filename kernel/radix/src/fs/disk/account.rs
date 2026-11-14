// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What each process and resource domain reads from disks and writes to
//! them (ADR 0476), as Linux's `/proc/PID/io` and a cgroup's `io.stat`
//! count it.
//!
//! A disk's queue blocks the thread that made a request until the device
//! is done ([`super::queue`]), so the thread that waited is the one the
//! request was for: a read is counted to it once the device has finished
//! it, and so is a write made to a disk directly, from a program's memory
//! (`write_frames`, as a raw device's `write` makes). A write to a file
//! leaves pages dirty in the page cache, which the page cache tells of
//! once the write is done ([`radix_vfs::set_dirtied`]): those are counted
//! to the writer then, a page each, and the request that writes them back
//! later is the kernel's, whoever makes it, so nothing is counted twice.
//! A kernel thread's reads are counted to its domain, the root's.
//!
//! The counts go to the running thread's process and to its own domain
//! alone, which no other domain's requests touch: a domain's total over
//! the domains below it is summed when asked
//! ([`crate::domains::Domain::disk_use`]).
//!
//! The same requests are held to the domains' rates ([`keep_to_rates`]):
//! a thread waits before it reads, or writes to a disk directly, and once
//! a write to a file has left pages dirty, so that what it reads and
//! writes keeps to the slowest rate of its domain and those above it.
//! The page cache's writes back are the kernel's, and held to none.

use radix_block::PAGE_SIZE;

use crate::{domains, sched};

/// Counts `bytes` in `requests` requests that the device finished for the
/// running thread: read, or written directly (`written`).
pub fn count(written: bool, bytes: u64, requests: u64) {
    let Some(me) = sched::running() else { return };
    me.domain().count_disk(written, bytes, requests);
    if let Some(user) = me.user() {
        user.process.note_disk(written, bytes);
    }
}

/// Holds the running thread to its domain's rates, and those of each
/// domain above it, for `bytes` in `requests` requests it is about to
/// read, or write (`written`): it sleeps for as long as the slowest says,
/// which its domain counts as time throttled. Nothing while no domain
/// has a rate.
pub fn keep_to_rates(written: bool, bytes: u64, requests: u64) {
    if !domains::io_rated() {
        return;
    }
    let Some(me) = sched::running() else { return };
    let domain = me.domain();
    let start = sched::now_ns();
    let wait = domain.take_io(written, bytes, requests, start);
    if wait > 0 {
        // Only a stop cuts it short: a signal does not, or a program
        // could signal itself past its rates.
        sched::sleep_until(start.saturating_add(wait));
        domain.count_disk_wait(0, sched::now_ns().saturating_sub(start));
    }
}

/// The page cache's report of `pages` that a write of the running thread
/// left dirty, clean before ([`radix_vfs::set_dirtied`]): counted, and
/// held to its domain's write rate.
pub fn dirtied(pages: u64) {
    let bytes = pages * PAGE_SIZE as u64;
    count(true, bytes, 0);
    keep_to_rates(true, bytes, 0);
}
