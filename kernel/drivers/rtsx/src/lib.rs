// SPDX-FileCopyrightText: 2006 Uwe Stuehler <uwe@openbsd.org>
// SPDX-FileCopyrightText: 2012 Stefan Sperling <stsp@openbsd.org>
// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: ISC
//
// Permission to use, copy, modify, and distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! Realtek's PCI Express card readers for the Radix kernel (ADR 0448):
//! the RTS5209, RTS5227, RTS5229, RTS522A, RTS5249 and RTS525A, and the
//! card readers of the RTL8402, RTL8411 and RTL8411B, which many laptops
//! have in place of an SD host controller. They follow no specification
//! of the SD Association's: [`Chip`] names them by their PCI device, and
//! the kernel finds them by it.
//!
//! Realtek publishes no datasheet. What the driver writes, and in what
//! order, is translated from OpenBSD's rtsx(4) (`sys/dev/ic/rtsx.c`
//! revision 1.22 and `rtsxreg.h`, by Uwe Stuehler and Stefan Sperling,
//! under the ISC licence above), which NetBSD's driver shares; FreeBSD's
//! draws on Linux's GPL driver and was not used. The chips OpenBSD drives are the chips
//! driven here, at the speeds it runs them: an SD card at 25 MHz, or at
//! High Speed's 50 MHz, on the four-bit bus. UHS-I, MMCs and Memory Stick
//! are left out, as they are there.
//!
//! Start at [`Reader`]. The reader has a few registers of its own, in its
//! first BAR (the RTS525A's second): two pairs that point it at a buffer of
//! commands and at a buffer of data and start them, one that reaches the
//! chip's internal registers a byte at a time, and two of interrupts, which
//! also say whether a card is in. Everything else is an internal register
//! ([`chip`]): [`Reader::new`] sets the chip up through them one at a time,
//! and an SD command is a [`Batch`] of internal register writes, a check
//! that the card answered, and reads of its answer, that the reader runs
//! from the command buffer by itself, writing what it read back into the
//! buffer. A card's data moves by the reader's scatter-gather DMA, from
//! descriptors in memory below 4 GiB, which is all it reaches.
//!
//! [`Reader::identify`] powers the card up and sets it up as the *SD
//! Specifications Part 1, Physical Layer Simplified Specification*
//! (version 9.10), section 4.2, says, as `radix_sdhci` does for an SD host
//! controller; the [`Card`] it gives is that crate's. [`Reader::start`]
//! starts a read or a write of whole blocks and [`Reader::poll`] moves it
//! along, each step of it a batch the reader runs by itself: the command,
//! the data, CMD12 after several blocks, and after a write CMD13 until
//! the card has written them. [`Detect`] is the reader's card detect,
//! which the thread watching it reads apart from the transfers.
//!
//! The crate is a compartment with no `unsafe` code; its tests run it
//! against a simulated reader and card.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use radix_hw::{Dma, Mmio, device_barrier};
use radix_sdhci::card::Long;
use radix_sdhci::{BLOCK, CARD_ERRORS, Card, Cid, Csd, Error, Family, NO_CHANGE, Op, Speed};
use radix_sdhci::{VOLTAGES, failed, ocr};

pub use radix_sdhci::{Events, Presence};

/// Realtek's PCI vendor ID.
pub const VENDOR: u16 = 0x10ec;
/// The PCI class the card readers say they are of: none (OpenBSD's
/// rtsx(4) takes only these). An RTS5209 that says it is an SD host
/// controller (class 08h, subclass 05h) is driven as one.
pub const PCI_CLASS: u8 = 0xff;
/// The PCI configuration register whose [`FPGA`] bit says the reader is
/// a development board's FPGA, not the chip, which the driver refuses.
pub const CONFIG: u16 = 0x1c;
/// [`CONFIG`]'s bit for an FPGA.
pub const FPGA: u32 = 1 << 4;
/// The bytes of the reader's own registers the driver reaches.
pub const REGISTERS: usize = 0x20;
/// The most buffers a request takes: the descriptors that fit their part
/// of the reader's page.
pub const MAX_BUFFERS: usize = (PAGE - DESCRIPTORS) / 8;
/// The longest buffer a descriptor takes.
pub const MAX_BUFFER: u32 = 1 << 19;

/// The reader's own registers, 32 bits each.
mod reg {
    /// Where the command buffer is.
    pub const COMMANDS: usize = 0x00;
    /// Runs the command buffer: its bytes, and [`super::START_COMMANDS`].
    pub const COMMAND_CONTROL: usize = 0x04;
    /// Where the data buffer, or the descriptors, are.
    pub const DATA: usize = 0x08;
    /// Moves the data: [`super::dma`]'s bits.
    pub const DATA_CONTROL: usize = 0x0c;
    /// An internal register, read or written through it.
    pub const INTERNAL: usize = 0x10;
    /// What happened, cleared by writing 1s ([`super::pending`]).
    pub const PENDING: usize = 0x14;
    /// Which of those interrupt.
    pub const ENABLED: usize = 0x18;
}

/// [`reg::INTERNAL`]'s bits: busy until the access is done, and a write
/// rather than a read. The register's address is in bits 16 to 29, a
/// write's mask in bits 8 to 15, and the byte in bits 0 to 7.
const INTERNAL_BUSY: u32 = 1 << 31;
const INTERNAL_WRITE: u32 = 1 << 30;
/// [`reg::COMMAND_CONTROL`]'s bits: run the buffer, and write what its
/// reads read back into it; stop it.
const START_COMMANDS: u32 = 1 << 31;
const AUTO_RESPONSE: u32 = 1 << 30;
const STOP_COMMANDS: u32 = 1 << 28;

/// [`reg::DATA_CONTROL`]'s bits.
mod dma {
    /// Start the transfer, of the bytes in bits 0 to 23.
    pub const START: u32 = 1 << 31;
    /// From the card to memory.
    pub const READ: u32 = 1 << 29;
    pub const STOP: u32 = 1 << 28;
    /// The address is of descriptors (scatter-gather), not of the data.
    pub const DESCRIBED: u32 = 2 << 26;
}

/// [`reg::PENDING`]'s and [`reg::ENABLED`]'s bits.
mod pending {
    /// The command buffer ran to its end, or failed.
    pub const DONE: u32 = 1 << 29;
    pub const FAILED: u32 = 1 << 28;
    /// A card went in or came out of the SD slot.
    pub const SD_CHANGED: u32 = 1 << 25;
    /// The card in the SD slot has its write-protect switch set.
    pub const WRITE_PROTECT: u32 = 1 << 19;
    /// A card is in the SD slot.
    pub const SD_IN: u32 = 1 << 16;
    /// What interrupts: a batch's end and a card's coming and going.
    pub const WATCHED: u32 = DONE | FAILED | SD_CHANGED;
}

/// The chip's internal registers, a byte each, and their bits, under
/// OpenBSD's names for them.
#[allow(missing_docs)] // Each is named as rtsxreg.h names it, where it is described.
pub mod chip {
    pub const FPDCTL: u16 = 0xfc00;
    pub const SSC_POWER_DOWN: u8 = 0x01;
    pub const CLK_CTL: u16 = 0xfc02;
    pub const CLK_LOW_FREQ: u8 = 0x01;
    pub const CLK_DIV: u16 = 0xfc03;
    pub const SSC_DIV_N_0: u16 = 0xfc0f;
    pub const SSC_CTL1: u16 = 0xfc11;
    pub const RSTB: u8 = 0x80;
    pub const SSC_8X_EN: u8 = 0x40;
    pub const SSC_SEL_4M: u8 = 0x10;
    pub const SSC_CTL2: u16 = 0xfc12;
    pub const SSC_DEPTH_MASK: u8 = 0x07;
    pub const RCCTL: u16 = 0xfc14;
    pub const RCCTL_F_2M: u8 = 0x01;
    pub const OLT_LED_CTL: u16 = 0xfc1e;
    pub const OLT_LED_PERIOD: u8 = 0x02;
    pub const OLT_LED_AUTOBLINK: u8 = 0x08;
    pub const GPIO_CTL: u16 = 0xfc1f;
    pub const GPIO_LED_ON: u8 = 0x02;
    pub const CARD_CLK_SOURCE: u16 = 0xfc2e;
    /// CRC fixed (0 in bits 0 and 1), SD 3.0 variable clock 0, sampling
    /// variable clock 1.
    pub const CLK_SOURCES: u8 = 0x01 << 2 | 0x02 << 4;
    pub const CARD_PWR_CTL: u16 = 0xfd50;
    pub const SD_PARTIAL_PWR_ON: u8 = 0x01;
    pub const SD_PWR_OFF: u8 = 0x03;
    pub const PMOS_STRG_800MA: u8 = 0x10;
    pub const CARD_SHARE_MODE: u16 = 0xfd52;
    pub const CARD_SHARE_48_SD: u8 = 0x04;
    pub const CARD_STOP: u16 = 0xfd54;
    pub const SD_STOP: u8 = 0x04;
    pub const SD_CLR_ERR: u8 = 0x40;
    pub const CARD_OE: u16 = 0xfd55;
    pub const SD_OUTPUT_EN: u8 = 0x04;
    /// The xD, SD and Memory Stick outputs.
    pub const CARD_OUTPUT_EN: u8 = 0x02 | 0x04 | 0x08;
    pub const CARD_AUTO_BLINK: u16 = 0xfd56;
    pub const LED_BLINK_EN: u8 = 0x08;
    pub const LED_BLINK_SPEED: u8 = 0x05;
    pub const CARD_GPIO_DIR: u16 = 0xfd57;
    pub const CARD_GPIO: u16 = 0xfd58;
    pub const CARD_GPIO_LED_OFF: u8 = 0x01;
    pub const CARD_DATA_SOURCE: u16 = 0xfd5b;
    pub const RING_BUFFER: u8 = 0x00;
    pub const PINGPONG_BUFFER: u8 = 0x01;
    pub const CARD_SELECT: u16 = 0xfd5c;
    pub const SD_MOD_SEL: u8 = 0x02;
    pub const CARD_PULL_CTL1: u16 = 0xfd60;
    pub const CARD_PULL_CTL2: u16 = 0xfd61;
    pub const CARD_PULL_CTL3: u16 = 0xfd62;
    pub const PULL_CTL_DISABLE12: u8 = 0x55;
    pub const PULL_CTL_DISABLE3: u8 = 0xd5;
    pub const PULL_CTL_DISABLE3_TYPE_C: u8 = 0xe5;
    pub const PULL_CTL_ENABLE12: u8 = 0xaa;
    pub const PULL_CTL_ENABLE3: u8 = 0xe9;
    pub const PULL_CTL_ENABLE3_TYPE_C: u8 = 0xd9;
    pub const CARD_CLK_EN: u16 = 0xfd69;
    pub const SD_CLK_EN: u8 = 0x04;
    /// The xD, SD, Memory Stick and SPI clocks.
    pub const CARD_CLK_EN_ALL: u8 = 0x02 | 0x04 | 0x08 | 0x10;
    pub const SD_CFG1: u16 = 0xfda0;
    pub const CLK_DIVIDE_128: u8 = 0x80;
    pub const CLK_DIVIDE_MASK: u8 = 0xc0;
    pub const SD_MODE_MASK: u8 = 0x0c;
    pub const BUS_WIDTH_1: u8 = 0x00;
    pub const BUS_WIDTH_4: u8 = 0x01;
    pub const BUS_WIDTH_MASK: u8 = 0x03;
    pub const SD_CFG2: u16 = 0xfda1;
    pub const SD_STAT1: u16 = 0xfda3;
    /// A response's, a read's, or a written block's CRC was wrong.
    pub const SD_CRC_ERR: u8 = 0x80 | 0x40 | 0x20;
    pub const SD_STAT2: u16 = 0xfda4;
    /// The card did not answer within 80 clocks.
    pub const SD_RSP_80CLK_TIMEOUT: u8 = 0x01;
    pub const SD_BUS_STAT: u16 = 0xfda5;
    pub const SD_CLK_FORCE_STOP: u8 = 0x40;
    pub const SD_SAMPLE_POINT_CTL: u16 = 0xfda7;
    pub const SD20_RX_SEL_MASK: u8 = 0x08;
    pub const SD_PUSH_POINT_CTL: u16 = 0xfda8;
    pub const SD20_TX_NEG_EDGE: u8 = 0x00;
    /// The command's index and argument, and the short response read back
    /// in their place; SD_CMD5 the last byte of a long one.
    pub const SD_CMD0: u16 = 0xfda9;
    pub const SD_CMD4: u16 = 0xfdad;
    pub const SD_CMD5: u16 = 0xfdae;
    pub const SD_BYTE_CNT_L: u16 = 0xfdaf;
    pub const SD_BYTE_CNT_H: u16 = 0xfdb0;
    pub const SD_BLOCK_CNT_L: u16 = 0xfdb1;
    pub const SD_BLOCK_CNT_H: u16 = 0xfdb2;
    pub const SD_TRANSFER: u16 = 0xfdb3;
    /// Send the command in SD_CMD0 to SD_CMD4 and take its response.
    pub const TM_CMD_RSP: u8 = 0x08;
    /// Move the data of a command already answered, from the card to the
    /// ring buffer or from it to the card, without CMD12.
    pub const TM_AUTO_READ3: u8 = 0x05;
    pub const TM_AUTO_WRITE3: u8 = 0x01;
    pub const SD_TRANSFER_START: u8 = 0x80;
    pub const SD_TRANSFER_END: u8 = 0x40;
    pub const SD_STAT_IDLE: u8 = 0x20;
    /// Ping-pong buffer 2, which holds a long response.
    pub const PPBUF_BASE2: u16 = 0xfa00;
    pub const IRQSTAT0: u16 = 0xfe21;
    pub const LINK_READY_INT: u8 = 0x20;
    pub const DMA_DONE_INT: u8 = 0x80;
    /// The bytes of the transfer, highest first.
    pub const DMATC3: u16 = 0xfe2b;
    pub const DMATC0: u16 = 0xfe28;
    pub const DMACTL: u16 = 0xfe2c;
    pub const DMA_EN: u8 = 0x01;
    pub const DMA_DIR_FROM_CARD: u8 = 0x02;
    pub const DMA_DIR_TO_CARD: u8 = 0x00;
    pub const DMA_RST: u8 = 0x80;
    pub const DMA_512: u8 = 2 << 4;
    pub const DMA_PACK_SIZE_MASK: u8 = 0x30;
    pub const RBCTL: u16 = 0xfe34;
    pub const RB_FLUSH: u8 = 0x80;
    pub const PHY_RWCTL: u16 = 0xfe3c;
    pub const PHY_BUSY: u8 = 0x80;
    pub const PHY_WRITE: u8 = 0x01;
    pub const PHY_DATA0: u16 = 0xfe3d;
    pub const PHY_DATA1: u16 = 0xfe3e;
    pub const PHY_ADDR: u16 = 0xfe3f;
    pub const PETXCFG: u16 = 0xfe49;
    pub const PETXCFG_CLKREQ_PIN: u8 = 0x08;
    pub const CHANGE_LINK_STATE: u16 = 0xfe5b;
    pub const FORCE_RST_CORE_EN: u8 = 0x02;
    pub const NON_STICKY_RST_N_DBG: u8 = 0x08;
    pub const MAC_PHY_RST_N_DBG: u8 = 0x10;
    pub const PERST_GLITCH_WIDTH: u16 = 0xfe5c;
    pub const SD30_DRIVE_SEL: u16 = 0xfe5e;
    pub const SD30_DRIVE_SEL_3V3: u8 = 0x01;
    pub const HOST_SLEEP_STATE: u16 = 0xfe60;
    pub const HOST_ENTER_S1: u8 = 0x01;
    pub const HOST_ENTER_S3: u8 = 0x02;
    pub const NFTS_TX_CTRL: u16 = 0xfe72;
    pub const INT_READ_CLR: u8 = 0x02;
    pub const PWR_GATE_CTRL: u16 = 0xfe75;
    pub const LDO3318_VCC1: u8 = 0x02;
    pub const LDO3318_VCC2: u8 = 0x04;
    pub const LDO3318_SUSPEND: u8 = 0x04;
    pub const LDO3318_OFF: u8 = 0x06;
    pub const LDO_PWR_SEL: u16 = 0xfe78;
    pub const LDO_PWR_SEL_3V3: u8 = 0x01;
    pub const LDO_PWR_SEL_DV33: u8 = 0x03;
    pub const DUMMY_REG: u16 = 0xfe90;
    pub const LDO_VCC_CFG1: u16 = 0xff73;
    pub const LDO_VCC_TUNE_MASK: u8 = 0x07;
    pub const LDO_VCC_3V3: u8 = 0x07;
}

/// SD_CFG2's response types: the response's length, whether its CRC7 is
/// checked, and for R1b whether the reader waits for the card's busy to
/// end.
mod response {
    /// None (CMD0).
    pub const NONE: u8 = 0x04;
    /// R1, R6 and R7: six bytes, checked.
    pub const R1: u8 = 0x01;
    /// R1 with the card busy after it.
    pub const R1B: u8 = 0x09;
    /// R2: seventeen bytes, the CID or the CSD.
    pub const R2: u8 = 0x02;
    /// R3: the OCR, six bytes without a CRC.
    pub const R3: u8 = 0x05;
}

/// SD_CFG2 for a transfer of data, after the command was answered: the
/// data's CRC16 checked, no response, not waiting for busy; a read's
/// CRC7 checked as well (OpenBSD's choice).
const DATA_CFG2: u8 = 0x20;
const READ_CFG2: u8 = DATA_CFG2;
const WRITE_CFG2: u8 = DATA_CFG2 | 0x80 | 0x04;

/// The reader's page of DMA memory, below 4 GiB: the command buffer,
/// the small blocks [`Reader::identify`] reads, and the descriptors.
const PAGE: usize = 4096;
const SMALL: usize = 1024;
const DESCRIPTORS: usize = 2048;
/// The most commands a batch holds: the command buffer's words.
const MAX_COMMANDS: usize = SMALL / 4;

/// A descriptor's bits: valid, the last, and one that moves data.
const DESCRIPTOR_VALID: u64 = 0x01;
const DESCRIPTOR_END: u64 = 0x02;
const DESCRIPTOR_DATA: u64 = 0x02 << 4;

/// The card status bits of an R1 response: ready for data, and the state
/// (bits 9 to 12), of which 4 is transfer, waiting for a command.
const READY_FOR_DATA: u32 = 1 << 8;
const STATE_TRANSFER: u32 = 4 << 9;
const STATE_MASK: u32 = 0xf << 9;
/// How many CMD13s a write waits for the card to have written its blocks,
/// each a few tens of microseconds: some seconds, longer than a card may
/// take (Physical Layer, 4.6.2.2).
const STATUS_TRIES: u32 = 200_000;

/// Which chip a reader is: OpenBSD's rtsx(4) treats them in three groups,
/// the RTS5209, the RTS5229 and RTS5249, and the rest, and the RTS525A
/// apart for its regulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// RTS5209 (10ec:5209).
    Rts5209,
    /// RTS5227 (10ec:5227).
    Rts5227,
    /// RTS5229 (10ec:5229).
    Rts5229,
    /// RTS522A (10ec:522a).
    Rts522a,
    /// RTS5249 (10ec:5249).
    Rts5249,
    /// RTS525A (10ec:525a).
    Rts525a,
    /// RTL8402 (10ec:5286), a network adapter's reader.
    Rtl8402,
    /// RTL8411 (10ec:5289), a network adapter's reader.
    Rtl8411,
    /// RTL8411B (10ec:5287), a network adapter's reader.
    Rtl8411b,
}

impl Chip {
    /// The chip of the PCI function `vendor:device` of class `class`, if
    /// it is one of these.
    pub fn of(vendor: u16, device: u16, class: u8) -> Option<Chip> {
        if vendor != VENDOR || class != PCI_CLASS {
            return None;
        }
        Some(match device {
            0x5209 => Chip::Rts5209,
            0x5227 => Chip::Rts5227,
            0x5229 => Chip::Rts5229,
            0x522a => Chip::Rts522a,
            0x5249 => Chip::Rts5249,
            0x525a => Chip::Rts525a,
            0x5286 => Chip::Rtl8402,
            0x5289 => Chip::Rtl8411,
            0x5287 => Chip::Rtl8411b,
            _ => return None,
        })
    }

    /// Its name, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Chip::Rts5209 => "RTS5209",
            Chip::Rts5227 => "RTS5227",
            Chip::Rts5229 => "RTS5229",
            Chip::Rts522a => "RTS522A",
            Chip::Rts5249 => "RTS5249",
            Chip::Rts525a => "RTS525A",
            Chip::Rtl8402 => "RTL8402",
            Chip::Rtl8411 => "RTL8411",
            Chip::Rtl8411b => "RTL8411B",
        }
    }

    /// The BAR its registers are in.
    pub fn bar(self) -> usize {
        if self == Chip::Rts525a { 1 } else { 0 }
    }

    /// The RTS5209, whose power gates and LED differ from the others'.
    fn is_5209(self) -> bool {
        self == Chip::Rts5209
    }

    /// The RTS5229 and RTS5249, which say their version in DUMMY_REG.
    fn is_5229(self) -> bool {
        matches!(self, Chip::Rts5229 | Chip::Rts5249)
    }
}

/// A batch of commands the reader runs by itself from its command buffer
/// ([`Reader::send`]): writes and reads of internal registers, and checks
/// that one holds a value, each a word. Each read and check writes a
/// byte back into the buffer, in order from its start.
pub struct Batch {
    words: [u32; MAX_COMMANDS],
    len: usize,
}

impl Batch {
    fn new() -> Batch {
        Batch { words: [0; MAX_COMMANDS], len: 0 }
    }

    fn push(&mut self, kind: u32, register: u16, mask: u8, value: u8) {
        debug_assert!(self.len < MAX_COMMANDS);
        self.words[self.len] = kind << 30
            | u32::from(register & 0x3fff) << 16
            | u32::from(mask) << 8
            | u32::from(value);
        self.len += 1;
    }

    /// Reads `register`.
    fn read(&mut self, register: u16) {
        self.push(0, register, 0, 0);
    }

    /// Writes the bits of `mask` in `register` from `value`.
    fn write(&mut self, register: u16, mask: u8, value: u8) {
        self.push(1, register, mask, value);
    }

    /// Waits for the bits of `mask` in `register` to be `value`.
    fn check(&mut self, register: u16, mask: u8, value: u8) {
        self.push(2, register, mask, value);
    }

    /// Writes command `index` with `argument`, of `response`, to be sent,
    /// and checks that it was answered; then reads the response back:
    /// the short one's five bytes, or the long one's sixteen. Their place
    /// in the buffer is [`Answer::read`]'s.
    fn command(&mut self, index: u8, argument: u32, response: u8) {
        self.write(chip::SD_CMD0, 0xff, 0x40 | index);
        for (i, byte) in argument.to_be_bytes().into_iter().enumerate() {
            self.write(chip::SD_CMD0 + 1 + i as u16, 0xff, byte);
        }
        self.write(chip::SD_CFG2, 0xff, response);
        self.write(chip::CARD_DATA_SOURCE, 0x01, chip::PINGPONG_BUFFER);
        self.write(chip::SD_TRANSFER, 0xff, chip::TM_CMD_RSP | chip::SD_TRANSFER_START);
        let ended = chip::SD_TRANSFER_END | chip::SD_STAT_IDLE;
        self.check(chip::SD_TRANSFER, ended, ended);
        if response == response::R2 {
            // Its bytes from the last to the second (ping-pong buffer 2's
            // first is the start bits), then SD_CMD5: the register as an
            // SD host controller holds it, the lowest byte first.
            for register in (chip::PPBUF_BASE2 + 1..=chip::PPBUF_BASE2 + 15).rev() {
                self.read(register);
            }
            self.read(chip::SD_CMD5);
        } else if response != response::NONE {
            for register in chip::SD_CMD0..=chip::SD_CMD4 {
                self.read(register);
            }
        }
    }

    /// Moves `blocks` blocks of `length` bytes by DMA, to or from the card
    /// as `op` says, for a command already answered.
    fn data(&mut self, op: Op, length: u16, blocks: u16) {
        let bytes = u32::from(length) * u32::from(blocks);
        let (cfg2, direction, mode) = match op {
            Op::Read => (READ_CFG2, chip::DMA_DIR_FROM_CARD, chip::TM_AUTO_READ3),
            Op::Write => (WRITE_CFG2, chip::DMA_DIR_TO_CARD, chip::TM_AUTO_WRITE3),
        };
        self.write(chip::SD_CFG2, 0xff, cfg2);
        let [length_low, length_high] = length.to_le_bytes();
        self.write(chip::SD_BYTE_CNT_L, 0xff, length_low);
        self.write(chip::SD_BYTE_CNT_H, 0xff, length_high);
        let [blocks_low, blocks_high] = blocks.to_le_bytes();
        self.write(chip::SD_BLOCK_CNT_L, 0xff, blocks_low);
        self.write(chip::SD_BLOCK_CNT_H, 0xff, blocks_high);
        self.write(chip::CARD_DATA_SOURCE, 0x01, chip::RING_BUFFER);
        self.write(chip::IRQSTAT0, chip::DMA_DONE_INT, chip::DMA_DONE_INT);
        for (i, byte) in bytes.to_be_bytes().into_iter().enumerate() {
            self.write(chip::DMATC3 - i as u16, 0xff, byte);
        }
        let mask = 0x03 | chip::DMA_PACK_SIZE_MASK;
        self.write(chip::DMACTL, mask, direction | chip::DMA_EN | chip::DMA_512);
        self.write(chip::SD_TRANSFER, 0xff, mode | chip::SD_TRANSFER_START);
        self.check(chip::SD_TRANSFER, chip::SD_TRANSFER_END, chip::SD_TRANSFER_END);
    }
}

/// What a command's batch read back: the check's byte, the command's
/// index, and the response.
struct Answer([u8; 17]);

impl Answer {
    /// A short response: the card's status, or its OCR.
    fn short(&self) -> u32 {
        u32::from_be_bytes([self.0[2], self.0[3], self.0[4], self.0[5]])
    }

    /// A long response, as `radix_sdhci` reads the CID and CSD from it:
    /// bits 8 to 127, the first word the lowest, with the 128th byte,
    /// SD_CMD5's, left out.
    fn long(&self) -> Long {
        let word = |i: usize| {
            let at = 1 + 4 * i;
            u32::from_le_bytes([self.0[at], self.0[at + 1], self.0[at + 2], self.0[at + 3]])
        };
        [word(0), word(1), word(2), word(3) & 0x00ff_ffff]
    }
}

/// A step of the request in flight, each a batch the reader runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// CMD17, CMD18, CMD24 or CMD25 was sent.
    Command,
    /// The data moves.
    Data,
    /// CMD12 was sent, after several blocks.
    Stop,
    /// CMD13 was sent, after a write, for the card's state.
    Status { tries: u32 },
}

/// The request in flight.
#[derive(Clone, Copy, Debug)]
struct Flight {
    op: Op,
    count: u32,
    step: Step,
}

/// A Realtek card reader and the SD card in it.
pub struct Reader {
    regs: Mmio,
    chip: Chip,
    /// An RTS5229 of version C, whose pull-ups differ.
    type_c: bool,
    /// The command buffer, small blocks, and the descriptors ([`PAGE`]).
    page: Dma,
    card: Option<Card>,
    flight: Option<Flight>,
    /// A transfer failed: the reader stops what it was doing before the
    /// next.
    stuck: bool,
}

impl Reader {
    /// Sets up the reader `chip` whose registers are `regs`, as OpenBSD's
    /// `rtsx_init` does: its interrupts cleared and on, its clocks, its
    /// PHY and its LED; its page of DMA memory comes from
    /// `alloc(pages, below_4g)`.
    pub fn new(
        regs: Mmio,
        chip: Chip,
        alloc: &mut dyn FnMut(usize, bool) -> Option<Dma>,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Reader, Error> {
        if regs.len() < REGISTERS {
            return Err(Error::Unsupported("its registers are too few"));
        }
        let page = alloc(1, true).ok_or(Error::Unsupported("out of memory below 4 GiB"))?;
        let mut reader =
            Reader { regs, chip, type_c: false, page, card: None, flight: None, stuck: false };
        reader.set_up(delay)?;
        Ok(reader)
    }

    /// The reader's chip.
    pub fn chip(&self) -> Chip {
        self.chip
    }

    fn set_up(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        use chip::*;
        if self.chip.is_5229() {
            // Versions A, B and D are alike; C has other pull-ups.
            match self.read(DUMMY_REG)? & 0x0f {
                0 | 1 | 3 => {}
                2 => self.type_c = true,
                _ => return Err(Error::Unsupported("it is of a version not driven")),
            }
        }
        // Its pending bits are cleared by writing them, not by reading.
        self.clear(NFTS_TX_CTRL, INT_READ_CLR)?;
        let pending = self.regs.read32(reg::PENDING);
        self.regs.write32(reg::PENDING, pending);
        self.regs.write32(reg::ENABLED, pending::WATCHED);
        self.clear(FPDCTL, SSC_POWER_DOWN)?;
        delay(200);
        let phy = if self.chip.is_5209() { 0xb966 } else { 0xba42 };
        self.write_phy(0x00, phy, delay)?;
        self.set(CLK_DIV, 0x07)?;
        self.clear(HOST_SLEEP_STATE, HOST_ENTER_S1 | HOST_ENTER_S3)?;
        self.clear(CARD_CLK_EN, CARD_CLK_EN_ALL)?;
        self.clear(CHANGE_LINK_STATE, FORCE_RST_CORE_EN | NON_STICKY_RST_N_DBG | 0x04)?;
        self.write(SD30_DRIVE_SEL, 0xff, SD30_DRIVE_SEL_3V3)?;
        self.write(SSC_CTL1, 0xff, SSC_8X_EN | SSC_SEL_4M)?;
        self.write(SSC_CTL2, 0xff, 0x12)?;
        self.set(CHANGE_LINK_STATE, MAC_PHY_RST_N_DBG)?;
        self.set(IRQSTAT0, LINK_READY_INT)?;
        self.write(PERST_GLITCH_WIDTH, 0xff, 0x80)?;
        // The RC oscillator at 400 kHz; the clock asked for on CLKREQ#.
        self.clear(RCCTL, RCCTL_F_2M)?;
        self.set(PETXCFG, PETXCFG_CLKREQ_PIN)?;
        if self.chip.is_5209() {
            self.write(CARD_GPIO, 0xff, 0x03)?;
            self.write(CARD_GPIO_DIR, 0xff, 0x03)?;
        } else {
            self.set(GPIO_CTL, GPIO_LED_ON)?;
            // The 3.3 V regulator fed from 3V3, not DV33.
            self.clear(LDO_PWR_SEL, LDO_PWR_SEL_DV33)?;
            self.set(LDO_PWR_SEL, LDO_PWR_SEL_3V3)?;
            self.set(OLT_LED_CTL, OLT_LED_PERIOD)?;
        }
        Ok(())
    }

    /// Reads the internal register `register`.
    fn read(&self, register: u16) -> Result<u8, Error> {
        internal(&self.regs, INTERNAL_BUSY | u32::from(register & 0x3fff) << 16, None)
    }

    /// Writes the bits of `mask` in the internal register `register` from
    /// `value`.
    fn write(&self, register: u16, mask: u8, value: u8) -> Result<(), Error> {
        write_internal(&self.regs, register, mask, value)
    }

    fn set(&self, register: u16, bits: u8) -> Result<(), Error> {
        self.write(register, bits, 0xff)
    }

    fn clear(&self, register: u16, bits: u8) -> Result<(), Error> {
        self.write(register, bits, 0)
    }

    /// Writes `value` to the PHY's register `address`.
    fn write_phy(&self, address: u8, value: u16, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let [low, high] = value.to_le_bytes();
        self.write(chip::PHY_DATA0, 0xff, low)?;
        self.write(chip::PHY_DATA1, 0xff, high)?;
        self.write(chip::PHY_ADDR, 0xff, address)?;
        self.write(chip::PHY_RWCTL, 0xff, chip::PHY_BUSY | chip::PHY_WRITE)?;
        for _ in 0..1000 {
            if self.read(chip::PHY_RWCTL)? & chip::PHY_BUSY == 0 {
                return Ok(());
            }
            delay(10);
        }
        Err(Error::Timeout)
    }

    /// Whether a card is in the reader.
    pub fn present(&self) -> bool {
        self.regs.read32(reg::PENDING) & pending::SD_IN != 0
    }

    /// The reader's card detect, for the thread that watches it.
    pub fn detect(&self) -> Detect {
        Detect { regs: self.regs.clone(), chip: self.chip, type_c: self.type_c }
    }

    /// The card set up in the reader, if one is.
    pub fn card(&self) -> Option<&Card> {
        self.card.as_ref()
    }

    /// Runs `batch` from the command buffer, without waiting.
    fn send(&self, batch: &Batch) {
        for (i, word) in batch.words[..batch.len].iter().enumerate() {
            self.page.write32(4 * i, *word);
        }
        device_barrier();
        self.regs.write32(reg::COMMANDS, self.page.phys() as u32);
        let bytes = (batch.len * 4) as u32 & 0x00ff_ffff;
        self.regs.write32(reg::COMMAND_CONTROL, bytes | START_COMMANDS | AUTO_RESPONSE);
    }

    /// Moves data by DMA from `address`: of descriptors with `described`,
    /// else of one buffer of `bytes`; to memory with `read`.
    fn move_data(&self, address: u64, bytes: u32, read: bool, described: bool) {
        let mut control = dma::START | if read { dma::READ } else { 0 };
        control |= if described { dma::DESCRIBED } else { bytes & 0x00ff_ffff };
        self.regs.write32(reg::DATA, address as u32);
        self.regs.write32(reg::DATA_CONTROL, control);
    }

    /// How the batch running ended, its pending bits cleared: `None` while
    /// it runs.
    fn ended(&mut self) -> Option<Result<(), Error>> {
        let bits = self.regs.read32(reg::PENDING) & (pending::DONE | pending::FAILED);
        if bits == 0 {
            return None;
        }
        self.regs.write32(reg::PENDING, bits);
        if bits & pending::FAILED == 0 {
            return Some(Ok(()));
        }
        let why = self.failure();
        self.stop();
        Some(Err(why))
    }

    /// Why the batch that failed did: the card did not answer, or its CRC
    /// was wrong.
    fn failure(&self) -> Error {
        let quiet =
            self.read(chip::SD_STAT2).is_ok_and(|bits| bits & chip::SD_RSP_80CLK_TIMEOUT != 0);
        let garbled = self.read(chip::SD_STAT1).is_ok_and(|bits| bits & chip::SD_CRC_ERR != 0);
        match (quiet, garbled) {
            (true, _) => Error::Command(failed::NO_ANSWER),
            (_, true) => Error::Command(failed::DATA_CRC),
            _ => Error::Command(0),
        }
    }

    /// Waits up to `us` microseconds for the batch running to end; one
    /// that does not is stopped.
    fn finish(&mut self, us: u32, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        for _ in 0..us.div_ceil(10) {
            if let Some(result) = self.ended() {
                return result;
            }
            delay(10);
        }
        if let Some(result) = self.ended() {
            return result;
        }
        self.stop();
        Err(Error::Timeout)
    }

    /// Stops the command buffer, the card's transfer and the DMA, and
    /// empties the ring buffer, as OpenBSD's `rtsx_soft_reset` does.
    fn stop(&mut self) {
        self.regs.write32(reg::COMMAND_CONTROL, STOP_COMMANDS);
        let stop = chip::SD_STOP | chip::SD_CLR_ERR;
        let _ = self.write(chip::CARD_STOP, stop, stop);
        self.regs.write32(reg::DATA_CONTROL, dma::STOP);
        let _ = self.write(chip::DMACTL, chip::DMA_RST, chip::DMA_RST);
        let _ = self.write(chip::RBCTL, chip::RB_FLUSH, chip::RB_FLUSH);
    }

    /// What the last batch read back.
    fn answer(&self) -> Answer {
        let mut bytes = [0; 17];
        self.page.read(0, &mut bytes);
        Answer(bytes)
    }

    /// Sends command `index` with `argument` and waits for its `response`.
    fn command(
        &mut self,
        index: u8,
        argument: u32,
        response: u8,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Answer, Error> {
        let mut batch = Batch::new();
        batch.command(index, argument, response);
        self.send(&batch);
        self.finish(1_000_000, delay)?;
        let answer = self.answer();
        if matches!(response, response::R1 | response::R1B)
            && answer.short() & CARD_ERRORS != 0
            && !matches!(index, 3 | 8)
        {
            return Err(Error::Card(answer.short()));
        }
        Ok(answer)
    }

    /// An application command (ACMD): CMD55 to the card at `rca`, then
    /// `index`.
    fn app_command(
        &mut self,
        rca: u16,
        index: u8,
        argument: u32,
        response: u8,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Answer, Error> {
        self.command(55, u32::from(rca) << 16, response::R1, delay)?;
        self.command(index, argument, response, delay)
    }

    /// Reads the one block of `out.len()` bytes that command `index` with
    /// `argument` gives (the ACMD to the card at `app`'s address, if
    /// given), through the page.
    fn read_small(
        &mut self,
        app: Option<u16>,
        (index, argument): (u8, u32),
        out: &mut [u8],
        delay: &mut dyn FnMut(u32),
    ) -> Result<(), Error> {
        match app {
            Some(rca) => self.app_command(rca, index, argument, response::R1, delay)?,
            None => self.command(index, argument, response::R1, delay)?,
        };
        let mut batch = Batch::new();
        batch.data(Op::Read, out.len() as u16, 1);
        self.send(&batch);
        let small = self.page.phys() + SMALL as u64;
        self.move_data(small, out.len() as u32, true, false);
        self.finish(1_000_000, delay)?;
        self.page.read(SMALL, out);
        Ok(())
    }

    /// Turns the card's clock, output and power off (OpenBSD's
    /// `rtsx_bus_power_off`).
    fn power_off(&self) -> Result<(), Error> {
        power_off(&self.regs, self.chip, self.type_c)
    }

    /// Powers the card on in two steps, to keep the current from peaking,
    /// with its pull-ups and the SD slot chosen (`rtsx_bus_power_on`).
    fn power_on(&self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        use chip::*;
        if self.chip == Chip::Rts525a {
            self.write(LDO_VCC_CFG1, LDO_VCC_TUNE_MASK, LDO_VCC_3V3)?;
        }
        self.write(CARD_SELECT, 0xff, SD_MOD_SEL)?;
        self.write(CARD_SHARE_MODE, 0xff, CARD_SHARE_48_SD)?;
        self.set(CARD_CLK_EN, SD_CLK_EN)?;
        self.write(CARD_PULL_CTL1, 0xff, PULL_CTL_ENABLE12)?;
        self.write(CARD_PULL_CTL2, 0xff, PULL_CTL_ENABLE12)?;
        let pull = if self.type_c { PULL_CTL_ENABLE3_TYPE_C } else { PULL_CTL_ENABLE3 };
        self.write(CARD_PULL_CTL3, 0xff, pull)?;
        self.set(CARD_PWR_CTL, SD_PARTIAL_PWR_ON)?;
        if self.chip.is_5209() {
            self.set(PWR_GATE_CTRL, LDO3318_SUSPEND)?;
        } else {
            self.set(PWR_GATE_CTRL, LDO3318_VCC1)?;
        }
        delay(200);
        self.clear(CARD_PWR_CTL, SD_PWR_OFF)?;
        if self.chip.is_5209() {
            self.clear(PWR_GATE_CTRL, LDO3318_OFF)?;
        } else {
            self.set(PWR_GATE_CTRL, LDO3318_VCC2)?;
        }
        self.write(CARD_OE, 0xff, SD_OUTPUT_EN)?;
        Ok(())
    }

    /// Sets the bus's width: 1 or 4 bits.
    fn set_width(&self, bits: u8) -> Result<(), Error> {
        let width = if bits == 4 { chip::BUS_WIDTH_4 } else { chip::BUS_WIDTH_1 };
        self.write(chip::SD_CFG1, chip::BUS_WIDTH_MASK, width)
    }

    /// Runs the card's clock at `khz`, rounded down to 400 kHz, 25 MHz or
    /// 50 MHz, as OpenBSD's `rtsx_bus_clock` does: the clock it runs at.
    fn set_clock(&self, khz: u32, delay: &mut dyn FnMut(u32)) -> Result<u32, Error> {
        use chip::*;
        // The SSC's N, the divider and the MCU's count.
        let (n, divider, mcu, khz) = if khz >= 50_000 {
            (100, 0x02, 7, 50_000)
        } else if khz >= 25_000 {
            (100, 0x03, 7, 25_000)
        } else {
            (80, 0x04, 7, 400)
        };
        if khz == 400 {
            self.set(SD_CFG1, CLK_DIVIDE_128)?;
        } else {
            self.clear(SD_CFG1, CLK_DIVIDE_MASK)?;
        }
        // SD 2.0's mode, the clock low while the SSC changes.
        self.clear(SD_CFG1, SD_MODE_MASK)?;
        self.set(CLK_CTL, CLK_LOW_FREQ)?;
        self.write(CARD_CLK_SOURCE, 0xff, CLK_SOURCES)?;
        self.clear(SD_SAMPLE_POINT_CTL, SD20_RX_SEL_MASK)?;
        self.write(SD_PUSH_POINT_CTL, 0xff, SD20_TX_NEG_EDGE)?;
        self.write(CLK_DIV, 0xff, divider << 4 | mcu)?;
        self.clear(SSC_CTL1, RSTB)?;
        self.clear(SSC_CTL2, SSC_DEPTH_MASK)?;
        self.write(SSC_DIV_N_0, 0xff, n)?;
        self.set(SSC_CTL1, RSTB)?;
        delay(100);
        self.clear(CLK_CTL, CLK_LOW_FREQ)?;
        Ok(khz)
    }

    /// Powers the card up and sets it up (Physical Layer, 4.2), ready for
    /// [`Reader::start`]: at High Speed's 50 MHz if it has it, else at
    /// the default speed's 25 MHz, on the four-bit bus.
    pub fn identify(&mut self, delay: &mut dyn FnMut(u32)) -> Result<Card, Error> {
        if !self.present() {
            return Err(Error::NoCard);
        }
        self.card = None;
        self.flight = None;
        self.stuck = false;
        self.power_off()?;
        delay(200);
        self.power_on(delay)?;
        self.set_width(1)?;
        self.set_clock(400, delay)?;
        // The card's power settles, then 74 clocks before the first
        // command.
        delay(10_000);
        self.command(0, 0, response::NONE, delay)?;
        let version2 = match self.command(8, 0x1aa, response::R1, delay) {
            Ok(answer) if answer.short() & 0xfff == 0x1aa => true,
            Ok(_) => return Err(Error::Unsupported("the card does not take the slot's voltage")),
            Err(Error::Command(_) | Error::Timeout) => false,
            Err(err) => return Err(err),
        };
        let asked = VOLTAGES | if version2 { ocr::HIGH_CAPACITY } else { 0 };
        let mut answer = 0;
        for _ in 0..100 {
            answer = match self.app_command(0, 41, asked, response::R3, delay) {
                Ok(answer) => answer.short(),
                Err(Error::Command(_) | Error::Timeout) => {
                    return Err(Error::Unsupported(
                        "it is an MMC, which a Realtek reader's driver does not set up",
                    ));
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
        let high_capacity = version2 && answer & ocr::HIGH_CAPACITY != 0;
        let cid = Cid::parse(&self.command(2, 0, response::R2, delay)?.long());
        let rca = (self.command(3, 0, response::R1, delay)?.short() >> 16) as u16;
        let csd = self.command(9, u32::from(rca) << 16, response::R2, delay)?.long();
        let blocks = match Csd::parse(&csd) {
            Csd::Standard(blocks) | Csd::High(blocks) => blocks,
            Csd::Ultra => return Err(Error::Unsupported("it is an SDUC card, not driven yet")),
            Csd::Unknown => return Err(Error::Unsupported("its CSD is of an unknown version")),
        };
        self.command(7, u32::from(rca) << 16, response::R1B, delay)?;
        let mut khz = self.set_clock(Speed::Default.khz(), delay)?;
        self.app_command(rca, 6, 2, response::R1, delay)?;
        self.set_width(4)?;
        if !high_capacity {
            self.command(16, BLOCK, response::R1, delay)?;
        }
        let mut speed = Speed::Default;
        if self.high_speed(rca, delay)? {
            speed = Speed::High;
            khz = self.set_clock(Speed::High.khz(), delay)?;
        }
        let read_only = self.regs.read32(reg::PENDING) & pending::WRITE_PROTECT != 0;
        let card = Card {
            family: Family::Sd,
            rca,
            high_capacity,
            blocks,
            cid,
            speed,
            khz,
            width: 4,
            read_only,
        };
        self.card = Some(card);
        let _ = led(&self.regs, self.chip, true);
        Ok(card)
    }

    /// Switches the card at `rca` to High Speed if it has it (CMD6, from
    /// version 1.10): true if it did.
    fn high_speed(&mut self, rca: u16, delay: &mut dyn FnMut(u32)) -> Result<bool, Error> {
        // The SCR's SD_SPEC: a card of version 1.0x has no CMD6.
        let mut scr = [0u8; 8];
        self.read_small(Some(rca), (51, 0), &mut scr, delay)?;
        if scr[0] & 0xf == 0 {
            return Ok(false);
        }
        let mut functions = [0u8; 64];
        self.read_small(None, (6, NO_CHANGE), &mut functions, delay)?;
        let modes = u16::from_be_bytes([functions[12], functions[13]]);
        if modes & 1 << 1 == 0 {
            return Ok(false);
        }
        let mut result = [0u8; 64];
        self.read_small(None, (6, 1 << 31 | NO_CHANGE & !0xf | 1), &mut result, delay)?;
        // Group 1's function, as switched to.
        Ok(result[16] & 0xf == 1)
    }

    /// Starts `op` on `count` blocks from block `block`, to or from
    /// `buffers` (physical address below 4 GiB and length), which
    /// [`Reader::poll`] moves along: the command goes first.
    pub fn start(
        &mut self,
        op: Op,
        block: u64,
        count: u32,
        buffers: &[(u64, u32)],
        delay: &mut dyn FnMut(u32),
    ) -> Result<(), Error> {
        let card = self.card.ok_or(Error::NoCard)?;
        let bytes: u64 = buffers.iter().map(|&(_, len)| u64::from(len)).sum();
        let fits = |&(phys, len): &(u64, u32)| {
            len > 0 && len <= MAX_BUFFER && phys + u64::from(len) <= 1 << 32
        };
        if self.flight.is_some()
            || count == 0
            || count > u32::from(u16::MAX)
            || buffers.is_empty()
            || buffers.len() > MAX_BUFFERS
            || bytes != u64::from(count) * u64::from(BLOCK)
            || !buffers.iter().all(fits)
            || block + u64::from(count) > card.blocks
            || (op == Op::Write && card.read_only)
        {
            return Err(Error::BadRequest);
        }
        if self.stuck {
            // A transfer failed: the card is stopped before the next.
            let _ = self.command(12, 0, response::R1B, delay);
            self.stuck = false;
        }
        for (i, &(phys, len)) in buffers.iter().enumerate() {
            let end = if i + 1 == buffers.len() { DESCRIPTOR_END } else { 0 };
            let bits = DESCRIPTOR_VALID | DESCRIPTOR_DATA | end;
            self.page.write64(DESCRIPTORS + 8 * i, phys << 32 | u64::from(len) << 12 | bits);
        }
        let address = if card.high_capacity { block } else { block * u64::from(BLOCK) };
        let argument = u32::try_from(address).map_err(|_| Error::BadRequest)?;
        let index = match (op, count > 1) {
            (Op::Read, false) => 17,
            (Op::Read, true) => 18,
            (Op::Write, false) => 24,
            (Op::Write, true) => 25,
        };
        let mut batch = Batch::new();
        batch.command(index, argument, response::R1);
        self.send(&batch);
        self.flight = Some(Flight { op, count, step: Step::Command });
        Ok(())
    }

    /// Moves the request in flight along: how it ended, or `None` while it
    /// goes on or if there is none.
    pub fn poll(&mut self) -> Option<Result<(), Error>> {
        let flight = self.flight?;
        let ended = self.ended()?;
        match self.next(flight, ended) {
            Some(step) => {
                self.flight = Some(Flight { step, ..flight });
                None
            }
            None => {
                self.flight = None;
                Some(ended.and_then(|()| self.status(flight)))
            }
        }
    }

    /// The step after `flight`'s, which ended as `ended`, sent; `None` if
    /// the request is done, or failed.
    fn next(&mut self, flight: Flight, ended: Result<(), Error>) -> Option<Step> {
        if ended.is_err() {
            self.stuck = true;
            return None;
        }
        let status = self.answer().short();
        let multiple = flight.count > 1;
        let mut batch = Batch::new();
        let step = match flight.step {
            Step::Command if status & CARD_ERRORS != 0 => return None,
            Step::Command => {
                batch.data(flight.op, BLOCK as u16, flight.count as u16);
                self.send(&batch);
                let table = self.page.phys() + DESCRIPTORS as u64;
                self.move_data(table, 0, flight.op == Op::Read, true);
                return Some(Step::Data);
            }
            Step::Data if multiple => {
                batch.command(12, 0, response::R1B);
                Step::Stop
            }
            Step::Data | Step::Stop if flight.op == Op::Write => Step::Status { tries: 0 },
            Step::Status { tries } if status & STATE_MASK != STATE_TRANSFER => {
                if tries >= STATUS_TRIES {
                    return None;
                }
                Step::Status { tries: tries + 1 }
            }
            _ => return None,
        };
        if let Step::Status { .. } = step {
            let rca = self.card.map_or(0, |card| card.rca);
            batch.command(13, u32::from(rca) << 16, response::R1);
        }
        self.send(&batch);
        Some(step)
    }

    /// How a request whose batches all ran ended, from the card's last
    /// answer: the status its command gave, refused, or CMD13's after a
    /// write. The data and CMD12 say nothing more: the end of a read to
    /// the card's last block may have CMD12 say it is out of range.
    fn status(&mut self, flight: Flight) -> Result<(), Error> {
        let status = self.answer().short();
        let failed = match flight.step {
            Step::Command | Step::Status { .. } if status & CARD_ERRORS != 0 => Error::Card(status),
            // The card never got back to waiting for a command.
            Step::Status { .. } if status & READY_FOR_DATA == 0 => Error::Timeout,
            Step::Status { .. } if status & STATE_MASK != STATE_TRANSFER => Error::Timeout,
            _ => return Ok(()),
        };
        self.stuck = true;
        Err(failed)
    }

    /// Gives up the request in flight, which took too long: the reader
    /// stops it, and the card is stopped before the next.
    pub fn abandon(&mut self, _delay: &mut dyn FnMut(u32)) {
        self.flight = None;
        self.stop();
        self.stuck = true;
    }
}

/// Reads or writes an internal register through [`reg::INTERNAL`], by
/// `request`, waiting for the reader to have done it: the byte it gives
/// back, which after a write is the byte written (`expect`).
fn internal(regs: &Mmio, request: u32, expect: Option<u8>) -> Result<u8, Error> {
    regs.write32(reg::INTERNAL, request);
    for _ in 0..1024 {
        let now = regs.read32(reg::INTERNAL);
        if now & INTERNAL_BUSY == 0 {
            let byte = now as u8;
            return match expect {
                Some(value) if value != byte => Err(Error::Command(0)),
                _ => Ok(byte),
            };
        }
    }
    Err(Error::Timeout)
}

/// Writes the bits of `mask` in the internal register `register` from
/// `value`.
fn write_internal(regs: &Mmio, register: u16, mask: u8, value: u8) -> Result<(), Error> {
    let request = INTERNAL_BUSY
        | INTERNAL_WRITE
        | u32::from(register & 0x3fff) << 16
        | u32::from(mask) << 8
        | u32::from(value);
    internal(regs, request, Some(value)).map(|_| ())
}

/// Turns the card's clock, output and power off, and its pull-ups.
fn power_off(regs: &Mmio, chip: Chip, type_c: bool) -> Result<(), Error> {
    use chip::*;
    let write = |register, mask, value| write_internal(regs, register, mask, value);
    write(CARD_CLK_EN, CARD_CLK_EN_ALL, 0)?;
    write(SD_BUS_STAT, SD_CLK_FORCE_STOP, 0xff)?;
    write(CARD_OE, CARD_OUTPUT_EN, 0)?;
    // The RTS5209's power gates are a mask of those off, the others' of
    // those on.
    if chip.is_5209() {
        write(PWR_GATE_CTRL, LDO3318_OFF, 0xff)?;
    } else {
        write(PWR_GATE_CTRL, LDO3318_VCC1 | LDO3318_VCC2, 0)?;
    }
    write(CARD_PWR_CTL, SD_PWR_OFF, 0xff)?;
    write(CARD_PWR_CTL, PMOS_STRG_800MA, 0)?;
    let pull = if type_c { PULL_CTL_DISABLE3_TYPE_C } else { PULL_CTL_DISABLE3 };
    write(CARD_PULL_CTL1, 0xff, PULL_CTL_DISABLE12)?;
    write(CARD_PULL_CTL2, 0xff, PULL_CTL_DISABLE12)?;
    write(CARD_PULL_CTL3, 0xff, pull)?;
    Ok(())
}

/// Turns the reader's LED, which blinks while a card is in it, `on` or
/// off.
fn led(regs: &Mmio, chip: Chip, on: bool) -> Result<(), Error> {
    use chip::*;
    let write = |register, mask, value| write_internal(regs, register, mask, value);
    let all = |on: bool| if on { 0xff } else { 0 };
    if chip.is_5209() {
        write(CARD_GPIO, CARD_GPIO_LED_OFF, all(!on))?;
        write(CARD_AUTO_BLINK, LED_BLINK_EN | LED_BLINK_SPEED, all(on))?;
    } else {
        write(GPIO_CTL, GPIO_LED_ON, all(on))?;
        write(OLT_LED_CTL, OLT_LED_AUTOBLINK, all(on))?;
    }
    Ok(())
}

/// A reader's card detect, apart from the [`Reader`] that moves the card's
/// data: the thread that watches the reader reads it, and so does its
/// interrupt handler, while the reader is in the disk's queue. It reads
/// and clears only the pending register's card bits, which nothing else
/// does; [`Detect::power_off`] alone reaches the internal registers, once
/// the queue no longer uses them.
#[derive(Clone)]
pub struct Detect {
    regs: Mmio,
    chip: Chip,
    type_c: bool,
}

impl Detect {
    /// Whether a card is in the reader. Its card detect is debounced: it
    /// does not settle as an SD host controller's may.
    pub fn presence(&self) -> Presence {
        let bits = self.regs.read32(reg::PENDING);
        if bits & pending::SD_IN == 0 {
            Presence::Empty
        } else {
            Presence::Card { protected: bits & pending::WRITE_PROTECT != 0 }
        }
    }

    /// What the card detect said since the last call, cleared. The reader
    /// says only that a card went in or came out: a change while a card
    /// is in is taken as the card taken out and another put in, as it may
    /// have been.
    pub fn events(&self) -> Events {
        let bits = self.regs.read32(reg::PENDING);
        if bits & pending::SD_CHANGED == 0 {
            return Events::default();
        }
        self.regs.write32(reg::PENDING, pending::SD_CHANGED);
        Events { inserted: bits & pending::SD_IN != 0, removed: true }
    }

    /// Turns the card's clock and power off, and the LED, as its card was
    /// taken out or let go.
    pub fn power_off(&self) {
        let _ = power_off(&self.regs, self.chip, self.type_c);
        let _ = led(&self.regs, self.chip, false);
    }

    /// Does nothing: the reader's interrupts stay on, as OpenBSD leaves
    /// them, and a reader that is polled has its PCI interrupt turned off
    /// instead.
    pub fn interrupts(&self, _on: bool) {}

    /// Whether the reader raised its interrupt.
    pub fn interrupting(&self) -> bool {
        let bits = self.regs.read32(reg::PENDING);
        // All 1s: the reader is gone from the bus.
        bits != u32::MAX && bits & self.regs.read32(reg::ENABLED) & pending::WATCHED != 0
    }
}
