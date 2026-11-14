// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The heap stress test.
//!
//! Each worker (a CPU in the kernel's `mm-stress` self-test, a thread in
//! the host tests) runs a random mix of allocations and frees against the
//! heap: sizes from 1 byte to 64 KiB, alignments up to a page, both
//! contexts, zeroed and not. It keeps up to [`SLOTS`] objects alive at
//! once, fills each with a pattern derived from its address and checks
//! the pattern before freeing it, so an object handed to two callers, or
//! damaged while in use, shows up as a mismatch. Zeroed objects are
//! checked to be zero. At the end the worker frees what it still holds.
//!
//! The heap's areas get a test of their own: [`break_up`] leaves memory in
//! pieces, with no block larger than a frame free, and each worker of
//! [`areas`] then makes large requests, which areas must serve, and frees
//! some of them where it may not wait to release them.

#![allow(unsafe_code)]

use core::alloc::Layout;
use core::ptr::NonNull;

use crate::context::Flags;
use crate::frames::Frames;
use crate::heap::Heap;
use crate::page::{Owner, PAGE_SHIFT, PAGE_SIZE};
use crate::raw;

/// Objects one worker keeps alive at most.
pub const SLOTS: usize = 512;

#[derive(Clone, Copy)]
#[repr(C)]
struct Slot {
    addr: usize,
    size: u32,
    align: u32,
}

/// What one worker did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StressReport {
    /// Successful allocations.
    pub allocations: u64,
    /// Frees.
    pub frees: u64,
    /// Allocations that failed for lack of memory.
    pub failures: u64,
    /// Objects whose contents were wrong when checked.
    pub mismatches: u64,
}

impl StressReport {
    /// Adds another worker's counts.
    pub fn add(&mut self, other: &StressReport) {
        self.allocations += other.allocations;
        self.frees += other.frees;
        self.failures += other.failures;
        self.mismatches += other.mismatches;
    }
}

/// A small, fast pseudo-random generator (xorshift64*).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn pattern(addr: usize, at: usize) -> usize {
    (addr ^ at.rotate_left(17)).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1
}

/// The word offsets of an object of `size` bytes that carry its pattern:
/// every word of small objects, the ends and middle of larger ones.
fn offsets(size: usize) -> impl Iterator<Item = usize> {
    let words = size / 8;
    let all = if words <= 32 { 0..words } else { 0..0 };
    let sparse = if words > 32 { [0, 1, words / 2, words - 2, words - 1] } else { [usize::MAX; 5] };
    all.chain(sparse.into_iter().filter(|&w| w != usize::MAX)).map(|w| w * 8)
}

/// Runs `operations` random allocations and frees on `heap`, seeded with
/// `seed`. Needs about 8 KiB of heap for its own table of live objects.
pub fn run(heap: &Heap, seed: u64, operations: u64) -> StressReport {
    let mut report = StressReport::default();
    let table = Layout::new::<[Slot; SLOTS]>();
    let Ok(base) = heap.alloc(table, Flags::MAY_SLEEP.zeroed()) else {
        report.failures += 1;
        return report;
    };
    // SAFETY: the heap gave us this zeroed, suitably aligned memory, and a
    // slot of zeros is a valid (empty) `Slot`.
    #[allow(clippy::cast_ptr_alignment)]
    let slots = unsafe { &mut *base.as_ptr().cast::<[Slot; SLOTS]>() };
    let mut random = Random(seed | 1);
    let mut live = 0;
    for _ in 0..operations {
        let index = random.below(SLOTS as u64) as usize;
        let slot = &mut slots[index];
        if slot.addr != 0 {
            free(heap, slot, &mut report);
            live -= 1;
            continue;
        }
        // Keep the table about half full, so frees and allocations mix.
        if live > SLOTS / 2 && random.below(2) == 0 {
            continue;
        }
        let roll = random.below(1000);
        let size = match roll {
            0..=599 => 1 + random.below(256),
            600..=949 => 257 + random.below(2048 - 256),
            950..=994 => 2049 + random.below(8192 - 2048),
            _ => 8193 + random.below(56 << 10),
        } as usize;
        let align = match random.below(16) {
            0 => 4096,
            1..=3 => 64,
            _ => 8,
        };
        let mut flags = if random.below(4) == 0 { Flags::ATOMIC } else { Flags::MAY_SLEEP };
        flags.zero = random.below(8) == 0;
        let Ok(layout) = Layout::from_size_align(size, align) else { continue };
        let Ok(ptr) = heap.alloc(layout, flags) else {
            report.failures += 1;
            continue;
        };
        let addr = ptr.as_ptr() as usize;
        report.allocations += 1;
        if !addr.is_multiple_of(align) {
            report.mismatches += 1;
        }
        for at in offsets(size) {
            // SAFETY: `at` is a word inside the `size` bytes just allocated
            // for us, and objects are at least 8-byte aligned.
            let word = unsafe { raw::read_word(addr + at) };
            if flags.zero && word != 0 {
                report.mismatches += 1;
            }
            // SAFETY: as above.
            unsafe { raw::write_word(addr + at, pattern(addr, at)) };
        }
        *slot = Slot { addr, size: size as u32, align: align as u32 };
        live += 1;
    }
    for slot in slots.iter_mut().filter(|slot| slot.addr != 0) {
        free(heap, slot, &mut report);
    }
    // SAFETY: the table came from `heap.alloc` with this layout, and the
    // references into it end here.
    unsafe { heap.free(base, table) };
    report
}

fn free(heap: &Heap, slot: &mut Slot, report: &mut StressReport) {
    let (addr, size) = (slot.addr, slot.size as usize);
    for at in offsets(size) {
        // SAFETY: the object is still ours, and `at` is a word inside it.
        if unsafe { raw::read_word(addr + at) } != pattern(addr, at) {
            report.mismatches += 1;
        }
    }
    if let (Ok(layout), Some(ptr)) =
        (Layout::from_size_align(size, slot.align as usize), NonNull::new(addr as *mut u8))
    {
        // SAFETY: the object came from `heap.alloc` with this layout and is
        // forgotten here.
        unsafe { heap.free(ptr, layout) };
        report.frees += 1;
    }
    slot.addr = 0;
}

/// Ends the chain of frames [`Pieces`] keeps.
const END: u64 = u64::MAX;

/// Memory left in pieces by [`break_up`]: the frames it keeps, chained
/// through their own first words, so that keeping them takes no memory.
pub struct Pieces<'a> {
    frames: &'a Frames,
    direct_map: usize,
    first: u64,
    kept: u64,
}

/// Takes every free frame, then frees each one whose frame number is odd:
/// half of the memory that was free is free again, but no two free frames
/// are buddies, so no block larger than a frame is free. The frames come
/// from the reserves too, so for as long as memory stays in pieces even
/// callers that may not sleep find no larger block. [`Pieces::join`] gives
/// the frames kept back.
pub fn break_up(frames: &Frames) -> Pieces<'_> {
    let Some(platform) = frames.platform() else {
        return Pieces { frames, direct_map: 0, first: END, kept: 0 };
    };
    let direct_map = platform.direct_map();
    let mut pieces = Pieces { frames, direct_map, first: END, kept: 0 };
    let mut odd = END;
    while let Ok(phys) = frames.alloc(0, Flags::ATOMIC, Owner::Kernel) {
        let chain = if (phys >> PAGE_SHIFT) & 1 == 0 { &mut pieces.first } else { &mut odd };
        // SAFETY: the frame was just allocated for this chain, and the
        // direct map maps every managed frame writable (`Platform`).
        unsafe { raw::write_word(direct_map + phys as usize, *chain as usize) };
        *chain = phys;
        pieces.kept += u64::from((phys >> PAGE_SHIFT) & 1 == 0);
    }
    pieces.give_back(odd);
    pieces
}

impl Pieces<'_> {
    /// How many frames are kept.
    pub fn kept(&self) -> u64 {
        self.kept
    }

    /// Gives back the frames kept, making memory whole again.
    pub fn join(self) {
        self.give_back(self.first);
    }

    /// Frees the chain of frames from `first`.
    fn give_back(&self, mut next: u64) {
        while next != END {
            // SAFETY: a frame on a chain is ours until freed below, and
            // its first word was written when it joined the chain.
            let after = unsafe { raw::read_word(self.direct_map + next as usize) } as u64;
            let _ = self.frames.free(next, 0, Owner::Kernel);
            next = after;
        }
    }
}

/// What one worker of [`areas`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AreaReport {
    /// Large requests that areas served.
    pub areas: u64,
    /// Large requests that blocks of frames served.
    pub blocks: u64,
    /// Large requests that failed.
    pub failures: u64,
    /// Requests whose memory was not zeroed, or whose contents were wrong
    /// when checked.
    pub mismatches: u64,
}

impl AreaReport {
    /// Adds another worker's counts.
    pub fn add(&mut self, other: &AreaReport) {
        self.areas += other.areas;
        self.blocks += other.blocks;
        self.failures += other.failures;
        self.mismatches += other.mismatches;
    }
}

/// The large requests each round of [`areas`] makes: just over the
/// largest object a slab holds (8 KiB), 68 KiB and 256 KiB.
const AREA_SIZES: [usize; 3] = [2 * PAGE_SIZE + 8, 17 * PAGE_SIZE, 64 * PAGE_SIZE];

/// Makes `rounds` rounds of large requests on `heap`, zeroed, from a
/// caller that may sleep: with memory in pieces ([`break_up`]), areas
/// serve them. Checks that each is zeroed, fills it with a pattern and
/// checks the pattern, then frees them, the first of each round with
/// interrupts disabled, where the heap may not wait to release it.
pub fn areas(heap: &Heap, rounds: u64) -> AreaReport {
    let mut report = AreaReport::default();
    let Some(platform) = heap.frames().platform() else { return report };
    for _ in 0..rounds {
        let mut live = [None; AREA_SIZES.len()];
        for (slot, size) in live.iter_mut().zip(AREA_SIZES) {
            let Ok(layout) = Layout::from_size_align(size, 8) else { continue };
            let Ok(ptr) = heap.alloc(layout, Flags::MAY_SLEEP.zeroed()) else {
                report.failures += 1;
                continue;
            };
            let addr = ptr.as_ptr() as usize;
            if platform.area_frame(addr).is_some() {
                report.areas += 1;
            } else {
                report.blocks += 1;
            }
            for at in offsets(size) {
                // SAFETY: `at` is a word inside the `size` bytes just
                // allocated for us, and the request is 8-byte aligned.
                if unsafe { raw::read_word(addr + at) } != 0 {
                    report.mismatches += 1;
                }
                // SAFETY: as above.
                unsafe { raw::write_word(addr + at, pattern(addr, at)) };
            }
            *slot = Some((ptr, layout));
        }
        for (index, (ptr, layout)) in live.into_iter().flatten().enumerate() {
            let addr = ptr.as_ptr() as usize;
            for at in offsets(layout.size()) {
                // SAFETY: the request is still ours, and `at` is a word
                // inside it.
                if unsafe { raw::read_word(addr + at) } != pattern(addr, at) {
                    report.mismatches += 1;
                }
            }
            let were_enabled = (index == 0).then(|| platform.disable_interrupts());
            // SAFETY: the request came from `heap.alloc` with this layout
            // and is forgotten here.
            unsafe { heap.free(ptr, layout) };
            if let Some(were_enabled) = were_enabled {
                platform.restore_interrupts(were_enabled);
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::heap_with_window;

    #[test]
    fn offsets_stay_inside_objects() {
        for size in [1, 8, 15, 16, 255, 256, 257, 4096, 65536] {
            for at in offsets(size) {
                assert!(at + 8 <= size.max(8), "{size} {at}");
            }
        }
        assert_eq!(offsets(7).count(), 0);
        assert_eq!(offsets(16).count(), 2);
        assert_eq!(offsets(4096).count(), 5);
    }

    #[test]
    fn areas_serve_large_requests_while_memory_is_in_pieces() {
        let (heap, _) = heap_with_window(1024, 512);
        let frames = heap.frames();
        let free = frames.free_pages();
        let pieces = break_up(frames);
        assert_eq!(pieces.kept(), 512);
        assert_eq!(frames.free_pages(), 512);
        let report = areas(heap, 4);
        assert_eq!(report, AreaReport { areas: 12, ..AreaReport::default() });
        pieces.join();
        heap.drain();
        assert_eq!(frames.free_pages(), free);
        assert_eq!((heap.stats().areas, heap.stats().corruptions), (0, 0));
        frames.check().unwrap();
    }
}
