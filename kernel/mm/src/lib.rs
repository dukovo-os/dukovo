// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Radix memory manager.
//!
//! This crate is the architecture-independent half of memory management:
//!
//! - [`Frames`], the frame allocator: [buddy](buddy) allocators per zone
//!   (`DMA32` and `Normal`) and per NUMA node, with reserves for atomic
//!   callers and fallback to the nearest node;
//! - [`Heap`], the kernel heap: slab caches per size class with per-CPU
//!   magazines in front ([`slab`], [`magazine`], [`class`]);
//! - [`Ranges`], the allocator of the kernel's mapping window, where
//!   stacks and device memory are mapped with guard gaps between them;
//! - [`Pcids`], the allocator of address-space identifiers;
//! - [`bytes`], byte vectors zeroed or filled with one string store;
//! - [`lease`], memory lent as a `&'static mut` slice that comes back
//!   when the slice is handed back, as smoltcp's socket buffers do;
//! - the heap [stress test](stress) the kernel runs on every CPU.
//!
//! Every allocation takes [`Flags`] that say whether the caller may sleep,
//! which memory it needs and from which node (spec 3.4), and every failure
//! is an [`AllocError`]: nothing here panics because memory ran out.
//!
//! The page tables themselves are architecture code (`radix-arch-x86_64`),
//! which hands physical memory to [`Frames`] at boot and implements
//! [`Platform`] for the heap.
//!
//! Most of the crate is safe code. The exceptions are the functions that
//! touch memory the allocators manage, each `unsafe` with its contract
//! written down: the `raw` module, the frame allocator's zeroing of the
//! blocks it hands out, the slab layer's free-object pattern, the heap's
//! `free`, the [`Platform`] trait, the stress test, the lengths
//! [`bytes`] sets and the memory a [`lease`] takes back.
//! The crate builds for the host too, where its unit and property tests
//! run, and for Kani, which proves the allocators' invariants
//! (`docs/design/memory.md`).

#![cfg_attr(not(any(test, kani)), no_std)]
#![deny(unsafe_code)]
// `AllocatorClone`, which `Arc`s in the heap need to be cloned.
#![cfg_attr(not(kani), feature(allocator_ext))]
// Kani's own, older nightly has the allocator API, which leases use,
// unstable still.
#![cfg_attr(kani, feature(allocator_api))]

extern crate alloc;

pub mod asid;
pub mod buddy;
#[cfg(not(kani))]
pub mod bytes;
pub mod class;
pub mod context;
pub mod frames;
pub mod heap;
pub mod lease;
pub mod magazine;
pub mod page;
pub mod platform;
mod raw;
pub mod slab;
pub mod stress;
pub mod vrange;

#[cfg(kani)]
mod proofs;

pub use asid::Pcids;
pub use buddy::{FreeError, MAX_ORDER};
pub use context::{AllocError, Context, Flags, NodePolicy, ZoneLimit};
pub use frames::{
    Frames, MAX_NODES, MAX_RETIRED, OOM_MAX_ORDER, RUN_FRAMES, Retired, SetupError, ZoneKind,
    ZoneStats,
};
pub use heap::{ClassStats, ExplicitOnly, Heap, HeapStats, Kmalloc};
pub use page::{Owner, PAGE_SHIFT, PAGE_SIZE, PageInfo, Region, State};
pub use platform::{AreaError, MAX_CPUS, Platform};
pub use slab::Corruption;
pub use vrange::{RangeError, Ranges};

/// The kernel's frame allocator.
pub static FRAMES: Frames = Frames::new();

/// The kernel heap.
pub static HEAP: Heap = Heap::new(&FRAMES);

#[cfg(test)]
pub(crate) mod tests {
    //! Helpers for the host tests: a platform whose "physical" memory is
    //! host memory (the direct map starts at 0), with the CPU number kept
    //! per thread, and a mapping window for the heap's areas if a test
    //! asks for one.

    use std::alloc::{Layout, alloc_zeroed};
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    thread_local! {
        static CPU: Cell<usize> = const { Cell::new(0) };
        static MAY_WAIT: Cell<bool> = const { Cell::new(true) };
        static MAPS_LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    /// Makes the calling thread CPU `cpu`.
    pub fn become_cpu(cpu: usize) {
        CPU.with(|c| c.set(cpu));
    }

    /// Makes the calling thread one that may wait for the other CPUs, or
    /// one that may not, as one holding a spin lock ([`Platform::may_wait`]).
    pub fn set_may_wait(may_wait: bool) {
        MAY_WAIT.with(|c| c.set(may_wait));
    }

    /// Lets the calling thread map `maps` more pages before
    /// [`Platform::map_page`] fails, as with no memory for a page table.
    pub fn set_maps_left(maps: usize) {
        MAPS_LEFT.with(|c| c.set(maps));
    }

    pub struct HostPlatform {
        nodes: usize,
        direct_map: usize,
        window: Option<Mutex<Window>>,
    }

    /// A mapping window of host memory: its free space, and the frame each
    /// mapped page stands for. Its memory is not the frames' memory, so
    /// what is written through an area stays in the window; the heap
    /// never reaches an area's frames through the direct map, so this is
    /// all its tests need.
    struct Window {
        ranges: Ranges,
        mapped: HashMap<usize, u64>,
    }

    impl HostPlatform {
        /// A platform with `nodes` nodes (CPU `n` is on node `n % nodes`)
        /// whose direct map starts at `direct_map`.
        pub fn leak(nodes: usize, direct_map: usize) -> &'static HostPlatform {
            Self::with_window(nodes, direct_map, 0)
        }

        /// [`HostPlatform::leak`], with a mapping window of `pages` pages
        /// of host memory, if any.
        pub fn with_window(nodes: usize, direct_map: usize, pages: usize) -> &'static HostPlatform {
            let window = (pages > 0).then(|| {
                let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
                // SAFETY: the layout has a non-zero size.
                #[allow(unsafe_code)]
                let start = unsafe { alloc_zeroed(layout) } as u64;
                assert!(start != 0);
                let mut ranges = Ranges::new();
                ranges.init(start, start + (pages * PAGE_SIZE) as u64);
                Mutex::new(Window { ranges, mapped: HashMap::new() })
            });
            Box::leak(Box::new(HostPlatform { nodes: nodes.max(1), direct_map, window }))
        }

        fn window(&self) -> Option<std::sync::MutexGuard<'_, Window>> {
            Some(self.window.as_ref()?.lock().unwrap())
        }

        /// Bytes of the mapping window free, and pages mapped in it.
        pub fn window_use(&self) -> (u64, usize) {
            let window = self.window().unwrap();
            (window.ranges.free_bytes(), window.mapped.len())
        }
    }

    // SAFETY: tests only give the allocators regions of host memory they
    // allocated and leaked, at `direct_map` + their physical address, or
    // regions they never touch through the direct map.
    #[allow(unsafe_code)]
    unsafe impl Platform for HostPlatform {
        fn current_cpu(&self) -> usize {
            CPU.with(Cell::get)
        }

        fn cpu_node(&self, cpu: usize) -> usize {
            cpu % self.nodes
        }

        fn disable_interrupts(&self) -> bool {
            false
        }

        fn restore_interrupts(&self, _were_enabled: bool) {}

        fn direct_map(&self) -> usize {
            self.direct_map
        }

        fn may_wait(&self) -> bool {
            MAY_WAIT.with(Cell::get)
        }

        fn reserve_area(&self, pages: usize) -> Option<usize> {
            let bytes = (pages as u64 + 1) * PAGE_SIZE as u64;
            let guard = self.window()?.ranges.alloc(bytes, PAGE_SIZE as u64)?;
            Some(guard as usize + PAGE_SIZE)
        }

        fn map_page(&self, virt: usize, phys: u64) -> Result<(), AllocError> {
            if MAPS_LEFT.with(|left| left.replace(left.get().saturating_sub(1))) == 0 {
                return Err(AllocError);
            }
            let mut window = self.window().ok_or(AllocError)?;
            assert!(window.mapped.insert(virt, phys).is_none(), "{virt:#x} is mapped already");
            Ok(())
        }

        fn area_frame(&self, virt: usize) -> Option<u64> {
            self.window()?.mapped.get(&(virt & !(PAGE_SIZE - 1))).copied()
        }

        fn release_area(
            &self,
            virt: usize,
            pages: usize,
            freed: &mut dyn FnMut(u64),
        ) -> Result<(), AreaError> {
            let mut window = self.window().ok_or(AreaError)?;
            let guard = virt.checked_sub(PAGE_SIZE).ok_or(AreaError)? as u64;
            let bytes = (pages as u64 + 1) * PAGE_SIZE as u64;
            window.ranges.check_taken(guard, bytes).map_err(|_| AreaError)?;
            for page in 0..pages {
                if let Some(phys) = window.mapped.remove(&(virt + page * PAGE_SIZE)) {
                    freed(phys);
                }
            }
            match window.ranges.release(guard, bytes) {
                Ok(()) | Err(RangeError::TooFragmented) => Ok(()),
                Err(err) => panic!("releasing {virt:#x}: {err:?}"),
            }
        }
    }

    /// `n` fresh frame records.
    pub fn records(n: usize) -> &'static [PageInfo] {
        Box::leak((0..n).map(|_| PageInfo::new()).collect::<Vec<_>>().into_boxed_slice())
    }

    /// Where the tests' "physical" memory starts: 4 GiB, so it is all in
    /// `Normal` zones.
    pub const PHYS_BASE: u64 = 1 << 32;

    /// A frame allocator and heap over `pages` frames of host memory, split
    /// evenly across `nodes` nodes.
    pub fn heap_with(pages: usize, nodes: usize) -> &'static Heap {
        build(pages, nodes, 0).0
    }

    /// A frame allocator and heap over `pages` frames of host memory, on
    /// one node, with a mapping window of `window` pages for the heap's
    /// areas, and the platform.
    pub fn heap_with_window(pages: usize, window: usize) -> (&'static Heap, &'static HostPlatform) {
        build(pages, 1, window)
    }

    fn build(pages: usize, nodes: usize, window: usize) -> (&'static Heap, &'static HostPlatform) {
        let layout = Layout::from_size_align(pages * PAGE_SIZE, 4 << 20).unwrap();
        // SAFETY: the layout has a non-zero size.
        #[allow(unsafe_code)]
        let host = unsafe { alloc_zeroed(layout) } as usize;
        assert!(host != 0);
        let frames: &'static Frames = Box::leak(Box::new(Frames::new()));
        let direct_map = host.wrapping_sub(PHYS_BASE as usize);
        let platform = HostPlatform::with_window(nodes, direct_map, window);
        frames.set_platform(platform);
        frames.set_topology(nodes, |a, b| if a == b { 10 } else { 20 });
        let per_node = pages / nodes;
        for node in 0..nodes {
            let base = (PHYS_BASE >> PAGE_SHIFT) + (node * per_node) as u64;
            frames.add_region(node, Region { base, pages: records(per_node) }).unwrap();
            frames.release(base << PAGE_SHIFT, (base + per_node as u64) << PAGE_SHIFT);
        }
        frames.start();
        (Box::leak(Box::new(Heap::new(frames))), platform)
    }
}
