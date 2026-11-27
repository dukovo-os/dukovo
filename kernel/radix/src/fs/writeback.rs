// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The writeback thread, and the reclaim thread beside it.
//!
//! Every [`INTERVAL_NS`] the writeback thread writes each filesystem whose
//! changes in memory are due to its disk (`Vfs::sync_due`): every change
//! for most, those 30 s old for Btrfs, so at most that much work is lost
//! when the power goes, as with Linux's flusher threads. Powering off
//! writes every change (`Vfs::sync_all`).
//!
//! The reclaim thread drops clean cached pages, which the disk has, when
//! memory runs short: when less than a 32nd of memory is free at its look
//! each second, when a writer finds it so ([`throttle`]), and whenever the
//! out-of-memory policy asks ([`reclaim`]) before it stops a resource
//! domain, as Linux's kswapd does beside its flushers. Dropping pages
//! takes the caches' locks, so it runs in a thread that holds none, never
//! inside an allocation, and in a thread of its own: a write-back of a
//! fifth of memory takes seconds on a slow disk, and the out-of-memory
//! policy waits a fifth of a second for a reclaim. It passes over a cache
//! whose lock is held, as by the write-back or by a writer that waits for
//! memory, so it never waits for one either.
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

/// How often the reclaim thread looks at free memory by itself.
const RECLAIM_LOOK_NS: u64 = 1_000_000_000;

/// Memory over the free pages under which memory is short, and clean
/// cached pages are dropped: a 32nd of it.
const SHORT_SHARE: usize = 32;

/// Memory over the growth of the caches after which a writer looks at
/// free memory again ([`throttle`]): a 256th of it, so that a writer that
/// fills the caches finds memory short before it has run out.
const LOOK_SHARE: usize = 256;

/// How long a writer that finds memory short waits for the reclaim
/// thread, at most.
const WRITER_RECLAIM_NS: u64 = 1_000_000_000;

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
/// The memory the kernel manages, in pages, as last worked out.
static MANAGED: AtomicUsize = AtomicUsize::new(0);
/// The pages the caches hold at which a writer next looks at free memory.
static NEXT_LOOK: AtomicUsize = AtomicUsize::new(0);

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
static RECLAIMER: Once<ThreadRef> = Once::new();

fn asked_key() -> usize {
    &raw const ASKED as usize
}

fn done_key() -> usize {
    &raw const DONE as usize
}

fn flush_asked_key() -> usize {
    &raw const FLUSH_ASKED as usize
}

fn flushed_key() -> usize {
    &raw const FLUSH_DONE as usize
}

/// Works the dirty pages' limits out from the memory the kernel manages
/// now, which a machine check that takes memory out of use changes;
/// returns the background limit.
fn set_limits() -> usize {
    let managed = FRAMES.managed_pages() as usize;
    MANAGED.store(managed, Ordering::Relaxed);
    let background = managed / BACKGROUND_SHARE;
    BACKGROUND_LIMIT.store(background, Ordering::Relaxed);
    WAIT_LIMIT.store(managed / WAIT_SHARE, Ordering::Relaxed);
    background
}

/// Starts the threads.
pub fn start() {
    set_limits();
    match sched::spawn("writeback", run, 0, Options::new().policy(Policy::Fair { nice: 0 })) {
        Ok(thread) => {
            let _ = THREAD.set(thread);
        }
        Err(err) => error!("Files: could not start the writeback thread: {err}"),
    }
    match sched::spawn("reclaim", run_reclaim, 0, Options::new().policy(Policy::Fair { nice: 0 })) {
        Ok(thread) => {
            let _ = RECLAIMER.set(thread);
        }
        Err(err) => error!("Files: could not start the reclaim thread: {err}"),
    }
}

/// Whether memory is short: less than a [`SHORT_SHARE`] of what the
/// kernel manages is free. Takes the frame allocator's locks.
fn short() -> bool {
    (FRAMES.free_pages() as usize) < MANAGED.load(Ordering::Relaxed) / SHORT_SHARE
}

fn run(_: usize) {
    let mut next_sync = sched::now_ns().saturating_add(INTERVAL_NS);
    loop {
        wait::wait_until_or_timeout(flush_asked_key(), "writeback", next_sync, || {
            FLUSH_ASKED.load(Ordering::Acquire) != FLUSH_DONE.load(Ordering::Acquire)
        });
        let flush = FLUSH_ASKED.load(Ordering::Acquire);
        let Some(vfs) = super::vfs() else {
            // No files, for now or for good: an ask is answered with
            // nothing written, and the next look waits its turn.
            FLUSH_DONE.store(flush, Ordering::Release);
            wait::wake_all(flushed_key());
            if sched::now_ns() >= next_sync {
                next_sync = sched::now_ns().saturating_add(INTERVAL_NS);
            }
            continue;
        };
        // Too much unwritten data: every filesystem goes to its disk now,
        // so that its pages can be dropped too.
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

/// The reclaim thread: drops a quarter of the clean cached pages, 256 at
/// least, when asked ([`reclaim`]) or when memory is short at its look.
fn run_reclaim(_: usize) {
    loop {
        let deadline = sched::now_ns().saturating_add(RECLAIM_LOOK_NS);
        wait::wait_until_or_timeout(asked_key(), "reclaim", deadline, || {
            ASKED.load(Ordering::Acquire) != DONE.load(Ordering::Acquire)
        });
        let asked = ASKED.load(Ordering::Acquire);
        if asked != DONE.load(Ordering::Acquire) || short() {
            // No files, for now or for good: an ask is answered with
            // nothing dropped.
            if let Some(vfs) = super::vfs() {
                let cached = radix_block::mem::pages() as usize;
                let dropped = vfs.shrink((cached / 4).max(RECLAIM_MIN));
                DROPPED.fetch_add(dropped as u64, Ordering::AcqRel);
            }
            DONE.store(asked, Ordering::Release);
            wait::wake_all(done_key());
        }
    }
}

/// Asks the reclaim thread to drop clean cached pages and waits for it,
/// up to `timeout_ns`. Returns whether any went. The reclaim thread
/// itself gets `false` at once: it cannot wait for itself.
pub fn reclaim(timeout_ns: u64) -> bool {
    let Some(thread) = RECLAIMER.get() else { return false };
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
/// not go down. Each time the caches have grown by a [`LOOK_SHARE`] of
/// memory, it also looks at free memory, and when memory is short it
/// waits for the reclaim thread to drop clean pages: a writer filling
/// the caches would otherwise run memory out while holding its file's
/// cache, whose clean pages the reclaim thread then cannot drop. The
/// caller holds no lock: the threads take the files'.
#[inline]
pub fn throttle() {
    if radix_vfs::cache::dirty_pages() >= BACKGROUND_LIMIT.load(Ordering::Relaxed) {
        hold();
    }
    if radix_block::mem::pages() as usize >= NEXT_LOOK.load(Ordering::Relaxed) {
        look_at_memory();
    }
}

/// [`throttle`]'s look at free memory once the caches have grown enough.
#[cold]
fn look_at_memory() {
    let step = (MANAGED.load(Ordering::Relaxed) / LOOK_SHARE).max(1);
    if short() {
        reclaim(WRITER_RECLAIM_NS);
    }
    NEXT_LOOK.store((radix_block::mem::pages() as usize).saturating_add(step), Ordering::Relaxed);
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
    wait::wake_all(flush_asked_key());
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
