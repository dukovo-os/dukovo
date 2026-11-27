// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A card a driver program holds (radix-device(2)), as e1000d(8) drives
//! Intel's network cards (ADR 0198): its registers, memory it reaches by
//! itself, and the link the kernel shares with the card's interface, each
//! mapped into the program by the kernel and reached only through what
//! checks each access.
//!
//! Each lasts for as long as the program holds the card's capability,
//! which a driver does until it ends: the kernel unmaps them when the
//! capability goes. A driver that deleted the capability while it used
//! them would reach memory no longer mapped, which this module cannot
//! stop; ADR 0730 keeps that gap, with the other calls of `radix_abi`
//! that pass the kernel an address.

use std::sync::atomic::{AtomicU32, Ordering};

use radix_abi::Status;
use radix_abi::device::{self, LINK_SIZE};
use radix_abi::errors::Kernel;
use radix_abi::startup::SLOT_SPACE;
use radix_hw::{Dma, Mmio};

use crate::kernel::SYS;

/// The registers of the first BAR of the card in `slot`, mapped
/// uncached where the kernel chooses.
///
/// # Errors
///
/// The kernel's status: `already_mapped` when they are mapped already,
/// `not_supported` for a card without such a BAR.
pub fn registers(slot: u32) -> Result<Mmio, Status> {
    let mapped = device::map_registers(&SYS, slot, SLOT_SPACE, 0, 0)?;
    let base = usize::try_from(mapped.at).map_err(|_| Kernel::InvalidArgument.status(0))?;
    let len = usize::try_from(mapped.size).map_err(|_| Kernel::InvalidArgument.status(0))?;
    // SAFETY: the kernel just mapped the card's first BAR, `len` bytes of
    // device memory, uncached, readable and writable at `base`, for as
    // long as the program holds the card (the module's note); nothing else
    // in the program knows the address, so nothing treats it as memory.
    Ok(unsafe { Mmio::new(base, len) })
}

/// DMA memory goes back with the card, when its capability goes: nothing
/// to do when a driver drops it.
fn keep(_virt: usize, _phys: u64, _len: usize, _tag: u32) {}

/// `pages` pages of zeroed memory the card in `slot` reaches by itself,
/// which follow one another at the address it reaches them at.
///
/// # Errors
///
/// The kernel's status: `exhausted` beyond the card's DMA_PAGES_MAX,
/// `no_memory`.
pub fn dma(slot: u32, pages: u32) -> Result<Dma, Status> {
    let given = device::map_dma(&SYS, slot, SLOT_SPACE, 0, pages)?;
    let virt = usize::try_from(given.at).map_err(|_| Kernel::InvalidArgument.status(0))?;
    let len = usize::try_from(pages)
        .ok()
        .and_then(|pages| pages.checked_mul(4096))
        .ok_or_else(|| Kernel::InvalidArgument.status(0))?;
    // SAFETY: the kernel just mapped `pages` pages of zeroed memory at
    // `virt`, which follow one another at `given.device_address` where
    // the card reaches them, for this card alone, until its capability
    // goes; `keep` gives nothing back before then, and nothing else in the
    // program knows the address.
    Ok(unsafe { Dma::new(virt, given.device_address, len, 0, keep) })
}

/// The link of a card's network interface: LINK_SIZE bytes the kernel's
/// stack and the driver share, which `attach_network` mapped. Its header
/// words are reached atomically, as the kernel reaches them; its frames'
/// slots by copies, at offsets checked against its size.
pub struct Link {
    base: usize,
}

/// The size of a link, which fits a usize.
const LINK_BYTES: usize = LINK_SIZE as usize;

/// The header's size, whose words the two sides reach atomically: the
/// link's first page.
const HEADER_BYTES: usize = 4096;

/// Attaches the card in `slot` to its network interface, which comes up
/// with MAC address `mac`, and maps their link. Returns it and the
/// interface's index.
///
/// # Errors
///
/// The kernel's status: `already` once it is attached, `not_supported`
/// for a card that is no network card.
pub fn attach_network(slot: u32, mac: &[u8; 6]) -> Result<(Link, u32), Status> {
    let attached = device::attach_network(&SYS, slot, SLOT_SPACE, 0, mac)?;
    let base = usize::try_from(attached.at).map_err(|_| Kernel::InvalidArgument.status(0))?;
    Ok((Link { base }, attached.interface))
}

impl Link {
    /// The header's word at `offset`.
    ///
    /// # Panics
    ///
    /// For an offset that is not 4-aligned or is past the header: a bug
    /// in the driver.
    fn word(&self, offset: usize) -> &AtomicU32 {
        assert!(offset.is_multiple_of(4) && offset + 4 <= HEADER_BYTES);
        // SAFETY: `attach_network` mapped the link readable and writable
        // at `base` for as long as the program holds the card; the word is
        // in its header page, 4-aligned, and the kernel reaches the
        // header's words only atomically too.
        unsafe { AtomicU32::from_ptr((self.base + offset) as *mut u32) }
    }

    /// Checks that `len` bytes from `offset` are in the link.
    fn check(offset: usize, len: usize) {
        assert!(offset.checked_add(len).is_some_and(|end| end <= LINK_BYTES));
    }

    /// The header's word at `offset`, with acquire ordering: what the
    /// kernel wrote before it stored it is seen after.
    ///
    /// # Panics
    ///
    /// As `word`'s.
    #[must_use]
    pub fn load(&self, offset: usize) -> u32 {
        self.word(offset).load(Ordering::Acquire)
    }

    /// Stores the header's word at `offset`, with release ordering: what
    /// the driver wrote before is seen by the kernel after it loads it.
    ///
    /// # Panics
    ///
    /// As `word`'s.
    pub fn store(&self, offset: usize, value: u32) {
        self.word(offset).store(value, Ordering::Release);
    }

    /// Copies the bytes from `offset` into `out`.
    ///
    /// # Panics
    ///
    /// For bytes that are not all in the link: a bug in the driver.
    pub fn read(&self, offset: usize, out: &mut [u8]) {
        Self::check(offset, out.len());
        // SAFETY: the bytes are in the mapping (checked); by the link's
        // rules the kernel does not write a slot while the driver owns it,
        // and the acquire load of its index ordered its writes before.
        unsafe {
            std::ptr::copy_nonoverlapping(
                (self.base + offset) as *const u8,
                out.as_mut_ptr(),
                out.len(),
            );
        }
    }

    /// Copies `data` into the link from `offset`.
    ///
    /// # Panics
    ///
    /// As [`Link::read`]'s.
    pub fn write(&self, offset: usize, data: &[u8]) {
        Self::check(offset, data.len());
        // SAFETY: as in `read`; the kernel reads the slot only after the
        // release store of the driver's index that follows.
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                (self.base + offset) as *mut u8,
                data.len(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{attach_network, dma, registers};

    #[test]
    fn without_a_card_the_kernel_says_why() {
        // The build machine has no Radix kernel, which these calls need.
        assert!(registers(64).is_err());
        assert!(dma(64, 1).is_err());
        assert!(attach_network(64, &[2, 0, 0, 0, 0, 1]).is_err());
    }
}
