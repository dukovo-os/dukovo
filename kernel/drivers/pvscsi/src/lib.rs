// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMware's paravirtual SCSI adapter, PVSCSI (ADR 0424), which VMware's
//! hypervisors give a guest's disks when asked for their fastest
//! controller, and QEMU emulates as its `pvscsi`.
//!
//! The adapter has few registers: commands to it go through a command
//! register, then their arguments a 32-bit word at a time through a data
//! register, and two doorbells say that requests wait. Everything else is
//! in memory the driver gives it:
//!
//! - the *rings' state*, a page holding the producer and consumer indexes
//!   of the three rings: the driver moves the requests' producer and the
//!   completions' and messages' consumers, the adapter the others;
//! - the *request ring*, 128-byte descriptors, each a SCSI command: its
//!   target and logical unit, its command descriptor block, where its data
//!   are (one run of memory, or a list of runs) and where its sense data go,
//!   and a context number the driver chooses, never 0;
//! - the *completion ring*, 32-byte descriptors, each the context of a
//!   command that ended with its adapter's and target's statuses and how
//!   much it moved;
//! - the *message ring*, if the adapter has one, 128-byte descriptors that
//!   say a disk was added or removed.
//!
//! The rings' sizes are powers of two, set by how many pages each has. The
//! driver keeps [`CONTEXTS`] commands in flight at most, each with a list
//! of up to [`MAX_SEGMENTS`] runs and room for [`SENSE_SIZE`] bytes of
//! sense data of its own. Reads and writes are announced on one doorbell,
//! other commands on the other, as the adapter asks.
//!
//! The layouts and the sequences follow FreeBSD's pvscsi driver, which
//! VMware wrote (BSD-2-Clause, credits/freebsd-pvscsi.md).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use radix_hw::{Dma, Mmio, device_barrier};

/// VMware's PCI vendor ID.
pub const VENDOR: u16 = 0x15ad;
/// The adapter's PCI device ID.
pub const DEVICE: u16 = 0x07c0;
/// The size of a page, the unit of the rings.
pub const PAGE: usize = 4096;
/// How much of the adapter's register BAR the driver uses.
pub const REGISTERS: usize = 0x4020;

/// The adapter's registers, by their offsets in its memory BAR.
mod reg {
    /// A command to the adapter (see [`super::command`]).
    pub const COMMAND: usize = 0x0000;
    /// The command's arguments, a 32-bit word at a time.
    pub const COMMAND_DATA: usize = 0x0004;
    /// What the last command answered.
    pub const COMMAND_STATUS: usize = 0x0008;
    /// The interrupts raised; written back to clear them.
    pub const INTERRUPT_STATUS: usize = 0x100c;
    /// The interrupts the adapter may raise.
    pub const INTERRUPT_MASK: usize = 0x2010;
    /// The doorbell for requests other than reads and writes.
    pub const KICK_OTHER: usize = 0x3014;
    /// The doorbell for reads and writes.
    pub const KICK_READ_WRITE: usize = 0x4018;
}

/// The commands the adapter takes.
mod command {
    pub const ADAPTER_RESET: u32 = 1;
    pub const SETUP_RINGS: u32 = 3;
    pub const SETUP_MESSAGE_RING: u32 = 8;
    pub const GET_MAX_TARGETS: u32 = 11;
}

/// What a command's status register says when the adapter does not know
/// the command.
const UNKNOWN: u32 = u32::MAX;

/// The interrupts: completions (two bits) and messages (two bits).
mod interrupt {
    pub const COMPLETIONS: u32 = 0b0011;
    pub const MESSAGES: u32 = 0b1100;
    pub const ALL: u32 = COMPLETIONS | MESSAGES;
}

/// A request's flags.
mod flag {
    /// Its data address is that of a list of runs.
    pub const WITH_LIST: u32 = 1 << 0;
    /// It moves no data.
    pub const NO_DATA: u32 = 1 << 2;
    /// Its data go to the host: a read.
    pub const TO_HOST: u32 = 1 << 3;
    /// Its data go to the device: a write.
    pub const TO_DEVICE: u32 = 1 << 4;
}

/// How the adapter says a command went, before the target's status: it
/// delivered the command, or why not.
pub mod host_status {
    /// The command reached the target, whose status says how it ended.
    pub const SUCCESS: u16 = 0x00;
    /// A linked command ended: as [`SUCCESS`].
    pub const LINKED_COMMAND_COMPLETED: u16 = 0x0a;
    /// A linked command ended with its flag: as [`SUCCESS`].
    pub const LINKED_COMMAND_COMPLETED_WITH_FLAG: u16 = 0x0b;
    /// The command moved less than its buffers held.
    pub const DATA_UNDERRUN: u16 = 0x0c;
    /// Nothing answered at the target: there is no disk there.
    pub const SELECTION_TIMEOUT: u16 = 0x11;
    /// The command moved more than its buffers held.
    pub const DATA_OVERRUN: u16 = 0x12;
    /// The target did not answer the adapter.
    pub const NO_RESPONSE: u16 = 0x21;
    /// The adapter reset the target.
    pub const SENT_RESET: u16 = 0x22;
    /// The target reset.
    pub const RECEIVED_RESET: u16 = 0x23;
    /// The bus was reset.
    pub const BUS_RESET: u16 = 0x25;
    /// The adapter dropped the command, which may go again.
    pub const ABORT_QUEUE: u16 = 0x26;
    /// The adapter timed the command out.
    pub const ADAPTER_TIMEOUT: u16 = 0x30;
}

/// The target's statuses the driver tells apart (SAM-5).
pub mod scsi_status {
    /// The command did what it asked.
    pub const GOOD: u16 = 0x00;
    /// The command failed; the sense data say why.
    pub const CHECK_CONDITION: u16 = 0x02;
    /// The target is busy: the command may go again.
    pub const BUSY: u16 = 0x08;
    /// The target's queue is full: the command may go again.
    pub const TASK_SET_FULL: u16 = 0x28;
}

/// The pages of each ring: 256 requests of 128 bytes, 256 completions of
/// 32 bytes, and 32 messages of 128 bytes.
const REQUEST_PAGES: usize = 8;
const COMPLETION_PAGES: usize = 2;
const MESSAGE_PAGES: usize = 1;
/// The size of a descriptor of each ring.
const REQUEST_SIZE: usize = 128;
const COMPLETION_SIZE: usize = 32;
const MESSAGE_SIZE: usize = 128;
/// The descriptors of each ring.
const REQUESTS: usize = REQUEST_PAGES * PAGE / REQUEST_SIZE;
const COMPLETIONS: usize = COMPLETION_PAGES * PAGE / COMPLETION_SIZE;
const MESSAGES: usize = MESSAGE_PAGES * PAGE / MESSAGE_SIZE;

/// The most commands in flight: one for each request descriptor.
pub const CONTEXTS: usize = REQUESTS;
/// The most runs of memory one command's data take.
pub const MAX_SEGMENTS: usize = 32;
/// The sense data kept for each command.
pub const SENSE_SIZE: usize = 96;
/// The longest command descriptor block.
pub const CDB_SIZE: usize = 16;
/// The size of a run in a list: its address, length and flags.
const RUN_SIZE: usize = 16;
/// The room for each command's list of runs.
const LIST_SIZE: usize = MAX_SEGMENTS * RUN_SIZE;
/// The task attribute of every command: a simple queue tag.
const SIMPLE_TAG: u8 = 0x20;

/// Where the rings' state page holds each index.
mod state {
    pub const REQUEST_PRODUCER: usize = 0;
    pub const COMPLETION_PRODUCER: usize = 12;
    pub const COMPLETION_CONSUMER: usize = 16;
    pub const MESSAGE_PRODUCER: usize = 128;
    pub const MESSAGE_CONSUMER: usize = 132;
}

/// Where a request descriptor holds each field.
mod request {
    pub const CONTEXT: usize = 0;
    pub const DATA_ADDRESS: usize = 8;
    pub const DATA_LENGTH: usize = 16;
    pub const SENSE_ADDRESS: usize = 24;
    pub const SENSE_LENGTH: usize = 32;
    pub const FLAGS: usize = 36;
    pub const CDB: usize = 40;
    pub const CDB_LENGTH: usize = 56;
    pub const LUN: usize = 57;
    pub const TAG: usize = 65;
    pub const BUS: usize = 66;
    pub const TARGET: usize = 67;
}

/// Where a completion descriptor holds each field.
mod completion {
    pub const CONTEXT: usize = 0;
    pub const DATA_LENGTH: usize = 8;
    pub const SENSE_LENGTH: usize = 16;
    pub const HOST_STATUS: usize = 20;
    pub const SCSI_STATUS: usize = 22;
}

/// What a message says happened.
mod message {
    pub const DEVICE_ADDED: u32 = 0;
    pub const DEVICE_REMOVED: u32 = 1;
}

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The register BAR is smaller than the adapter's registers.
    Registers,
    /// The memory for the rings could not be had.
    OutOfMemory,
    /// The adapter did not do what it was asked: what.
    Device(&'static str),
    /// A command the adapter cannot carry: a command descriptor block over
    /// [`CDB_SIZE`] bytes, or data in more than [`MAX_SEGMENTS`] runs.
    BadRequest,
    /// Every context, or every request descriptor, is taken.
    Full,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Registers => f.write_str("its registers are not all mapped"),
            Error::OutOfMemory => f.write_str("out of memory"),
            Error::Device(what) => f.write_str(what),
            Error::BadRequest => f.write_str("a command it cannot carry"),
            Error::Full => f.write_str("its rings are full"),
        }
    }
}

/// Where a command goes: a target, and a logical unit of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Address {
    /// The target, below [`Pvscsi::targets`].
    pub target: u8,
    /// The logical unit of the target.
    pub lun: u8,
}

/// Which way a command's data go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// It moves none.
    None,
    /// From the device into memory: a read.
    FromDevice,
    /// From memory to the device: a write.
    ToDevice,
}

/// How a command ended.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    /// The adapter's status ([`host_status`]).
    pub host_status: u16,
    /// The target's status ([`scsi_status`]).
    pub scsi_status: u16,
    /// How many bytes it moved.
    pub moved: u64,
    /// How many bytes it was asked to move.
    pub asked: u64,
    sense: [u8; SENSE_SIZE],
    sense_len: usize,
}

impl Answer {
    /// The sense data of a command that failed.
    pub fn sense(&self) -> &[u8] {
        &self.sense[..self.sense_len]
    }

    /// Whether the command went through and the target took it.
    pub fn good(&self) -> bool {
        self.host_status == host_status::SUCCESS && self.scsi_status == scsi_status::GOOD
    }
}

/// What a message says: a disk added at an address, or removed from one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// A disk was added at this address.
    Added(Address),
    /// The disk at this address was removed.
    Removed(Address),
    /// A message of a kind the driver does not know.
    Other(u32),
}

/// A PVSCSI adapter, set up.
pub struct Pvscsi {
    regs: Mmio,
    state: Dma,
    requests: Dma,
    completions: Dma,
    messages: Option<Dma>,
    /// Each context's list of runs, [`LIST_SIZE`] bytes apiece.
    lists: Dma,
    /// Each context's sense data, [`SENSE_SIZE`] bytes apiece.
    sense: Dma,
    /// How much each context asked to move, 8 bytes apiece: the driver's
    /// alone, kept off the stack the value moves through.
    asked: Dma,
    /// The contexts in flight, a bit each.
    busy: [u64; CONTEXTS / 64],
    /// How many targets the adapter has.
    targets: u16,
}

impl Pvscsi {
    /// Resets the adapter whose registers are `regs` and gives it its
    /// rings, from `alloc`, which returns that many zeroed pages of DMA
    /// memory. Its interrupts stay off until [`Pvscsi::enable_interrupts`].
    pub fn new(regs: Mmio, alloc: &mut dyn FnMut(usize) -> Option<Dma>) -> Result<Pvscsi, Error> {
        if regs.len() < REGISTERS {
            return Err(Error::Registers);
        }
        regs.write32(reg::INTERRUPT_MASK, 0);
        write_command(&regs, command::ADAPTER_RESET, &[]);
        // Reading the interrupt status lets the reset finish.
        let _ = regs.read32(reg::INTERRUPT_STATUS);
        write_command(&regs, command::GET_MAX_TARGETS, &[]);
        let targets = match regs.read32(reg::COMMAND_STATUS) {
            UNKNOWN | 0 => 16,
            count => count.min(256) as u16,
        };
        // An adapter that has a message ring says so by knowing the
        // command that sets it up.
        regs.write32(reg::COMMAND, command::SETUP_MESSAGE_RING);
        let has_messages = regs.read32(reg::COMMAND_STATUS) != UNKNOWN;
        let mut pages = |count: usize| alloc(count).ok_or(Error::OutOfMemory);
        let state = pages(1)?;
        let requests = pages(REQUEST_PAGES)?;
        let completions = pages(COMPLETION_PAGES)?;
        let lists = pages((CONTEXTS * LIST_SIZE).div_ceil(PAGE))?;
        let sense = pages((CONTEXTS * SENSE_SIZE).div_ceil(PAGE))?;
        let asked = pages((CONTEXTS * 8).div_ceil(PAGE))?;
        let messages = if has_messages { Some(pages(MESSAGE_PAGES)?) } else { None };
        for dma in [&state, &requests, &completions, &lists, &sense] {
            dma.zero();
        }
        // SETUP_RINGS: the pages of each ring, and the state's.
        let mut words = [0u32; 4 + 4 * 32];
        words[0] = REQUEST_PAGES as u32;
        words[1] = COMPLETION_PAGES as u32;
        put_page_number(&mut words, 2, state.phys());
        for page in 0..REQUEST_PAGES {
            put_page_number(&mut words, 4 + 2 * page, requests.phys() + (page * PAGE) as u64);
        }
        for page in 0..COMPLETION_PAGES {
            let at = 4 + 2 * 32 + 2 * page;
            put_page_number(&mut words, at, completions.phys() + (page * PAGE) as u64);
        }
        device_barrier();
        write_command(&regs, command::SETUP_RINGS, &words);
        if let Some(ring) = &messages {
            ring.zero();
            // SETUP_MSG_RING: its pages, after a word of padding.
            let mut words = [0u32; 2 + 2 * 16];
            words[0] = MESSAGE_PAGES as u32;
            for page in 0..MESSAGE_PAGES {
                put_page_number(&mut words, 2 + 2 * page, ring.phys() + (page * PAGE) as u64);
            }
            device_barrier();
            write_command(&regs, command::SETUP_MESSAGE_RING, &words);
        }
        // The adapter says how big it took each ring to be.
        device_barrier();
        if state.read32(8) != REQUESTS.trailing_zeros()
            || state.read32(20) != COMPLETIONS.trailing_zeros()
        {
            return Err(Error::Device("the adapter did not take its rings"));
        }
        Ok(Pvscsi {
            regs,
            state,
            requests,
            completions,
            messages,
            lists,
            sense,
            asked,
            busy: [0; CONTEXTS / 64],
            targets,
        })
    }

    /// How many targets the adapter has: 0 to this less one.
    pub fn targets(&self) -> u16 {
        self.targets
    }

    /// Whether the adapter says when disks come and go.
    pub fn has_messages(&self) -> bool {
        self.messages.is_some()
    }

    /// Lets the adapter interrupt for completions and messages.
    pub fn enable_interrupts(&self) {
        let mask = if self.messages.is_some() { interrupt::ALL } else { interrupt::COMPLETIONS };
        self.regs.write32(reg::INTERRUPT_MASK, mask);
    }

    /// Stops the adapter interrupting.
    pub fn disable_interrupts(&self) {
        self.regs.write32(reg::INTERRUPT_MASK, 0);
    }

    /// Clears the interrupts the adapter raised; returns them.
    pub fn acknowledge(&self) -> u32 {
        let raised = self.regs.read32(reg::INTERRUPT_STATUS) & interrupt::ALL;
        if raised != 0 {
            self.regs.write32(reg::INTERRUPT_STATUS, raised);
        }
        raised
    }

    /// How many more commands can go now.
    pub fn room(&self) -> usize {
        let free =
            CONTEXTS - self.busy.iter().map(|word| word.count_ones() as usize).sum::<usize>();
        let produced = self.state.read32(state::REQUEST_PRODUCER);
        let consumed = self.state.read32(state::COMPLETION_CONSUMER);
        let ring = REQUESTS.saturating_sub(produced.wrapping_sub(consumed) as usize);
        free.min(ring)
    }

    /// Puts the command `cdb` to `at`, whose data are the runs `data`
    /// (physical address, length) going `direction`, on the request ring.
    /// Returns its context, which its completion carries. The adapter is
    /// not told until [`Pvscsi::kick`].
    pub fn submit(
        &mut self,
        at: Address,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        if cdb.is_empty() || cdb.len() > CDB_SIZE || data.len() > MAX_SEGMENTS {
            return Err(Error::BadRequest);
        }
        if self.room() == 0 {
            return Err(Error::Full);
        }
        let context = self.take_context().ok_or(Error::Full)?;
        let produced = self.state.read32(state::REQUEST_PRODUCER);
        let slot = (produced as usize % REQUESTS) * REQUEST_SIZE;
        let ring = &self.requests;
        ring.write(slot, &[0; REQUEST_SIZE]);
        let length = data.iter().map(|&(_, len)| u64::from(len)).sum::<u64>();
        let (address, list) = match data {
            [] => (0, 0),
            [(phys, _)] => (*phys, 0),
            runs => {
                let base = usize::from(context) * LIST_SIZE;
                for (n, &(phys, len)) in runs.iter().enumerate() {
                    let at = base + n * RUN_SIZE;
                    self.lists.write64(at, phys);
                    self.lists.write32(at + 8, len);
                    self.lists.write32(at + 12, 0);
                }
                (self.lists.phys() + base as u64, flag::WITH_LIST)
            }
        };
        let way = match direction {
            Direction::None => flag::NO_DATA,
            Direction::FromDevice => flag::TO_HOST,
            Direction::ToDevice => flag::TO_DEVICE,
        };
        let sense = self.sense.phys() + (usize::from(context) * SENSE_SIZE) as u64;
        ring.write64(slot + request::CONTEXT, u64::from(context) + 1);
        ring.write64(slot + request::DATA_ADDRESS, address);
        ring.write64(slot + request::DATA_LENGTH, length);
        ring.write64(slot + request::SENSE_ADDRESS, sense);
        ring.write32(slot + request::SENSE_LENGTH, SENSE_SIZE as u32);
        ring.write32(slot + request::FLAGS, way | list);
        ring.write(slot + request::CDB, cdb);
        ring.write8(slot + request::CDB_LENGTH, cdb.len() as u8);
        // A single-level logical unit number, in the second byte.
        ring.write8(slot + request::LUN + 1, at.lun);
        ring.write8(slot + request::TAG, SIMPLE_TAG);
        ring.write8(slot + request::BUS, 0);
        ring.write8(slot + request::TARGET, at.target);
        self.asked.write64(usize::from(context) * 8, length);
        device_barrier();
        self.state.write32(state::REQUEST_PRODUCER, produced.wrapping_add(1));
        Ok(context)
    }

    /// Tells the adapter that requests wait: on the doorbell for reads
    /// and writes if `read_write`, else on the other.
    pub fn kick(&self, read_write: bool) {
        device_barrier();
        let doorbell = if read_write { reg::KICK_READ_WRITE } else { reg::KICK_OTHER };
        self.regs.write32(doorbell, 0);
    }

    /// The next command that ended: its context, and how.
    pub fn complete(&mut self) -> Option<(u16, Answer)> {
        loop {
            device_barrier();
            let consumed = self.state.read32(state::COMPLETION_CONSUMER);
            if consumed == self.state.read32(state::COMPLETION_PRODUCER) {
                return None;
            }
            let slot = (consumed as usize % COMPLETIONS) * COMPLETION_SIZE;
            let ring = &self.completions;
            let context = ring.read64(slot + completion::CONTEXT);
            let moved = ring.read64(slot + completion::DATA_LENGTH);
            let sense_len = ring.read32(slot + completion::SENSE_LENGTH) as usize;
            let host_status = ring.read16(slot + completion::HOST_STATUS);
            let scsi_status = ring.read16(slot + completion::SCSI_STATUS);
            device_barrier();
            self.state.write32(state::COMPLETION_CONSUMER, consumed.wrapping_add(1));
            // A context the driver did not give out is passed over.
            let Some(context) = context.checked_sub(1).and_then(|c| u16::try_from(c).ok()) else {
                continue;
            };
            if !self.give_back(context) {
                continue;
            }
            let asked = self.asked.read64(usize::from(context) * 8);
            // A command that ended well moved all it was asked to: the
            // adapter's count is only meant for one that came up short,
            // and some adapters leave it at zero otherwise.
            let ended_well =
                host_status == host_status::SUCCESS && scsi_status == scsi_status::GOOD;
            let mut answer = Answer {
                host_status,
                scsi_status,
                moved: if ended_well { asked } else { moved },
                asked,
                sense: [0; SENSE_SIZE],
                sense_len: 0,
            };
            if scsi_status == scsi_status::CHECK_CONDITION {
                answer.sense_len = sense_len.min(SENSE_SIZE);
                let at = usize::from(context) * SENSE_SIZE;
                self.sense.read(at, &mut answer.sense[..answer.sense_len]);
            }
            return Some((context, answer));
        }
    }

    /// The next message the adapter sent: a disk added or removed.
    pub fn message(&mut self) -> Option<Message> {
        let ring = self.messages.as_ref()?;
        device_barrier();
        let consumed = self.state.read32(state::MESSAGE_CONSUMER);
        if consumed == self.state.read32(state::MESSAGE_PRODUCER) {
            return None;
        }
        let slot = (consumed as usize % MESSAGES) * MESSAGE_SIZE;
        let kind = ring.read32(slot);
        // Its bus (4), target (8) and logical unit (12, in the second byte).
        let at = Address { target: ring.read32(slot + 8) as u8, lun: ring.read8(slot + 13) };
        device_barrier();
        self.state.write32(state::MESSAGE_CONSUMER, consumed.wrapping_add(1));
        Some(match kind {
            message::DEVICE_ADDED => Message::Added(at),
            message::DEVICE_REMOVED => Message::Removed(at),
            other => Message::Other(other),
        })
    }

    /// Resets the adapter, which drops every command in flight: when it is
    /// given up.
    pub fn reset(&mut self) {
        self.disable_interrupts();
        write_command(&self.regs, command::ADAPTER_RESET, &[]);
        let _ = self.regs.read32(reg::INTERRUPT_STATUS);
        self.busy = [0; CONTEXTS / 64];
    }

    /// A free context, now taken.
    fn take_context(&mut self) -> Option<u16> {
        for (n, word) in self.busy.iter_mut().enumerate() {
            if *word != u64::MAX {
                let bit = (!*word).trailing_zeros();
                *word |= 1 << bit;
                return Some((n * 64) as u16 + bit as u16);
            }
        }
        None
    }

    /// Frees `context`; whether it was in flight.
    fn give_back(&mut self, context: u16) -> bool {
        let (word, bit) = (usize::from(context) / 64, context % 64);
        match self.busy.get_mut(word) {
            Some(word) if *word & (1 << bit) != 0 => {
                *word &= !(1 << bit);
                true
            }
            _ => false,
        }
    }
}

/// Whether a command whose operation code is `opcode` is a read or a
/// write, which go on their own doorbell.
pub fn is_read_write(opcode: u8) -> bool {
    // READ and WRITE (6), (10), (12) and (16).
    matches!(opcode, 0x08 | 0x0a | 0x28 | 0x2a | 0xa8 | 0xaa | 0x88 | 0x8a)
}

/// Writes `command` to the command register, then each of `words` to the
/// data register.
fn write_command(regs: &Mmio, command: u32, words: &[u32]) {
    regs.write32(reg::COMMAND, command);
    for &word in words {
        regs.write32(reg::COMMAND_DATA, word);
    }
}

/// Puts the page number of `phys` as two words at `at` of `words`.
fn put_page_number(words: &mut [u32], at: usize, phys: u64) {
    let number = phys / PAGE as u64;
    words[at] = number as u32;
    words[at + 1] = (number >> 32) as u32;
}
