// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The memory test at the start (`memtest=once`, hardware(8), ADR 0565).
//!
//! Run on the boot CPU before the first program, once the disks are
//! found: it takes the free frames, in blocks of 2 MiB and then smaller,
//! until 64 MiB are left for the kernel's threads; runs the test of
//! [`radix_mm::memtest`] over them; gives them back; and takes each frame
//! with an error out of use, as a machine check does. It stops after an
//! hour. It pets the watchdog when one runs, and says how far it got in
//! the log at each tenth.
//!
//! `memtest=fault` runs the same test and flips one bit of the first page
//! it holds after the first step, so that the boot tests see the errors'
//! path under QEMU, whose memory never fails.
//!
//! The result is `/kernel/boot/memtest` (radix-sysfs(5)), there only when
//! the test ran: `result passed` or `errors`, `complete yes` or `no`,
//! `tested` and `total` bytes, `errors`, `seconds`, `retired`, then a
//! `bad FROM TO N` line for each range of bad frames, at most 64, in
//! physical addresses of hexadecimal. hardware(8) records it and raises
//! the alert once the system runs.

use alloc::vec::Vec;
use core::fmt;

use radix_hal::UserMode;
use radix_klog::{error, info, warn};
use radix_mm::memtest::{self, BAD_MAX, Ram, SCRATCH, Test};
use radix_mm::{FRAMES, Flags, HEAP, Kmalloc, Owner, PAGE_SIZE, Retired};
use radix_sync::Once;

use crate::{Arch, sched, watchdog};

/// The blocks taken first: 2^9 frames, 2 MiB.
const BLOCK_ORDER: u32 = 9;
/// What is left free for the kernel's threads while the test runs.
const KEEP_BYTES: u64 = 64 << 20;
/// The longest the test runs.
const LIMIT_NS: u64 = 60 * 60 * 1_000_000_000;

/// How the test ended, for `/kernel/boot/memtest`.
struct Done {
    complete: bool,
    tested: u64,
    total: u64,
    errors: u64,
    seconds: u64,
    retired: u64,
    /// First byte, last byte and errors of each range of bad frames.
    ranges: Vec<(u64, u64, u64), Kmalloc>,
}

static DONE: Once<Done> = Once::new();

/// The memory, reached through the kernel's copies to and from frames.
struct Frames;

impl Ram for Frames {
    fn write(&mut self, at: u64, bytes: &[u8; PAGE_SIZE]) {
        Arch::write_frames(at, bytes);
    }

    fn read(&mut self, at: u64, bytes: &mut [u8; PAGE_SIZE]) {
        Arch::read_frames(at, bytes);
    }
}

/// Runs the test, if the command line's `memtest` asks for it: `once`, or
/// `fault` for the boot tests.
pub fn run_if_asked(asked: Option<&str>) {
    let fault = match asked {
        None => return,
        Some("once") => false,
        Some("fault") => true,
        Some(other) => {
            warn!("Memory test: `memtest={other}` is not `once` or `fault`: not run");
            return;
        }
    };
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let total = FRAMES.managed_pages() * PAGE_SIZE as u64;
    let Some((mut scratch, mut room, mut blocks)) = room_for(heap) else {
        error!("Memory test: no memory for its own lists: not run");
        return;
    };
    take(&mut blocks);
    let tested: u64 = blocks.iter().map(|&(_, pages)| pages * PAGE_SIZE as u64).sum();
    info!("Memory test: testing {} of {} MiB", tested >> 20, total >> 20);
    let started = sched::now_ns();
    let Ok(scratch) = <&mut [u8; SCRATCH]>::try_from(&mut scratch[..]) else { return };
    let test = Test { blocks: &blocks, seed: radix_random::u64(), fault, scratch, room: &mut room };
    let mut tenth = 0;
    let outcome = memtest::run(
        &mut Frames,
        test,
        || sched::now_ns().saturating_sub(started) >= LIMIT_NS,
        |done, all| {
            watchdog::pet();
            let now = done * 10 / all.max(1);
            if now > tenth && now < 10 {
                tenth = now;
                info!("Memory test: {}% of {} MiB", now * 10, tested >> 20);
            }
            sched::yield_now();
        },
    );
    let seconds = sched::now_ns().saturating_sub(started) / 1_000_000_000;
    for &(at, pages) in &blocks {
        let order = pages.trailing_zeros();
        if FRAMES.free(at, order, Owner::Kernel).is_err() {
            error!("Memory test: the block at {at:#x} could not be given back");
        }
    }
    let mut retired = 0;
    for &(frame, _) in outcome.bad.frames() {
        match FRAMES.retire(frame) {
            Retired::Now | Retired::WhenFreed => retired += 1,
            Retired::NotManaged => {}
            Retired::TooMany => {
                error!("Memory test: the page at {frame:#x} cannot be retired: too many are");
            }
        }
    }
    let mut ranges = Vec::new_in(heap);
    if ranges.try_reserve_exact(memtest::RANGES_MAX).is_ok() {
        for range in outcome.bad.ranges() {
            let _ = ranges.push_within_capacity(range);
        }
    }
    let done = Done {
        complete: outcome.complete,
        tested,
        total,
        errors: outcome.errors,
        seconds,
        retired,
        ranges,
    };
    say(&done);
    let _ = DONE.set(done);
}

/// The scratch, the room for bad frames and the list of blocks, on the
/// heap: a kernel thread's stack holds none of them.
#[allow(clippy::type_complexity)]
fn room_for(
    heap: Kmalloc,
) -> Option<(Vec<u8, Kmalloc>, Vec<(u64, u32), Kmalloc>, Vec<(u64, u64), Kmalloc>)> {
    let scratch = radix_mm::bytes::zeroed(SCRATCH, heap).ok()?;
    let room = radix_mm::bytes::filled((0u64, 0u32), BAD_MAX, heap).ok()?;
    // A block of 2 MiB for each 2 MiB, and room for smaller ones where
    // memory is in pieces.
    let mut blocks = Vec::new_in(heap);
    let most = (FRAMES.managed_pages() >> BLOCK_ORDER) as usize + 4096;
    blocks.try_reserve_exact(most).ok()?;
    Some((scratch, room, blocks))
}

/// Takes free frames into `blocks`, as each its address and pages, the
/// largest first, until [`KEEP_BYTES`] are left or `blocks` is full.
fn take(blocks: &mut Vec<(u64, u64), Kmalloc>) {
    let keep = KEEP_BYTES / PAGE_SIZE as u64;
    let mut order = BLOCK_ORDER;
    while blocks.len() < blocks.capacity() {
        let pages = 1u64 << order;
        if FRAMES.free_pages() < keep + pages {
            if order == 0 {
                break;
            }
            order -= 1;
            continue;
        }
        match FRAMES.alloc_spare(order, Owner::Kernel) {
            // The loop stops before `blocks` is full, so it has room.
            Ok(at) => {
                if blocks.push_within_capacity((at, pages)).is_err() {
                    let _ = FRAMES.free(at, order, Owner::Kernel);
                    break;
                }
            }
            Err(_) if order == 0 => break,
            Err(_) => order -= 1,
        }
    }
}

/// Says how the test ended in the log.
fn say(done: &Done) {
    let (tested, total) = (done.tested >> 20, done.total >> 20);
    let ended = if done.complete { "" } else { ", stopped after an hour" };
    if done.errors == 0 {
        info!("Memory test: passed: {tested} of {total} MiB tested in {} s{ended}", done.seconds);
        return;
    }
    error!(
        "Memory test: {} errors in {tested} of {total} MiB{ended}; {} pages are out of use until \
         the next restart",
        done.errors, done.retired
    );
    for &(first, last, errors) in &done.ranges {
        error!("Memory test: {errors} errors from {first:#x} to {last:#x}");
    }
}

/// Whether the test ran this start, for `/kernel/boot/memtest`.
pub fn ran() -> bool {
    DONE.get().is_some()
}

/// `/kernel/boot/memtest`.
pub fn write(text: &mut impl fmt::Write) -> fmt::Result {
    let Some(done) = DONE.get() else { return Ok(()) };
    let result = if done.errors == 0 { "passed" } else { "errors" };
    writeln!(text, "result {result}")?;
    writeln!(text, "complete {}", if done.complete { "yes" } else { "no" })?;
    writeln!(text, "tested {}", done.tested)?;
    writeln!(text, "total {}", done.total)?;
    writeln!(text, "errors {}", done.errors)?;
    writeln!(text, "seconds {}", done.seconds)?;
    writeln!(text, "retired {}", done.retired)?;
    for &(first, last, errors) in &done.ranges {
        writeln!(text, "bad {first:#x} {last:#x} {errors}")?;
    }
    Ok(())
}
