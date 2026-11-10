// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SD card readers for the Radix kernel (Phase 11g, ADR 0247): SD host
//! controllers as the *SD Specifications Part A2, SD Host Controller
//! Simplified Specification* (version 4.20) describes them, and the SD
//! memory cards in them, as the *Part 1, Physical Layer Simplified
//! Specification* (version 9.10) does. Laptops' readers that are on PCI
//! (class 08h, subclass 05h) are such controllers, as are the eMMC and SD
//! controllers of many chipsets; QEMU's `sdhci-pci` is one.
//!
//! A controller has one to six slots, each with its own 256 bytes of
//! registers in a BAR of its own ([`Slot`]). [`Slot::new`] resets one and
//! reads what it can do; it moves data by ADMA2, the controller's
//! scatter-gather DMA, which most controllers since version 2.00 have,
//! with its 64-bit descriptors where the controller has them (version
//! 3.00 and later). A controller without ADMA2, or whose ADMA2 is broken,
//! moves data by SDMA, from one buffer below 4 GiB ([`Slot::contiguous`]).
//! What some controllers need beyond the specification is in [`quirks`]
//! (ADR 0445).
//! [`Slot::identify`] powers the card up and sets it up as section 4.2 of
//! the physical layer says: CMD0, CMD8 (which only version 2.00 cards
//! answer), ACMD41 until the card is ready (SDHC and SDXC cards say they
//! are addressed in blocks), CMD2 for its CID, CMD3 for its address, CMD9
//! for its CSD and so its capacity ([`card`]), CMD7 to select it, then the
//! default speed's 25 MHz and the four-bit bus every SD memory card has
//! (ACMD6), then the fastest bus speed mode both can (ADR 0438, below).
//! [`Slot::start`] then starts a read or a write of whole
//! 512-byte blocks to or from a list of buffers (CMD17, CMD18, CMD24 and
//! CMD25, the controller sending CMD12 after the multiple ones), and
//! [`Slot::poll`] says when it is done: the controller's interrupt is
//! left off, and the kernel polls it as it does an IDE channel.
//!
//! Cards come and go (ADR 0437). [`Detect`] is a slot's card detect, which
//! the thread watching the slot reads apart from the slot moving the
//! card's data: whether a card is in it and its write-protect switch
//! ([`Detect::presence`]), and the card put in or taken out since the last
//! look ([`Detect::events`]), two status bits that only it clears. A slot
//! is polled by default; [`Detect::interrupts`] has it interrupt instead
//! for a transfer's end, an error, and a card put in or taken out, on a
//! controller whose interrupt reaches the kernel.
//!
//! Speeds (ADR 0438): a card whose SCR says it takes CMD6 is asked which
//! functions of group 1, its bus speed modes, it has. At 3.3 V, a card
//! with High Speed in a slot that has it is switched to it, and runs at
//! up to 50 MHz. A controller of version 3.00 or later that gives 1.8 V
//! and one of UHS-I's modes asks the card, in ACMD41, to signal at 1.8 V
//! (S18R); a card that agrees (S18A) is switched with CMD11 as section
//! 3.6.1 of the host specification says, and then to the fastest UHS-I
//! mode both have, SDR104, DDR50, SDR50 or SDR25 in that order, with the
//! most current the card takes and the slot gives (group 4). SDR104, and
//! SDR50 where the controller asks, is tuned (CMD19, Execute Tuning); a
//! mode that does not tune gives way to the next. A voltage switch that
//! fails powers the card off, and it is set up again at 3.3 V. A data CRC
//! error in a tuned mode has the slot tuned again before the next
//! transfer, and one that no longer tunes falls back to SDR25.
//!
//! MMCs and eMMCs (ADR 0439, the [`mmc`] module): a card that does not
//! answer ACMD41 is set up as an MMC, with CMD1, and an eMMC soldered to
//! the board is one. Its EXT_CSD gives its capacity and speeds; its bus
//! is made as wide as both it and the slot are, eight bits where the
//! slot has them, and read back to check the lines, and it runs at
//! HS200, tuned with CMD21, or High Speed's 52 MHz.
//!
//! The crate is a compartment with no `unsafe` code; its tests run it
//! against a simulated controller and card.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod card;
pub mod mmc;
pub mod quirks;
#[cfg(test)]
mod tests;

use radix_hw::{Dma, Mmio, device_barrier};

pub use card::{Card, Cid, Csd, Family, Speed};
pub use quirks::Quirks;

/// The PCI class and subclass of an SD host controller.
pub const PCI_CLASS: (u8, u8) = (0x08, 0x05);
/// The PCI configuration register that says where the slots are: bits 0
/// to 2 the first slot's BAR, 4 to 6 the slots less one.
pub const SLOT_INFORMATION: u16 = 0x40;
/// The bytes of a slot's registers.
pub const REGISTERS: usize = 0x100;
/// The bytes of a block.
pub const BLOCK: u32 = 512;
/// The most buffers a request takes: the descriptors in a page.
pub const MAX_BUFFERS: usize = 128;
/// The longest buffer a descriptor takes.
pub const MAX_BUFFER: u32 = 65536 - 4096;

/// The registers of a slot (2.1).
pub mod reg {
    /// 32-bit: where SDMA moves data to or from next.
    pub const SDMA_ADDRESS: usize = 0x00;
    /// 32-bit: the argument of the command.
    pub const ARGUMENT: usize = 0x08;
    /// 16-bit: the bytes of a block.
    pub const BLOCK_SIZE: usize = 0x04;
    /// 16-bit: the blocks of a transfer.
    pub const BLOCK_COUNT: usize = 0x06;
    /// 16-bit: how the command's data moves.
    pub const TRANSFER_MODE: usize = 0x0c;
    /// 16-bit: the command; writing it sends it.
    pub const COMMAND: usize = 0x0e;
    /// 4 × 32-bit: the card's response.
    pub const RESPONSE: usize = 0x10;
    /// 32-bit: the data of a transfer without DMA, a word at a time.
    pub const BUFFER: usize = 0x20;
    /// 32-bit: what is going on.
    pub const PRESENT_STATE: usize = 0x24;
    /// 8-bit: the bus's width and the DMA used.
    pub const HOST_CONTROL: usize = 0x28;
    /// 8-bit: the card's power and voltage.
    pub const POWER_CONTROL: usize = 0x29;
    /// 16-bit: the card's clock.
    pub const CLOCK_CONTROL: usize = 0x2c;
    /// 8-bit: how long data may take.
    pub const TIMEOUT_CONTROL: usize = 0x2e;
    /// 8-bit: resets.
    pub const SOFTWARE_RESET: usize = 0x2f;
    /// 16-bit: what happened, cleared by writing 1s.
    pub const NORMAL_STATUS: usize = 0x30;
    /// 16-bit: the errors that happened.
    pub const ERROR_STATUS: usize = 0x32;
    /// 16-bit each: which of those are reported, and which interrupt.
    pub const NORMAL_STATUS_ENABLE: usize = 0x34;
    /// The errors reported.
    pub const ERROR_STATUS_ENABLE: usize = 0x36;
    /// The events that interrupt.
    pub const NORMAL_SIGNAL_ENABLE: usize = 0x38;
    /// The errors that interrupt.
    pub const ERROR_SIGNAL_ENABLE: usize = 0x3a;
    /// 16-bit, from version 3.00: the UHS-I mode, 1.8 V signaling and
    /// tuning.
    pub const HOST_CONTROL2: usize = 0x3e;
    /// 64-bit: what the controller can do.
    pub const CAPABILITIES: usize = 0x40;
    /// 64-bit: the most current it gives at each voltage, in 4 mA.
    pub const MAX_CURRENT: usize = 0x48;
    /// 64-bit: the ADMA descriptors' address.
    pub const ADMA_ADDRESS: usize = 0x58;
    /// 16-bit: the specification's version the controller follows.
    pub const VERSION: usize = 0xfe;
}

/// Present State's bits.
mod present {
    pub const COMMAND_INHIBIT: u32 = 1 << 0;
    pub const DATA_INHIBIT: u32 = 1 << 1;
    pub const CARD_INSERTED: u32 = 1 << 16;
    /// The card detect is settled: the slot has debounced it.
    pub const CARD_STABLE: u32 = 1 << 17;
    pub const WRITABLE: u32 = 1 << 19;
    /// The levels of DAT0 to DAT3: high while the card is idle.
    pub const DAT_LINES: u32 = 0xf << 20;
}

/// Normal Interrupt Status's bits.
mod status {
    pub const COMMAND_COMPLETE: u16 = 1 << 0;
    pub const TRANSFER_COMPLETE: u16 = 1 << 1;
    /// SDMA stopped at a boundary of its buffer, for the next address.
    pub const DMA_INTERRUPT: u16 = 1 << 3;
    /// A block can be read from the Buffer Data Port.
    pub const BUFFER_READ_READY: u16 = 1 << 5;
    pub const CARD_INSERTION: u16 = 1 << 6;
    pub const CARD_REMOVAL: u16 = 1 << 7;
    pub const ERROR: u16 = 1 << 15;
    /// A card went in or came out: [`super::Detect::events`] alone clears
    /// them, as the thread watching the slot reads them.
    pub const CARD_EVENTS: u16 = CARD_INSERTION | CARD_REMOVAL;
    /// Every bit but the card's events, which a command clears before it
    /// goes and once it ends.
    pub const ALL_BUT_CARD: u16 = !CARD_EVENTS;
    /// Error Interrupt Status: the data's CRC or end bit was wrong, as a
    /// sampling point that drifted gives.
    pub const DATA_GARBLED: u16 = 1 << 5 | 1 << 6;
}

/// Host Control 1's bits.
mod host {
    /// The four-bit bus.
    pub const WIDTH_4: u8 = 1 << 1;
    /// High Speed, and every UHS-I mode faster than SDR12.
    pub const HIGH_SPEED: u8 = 1 << 2;
    /// The eight-bit bus, an embedded device's (Extended Data Transfer
    /// Width).
    pub const WIDTH_8: u8 = 1 << 5;
}

/// Host Control 2's bits (version 3.00 and later).
mod host2 {
    /// The UHS-I mode: 0 SDR12, 1 SDR25, 2 SDR50, 3 SDR104, 4 DDR50.
    pub const UHS_MODE: u16 = 0x7;
    pub const SIGNALING_1_8: u16 = 1 << 3;
    /// Set to tune; the controller clears it when tuning ends.
    pub const EXECUTE_TUNING: u16 = 1 << 6;
    /// Set by the controller when tuning found a sampling point.
    pub const SAMPLING_CLOCK: u16 = 1 << 7;
}

/// The OCR's bits that ACMD41 asks with and the card answers.
pub mod ocr {
    /// The card is ready.
    pub const READY: u32 = 1 << 31;
    /// Asked: the host takes high capacity cards; answered: it is one.
    pub const HIGH_CAPACITY: u32 = 1 << 30;
    /// Asked: switch to 1.8 V (S18R); answered: the card will (S18A).
    pub const SWITCH_1_8: u32 = 1 << 24;
    /// CMD1's: the MMC takes 1.70 to 1.95 V.
    pub const MMC_1_8: u32 = 1 << 7;
}

/// The Normal Interrupt Signal Enable's bits while the slot interrupts:
/// a transfer's end, an error, and a card put in or taken out. A command
/// without data is waited for by polling, while the slot is being set up.
const SIGNALLED: u16 = status::TRANSFER_COMPLETE | status::DMA_INTERRUPT | status::CARD_EVENTS;
/// The Error Interrupt Signal Enable's bits while the slot interrupts:
/// every error the slot reports.
const ERRORS_SIGNALLED: u16 = 0x03ff;

/// Capabilities' bits.
mod caps {
    /// The slot has the eight lines an embedded device's bus can have.
    pub const BUS_8: u64 = 1 << 18;
    pub const ADMA2: u64 = 1 << 19;
    pub const HIGH_SPEED: u64 = 1 << 21;
    pub const SDMA: u64 = 1 << 22;
    pub const VOLTS_3_3: u64 = 1 << 24;
    pub const VOLTS_3_0: u64 = 1 << 25;
    pub const VOLTS_1_8: u64 = 1 << 26;
    pub const SYSTEM_64: u64 = 1 << 28;
    /// Bits 30 and 31: the slot's type, 0 for a removable card's slot, 1
    /// for a device soldered to the board (eMMC), 2 for a shared bus.
    pub const SLOT_TYPE_SHIFT: u32 = 30;
    /// UHS-I's modes the controller has.
    pub const SDR50: u64 = 1 << 32;
    pub const SDR104: u64 = 1 << 33;
    pub const DDR50: u64 = 1 << 34;
    /// SDR50 is tuned, as SDR104 always is.
    pub const TUNE_SDR50: u64 = 1 << 45;
}

/// The specification's versions, as the Version register says them.
pub mod version {
    /// 2.00.
    pub const V2: u8 = 1;
    /// 3.00.
    pub const V3: u8 = 2;
}

/// The card status bits of an R1 response that say a command failed.
pub const CARD_ERRORS: u32 = 0xfdf8_0000;
/// The voltages ACMD41 offers: 2.7 to 3.6 V.
pub const VOLTAGES: u32 = 0x00ff_8000;
/// CMD6's argument that changes no group's function.
pub const NO_CHANGE: u32 = 0x00ff_ffff;
/// The Clock Control register's bit that runs the card's clock.
const CLOCK_ON: u16 = 1 << 2;
/// An SD card's tuning block: CMD19's 64 bytes.
const SD_TUNING: (u16, u16) = (19, 64);

/// How a slot or its card failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No card is in the slot.
    NoCard,
    /// The controller did not finish in time.
    Timeout,
    /// The command failed: the Error Interrupt Status.
    Command(u16),
    /// The card said its command failed: its status.
    Card(u32),
    /// The controller or the card is of a kind the driver does not drive.
    Unsupported(&'static str),
    /// A request the driver cannot make.
    BadRequest,
}

/// The bits of [`Error::Command`] that [`Error::as_str`] words: those of
/// an SD host controller's Error Interrupt Status, in which the driver of
/// Realtek's card readers (radix-rtsx) says its failures too.
pub mod failed {
    /// The card did not answer the command.
    pub const NO_ANSWER: u16 = 1 << 0;
    /// The card's data did not come in time.
    pub const DATA_TIMEOUT: u16 = 1 << 4;
    /// The card's data was garbled: its CRC was wrong.
    pub const DATA_CRC: u16 = 1 << 5;
    /// The controller's DMA failed.
    pub const DMA: u16 = 1 << 9;
}

impl Error {
    /// Why, in words.
    pub fn as_str(&self) -> &'static str {
        match self {
            Error::NoCard => "no card is in the slot",
            Error::Timeout => "the controller did not finish in time",
            Error::Command(bits) if bits & failed::NO_ANSWER != 0 => "the card did not answer",
            Error::Command(bits) if bits & failed::DMA != 0 => "the controller's DMA failed",
            Error::Command(bits) if bits & failed::DATA_TIMEOUT != 0 => {
                "the card's data did not come in time"
            }
            Error::Command(bits) if bits & failed::DATA_CRC != 0 => {
                "the card's data was garbled (CRC)"
            }
            Error::Command(_) => "the command failed",
            Error::Card(bits) if bits & (1 << 26) != 0 => "the card is write-protected",
            Error::Card(bits) if bits & (1 << 31) != 0 => "beyond the card's end",
            Error::Card(_) => "the card refused the command",
            Error::Unsupported(why) => why,
            Error::BadRequest => "a request the driver cannot make",
        }
    }
}

/// Which way a transfer goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// From the card.
    Read,
    /// To the card.
    Write,
}

/// The responses (Physical Layer, 4.9), as the Command register's flags
/// ask for them: their length, and whether the CRC and index are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Response {
    None,
    /// R1, the card's status, and R6 and R7, laid out as it is.
    R1,
    /// R1 with the card busy after it.
    R1b,
    /// R2: the CID or the CSD.
    R2,
    /// R3: the OCR, unchecked.
    R3,
}

impl Response {
    fn flags(self) -> u16 {
        match self {
            Response::None => 0x00,
            Response::R1 => 0x1a,
            Response::R1b => 0x1b,
            Response::R2 => 0x09,
            Response::R3 => 0x02,
        }
    }
}

/// The Command register's bit that says data goes with the command.
const DATA_PRESENT: u16 = 1 << 5;
/// The Transfer Mode register's bits.
mod mode {
    pub const DMA: u16 = 1 << 0;
    pub const BLOCK_COUNT: u16 = 1 << 1;
    pub const AUTO_CMD12: u16 = 1 << 2;
    pub const READ: u16 = 1 << 4;
    pub const MULTIPLE: u16 = 1 << 5;
}

/// A request waiting for the lines to be free, or in flight.
#[derive(Clone, Copy, Debug)]
enum Flight {
    /// Its command is ready to go once neither line is inhibited.
    Ready { command: u16, mode: u16, argument: u32, count: u16, sdma: Option<u32> },
    /// Sent: the card's status once it answered.
    Sent { status: Option<u32> },
}

/// A slot of an SD host controller.
pub struct Slot {
    regs: Mmio,
    version: u8,
    caps: u64,
    /// The ADMA2 descriptors are 64-bit ones (12 bytes).
    adma64: bool,
    /// Transfers go by SDMA, to or from one buffer below 4 GiB: the
    /// controller has no ADMA2, or its ADMA2 does not work.
    sdma: bool,
    /// What the controller needs beyond the specification.
    quirks: Quirks,
    /// The descriptor table.
    table: Dma,
    card: Option<Card>,
    flight: Option<Flight>,
    /// A transfer failed: the card is stopped and the lines reset first.
    stuck: bool,
    /// The card runs in a mode that was tuned.
    tuned: bool,
    /// A transfer in that mode came back garbled: the slot is tuned again
    /// before the next.
    retune: bool,
}

/// Whether a card is in a slot, as its card detect says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// No card is in it.
    Empty,
    /// A card is in it.
    Card {
        /// Its write-protect switch is set.
        protected: bool,
    },
    /// The card detect is still settling: a card is going in or coming
    /// out. Look again in a few milliseconds.
    Settling,
}

/// What a slot's card detect said since it was last read: a card went in,
/// came out, or both, as one taken out and another put in between two
/// looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events {
    /// A card went in.
    pub inserted: bool,
    /// A card came out.
    pub removed: bool,
}

impl Events {
    /// Whether anything happened.
    pub fn any(&self) -> bool {
        self.inserted || self.removed
    }
}

/// A slot's card detect, apart from the [`Slot`] that moves the card's
/// data: the thread that watches the slot for cards put in and taken out
/// reads it, and so does the controller's interrupt handler, while the
/// slot itself is in the disk's queue. It reads Present State and the two
/// card bits of Normal Interrupt Status, and clears those, which nothing
/// else does; it powers a card taken out off.
#[derive(Clone)]
pub struct Detect {
    regs: Mmio,
}

impl Detect {
    /// The card detect of the slot whose registers are `regs`, which
    /// [`Slot::new`] has set up.
    pub fn new(regs: Mmio) -> Option<Detect> {
        (regs.len() >= REGISTERS).then_some(Detect { regs })
    }

    /// Whether a card is in the slot.
    pub fn presence(&self) -> Presence {
        let state = self.regs.read32(reg::PRESENT_STATE);
        if state & present::CARD_STABLE == 0 {
            Presence::Settling
        } else if state & present::CARD_INSERTED == 0 {
            Presence::Empty
        } else {
            Presence::Card { protected: state & present::WRITABLE == 0 }
        }
    }

    /// What the card detect said since the last call, cleared: a card
    /// put in, or taken out. A controller holds a card put in back from
    /// Present State until the removal before it is cleared (QEMU's
    /// does), so the thread watching the slot clears them at each look.
    pub fn events(&self) -> Events {
        let bits = self.regs.read16(reg::NORMAL_STATUS) & status::CARD_EVENTS;
        if bits != 0 {
            self.regs.write16(reg::NORMAL_STATUS, bits);
        }
        Events {
            inserted: bits & status::CARD_INSERTION != 0,
            removed: bits & status::CARD_REMOVAL != 0,
        }
    }

    /// Stops what the slot was doing, and turns the card's clock and
    /// power off, as its card was taken out or let go (the controller may
    /// have turned them off itself).
    pub fn power_off(&self) {
        self.regs.write8(reg::SOFTWARE_RESET, 6);
        self.regs.write16(reg::CLOCK_CONTROL, 0);
        self.regs.write8(reg::POWER_CONTROL, 0);
    }

    /// Has the slot interrupt, `on`, for a transfer's end, an error and a
    /// card put in or taken out, or not, when it is polled. Its status is
    /// reported either way. [`Slot::new`] turns it off.
    pub fn interrupts(&self, on: bool) {
        let (normal, errors) = if on { (SIGNALLED, ERRORS_SIGNALLED) } else { (0, 0) };
        self.regs.write16(reg::ERROR_SIGNAL_ENABLE, errors);
        self.regs.write16(reg::NORMAL_SIGNAL_ENABLE, normal);
    }

    /// Whether the slot raised its interrupt: a status bit it signals is
    /// set. The slots of a controller share its interrupt, and the
    /// handler asks each.
    pub fn interrupting(&self) -> bool {
        let normal = self.regs.read16(reg::NORMAL_STATUS);
        if normal & self.regs.read16(reg::NORMAL_SIGNAL_ENABLE) != 0 {
            return true;
        }
        normal & status::ERROR != 0
            && self.regs.read16(reg::ERROR_STATUS) & self.regs.read16(reg::ERROR_SIGNAL_ENABLE) != 0
    }
}

/// Waits until `done` or `us` microseconds have passed, `delay`ing 10 at
/// a time; says whether it is done.
fn wait(delay: &mut dyn FnMut(u32), us: u32, mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..us.div_ceil(10) {
        if done() {
            return true;
        }
        delay(10);
    }
    done()
}

impl Slot {
    /// Resets the slot whose registers are `regs` and reads what it can
    /// do, its controller needing `quirks`; its descriptor table comes
    /// from `alloc(pages, below_4g)`.
    pub fn new(
        regs: Mmio,
        alloc: &mut dyn FnMut(usize, bool) -> Option<Dma>,
        delay: &mut dyn FnMut(u32),
        quirks: Quirks,
    ) -> Result<Slot, Error> {
        if regs.len() < REGISTERS {
            return Err(Error::BadRequest);
        }
        let version = (regs.read16(reg::VERSION) & 0xff) as u8;
        regs.write8(reg::SOFTWARE_RESET, 1);
        if !wait(delay, 100_000, || regs.read8(reg::SOFTWARE_RESET) & 1 == 0) {
            return Err(Error::Timeout);
        }
        let caps = u64::from(regs.read32(reg::CAPABILITIES))
            | u64::from(regs.read32(reg::CAPABILITIES + 4)) << 32;
        let sdma = caps & caps::ADMA2 == 0 || quirks.broken_adma;
        if sdma && caps & caps::SDMA == 0 {
            return Err(Error::Unsupported("it has neither ADMA2 nor SDMA"));
        }
        let adma64 = !sdma && version >= version::V3 && caps & caps::SYSTEM_64 != 0;
        let table = alloc(1, !adma64).ok_or(Error::Unsupported("out of memory"))?;
        // Everything reported, nothing interrupting: the slot is polled.
        regs.write16(reg::NORMAL_STATUS_ENABLE, 0x00ff);
        regs.write16(reg::ERROR_STATUS_ENABLE, 0x03ff);
        regs.write16(reg::NORMAL_SIGNAL_ENABLE, 0);
        regs.write16(reg::ERROR_SIGNAL_ENABLE, 0);
        regs.write8(reg::TIMEOUT_CONTROL, 0x0e);
        Ok(Slot {
            regs,
            version,
            caps,
            adma64,
            sdma,
            quirks,
            table,
            card: None,
            flight: None,
            stuck: false,
            tuned: false,
            retune: false,
        })
    }

    /// The version of the specification it follows: (major, minor).
    pub fn version(&self) -> (u8, u8) {
        match self.version {
            0 => (1, 0),
            1 => (2, 0),
            2 => (3, 0),
            3 => (4, 0),
            4 => (4, 10),
            5 => (4, 20),
            _ => (4, 20),
        }
    }

    /// Whether its DMA reaches all of memory, not only the first 4 GiB.
    pub fn reaches_64(&self) -> bool {
        self.adma64
    }

    /// Whether its transfers go by SDMA: to or from one buffer below
    /// 4 GiB, which [`Slot::start`] takes as buffers that each follow the
    /// one before in memory.
    pub fn contiguous(&self) -> bool {
        self.sdma
    }

    /// Whether a card is in the slot.
    pub fn present(&self) -> bool {
        self.regs.read32(reg::PRESENT_STATE) & present::CARD_INSERTED != 0
    }

    /// The slot's card detect, for the thread that watches it and the
    /// interrupt handler.
    pub fn detect(&self) -> Detect {
        Detect { regs: self.regs.clone() }
    }

    /// Whether the slot takes a card that comes and goes; not one for a
    /// device soldered to the board, or a shared bus's.
    pub fn removable(&self) -> bool {
        self.caps >> caps::SLOT_TYPE_SHIFT & 3 == 0
    }

    /// The card set up, if there is one.
    pub fn card(&self) -> Option<&Card> {
        self.card.as_ref()
    }

    /// The last transfer came back garbled in a tuned mode: the next
    /// [`Slot::start`] tunes the slot first, and the transfer is worth
    /// starting again.
    pub fn retuning(&self) -> bool {
        self.retune
    }

    /// The base clock, in kHz.
    fn base_khz(&self) -> u32 {
        let mask = if self.version >= version::V3 { 0xff } else { 0x3f };
        ((self.caps >> 8) & mask) as u32 * 1000
    }

    /// Runs the card's clock as near `khz` as the controller can without
    /// going over; returns the speed it runs at.
    fn set_clock(&mut self, khz: u32, delay: &mut dyn FnMut(u32)) -> Result<u32, Error> {
        let base = self.base_khz();
        if base == 0 {
            return Err(Error::Unsupported("it does not say its base clock"));
        }
        self.regs.write16(reg::CLOCK_CONTROL, 0);
        // The clock is base / (2 * N), or base with N = 0: any N up to
        // 1023 from 3.00 on, a power of two up to 128 before.
        let (value, actual) = if base <= khz {
            (0, base)
        } else if self.version >= version::V3 {
            let n = base.div_ceil(2 * khz).min(1023);
            ((n & 0xff) << 8 | (n >> 8 & 3) << 6, base / (2 * n))
        } else {
            let mut n = 1;
            while n < 128 && base / (2 * n) > khz {
                n *= 2;
            }
            (n << 8, base / (2 * n))
        };
        let value = value as u16;
        self.regs.write16(reg::CLOCK_CONTROL, value | 1);
        if !wait(delay, 150_000, || self.regs.read16(reg::CLOCK_CONTROL) & 2 != 0) {
            return Err(Error::Timeout);
        }
        self.regs.write16(reg::CLOCK_CONTROL, value | 1 | CLOCK_ON);
        Ok(actual)
    }

    /// Resets the command line, and the data line with `data`.
    fn reset_lines(&self, data: bool, delay: &mut dyn FnMut(u32)) {
        let bits = if data { 6 } else { 2 };
        self.regs.write8(reg::SOFTWARE_RESET, bits);
        wait(delay, 100_000, || self.regs.read8(reg::SOFTWARE_RESET) & bits == 0);
    }

    /// Sends command `index` with `argument`, without data, and waits for
    /// its `response`.
    fn command(
        &mut self,
        index: u8,
        argument: u32,
        response: Response,
        delay: &mut dyn FnMut(u32),
    ) -> Result<card::Long, Error> {
        let regs = &self.regs;
        let busy = present::COMMAND_INHIBIT
            | if response == Response::R1b { present::DATA_INHIBIT } else { 0 };
        if !wait(delay, 10_000, || regs.read32(reg::PRESENT_STATE) & busy == 0) {
            return Err(Error::Timeout);
        }
        regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
        regs.write16(reg::ERROR_STATUS, 0xffff);
        regs.write32(reg::ARGUMENT, argument);
        regs.write16(reg::TRANSFER_MODE, 0);
        regs.write16(reg::COMMAND, u16::from(index) << 8 | response.flags());
        let done = status::COMMAND_COMPLETE | status::ERROR;
        if !wait(delay, 1_000_000, || regs.read16(reg::NORMAL_STATUS) & done != 0) {
            self.reset_lines(true, delay);
            return Err(Error::Timeout);
        }
        if regs.read16(reg::NORMAL_STATUS) & status::ERROR != 0 {
            let error = regs.read16(reg::ERROR_STATUS);
            regs.write16(reg::ERROR_STATUS, 0xffff);
            regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
            self.reset_lines(true, delay);
            return Err(Error::Command(error));
        }
        regs.write16(reg::NORMAL_STATUS, status::COMMAND_COMPLETE);
        if response == Response::R1b {
            let done = status::TRANSFER_COMPLETE | status::ERROR;
            if !wait(delay, 1_000_000, || regs.read16(reg::NORMAL_STATUS) & done != 0) {
                return Err(Error::Timeout);
            }
            regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
        }
        let long = core::array::from_fn(|i| regs.read32(reg::RESPONSE + 4 * i));
        if matches!(response, Response::R1 | Response::R1b)
            && long[0] & CARD_ERRORS != 0
            && !matches!(index, 3 | 8)
        {
            return Err(Error::Card(long[0]));
        }
        Ok(long)
    }

    /// An application command (ACMD): CMD55 to the card at `rca`, then
    /// `index`.
    fn app_command(
        &mut self,
        rca: u16,
        index: u8,
        argument: u32,
        response: Response,
        delay: &mut dyn FnMut(u32),
    ) -> Result<card::Long, Error> {
        self.command(55, u32::from(rca) << 16, Response::R1, delay)?;
        self.command(index, argument, response, delay)
    }

    /// Powers the card up and sets it up (Physical Layer, 4.2), ready for
    /// [`Slot::start`], in the fastest bus speed mode both it and the slot
    /// have.
    pub fn identify(&mut self, delay: &mut dyn FnMut(u32)) -> Result<Card, Error> {
        if !self.present() {
            return Err(Error::NoCard);
        }
        let uhs = self.version >= version::V3
            && self.caps & caps::VOLTS_1_8 != 0
            && self.caps & (caps::SDR50 | caps::SDR104 | caps::DDR50) != 0;
        if let Some(card) = self.set_up(uhs, delay)? {
            return Ok(card);
        }
        // The card said it would signal at 1.8 V and did not: powered off,
        // it is set up again at 3.3 V (Physical Layer, 4.2.4.2).
        self.power_off(delay);
        self.set_up(false, delay)?.ok_or(Error::Unsupported("the card would not set up"))
    }

    /// Turns the card's power and clock off, and its signaling back to
    /// 3.3 V.
    fn power_off(&mut self, delay: &mut dyn FnMut(u32)) {
        self.regs.write16(reg::CLOCK_CONTROL, 0);
        if self.version >= version::V3 {
            self.regs.write16(reg::HOST_CONTROL2, 0);
        }
        self.regs.write8(reg::POWER_CONTROL, 0);
        delay(10_000);
    }

    /// Sets the card up, asking it to signal at 1.8 V with `uhs`; `None`
    /// if it agreed and then did not.
    fn set_up(&mut self, uhs: bool, delay: &mut dyn FnMut(u32)) -> Result<Option<Card>, Error> {
        self.card = None;
        self.tuned = false;
        self.retune = false;
        // The highest voltage it gives: 3.3, 3.0 or 1.8 V.
        let volts = if self.caps & caps::VOLTS_3_3 != 0 {
            0b111
        } else if self.caps & caps::VOLTS_3_0 != 0 {
            0b110
        } else if self.caps & caps::VOLTS_1_8 != 0 {
            0b101
        } else {
            return Err(Error::Unsupported("it says it gives no voltage"));
        };
        if self.version >= version::V3 {
            self.regs.write16(reg::HOST_CONTROL2, 0);
        }
        if !self.quirks.single_power_write {
            self.regs.write8(reg::POWER_CONTROL, volts << 1);
        }
        self.regs.write8(reg::POWER_CONTROL, volts << 1 | 1);
        delay(10_000);
        self.regs.write8(reg::HOST_CONTROL, 0);
        self.set_clock(400, delay)?;
        // 74 clocks before the first command.
        delay(1_000);
        self.command(0, 0, Response::None, delay)?;
        // Only cards of version 2.00 and later answer CMD8, with the
        // pattern sent back.
        let version2 = match self.command(8, 0x1aa, Response::R1, delay) {
            Ok(long) if long[0] & 0xfff == 0x1aa => true,
            Ok(_) => return Err(Error::Unsupported("the card does not take the slot's voltage")),
            Err(Error::Command(_) | Error::Timeout) => false,
            Err(err) => return Err(err),
        };
        let asked = VOLTAGES
            | if version2 { ocr::HIGH_CAPACITY } else { 0 }
            | if version2 && uhs { ocr::SWITCH_1_8 } else { 0 };
        let mut answer = 0;
        for _ in 0..100 {
            answer = match self.app_command(0, 41, asked, Response::R3, delay) {
                Ok(long) => long[0],
                // An MMC has no ACMD41 (Physical Layer, 4.2.3).
                Err(Error::Command(_) | Error::Timeout) => return self.set_up_mmc(delay).map(Some),
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
        let low_voltage = asked & answer & ocr::SWITCH_1_8 != 0;
        if low_voltage && !self.switch_voltage(delay)? {
            return Ok(None);
        }
        let cid = Cid::parse(&self.command(2, 0, Response::R2, delay)?);
        let rca = (self.command(3, 0, Response::R1, delay)?[0] >> 16) as u16;
        let csd = self.command(9, u32::from(rca) << 16, Response::R2, delay)?;
        let blocks = match Csd::parse(&csd) {
            Csd::Standard(blocks) | Csd::High(blocks) => blocks,
            Csd::Ultra => return Err(Error::Unsupported("it is an SDUC card, not driven yet")),
            Csd::Unknown => return Err(Error::Unsupported("its CSD is of an unknown version")),
        };
        self.command(7, u32::from(rca) << 16, Response::R1b, delay)?;
        let khz = self.set_clock(Speed::Default.khz(), delay)?;
        // Every SD memory card has a four-bit bus.
        self.app_command(rca, 6, 2, Response::R1, delay)?;
        self.regs.write8(reg::HOST_CONTROL, host::WIDTH_4);
        if !high_capacity {
            self.command(16, BLOCK, Response::R1, delay)?;
        }
        let slowest = if low_voltage { Speed::Sdr12 } else { Speed::Default };
        let (speed, khz) = self.speed_up(rca, low_voltage, delay)?.unwrap_or((slowest, khz));
        let read_only = self.regs.read32(reg::PRESENT_STATE) & present::WRITABLE == 0;
        let family = Family::Sd;
        let card =
            Card { family, rca, high_capacity, blocks, cid, speed, khz, width: 4, read_only };
        self.card = Some(card);
        Ok(Some(card))
    }

    /// Switches the card's signaling to 1.8 V after it agreed to
    /// (Host Controller, 3.6.1): CMD11, the clock stopped while the card
    /// holds DAT0 to DAT3 low, 1.8 V, the clock again, and the card lets
    /// them go high. False if a step did not happen.
    fn switch_voltage(&mut self, delay: &mut dyn FnMut(u32)) -> Result<bool, Error> {
        match self.command(11, 0, Response::R1, delay) {
            Ok(_) => {}
            Err(Error::Command(_) | Error::Timeout | Error::Card(_)) => return Ok(false),
            Err(err) => return Err(err),
        }
        let regs = &self.regs;
        let clock = regs.read16(reg::CLOCK_CONTROL);
        regs.write16(reg::CLOCK_CONTROL, clock & !CLOCK_ON);
        if regs.read32(reg::PRESENT_STATE) & present::DAT_LINES != 0 {
            return Ok(false);
        }
        regs.write16(reg::HOST_CONTROL2, regs.read16(reg::HOST_CONTROL2) | host2::SIGNALING_1_8);
        delay(5_000);
        // The controller clears it again if its regulator did not switch.
        if regs.read16(reg::HOST_CONTROL2) & host2::SIGNALING_1_8 == 0 {
            return Ok(false);
        }
        regs.write16(reg::CLOCK_CONTROL, clock | CLOCK_ON);
        delay(1_000);
        Ok(regs.read32(reg::PRESENT_STATE) & present::DAT_LINES == present::DAT_LINES)
    }

    /// Switches the card at `rca`, on the four-bit bus at the default
    /// speed (or SDR12 with `low_voltage`), to the fastest mode it and the
    /// slot both have: the mode and the clock it runs at, or `None` to
    /// stay.
    fn speed_up(
        &mut self,
        rca: u16,
        low_voltage: bool,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Option<(Speed, u32)>, Error> {
        // The SCR's SD_SPEC: a card of version 1.0x has no CMD6.
        let mut scr = [0u8; 8];
        self.read_block(Some(rca), 51, 0, &mut scr, delay)?;
        if scr[0] & 0xf == 0 {
            return Ok(None);
        }
        // CMD6's check mode, changing nothing: the functions it has.
        let mut functions = [0u8; 64];
        self.read_block(None, 6, NO_CHANGE, &mut functions, delay)?;
        let modes = u16::from_be_bytes([functions[12], functions[13]]);
        if !low_voltage {
            if self.caps & caps::HIGH_SPEED == 0 || modes & 1 << 1 == 0 {
                return Ok(None);
            }
            if !self.switch(1, None, delay)? {
                return Ok(None);
            }
            let control = self.regs.read8(reg::HOST_CONTROL);
            self.regs.write8(reg::HOST_CONTROL, control | host::HIGH_SPEED);
            let khz = self.set_clock(Speed::High.khz(), delay)?;
            return Ok(Some((Speed::High, khz)));
        }
        let currents = u16::from_be_bytes([functions[6], functions[7]]);
        let limit = self.current_limit(currents);
        for (speed, function, needs) in [
            (Speed::Sdr104, 3, caps::SDR104),
            (Speed::Ddr50, 4, caps::DDR50),
            (Speed::Sdr50, 2, caps::SDR50),
            (Speed::Sdr25, 1, 0),
        ] {
            if modes & 1 << function == 0 || self.caps & needs != needs {
                continue;
            }
            if let Some(khz) = self.switch_uhs(speed, function, limit, delay)? {
                return Ok(Some((speed, khz)));
            }
        }
        Ok(None)
    }

    /// The current limit (group 4) to ask for: the most of 400, 600 and
    /// 800 mA that the card takes and the slot gives at 3.3 V, or `None`
    /// to leave the card's 200 mA.
    fn current_limit(&self, card: u16) -> Option<u8> {
        let slot_ma = (self.regs.read32(reg::MAX_CURRENT) & 0xff) * 4;
        [(3, 800), (2, 600), (1, 400)]
            .into_iter()
            .find(|&(function, ma)| card & 1 << function != 0 && slot_ma >= ma)
            .map(|(function, _)| function)
    }

    /// CMD6 in switch mode: function `function` of group 1, and `limit`
    /// of group 4 if given. True if the card switched to it.
    fn switch(
        &mut self,
        function: u8,
        limit: Option<u8>,
        delay: &mut dyn FnMut(u32),
    ) -> Result<bool, Error> {
        let mut argument = 1 << 31 | NO_CHANGE & !0xf | u32::from(function);
        if let Some(limit) = limit {
            argument = argument & !0xf000 | u32::from(limit) << 12;
        }
        let mut result = [0u8; 64];
        self.read_block(None, 6, argument, &mut result, delay)?;
        // Group 1's function, as switched to: bits 379 to 376.
        Ok(result[16] & 0xf == function)
    }

    /// Switches the card to UHS-I mode `speed`, its function `function`
    /// with current limit `limit`, and the slot with it, tuning where it
    /// must: the clock it runs at, or `None` if it did not take or tune,
    /// the slot set back to SDR12.
    fn switch_uhs(
        &mut self,
        speed: Speed,
        function: u8,
        limit: Option<u8>,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Option<u32>, Error> {
        let limit = if matches!(speed, Speed::Sdr25) { None } else { limit };
        if !self.switch(function, limit, delay)? {
            return Ok(None);
        }
        let khz = self.uhs_mode(speed, delay)?;
        let tunes =
            speed == Speed::Sdr104 || (speed == Speed::Sdr50 && self.caps & caps::TUNE_SDR50 != 0);
        if tunes && !self.tune(SD_TUNING, delay) {
            self.uhs_mode(Speed::Sdr12, delay)?;
            return Ok(None);
        }
        self.tuned = tunes;
        Ok(Some(khz))
    }

    /// Sets the slot to UHS-I mode `speed`, with its clock stopped while
    /// the mode changes: the clock it runs at.
    fn uhs_mode(&mut self, speed: Speed, delay: &mut dyn FnMut(u32)) -> Result<u32, Error> {
        // HS200 is timed as SDR104 is (Host Controller, 2.2.30).
        let mode = match speed {
            Speed::Sdr25 => 1,
            Speed::Sdr50 => 2,
            Speed::Sdr104 | Speed::Hs200 => 3,
            Speed::Ddr50 => 4,
            Speed::Default | Speed::High | Speed::Sdr12 | Speed::MmcHigh => 0,
        };
        let regs = &self.regs;
        regs.write16(reg::CLOCK_CONTROL, regs.read16(reg::CLOCK_CONTROL) & !CLOCK_ON);
        let control2 = regs.read16(reg::HOST_CONTROL2) & !host2::UHS_MODE;
        regs.write16(reg::HOST_CONTROL2, control2 | mode);
        let control = regs.read8(reg::HOST_CONTROL) & !host::HIGH_SPEED;
        let fast = if mode == 0 { 0 } else { host::HIGH_SPEED };
        regs.write8(reg::HOST_CONTROL, control | fast);
        self.set_clock(speed.khz(), delay)
    }

    /// Tunes the slot's sampling point for the mode it runs in (Host
    /// Controller, 2.2.26): the tuning blocks of `block` (its command and
    /// size), 40 at most, until the controller says it is done. True if
    /// it found one.
    fn tune(&mut self, (command, size): (u16, u16), delay: &mut dyn FnMut(u32)) -> bool {
        let regs = &self.regs;
        let control2 = regs.read16(reg::HOST_CONTROL2) & !host2::SAMPLING_CLOCK;
        regs.write16(reg::HOST_CONTROL2, control2 | host2::EXECUTE_TUNING);
        let busy = present::COMMAND_INHIBIT | present::DATA_INHIBIT;
        for _ in 0..40 {
            if !wait(delay, 10_000, || regs.read32(reg::PRESENT_STATE) & busy == 0) {
                break;
            }
            regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
            regs.write16(reg::ERROR_STATUS, 0xffff);
            regs.write16(reg::BLOCK_SIZE, size);
            regs.write16(reg::BLOCK_COUNT, 1);
            regs.write32(reg::ARGUMENT, 0);
            regs.write16(reg::TRANSFER_MODE, mode::READ);
            regs.write16(reg::COMMAND, command << 8 | Response::R1.flags() | DATA_PRESENT);
            let done = status::BUFFER_READ_READY | status::ERROR;
            let came = wait(delay, 10_000, || regs.read16(reg::NORMAL_STATUS) & done != 0);
            regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
            regs.write16(reg::ERROR_STATUS, 0xffff);
            if !came {
                self.reset_lines(true, delay);
            }
            if regs.read16(reg::HOST_CONTROL2) & host2::EXECUTE_TUNING == 0 {
                break;
            }
        }
        let control2 = regs.read16(reg::HOST_CONTROL2);
        if control2 & host2::EXECUTE_TUNING == 0 && control2 & host2::SAMPLING_CLOCK != 0 {
            return true;
        }
        let untuned = control2 & !(host2::EXECUTE_TUNING | host2::SAMPLING_CLOCK);
        regs.write16(reg::HOST_CONTROL2, untuned);
        self.reset_lines(true, delay);
        false
    }

    /// Tunes the slot again after a transfer came back garbled; a card
    /// whose mode no longer tunes falls back to SDR25, or an MMC to High
    /// Speed, which are not tuned.
    fn retune(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        self.retune = false;
        let block = match self.card {
            Some(card) if card.family != Family::Sd => mmc::tuning(card.width),
            _ => SD_TUNING,
        };
        if self.tune(block, delay) {
            return Ok(());
        }
        self.tuned = false;
        if let Some(card) = self.card
            && card.family != Family::Sd
        {
            let speed = self.mmc_fall_back(delay)?;
            if let Some(card) = &mut self.card {
                (card.speed, card.khz) = speed;
            }
            return Ok(());
        }
        let khz = self.uhs_mode(Speed::Sdr12, delay)?;
        let speed = match self.switch_uhs(Speed::Sdr25, 1, None, delay)? {
            Some(khz) => (Speed::Sdr25, khz),
            None => (Speed::Sdr12, khz),
        };
        if let Some(card) = &mut self.card {
            (card.speed, card.khz) = speed;
        }
        Ok(())
    }

    /// Reads `out`, one block of at most 512 bytes, a multiple of four,
    /// from the card with command `index` (an ACMD if `app` gives the
    /// card's address), through the Buffer Data Port: the SCR and CMD6's
    /// status, which are too small to be worth a descriptor.
    fn read_block(
        &mut self,
        app: Option<u16>,
        index: u8,
        argument: u32,
        out: &mut [u8],
        delay: &mut dyn FnMut(u32),
    ) -> Result<(), Error> {
        if let Some(rca) = app {
            self.command(55, u32::from(rca) << 16, Response::R1, delay)?;
        }
        let regs = &self.regs;
        let busy = present::COMMAND_INHIBIT | present::DATA_INHIBIT;
        if !wait(delay, 10_000, || regs.read32(reg::PRESENT_STATE) & busy == 0) {
            return Err(Error::Timeout);
        }
        regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
        regs.write16(reg::ERROR_STATUS, 0xffff);
        regs.write16(reg::BLOCK_SIZE, out.len() as u16);
        regs.write16(reg::BLOCK_COUNT, 1);
        regs.write32(reg::ARGUMENT, argument);
        regs.write16(reg::TRANSFER_MODE, mode::READ);
        regs.write16(reg::COMMAND, u16::from(index) << 8 | Response::R1.flags() | DATA_PRESENT);
        let done = status::BUFFER_READ_READY | status::ERROR;
        if !wait(delay, 1_000_000, || regs.read16(reg::NORMAL_STATUS) & done != 0) {
            self.reset_lines(true, delay);
            return Err(Error::Timeout);
        }
        if regs.read16(reg::NORMAL_STATUS) & status::ERROR != 0 {
            let error = regs.read16(reg::ERROR_STATUS);
            regs.write16(reg::ERROR_STATUS, 0xffff);
            regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
            self.reset_lines(true, delay);
            return Err(Error::Command(error));
        }
        regs.write16(reg::NORMAL_STATUS, status::BUFFER_READ_READY);
        let (words, _) = out.as_chunks_mut::<4>();
        for word in words {
            *word = regs.read32(reg::BUFFER).to_le_bytes();
        }
        let done = status::TRANSFER_COMPLETE | status::ERROR;
        let ended = wait(delay, 1_000_000, || regs.read16(reg::NORMAL_STATUS) & done != 0);
        regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
        regs.write16(reg::ERROR_STATUS, 0xffff);
        let card_status = regs.read32(reg::RESPONSE);
        if !ended {
            self.reset_lines(true, delay);
            return Err(Error::Timeout);
        }
        if card_status & CARD_ERRORS != 0 {
            return Err(Error::Card(card_status));
        }
        Ok(())
    }

    /// Starts `op` on `count` blocks from block `block`, to or from
    /// `buffers` (physical address and length, each a multiple of four
    /// bytes, together the blocks): the command goes once the lines are
    /// free, which [`Slot::poll`] looks for.
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
            len > 0
                && len <= MAX_BUFFER
                && len % 4 == 0
                && phys % 4 == 0
                && (self.adma64 || phys + u64::from(len) <= 1 << 32)
        };
        // SDMA moves one buffer: each follows the one before.
        let together = buffers.windows(2).all(|pair| pair[0].0 + u64::from(pair[0].1) == pair[1].0);
        if self.flight.is_some()
            || count == 0
            || count > u32::from(u16::MAX)
            || buffers.is_empty()
            || buffers.len() > MAX_BUFFERS
            || bytes != u64::from(count) * u64::from(BLOCK)
            || !buffers.iter().all(fits)
            || (self.sdma && !together)
            || block + u64::from(count) > card.blocks
            || (op == Op::Write && card.read_only)
        {
            return Err(Error::BadRequest);
        }
        if self.stuck {
            // A transfer failed: the card is stopped, and both lines start
            // over.
            self.reset_lines(true, delay);
            let _ = self.command(12, 0, Response::R1b, delay);
            self.stuck = false;
        }
        if self.retune {
            self.retune(delay)?;
        }
        // SDMA starts at the first buffer's address; ADMA2 follows the
        // descriptors.
        let sdma = self.sdma.then(|| buffers[0].0 as u32);
        if !self.sdma {
            self.describe(buffers);
        }
        device_barrier();
        let address = if card.high_capacity { block } else { block * u64::from(BLOCK) };
        let argument = u32::try_from(address).map_err(|_| Error::BadRequest)?;
        let multiple = count > 1;
        let index: u16 = match (op, multiple) {
            (Op::Read, false) => 17,
            (Op::Read, true) => 18,
            (Op::Write, false) => 24,
            (Op::Write, true) => 25,
        };
        let mut transfer = mode::DMA | mode::BLOCK_COUNT;
        if multiple {
            transfer |= mode::MULTIPLE | mode::AUTO_CMD12;
        }
        if op == Op::Read {
            transfer |= mode::READ;
        }
        let command = index << 8 | Response::R1.flags() | DATA_PRESENT;
        let count = count as u16;
        self.flight = Some(Flight::Ready { command, mode: transfer, argument, count, sdma });
        self.issue();
        Ok(())
    }

    /// Writes the ADMA2 descriptors of `buffers`: Valid and Act=Tran, End
    /// on the last.
    fn describe(&mut self, buffers: &[(u64, u32)]) {
        let size = if self.adma64 { 12 } else { 8 };
        for (i, &(phys, len)) in buffers.iter().enumerate() {
            let last = i + 1 == buffers.len();
            let attributes: u16 = 0x21 | if last { 0x02 } else { 0 };
            let at = i * size;
            self.table.write16(at, attributes);
            self.table.write16(at + 2, len as u16);
            self.table.write32(at + 4, phys as u32);
            if self.adma64 {
                self.table.write32(at + 8, (phys >> 32) as u32);
            }
        }
    }

    /// Sends the command of the request that waits, if the lines are free.
    fn issue(&mut self) {
        let Some(Flight::Ready { command, mode, argument, count, sdma }) = self.flight else {
            return;
        };
        let regs = &self.regs;
        let busy = present::COMMAND_INHIBIT | present::DATA_INHIBIT;
        if regs.read32(reg::PRESENT_STATE) & busy != 0 {
            return;
        }
        // The lines may still be resetting after the last request, on a
        // controller that resets them after each.
        if self.quirks.reset_after_request && regs.read8(reg::SOFTWARE_RESET) != 0 {
            return;
        }
        // The DMA select: SDMA, 32-bit ADMA2 or 64-bit ADMA2. SDMA stops
        // at each 512 KiB boundary of memory it crosses (Block Size's
        // boundary 7), for the address to be written again.
        let (dma, boundary) = match sdma {
            Some(address) => {
                regs.write32(reg::SDMA_ADDRESS, address);
                (0b00 << 3, 7 << 12)
            }
            None => {
                let table = self.table.phys();
                regs.write32(reg::ADMA_ADDRESS, table as u32);
                regs.write32(reg::ADMA_ADDRESS + 4, (table >> 32) as u32);
                (if self.adma64 { 0b11 << 3 } else { 0b10 << 3 }, 0)
            }
        };
        regs.write8(reg::HOST_CONTROL, regs.read8(reg::HOST_CONTROL) & !(0b11 << 3) | dma);
        regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
        regs.write16(reg::ERROR_STATUS, 0xffff);
        regs.write16(reg::BLOCK_SIZE, BLOCK as u16 | boundary);
        regs.write16(reg::BLOCK_COUNT, count);
        regs.write32(reg::ARGUMENT, argument);
        regs.write16(reg::TRANSFER_MODE, mode);
        regs.write16(reg::COMMAND, command);
        self.flight = Some(Flight::Sent { status: None });
    }

    /// How the request in flight ended; `None` while it goes on, or if
    /// there is none.
    pub fn poll(&mut self) -> Option<Result<(), Error>> {
        self.issue();
        let Some(Flight::Sent { status }) = self.flight else { return None };
        let regs = &self.regs;
        let normal = regs.read16(reg::NORMAL_STATUS);
        if normal & status::ERROR != 0 {
            let error = regs.read16(reg::ERROR_STATUS);
            regs.write16(reg::ERROR_STATUS, 0xffff);
            regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
            self.flight = None;
            self.stuck = true;
            self.retune = self.tuned && error & status::DATA_GARBLED != 0;
            return Some(Err(Error::Command(error)));
        }
        if normal & status::DMA_INTERRUPT != 0 {
            // At a boundary: the address it stopped at, written back, has
            // it go on (Host Controller, 1.13.1).
            regs.write16(reg::NORMAL_STATUS, status::DMA_INTERRUPT);
            regs.write32(reg::SDMA_ADDRESS, regs.read32(reg::SDMA_ADDRESS));
        }
        let mut status = status;
        if normal & status::COMMAND_COMPLETE != 0 {
            regs.write16(reg::NORMAL_STATUS, status::COMMAND_COMPLETE);
            status = Some(regs.read32(reg::RESPONSE));
            self.flight = Some(Flight::Sent { status });
        }
        if normal & status::TRANSFER_COMPLETE == 0 {
            return None;
        }
        regs.write16(reg::NORMAL_STATUS, status::ALL_BUT_CARD);
        self.flight = None;
        if self.quirks.reset_after_request {
            // The lines' reset ends within a few of the controller's
            // clocks; the next request waits for them anyway.
            regs.write8(reg::SOFTWARE_RESET, 6);
        }
        match status {
            Some(bits) if bits & CARD_ERRORS != 0 => {
                self.stuck = true;
                Some(Err(Error::Card(bits)))
            }
            _ => Some(Ok(())),
        }
    }

    /// Gives up the request in flight, which took too long: the lines are
    /// reset, and the card stopped before the next.
    pub fn abandon(&mut self, delay: &mut dyn FnMut(u32)) {
        self.flight = None;
        self.reset_lines(true, delay);
        self.stuck = true;
    }
}
