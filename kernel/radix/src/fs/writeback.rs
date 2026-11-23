// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The writeback thread.
//!
//! Every [`INTERVAL_NS`] it writes each filesystem whose changes in memory
//! are due to its disk (`Vfs::sync_due`): every change for most, those 30 s
//! old for Btrfs, so at most that much work is lost when the power goes,
//! as with Linux's flusher threads. Powering off writes every change
//! (`Vfs::sync_all`). It also drops clean
//! cached pages, which the disk has, when memory runs short: when the
//! frame allocator's free pages fall below 1/64 of memory at a wake-up,
//! and whenever the out-of-memory policy asks ([`reclaim`]) before it
//! stops a resource domain. Dropping pages takes the caches' locks, so it
//! runs here, in a thread that holds none, never inside an allocation.
//!
//! Unwritten data cannot fill memory either, as it did when a program
//! wrote a file faster than its disk took it: once the pages of files
//! waiting to be written ([`radix_vfs::cache::dirty_pages`]) pass a tenth
//! of memory, a write asks the thread to write every filesystem back at
//! once, however new its changes, and once they pass a fifth, the writer
//! waits for it before it goes on ([`throttle`]), as Linux's
//! `dirty_background_ratio` and `dirty_ratio` have it.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use radix_klog::{error, warn};
use radix_mm::FRAMES;
use radix_sync::Once;

use crate::sched::{self, Options, Policy, ThreadRef, wait};

/// How often dirty filesystems are written back.
pub const INTERVAL_NS: u64 = 5_000_000_000;

/// The fewest pages a reclaim drops, when the caches hold that many.
const RECLAIM_MIN: usize = 256;

/// Memory over the dirty pages that have the thread write back at once:
/// a tenth of it.
const BACKGROUND_SHARE: usize = 10;
/// Memory over the dirty pages that have a writer wait: a fifth of it.
const WAIT_SHARE: usize = 5;
/// How long a waiting writer sleeps before it looks again.
const WAIT_SLICE_NS: u64 = 100_000_000;
/// How long a writer waits, at most, while the dirty pages do not go
/// down: a disk that takes nothing must not hold its writers for good.
const WAIT_STUCK_NS: u64 = 30_000_000_000;

/// The dirty pages over which the thread writes back at once, and over
/// which a writer waits: shares of the memory the kernel manages, which
/// the thread works out again at each look ([`set_limits`]), as taking
/// the frame allocator's locks at each write would cost too much. None
/// until the thread starts.
static BACKGROUND_LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static WAIT_LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Write-backs of everything asked for by writers, and done.
static FLUSH_ASKED: AtomicU64 = AtomicU64::new(0);
static FLUSH_DONE: AtomicU64 = AtomicU64::new(0);
/// Writers that waited, and that gave up waiting on a stuck disk.
static THROTTLED: AtomicU64 = AtomicU64::new(0);
static STUCK: AtomicU64 = AtomicU64::new(0);

/// Reclaims asked for, and done: the thread works while they differ.
static ASKED: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
/// Pages dropped so far.
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Write-backs that failed, for the log.
static FAILED: AtomicU64 = AtomicU64::new(0);

static THREAD: Once<ThreadRef> = Once::new();

fn asked_key() -> usize {
    &raw const ASKED as usize
}

fn done_key() -> usize {
    &raw const DONE as usize
}

fn flushed_key() -> usize {
    &raw const FLUSH_DONE as usize
}

/// Works the dirty pages' limits out from the memory the kernel manages
/// now, which a machine check that takes memory out of use changes;
/// returns the background limit.
fn set_limits() -> usize {
    let managed = FRAMES.managed_pages() as usize;
    let background = managed / BACKGROUND_SHARE;
    BACKGROUND_LIMIT.store(background, Ordering::Relaxed);
    WAIT_LIMIT.store(managed / WAIT_SHARE, Ordering::Relaxed);
    background
}

/// Starts the thread.
pub fn start() {
    set_limits();
    match sched::spawn("writeback", run, 0, Options::new().policy(Policy::Fair { nice: 0 })) {
        Ok(thread) => {
            let _ = THREAD.set(thread);
        }
        Err(err) => error!("Files: could not start the writeback thread: {err}"),
    }
}

fn run(_: usize) {
    let mut next_sync = sched::now_ns().saturating_add(INTERVAL_NS);
    loop {
        wait::wait_until_or_timeout(asked_key(), "writeback", next_sync, || {
            ASKED.load(Ordering::Acquire) != DONE.load(Ordering::Acquire)
                || FLUSH_ASKED.load(Ordering::Acquire) != FLUSH_DONE.load(Ordering::Acquire)
        });
        let asked = ASKED.load(Ordering::Acquire);
        let flush = FLUSH_ASKED.load(Ordering::Acquire);
        let Some(vfs) = super::vfs() else {
            // No files, for now or for good: an ask is answered with
            // nothing dropped, and the next look waits its turn.
            DONE.store(asked, Ordering::Release);
            wait::wake_all(done_key());
            FLUSH_DONE.store(flush, Ordering::Release);
            wait::wake_all(flushed_key());
            if sched::now_ns() >= next_sync {
                next_sync = sched::now_ns().saturating_add(INTERVAL_NS);
            }
            continue;
        };
        // Too much unwritten data: every filesystem goes to its disk now,
        // before clean pages are dropped, so that its pages can go too.
        if flush != FLUSH_DONE.load(Ordering::Acquire)
            || radix_vfs::cache::dirty_pages() >= set_limits()
        {
            if let Err(err) = vfs.sync_all()
                && FAILED.fetch_add(1, Ordering::Relaxed) == 0
            {
                warn!("Files: writing back to a disk failed: {err}; changes stay in memory");
            }
            FLUSH_DONE.store(flush, Ordering::Release);
            wait::wake_all(flushed_key());
        }
        let short = FRAMES.free_pages() < FRAMES.managed_pages() / 64;
        if asked != DONE.load(Ordering::Acquire) || short {
            let cached = radix_block::mem::pages() as usize;
            let dropped = vfs.shrink((cached / 4).max(RECLAIM_MIN));
            DROPPED.fetch_add(dropped as u64, Ordering::AcqRel);
            DONE.store(asked, Ordering::Release);
            wait::wake_all(done_key());
        }
        if sched::now_ns() >= next_sync {
            if let Err(err) = vfs.sync_due() {
                // Said once, then counted: a failing disk would fill the
                // log every five seconds.
                if FAILED.fetch_add(1, Ordering::Relaxed) == 0 {
                    warn!("Files: writing back to a disk failed: {err}; changes stay in memory");
                }
            }
            super::warn_failed_disks();
            next_sync = sched::now_ns().saturating_add(INTERVAL_NS);
        }
    }
}

/// Asks the thread to drop clean cached pages and waits for it, up to
/// `timeout_ns`. Returns whether any went. The writeback thread itself
/// gets `false` at once: it cannot wait for itself.
pub fn reclaim(timeout_ns: u64) -> bool {
    let Some(thread) = THREAD.get() else { return false };
    if sched::current().is_some_and(|me| ThreadRef::ptr_eq(&me, thread)) {
        return false;
    }
    let before = DROPPED.load(Ordering::Acquire);
    let ticket = ASKED.fetch_add(1, Ordering::AcqRel) + 1;
    wait::wake_all(asked_key());
    let deadline = sched::now_ns().saturating_add(timeout_ns);
    wait::wait_until_or_timeout(done_key(), "reclaim", deadline, || {
        DONE.load(Ordering::Acquire) >= ticket
    });
    DROPPED.load(Ordering::Acquire) > before
}

/// Holds the running thread, which has just written to a file, to the
/// dirty pages' limits: past a tenth of memory it asks the thread to
/// write everything back; past a fifth it waits until the thread has
/// brought them under, looking again every [`WAIT_SLICE_NS`]. Only a stop
/// cuts the wait short, or [`WAIT_STUCK_NS`] in which the dirty pages did
/// not go down. The caller holds no lock: the thread takes the files'.
#[inline]
pub fn throttle() {
    if radix_vfs::cache::dirty_pages() >= BACKGROUND_LIMIT.load(Ordering::Relaxed) {
        hold();
    }
}

/// [`throttle`]'s work once the dirty pages are over the background
/// limit.
#[cold]
fn hold() {
    let Some(thread) = THREAD.get() else { return };
    let Some(me) = sched::current() else { return };
    if ThreadRef::ptr_eq(&me, thread) {
        return;
    }
    let most = WAIT_LIMIT.load(Ordering::Relaxed);
    let dirty = radix_vfs::cache::dirty_pages();
    let mut ticket = ask_flush();
    if dirty < most {
        return;
    }
    THROTTLED.fetch_add(1, Ordering::Relaxed);
    let mut least = dirty;
    let mut since = sched::now_ns();
    loop {
        let deadline = sched::now_ns().saturating_add(WAIT_SLICE_NS);
        wait::wait_until_or_timeout(flushed_key(), "dirty", deadline, || {
            FLUSH_DONE.load(Ordering::Acquire) >= ticket || radix_vfs::cache::dirty_pages() < most
        });
        let dirty = radix_vfs::cache::dirty_pages();
        if dirty < most || me.should_stop() {
            return;
        }
        let now = sched::now_ns();
        if dirty < least {
            (least, since) = (dirty, now);
        } else if now.saturating_sub(since) >= WAIT_STUCK_NS {
            // Said once, then counted.
            if STUCK.fetch_add(1, Ordering::Relaxed) == 0 {
                warn!(
                    "Files: {dirty} pages wait to be written and none went in {} s; writers go on",
                    WAIT_STUCK_NS / 1_000_000_000
                );
            }
            return;
        }
        if FLUSH_DONE.load(Ordering::Acquire) >= ticket {
            ticket = ask_flush();
        }
    }
}

/// Asks the thread to write every filesystem back, unless an ask waits
/// already: each write past the limit would otherwise have it commit
/// again for a few pages. Returns the ticket that [`FLUSH_DONE`] reaches
/// once it has.
fn ask_flush() -> u64 {
    let asked = FLUSH_ASKED.load(Ordering::Acquire);
    if asked != FLUSH_DONE.load(Ordering::Acquire) {
        return asked;
    }
    let ticket = FLUSH_ASKED.fetch_add(1, Ordering::AcqRel) + 1;
    wait::wake_all(asked_key());
    ticket
}

/// Pages dropped and failed write-backs so far, for `kdb`.
pub fn stats() -> (u64, u64) {
    (DROPPED.load(Ordering::Relaxed), FAILED.load(Ordering::Relaxed))
}

/// Writers held to the dirty pages' limit, and those that went on as a
/// disk took nothing, so far.
pub fn throttle_stats() -> (u64, u64) {
    (THROTTLED.load(Ordering::Relaxed), STUCK.load(Ordering::Relaxed))
}
