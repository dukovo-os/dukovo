// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The NVM of the Ethernet in the chipset: the GbE region of the
//! chipset's SPI flash, which the driver reads through the flash
//! registers, one cycle at a time (FreeBSD's e1000_read_nvm_ich8lan and
//! e1000_read_nvm_spt). On the 82579 they are 16-bit registers in a BAR
//! of their own, and the region is where GFPREG says; on the I219 they
//! are in BAR 0, at 0xe000, written 32 bits at a time, and the region
//! starts at 0 with the size STRAP says.
//!
//! The region holds two banks, one of them valid, as its signature says;
//! the driver reads only, so it never changes which.

use radix_hw::Mmio;
use radix_nic::wait_for;

use super::Mac;

/// The flash registers.
const GFPREG: usize = 0x00;
const HSFSTS: usize = 0x04;
const HSFCTL: usize = 0x06;
const FADDR: usize = 0x08;
const FDATA0: usize = 0x10;
/// How far they go.
pub const END: usize = FDATA0 + 4;

/// HSFSTS's bits: the cycle is done, it failed, a direct access error,
/// a cycle runs, and the flash descriptor is valid.
const FLCDONE: u16 = 1 << 0;
const FLCERR: u16 = 1 << 1;
const DAEL: u16 = 1 << 2;
const FLCINPROG: u16 = 1 << 5;
const FLDESVALID: u16 = 1 << 14;
/// HSFCTL's: go, a read cycle (0 in bits 1 and 2), and the bytes less
/// one.
const FLCGO: u16 = 1 << 0;
const FLCYCLE_MASK: u16 = 3 << 1;
const FLDBCOUNT_SHIFT: u16 = 8;
const FLDBCOUNT_MASK: u16 = 3 << FLDBCOUNT_SHIFT;

/// GFPREG's sectors, of 4 KiB.
const SECTOR_MASK: u32 = 0x1fff;
const SECTOR_SHIFT: u32 = 12;
/// The flash addresses the registers take.
const LINEAR_MASK: u32 = 0x00ff_ffff;

/// The word that holds a bank's signature, and its high byte's bits.
const SIGNATURE_WORD: u32 = 0x13;
const SIGNATURE_MASK: u16 = 0xc0 << 8;
const SIGNATURE: u16 = 0x80 << 8;

/// How long a cycle may take, and how often one that failed is tried.
const CYCLE_US: u32 = 500;
const TRIES: u32 = 10;

/// The region, and how to reach it.
pub struct Nvm {
    regs: Mmio,
    /// Whether its registers take only 32-bit writes, as the I219's.
    wide: bool,
    /// Where the region starts in the flash, in bytes.
    base: u32,
    /// How many words a bank holds.
    bank_words: u32,
}

impl Nvm {
    /// The NVM of a chip `mac` whose registers are `regs`, its flash
    /// registers being `flash` on the 82579; none if they are not there.
    pub fn new(mac: Mac, regs: &Mmio, flash: Option<Mmio>) -> Option<Nvm> {
        match mac {
            Mac::Pch2 => {
                let flash = flash.filter(|flash| flash.len() >= END)?;
                let sectors = flash.read32(GFPREG);
                let start = sectors & SECTOR_MASK;
                let end = ((sectors >> 16) & SECTOR_MASK) + 1;
                let bytes = end.checked_sub(start).filter(|&n| n > 0)? << SECTOR_SHIFT;
                Some(Nvm {
                    regs: flash,
                    wide: false,
                    base: start << SECTOR_SHIFT,
                    bank_words: bytes / 4,
                })
            }
            Mac::Spt => {
                let flash = regs.window(super::reg::FLASH, END)?;
                let size = (((regs.read32(super::reg::STRAP) >> 1) & 0x1f) + 1) * 4096;
                Some(Nvm { regs: flash, wide: true, base: 0, bank_words: size / 4 })
            }
        }
    }

    /// Where the valid bank starts, in words: the first whose signature
    /// says so, or the first if neither does or it cannot be read, as
    /// FreeBSD reads it then.
    pub fn bank(&self, delay: &mut dyn FnMut(u32)) -> u32 {
        for bank in [0, self.bank_words] {
            if let Some(word) = self.word(bank + SIGNATURE_WORD, delay)
                && word & SIGNATURE_MASK == SIGNATURE
            {
                return bank;
            }
        }
        0
    }

    /// The word `at` words into the region.
    pub fn word(&self, at: u32, delay: &mut dyn FnMut(u32)) -> Option<u16> {
        if self.wide {
            // Dwords only: the even word's, which holds this one.
            let dword = self.read(at & !1, 4, delay)?;
            Some(if at & 1 == 0 { dword as u16 } else { (dword >> 16) as u16 })
        } else {
            self.read(at, 2, delay).map(|data| data as u16)
        }
    }

    /// Reads `bytes` bytes from the word `at`, with a flash cycle tried
    /// up to ten times while it fails.
    fn read(&self, at: u32, bytes: u16, delay: &mut dyn FnMut(u32)) -> Option<u32> {
        let address = ((at << 1) & LINEAR_MASK).checked_add(self.base)?;
        for _ in 0..TRIES {
            delay(1);
            if !self.ready(delay) {
                return None;
            }
            let control = (self.control() & !(FLCYCLE_MASK | FLDBCOUNT_MASK))
                | ((bytes - 1) << FLDBCOUNT_SHIFT);
            self.set_control(control);
            self.regs.write32(FADDR, address);
            self.set_control(control | FLCGO);
            let mut status = 0;
            wait_for(delay, CYCLE_US, 1, || {
                status = self.status();
                status & FLCDONE != 0
            });
            if status & FLCDONE != 0 && status & FLCERR == 0 {
                return Some(self.regs.read32(FDATA0));
            }
            if status & FLCERR == 0 {
                // Not done, and no error: it will not be.
                return None;
            }
        }
        None
    }

    /// Readies the flash for a cycle: its descriptor valid, the last
    /// cycle's errors cleared and any cycle that runs ended, then done
    /// set, as FreeBSD's e1000_flash_cycle_init_ich8lan does.
    fn ready(&self, delay: &mut dyn FnMut(u32)) -> bool {
        let status = self.status();
        if status & FLDESVALID == 0 {
            return false;
        }
        // The error bits clear when written as ones.
        let status = status | FLCERR | DAEL;
        self.set_status(status);
        let idle = status & FLCINPROG == 0
            || wait_for(delay, CYCLE_US, 1, || self.status() & FLCINPROG == 0);
        if idle {
            self.set_status(status | FLCDONE);
        }
        idle
    }

    fn status(&self) -> u16 {
        if self.wide { self.regs.read32(HSFSTS) as u16 } else { self.regs.read16(HSFSTS) }
    }

    fn control(&self) -> u16 {
        if self.wide { (self.regs.read32(HSFSTS) >> 16) as u16 } else { self.regs.read16(HSFCTL) }
    }

    /// Writes HSFSTS; on the I219, with HSFCTL zero in the same dword.
    fn set_status(&self, status: u16) {
        if self.wide {
            self.regs.write32(HSFSTS, u32::from(status));
        } else {
            self.regs.write16(HSFSTS, status);
        }
    }

    /// Writes HSFCTL; on the I219, with HSFSTS zero in the same dword,
    /// which clears nothing.
    fn set_control(&self, control: u16) {
        if self.wide {
            self.regs.write32(HSFSTS, u32::from(control) << 16);
        } else {
            self.regs.write16(HSFCTL, control);
        }
    }
}
