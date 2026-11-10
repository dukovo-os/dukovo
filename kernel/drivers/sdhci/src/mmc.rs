// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! MultiMediaCards and eMMCs (ADR 0439), as JEDEC's *Embedded
//! MultiMediaCard (e•MMC) Electrical Standard 5.1* (JESD84-B51) describes
//! them: a card in an SD host controller's slot that does not answer
//! ACMD41 is set up here, and an eMMC soldered to the board is such a
//! card in a slot that is not removable.
//!
//! Start at [`Slot::set_up_mmc`], which follows section 6.4.4 of the
//! standard: CMD0, CMD1 until the device is ready (one over 2 GB says it
//! is addressed in sectors), CMD2 for its CID, CMD3 to give it address 1,
//! CMD9 for its CSD, CMD7 to select it, then CMD8 for its EXT_CSD, the
//! 512 bytes that give its capacity and speeds from version 4 on.
//! [`Slot::mmc_width`] makes its bus as wide as both it and the slot are,
//! reading the EXT_CSD back at each width to see that every line is
//! there, as a slot whose upper four lines go nowhere garbles it. Then
//! [`Slot::mmc_speed_up`] runs it at HS200, at 1.8 V and tuned with
//! CMD21, where the device, the slot and the bus allow it, or at High
//! Speed's 52 MHz. CMD6 here writes a byte of the EXT_CSD, and CMD13 says
//! whether the device took it.
//!
//! The device's cache is left off, as it is at power on, so a write is
//! done when it ends; its boot partitions and RPMB are left alone, and
//! only its user area is a disk.

use super::card::{self, Card, Cid, Family, Speed};
use super::{BLOCK, Error, Response, Slot, VOLTAGES, caps, host, host2, ocr, reg, version};

/// The EXT_CSD's bytes that the driver reads or writes (JESD84-B51, 7.4).
mod ext {
    /// The bus's width: 0 one bit, 1 four, 2 eight.
    pub const BUS_WIDTH: u8 = 183;
    /// The timing: 0 the default's, 1 High Speed, 2 HS200.
    pub const HS_TIMING: u8 = 185;
    /// The first of the read-only properties: what is read back after
    /// the bus's width changes.
    pub const PROPERTIES: usize = 192;
    /// The EXT_CSD's revision.
    pub const REV: usize = 192;
    /// The speeds the device has ([`super::kinds`]).
    pub const CARD_TYPE: usize = 196;
    /// Its capacity in 512-byte sectors, little-endian, for a device
    /// addressed in sectors.
    pub const SEC_COUNT: usize = 212;
}

/// CARD_TYPE's bits.
mod kinds {
    /// High Speed at up to 52 MHz.
    pub const HS_52: u8 = 1 << 1;
    /// HS200 with its I/O at 1.8 V.
    pub const HS200_1_8: u8 = 1 << 4;
}

/// The card status's bit that says the last CMD6 was refused.
const SWITCH_ERROR: u32 = 1 << 7;
/// The address the device is given (CMD3): an MMC's is the host's choice.
const RCA: u16 = 1;

/// An MMC's tuning block on a bus of `width` bits: CMD21's, 128 bytes on
/// eight bits and 64 on four (JESD84-B51, 6.6.5.1).
pub(super) fn tuning(width: u8) -> (u16, u16) {
    (21, if width == 8 { 128 } else { 64 })
}

impl Slot {
    /// Sets the card up as an MMC, powered on and having had CMD0, CMD8
    /// and ACMD41 go unanswered.
    pub(super) fn set_up_mmc(&mut self, delay: &mut dyn FnMut(u32)) -> Result<Card, Error> {
        self.command(0, 0, Response::None, delay)?;
        // Sector addressing is asked for; a device of 2 GB or less says
        // it is addressed in bytes.
        let asked = VOLTAGES | ocr::MMC_1_8 | ocr::HIGH_CAPACITY;
        let mut answer = 0;
        for _ in 0..100 {
            answer = match self.command(1, asked, Response::R3, delay) {
                Ok(long) => long[0],
                Err(Error::Command(_) | Error::Timeout) => {
                    return Err(Error::Unsupported("the card answers neither ACMD41 nor CMD1"));
                }
                Err(err) => return Err(err),
            };
            if answer & ocr::READY != 0 {
                break;
            }
            delay(10_000);
        }
        if answer & ocr::READY == 0 {
            return Err(Error::Timeout);
        }
        let sectors = answer & ocr::HIGH_CAPACITY != 0;
        let cid = self.command(2, 0, Response::R2, delay)?;
        self.command(3, u32::from(RCA) << 16, Response::R1, delay)?;
        let csd = self.command(9, u32::from(RCA) << 16, Response::R2, delay)?;
        let (spec, csd_blocks) = card::Csd::parse_mmc(&csd);
        self.command(7, u32::from(RCA) << 16, Response::R1b, delay)?;
        let family = if self.removable() { Family::Mmc } else { Family::Emmc };
        let mut khz = self.set_clock(Speed::Default.khz(), delay)?;
        let mut card = Card {
            family,
            rca: RCA,
            high_capacity: sectors,
            blocks: csd_blocks,
            cid: Cid::parse_mmc(&cid, 0),
            speed: Speed::Default,
            khz,
            width: 1,
            read_only: false,
        };
        if spec < 4 {
            // Before version 4, no EXT_CSD and no CMD6: one bit, at up to
            // 20 MHz.
            if sectors {
                return Err(Error::Unsupported("an MMC addressed in sectors has no EXT_CSD"));
            }
            khz = self.set_clock(20_000, delay)?;
            self.command(16, BLOCK, Response::R1, delay)?;
            card.khz = khz;
            self.card = Some(card);
            return Ok(card);
        }
        let mut ext = [0u8; 512];
        self.read_block(None, 8, 0, &mut ext, delay)?;
        card.cid = Cid::parse_mmc(&cid, ext[ext::REV]);
        if sectors {
            let count = ext[ext::SEC_COUNT..ext::SEC_COUNT + 4].try_into();
            card.blocks = u64::from(u32::from_le_bytes(count.unwrap_or_default()));
        } else {
            self.command(16, BLOCK, Response::R1, delay)?;
        }
        card.width = self.mmc_width(&ext, delay)?;
        (card.speed, card.khz) =
            self.mmc_speed_up(&ext, card.width, delay)?.unwrap_or((Speed::Default, khz));
        self.card = Some(card);
        Ok(card)
    }

    /// Writes `value` to the EXT_CSD's byte `index` (CMD6's Write Byte):
    /// true if the device took it, as CMD13's status says once it is no
    /// longer busy.
    fn mmc_switch(
        &mut self,
        index: u8,
        value: u8,
        delay: &mut dyn FnMut(u32),
    ) -> Result<bool, Error> {
        let argument = 3 << 24 | u32::from(index) << 16 | u32::from(value) << 8;
        match self.command(6, argument, Response::R1b, delay) {
            Ok(_) => {}
            Err(Error::Card(_)) => return Ok(false),
            Err(err) => return Err(err),
        }
        let status = self.command(13, u32::from(RCA) << 16, Response::R1, delay)?[0];
        Ok(status & SWITCH_ERROR == 0)
    }

    /// Sets the host's bus width to `bits`: 1, 4 or 8.
    fn host_width(&mut self, bits: u8) {
        let control = self.regs.read8(reg::HOST_CONTROL) & !(host::WIDTH_4 | host::WIDTH_8);
        let width = match bits {
            8 => host::WIDTH_8,
            4 => host::WIDTH_4,
            _ => 0,
        };
        self.regs.write8(reg::HOST_CONTROL, control | width);
    }

    /// Makes the bus as wide as the device and the slot can, eight bits
    /// where the slot has them and then four, the EXT_CSD read back at
    /// each to see that it comes in whole (`ext`, as read on one bit):
    /// the width it runs at.
    fn mmc_width(&mut self, ext: &[u8; 512], delay: &mut dyn FnMut(u32)) -> Result<u8, Error> {
        for (bits, value) in [(8, 2), (4, 1)] {
            if bits == 8 && self.caps & caps::BUS_8 == 0 {
                continue;
            }
            if !self.mmc_switch(ext::BUS_WIDTH, value, delay)? {
                continue;
            }
            self.host_width(bits);
            let mut again = [0u8; 512];
            let read = self.read_block(None, 8, 0, &mut again, delay);
            if read.is_ok() && again[ext::PROPERTIES..] == ext[ext::PROPERTIES..] {
                return Ok(bits);
            }
        }
        self.mmc_switch(ext::BUS_WIDTH, 0, delay)?;
        self.host_width(1);
        Ok(1)
    }

    /// Runs the device as fast as it and the slot can: HS200 where the
    /// device has it with its I/O at 1.8 V, the controller is of version
    /// 3.00 or later with 1.8 V and SDR104, and the bus is four or eight
    /// bits wide; else High Speed's 52 MHz where both have it. The mode
    /// and the clock it runs at, or `None` to stay.
    fn mmc_speed_up(
        &mut self,
        ext: &[u8; 512],
        width: u8,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Option<(Speed, u32)>, Error> {
        let kinds = ext[ext::CARD_TYPE];
        let hs200 = kinds & kinds::HS200_1_8 != 0
            && self.version >= version::V3
            && self.caps & caps::VOLTS_1_8 != 0
            && self.caps & caps::SDR104 != 0
            && width > 1;
        if hs200 && let Some(khz) = self.hs200(width, delay)? {
            return Ok(Some((Speed::Hs200, khz)));
        }
        if kinds & kinds::HS_52 == 0 || self.caps & caps::HIGH_SPEED == 0 {
            return Ok(None);
        }
        self.high_speed(delay)
    }

    /// Switches the device to HS200: its I/O to 1.8 V, HS_TIMING 2, the
    /// slot to SDR104's timing at up to 200 MHz, and tuned. The clock it
    /// runs at, or `None` with the slot set back to the default speed's
    /// timing if a step did not happen. A device with HS200 at 1.8 V
    /// takes its I/O at 1.8 V at every speed, so that stays.
    fn hs200(&mut self, width: u8, delay: &mut dyn FnMut(u32)) -> Result<Option<u32>, Error> {
        let control2 = self.regs.read16(reg::HOST_CONTROL2);
        self.regs.write16(reg::HOST_CONTROL2, control2 | host2::SIGNALING_1_8);
        delay(5_000);
        // The controller clears it again if its regulator did not switch.
        let switched = self.regs.read16(reg::HOST_CONTROL2) & host2::SIGNALING_1_8 != 0;
        if switched && self.mmc_switch(ext::HS_TIMING, 2, delay)? {
            let khz = self.uhs_mode(Speed::Hs200, delay)?;
            if self.tune(tuning(width), delay) {
                self.tuned = true;
                return Ok(Some(khz));
            }
        }
        self.uhs_mode(Speed::Default, delay)?;
        Ok(None)
    }

    /// Switches the device to High Speed (HS_TIMING 1) and the slot with
    /// it: the mode and clock it runs at, or `None` if the device would
    /// not.
    fn high_speed(&mut self, delay: &mut dyn FnMut(u32)) -> Result<Option<(Speed, u32)>, Error> {
        if !self.mmc_switch(ext::HS_TIMING, 1, delay)? {
            return Ok(None);
        }
        let control = self.regs.read8(reg::HOST_CONTROL);
        self.regs.write8(reg::HOST_CONTROL, control | host::HIGH_SPEED);
        let khz = self.set_clock(Speed::MmcHigh.khz(), delay)?;
        Ok(Some((Speed::MmcHigh, khz)))
    }

    /// After HS200 no longer tunes: the slot back to the default speed's
    /// timing, and the device to High Speed, or to the default speed if it
    /// will not. The mode and clock it then runs at.
    pub(super) fn mmc_fall_back(
        &mut self,
        delay: &mut dyn FnMut(u32),
    ) -> Result<(Speed, u32), Error> {
        let khz = self.uhs_mode(Speed::Default, delay)?;
        match self.high_speed(delay)? {
            Some(speed) => Ok(speed),
            None => {
                self.mmc_switch(ext::HS_TIMING, 0, delay)?;
                Ok((Speed::Default, khz))
            }
        }
    }
}
