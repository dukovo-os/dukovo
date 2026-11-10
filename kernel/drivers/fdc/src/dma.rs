// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Channel 2 of the PC's ISA DMA controller, an Intel 8237A or the
//! chipset's copy of one, which moves a floppy disk's data between the
//! controller and memory.
//!
//! The controller's registers for the channel are 16-bit, written a byte
//! at a time, low byte first, after a write to clear its flip-flop: the
//! address of the buffer's first byte, and its length less one. The top
//! eight bits of the 24-bit address are in a page register of its own.
//! The address counts up within its 64 KiB, so a buffer must not cross a
//! 64 KiB line, and the 24 bits reach the first 16 MiB.

use radix_hw::IoPorts;

use crate::Error;

/// The DMA controller's ports, from 0: those of the first four channels.
pub const PORTS: (u16, u16) = (0x00, 0x10);
/// The page registers' ports, from 0x80.
pub const PAGE_PORTS: (u16, u16) = (0x80, 0x10);
/// The channel a floppy disk controller uses.
pub const CHANNEL: u8 = 2;
/// The memory ISA DMA reaches: the first 16 MiB.
pub const REACH: u64 = 1 << 24;
/// A buffer may not cross a line of this many bytes.
pub const LINE: u64 = 1 << 16;

/// Channel 2's address and count registers, and the controller's.
const ADDRESS: u16 = 0x04;
const COUNT: u16 = 0x05;
/// Writes one channel's mask bit: the channel, and bit 2 to mask it.
const SINGLE_MASK: u16 = 0x0a;
const MASKED: u8 = 1 << 2;
const MODE: u16 = 0x0b;
/// Any write clears the flip-flop, so that the next byte is a low one.
const CLEAR_FLIP_FLOP: u16 = 0x0c;
/// Channel 2's page register, from 0x80.
const PAGE: u16 = 0x01;

/// The mode register: single transfers, the address counting up, no
/// auto-initialization, and the channel, with the direction: into
/// memory (a "write" in the 8237A's terms) or out of it.
const MODE_SINGLE: u8 = 0x40;
const INTO_MEMORY: u8 = 0x04;
const OUT_OF_MEMORY: u8 = 0x08;

/// Channel 2 of the ISA DMA controller.
pub(crate) struct IsaDma<P> {
    controller: P,
    pages: P,
}

impl<P: IoPorts> IsaDma<P> {
    pub(crate) fn new(controller: P, pages: P) -> IsaDma<P> {
        IsaDma { controller, pages }
    }

    /// Readies the channel to move `len` bytes at `buffer`, into memory
    /// or out of it; the floppy disk controller starts it with a command.
    /// Fails with [`Error::BadRequest`] for a buffer it cannot reach.
    pub(crate) fn start(&self, buffer: u64, len: usize, into_memory: bool) -> Result<(), Error> {
        let len = len as u64;
        if len == 0 || len > LINE || buffer + len > REACH || buffer % LINE + len > LINE {
            return Err(Error::BadRequest);
        }
        let count = len - 1;
        let direction = if into_memory { INTO_MEMORY } else { OUT_OF_MEMORY };
        let regs = &self.controller;
        regs.write8(SINGLE_MASK, MASKED | CHANNEL);
        regs.write8(CLEAR_FLIP_FLOP, 0);
        regs.write8(MODE, MODE_SINGLE | direction | CHANNEL);
        regs.write8(ADDRESS, buffer as u8);
        regs.write8(ADDRESS, (buffer >> 8) as u8);
        self.pages.write8(PAGE, (buffer >> 16) as u8);
        regs.write8(CLEAR_FLIP_FLOP, 0);
        regs.write8(COUNT, count as u8);
        regs.write8(COUNT, (count >> 8) as u8);
        regs.write8(SINGLE_MASK, CHANNEL);
        Ok(())
    }

    /// Masks the channel again: what is left of a transfer that failed
    /// goes nowhere.
    pub(crate) fn stop(&self) {
        self.controller.write8(SINGLE_MASK, MASKED | CHANNEL);
    }
}
