// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The vDSO and the clock page (ADR 0613): what `exec` maps into every
//! program so that it reads the clocks without a system call.
//!
//! At boot, once the clock is calibrated, the vDSO's image
//! (`radix_vdso::IMAGE`) is copied into frames of its own, and the clock
//! page the architecture keeps (`Timer::clock_page`, the wall clock's
//! `radix_hal::clock::ClockPage`) gets a frame object too. Both are kept
//! for good: nothing names them with a capability, so they are never
//! closed, zeroed or freed. `exec` maps the clock page read-only and the
//! vDSO read and execute right above it, where anonymous memory goes, and
//! gives the vDSO's address as AT_SYSINFO_EHDR; `fork` shares both, as it
//! shares every frame. A program may unmap them, which only costs it its
//! fast clocks, and may not make them writable: protection changes are
//! for anonymous memory only.
//!
//! Where there is no image (other architectures) or programs may not read
//! the clock's counter, nothing is mapped and no AT_SYSINFO_EHDR given: C
//! libraries then make system calls for the time, as before. Programs the
//! kernel starts itself (`super::loader`) get no vDSO either, as they get
//! no auxiliary vector.

use radix_abi::address_space::PAGE_SIZE;
use radix_hal::{Timer, UserMode};
use radix_klog::{info, warn};
use radix_mm::{FRAMES, Flags, Owner};
use radix_sync::Once;

use super::Result;
use super::memory::{FrameObject, FrameRef, SpaceObject};
use crate::Arch;

/// The clock page's frame and the vDSO's.
struct Pages {
    clock: FrameRef,
    image: FrameRef,
}

static PAGES: Once<Pages> = Once::new();

/// Makes the vDSO's frames, if programs are to have one. Called once at
/// boot, after the clock is calibrated and before any program runs.
pub fn init() {
    let image = radix_vdso::IMAGE;
    if image.is_empty() {
        return;
    }
    let Some(clock) = <Arch as Timer>::clock_page() else {
        info!("vDSO: none, as programs may not read the clock's counter");
        return;
    };
    let pages = (image.len() as u64).div_ceil(PAGE_SIZE);
    let order = pages.next_power_of_two().trailing_zeros();
    let flags = Flags { zero: true, ..Flags::MAY_SLEEP };
    let Ok(phys) = FRAMES.alloc(order, flags, Owner::Kernel) else {
        warn!("vDSO: none, as there was no memory for it");
        return;
    };
    Arch::write_frames(phys, image);
    let made = FrameObject::kept(clock, 1).zip(FrameObject::kept(phys, pages));
    let Some((clock, image)) = made else {
        warn!("vDSO: none, as there was no memory for it");
        return;
    };
    let _ = PAGES.set(Pages { clock, image });
    info!("vDSO: {pages} page(s), above the clock page");
}

/// Maps the clock page and the vDSO into `space`, a new space no thread
/// runs in: the vDSO's address, for AT_SYSINFO_EHDR, or `None` if there
/// is no vDSO.
///
/// # Errors
///
/// no_memory if the space has no room or no memory for them.
pub fn map(space: &SpaceObject) -> Result<Option<u64>> {
    let Some(pages) = PAGES.get() else { return Ok(None) };
    space.map_vdso(&pages.clock, &pages.image).map(Some)
}
