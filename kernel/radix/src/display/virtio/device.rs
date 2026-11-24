// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A virtio graphics card's device, and the pictures made on it.
//!
//! Every picture a screen shows is a *resource* of the host's, known by
//! a number the driver gives it. A picture whose pixels a program draws
//! is given the program's own pages as its backing: the device is told
//! each run of memory they lie in, and reaches them behind an IOMMU for
//! as long as the picture lives. A picture the card keeps has the text
//! band as its backing, or a store of its own on a card with none.

use alloc::boxed::Box;
use alloc::vec::Vec;

use radix_display::state::PictureShape;
use radix_hal::Devices;
use radix_hw::Dma;
use radix_hw::pci::Address;
use radix_klog::warn;
use radix_mm::Kmalloc;
use radix_virtio::Error;
use radix_virtio::gpu::{self, Band, ENTRY_SIZE, Gpu, SCANOUTS_MAX, Scanout, format};

use super::super::{PAGE, Pages};
use super::room;
use super::shown::Shown;
use crate::Arch;

/// The most the stores of kept pictures take on a card with no band.
const STORES_MOST: usize = 256 << 20;

/// The device and what the card keeps of it.
pub(super) struct Device {
    /// On the heap: its queues' lists are too large to be passed about.
    pub(super) gpu: Box<Gpu, Kmalloc>,
    /// The card's PCI function, which its memory is for.
    pub(super) source: Address,
    /// Its interrupt vectors, to give back.
    pub(super) vectors: [Option<u32>; 2],
    /// The text band, or `None` where each kept picture has a store.
    pub(super) band: Option<Band>,
    /// The store of each kept picture on a card with no band, by its
    /// resource, and the bytes of them all.
    pub(super) stores: Vec<(u32, Dma), Kmalloc>,
    pub(super) stored: usize,
    /// The number the next resource gets.
    pub(super) next: u32,
    pub(super) shown: Shown,
    /// What the device last said of each screen.
    pub(super) found: [Scanout; SCANOUTS_MAX],
    /// The commands the device refused that nobody waited for, as last
    /// logged.
    pub(super) lost: u32,
}

impl Drop for Device {
    fn drop(&mut self) {
        // The card first, so that it raises nothing after its vectors go.
        self.gpu.reset();
        for vector in self.vectors.into_iter().flatten() {
            <Arch as Devices>::free_interrupt(vector);
        }
    }
}

impl Device {
    /// The number for a new resource. They are not used again: a card
    /// that has made four thousand million pictures makes no more.
    pub(super) fn number(&mut self) -> Result<u32, Error> {
        let number = self.next;
        self.next = number.checked_add(1).ok_or(Error::NoMemory)?;
        Ok(number)
    }

    /// Has the host let go of `resource`, and waits for it to have.
    /// Nothing for resource 0, which is none.
    pub(super) fn forget(&mut self, resource: u32, wait: &mut dyn FnMut() -> bool) {
        if resource == 0 {
            return;
        }
        for shows in &mut self.shown.screens {
            // The host shows nothing where a resource that goes showed.
            if shows.is_some_and(|shows| shows.resource == resource) {
                *shows = None;
            }
        }
        // A device that does not answer was reset, and has no resources.
        let gpu = &mut self.gpu;
        let _ = room(gpu, wait).and_then(|()| gpu.unref(resource)).and_then(|()| gpu.finish(wait));
    }

    /// Makes `resource` of a picture whose pixels are in `pages`: as
    /// wide as the picture's rows are long, so that a row of the one is
    /// a row of the other, with the pages as its backing. On a card
    /// that shares pages the resource is those pages and nothing more.
    pub(super) fn make_backed(
        &mut self,
        resource: u32,
        (shape, row_bytes): (&PictureShape, u32),
        pages: &Pages,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let bytes =
            radix_display::picture_bytes(shape.format, shape.width, shape.height, row_bytes)
                .and_then(|bytes| usize::try_from(bytes).ok())
                .ok_or(Error::BadRequest)?;
        let mut runs: u32 = 0;
        pages.runs(bytes, &mut |_, _| runs += 1);
        // The table of the runs is the device's until it has answered.
        let table_pages = (runs as usize * ENTRY_SIZE).div_ceil(PAGE).max(1);
        let table =
            <Arch as Devices>::alloc_dma(self.source, table_pages, false).ok_or(Error::NoMemory)?;
        let mut at = 0;
        pages.runs(bytes, &mut |phys, len| {
            table.write(at, &gpu::entry(phys, len));
            at += ENTRY_SIZE;
        });
        if !reach(self.source, pages, bytes) {
            return Err(Error::NoMemory);
        }
        let gpu = &mut self.gpu;
        // With nothing in flight there is room for both commands.
        let queued = gpu.finish(wait).and_then(|()| {
            if gpu.shares_pages() {
                gpu.create_shared(resource, bytes as u64, table.phys(), runs)
            } else {
                gpu.create(resource, format::BGRX, row_bytes / 4, shape.height)
                    .and_then(|()| gpu.attach_table(resource, table.phys(), runs))
            }
        });
        let made = queued.and_then(|()| gpu.finish(wait));
        if made.is_err() {
            self.forget(resource, wait);
            unreach(self.source, pages, bytes, u32::MAX);
        }
        made
    }

    /// Makes `resource` of a picture the card keeps: its backing is the
    /// text band, or on a card with none a store of its own.
    pub(super) fn make_kept(
        &mut self,
        resource: u32,
        shape: &PictureShape,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let backing = match &self.band {
            Some(band) => band.backing(),
            None => {
                let bytes = shape.width as usize * shape.height as usize * 4;
                if self.stored + bytes > STORES_MOST || self.stores.try_reserve(1).is_err() {
                    return Err(Error::NoMemory);
                }
                let store = <Arch as Devices>::alloc_dma(self.source, bytes.div_ceil(PAGE), false)
                    .ok_or(Error::NoMemory)?;
                let backing = (store.phys(), store.len() as u32);
                self.stored += store.len();
                // Room was reserved.
                let _ = self.stores.push_within_capacity((resource, store));
                backing
            }
        };
        let made = self.make(resource, (shape.width, shape.height), backing, wait);
        if made.is_err() {
            self.forget(resource, wait);
            self.drop_store(resource);
        }
        made
    }

    /// Makes `resource` a picture of `size`, wide and high, whose rows
    /// are copied to the host from `backing`, an address and a length.
    /// It waits for the device to say it has.
    pub(super) fn make(
        &mut self,
        resource: u32,
        size: (u32, u32),
        backing: (u64, u32),
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let gpu = &mut self.gpu;
        // With nothing in flight there is room for both commands.
        gpu.finish(wait)?;
        gpu.create(resource, format::BGRX, size.0, size.1)?;
        gpu.attach(resource, backing.0, backing.1)?;
        gpu.finish(wait)
    }

    /// Lets go of the store of the kept picture `resource`, if it has
    /// one.
    pub(super) fn drop_store(&mut self, resource: u32) {
        if let Some(at) = self.stores.iter().position(|(of, _)| *of == resource) {
            self.stored -= self.stores.swap_remove(at).1.len();
        }
    }

    /// Says in the log how many commands the device refused that nobody
    /// waited for, if more were since it last said.
    pub(super) fn say_lost(&mut self, card: u32) {
        let lost = self.gpu.lost();
        if lost != self.lost {
            warn!("Display: card {card} refused {} commands", lost.wrapping_sub(self.lost));
            self.lost = lost;
        }
    }
}

/// Lets the card behind `source` reach each run of memory a picture's
/// first `bytes` bytes are in, behind an IOMMU. Returns whether it
/// reaches them all: if not, it reaches none.
fn reach(source: Address, pages: &Pages, bytes: usize) -> bool {
    let (mut done, mut failed) = (0, false);
    pages.runs(bytes, &mut |phys, len| {
        if !failed && <Arch as Devices>::map_dma(source, &[(phys, len)]) {
            done += 1;
        } else {
            failed = true;
        }
    });
    if failed {
        unreach(source, pages, bytes, done);
    }
    !failed
}

/// Takes back the first `count` runs of a picture's memory that
/// [`reach`] gave the card.
pub(super) fn unreach(source: Address, pages: &Pages, bytes: usize, count: u32) {
    let mut left = count;
    pages.runs(bytes, &mut |phys, len| {
        if left > 0 {
            left -= 1;
            <Arch as Devices>::unmap_dma(source, &[(phys, len)]);
        }
    });
}
