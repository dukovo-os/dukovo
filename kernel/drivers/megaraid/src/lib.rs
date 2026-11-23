// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Broadcom MegaRAID RAID controllers (ADR 0664): Dell's PERC 6 to 11,
//! Lenovo's ThinkSystem RAID 5xx and 9xx, and Broadcom's (LSI's) own
//! 92xx to 95xx cards.
//!
//! The controller's firmware runs the RAID: the host sees its volumes as
//! SCSI targets, and the disks it passes through (JBOD) as more. The host
//! manages it with *DCMD* frames, each an opcode, a 12-byte mailbox and a
//! buffer, and reaches a physical disk itself with SCSI pass-through
//! frames. Two interfaces carry them:
//!
//! - *MFI* ([`mfi`]), the older one, of the SAS1078 and SAS2108 parts
//!   (PERC 6, H700, H800) and QEMU's `megasas` and `megasas-gen2`: the
//!   host writes a frame's address to the inbound queue port, and the
//!   firmware writes the frame's context to a reply queue in host memory.
//! - *Fusion* ([`fusion`]), of every card since the SAS2208 (PERC H330,
//!   H730, H740P, H755): the host posts 8-byte MPI2 request descriptors
//!   naming 256-byte request frames, and the firmware answers with reply
//!   descriptors on a ring. A DCMD frame goes inside a pass-through
//!   request; reads and writes of volumes and pass-through disks go as
//!   LD I/O requests.
//!
//! [`frame`] holds the MFI frames both interfaces carry and [`dcmd`] the
//! commands and the parsers of what they answer. Nothing here allocates:
//! the transports are given their DMA memory, and the parsers are views
//! over the bytes the controller wrote.
//!
//! The layouts and sequences follow FreeBSD's `mrsas` and `mfi` drivers,
//! which LSI and Avago wrote (BSD-2-Clause, credits/freebsd-mrsas.md),
//! and what QEMU's `megasas` model answers. No code is copied.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod dcmd;
pub mod frame;
pub mod fusion;
pub mod mfi;
pub mod regs;
pub mod report;

#[cfg(test)]
mod sim;
#[cfg(test)]
mod tests;

use radix_hw::{Dma, Mmio};

pub use frame::SENSE_SIZE;

/// LSI's (Broadcom's) PCI vendor ID.
pub const VENDOR: u16 = 0x1000;
/// The most commands the driver keeps in flight on one controller.
pub const CONTEXTS: usize = 256;
/// The most runs of memory one command's data take.
pub const MAX_SEGMENTS: usize = 32;
/// The longest command descriptor block.
pub const CDB_SIZE: usize = frame::CDB_SIZE;
/// The size of a page, the unit the transports are given memory in.
pub const PAGE: usize = 4096;

/// Which interface and register set a controller has, by its PCI
/// device ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// MFI on the SAS1078 (PERC 6, QEMU's `megasas`).
    Mfi1078,
    /// MFI on the SAS2108 (PERC H700 and H800, QEMU's `megasas-gen2`).
    MfiGen2,
    /// MFI on the SAS2008 "skinny" parts (PERC H310): the inbound
    /// doorbell at offset 0.
    MfiSkinny,
    /// Fusion on the SAS2208 (Thunderbolt).
    Thunderbolt,
    /// Fusion on the SAS3108 and SAS3008 (Invader and Fury: PERC H730,
    /// H330) and their kin (Intruder, Cutlass).
    Invader,
    /// Fusion on the SAS3508 and later (Ventura, Crusader, Harpoon,
    /// Tomcat: PERC H740P), which lays out the RAID context anew.
    Ventura,
    /// Fusion on the SAS3908 and later (Aero: PERC H755).
    Aero,
}

impl Family {
    /// The family of the MegaRAID controller with PCI device ID `device`,
    /// or none if it is not one this driver takes.
    pub fn of(device: u16) -> Option<Family> {
        Some(match device {
            0x0060 | 0x007c => Family::Mfi1078,
            0x0078 | 0x0079 | 0x002f => Family::MfiGen2,
            0x0071 | 0x0073 => Family::MfiSkinny,
            0x005b => Family::Thunderbolt,
            0x005d | 0x005f | 0x00ce | 0x00cf | 0x0052 | 0x0053 => Family::Invader,
            0x0014 | 0x0015 | 0x0016 | 0x0017 | 0x001b | 0x001c => Family::Ventura,
            // The Aero IDs 0x10e0, 0x10e3, 0x10e4 and 0x10e7 are parts whose
            // secure boot failed or is not set up; their firmware refuses
            // a driver, as Linux's does.
            0x10e1 | 0x10e2 | 0x10e5 | 0x10e6 => Family::Aero,
            _ => return None,
        })
    }

    /// Whether it has the fusion interface.
    pub fn fusion(self) -> bool {
        matches!(self, Family::Thunderbolt | Family::Invader | Family::Ventura | Family::Aero)
    }

    /// Whether its RAID context has the Ventura layout.
    pub fn ventura(self) -> bool {
        matches!(self, Family::Ventura | Family::Aero)
    }

    /// The part's name, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Family::Mfi1078 => "SAS1078",
            Family::MfiGen2 => "SAS2108",
            Family::MfiSkinny => "SAS2008",
            Family::Thunderbolt => "SAS2208",
            Family::Invader => "SAS3108",
            Family::Ventura => "SAS3508",
            Family::Aero => "SAS3908",
        }
    }
}

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The register BAR is smaller than the registers.
    Registers,
    /// The memory for the queues could not be had.
    OutOfMemory,
    /// The firmware is in its fault state, with this code.
    Fault(u32),
    /// The firmware did not reach the ready state: the state it stayed in.
    NotReady(u32),
    /// The firmware did not do what it was asked: what.
    Device(&'static str),
    /// A command the controller cannot carry: data in more runs than its
    /// lists hold, or a command descriptor block over 16 bytes.
    BadRequest,
    /// Every command is in flight.
    Full,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Registers => f.write_str("its registers are not all mapped"),
            Error::OutOfMemory => f.write_str("out of memory"),
            Error::Fault(code) => write!(f, "its firmware faulted (code {code:#x})"),
            Error::NotReady(state) => {
                write!(f, "its firmware did not become ready (state {:#x})", state >> 28)
            }
            Error::Device(what) => f.write_str(what),
            Error::BadRequest => f.write_str("a command it cannot carry"),
            Error::Full => f.write_str("every command is in flight"),
        }
    }
}

/// Where a SCSI command goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A volume (a logical drive), by its target ID.
    Volume(u16),
    /// A disk the controller passes through to the host (JBOD, a
    /// *system* disk), or any physical disk for a command the firmware
    /// lets through, by its device ID.
    Disk(u16),
}

/// Which way a command's data go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// It moves none.
    None,
    /// From the controller into memory.
    FromDevice,
    /// From memory to the controller.
    ToDevice,
}

/// How a command ended.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    /// The firmware's status ([`frame::status`]).
    pub status: u8,
    /// The target's SCSI status, for a SCSI command.
    pub scsi_status: u8,
    /// How many bytes it moved.
    pub moved: u64,
    /// How many bytes it was asked to move.
    pub asked: u64,
    sense: [u8; SENSE_SIZE],
    sense_len: usize,
}

/// The SCSI status that says sense data follow.
const CHECK_CONDITION: u8 = 0x02;

impl Answer {
    /// An answer with `status` and `scsi_status`, of a command that was
    /// asked to move `asked` bytes and moved `moved`.
    fn new(status: u8, scsi_status: u8, asked: u64, moved: u64) -> Answer {
        Answer { status, scsi_status, moved, asked, sense: [0; SENSE_SIZE], sense_len: 0 }
    }

    /// Whether the command went through.
    pub fn good(&self) -> bool {
        self.status == frame::status::OK
    }

    /// Whether the target ended the command with sense data.
    pub fn check_condition(&self) -> bool {
        self.status == frame::status::SCSI_DONE_WITH_ERROR && self.scsi_status == CHECK_CONDITION
    }

    /// The sense data of a command that failed with them.
    pub fn sense(&self) -> &[u8] {
        &self.sense[..self.sense_len]
    }

    /// The firmware's status in words.
    pub fn why(&self) -> &'static str {
        frame::status_text(self.status)
    }

    /// Copies sense data from `memory` at `at` if the target sent them.
    fn take_sense(&mut self, memory: &Dma, at: usize, len: usize) {
        if self.check_condition() {
            self.sense_len = len.min(SENSE_SIZE);
            memory.read(at, &mut self.sense[..self.sense_len]);
        }
    }
}

/// Which commands are in flight: a bit each.
#[derive(Clone, Debug)]
struct Slots {
    busy: [u64; CONTEXTS / 64],
    count: usize,
}

impl Slots {
    /// `count` slots, none taken.
    fn new(count: usize) -> Slots {
        Slots { busy: [0; CONTEXTS / 64], count: count.min(CONTEXTS) }
    }

    /// A free slot, now taken.
    fn take(&mut self) -> Option<u16> {
        for (n, word) in self.busy.iter_mut().enumerate() {
            if *word != u64::MAX {
                let bit = (!*word).trailing_zeros() as usize;
                let slot = n * 64 + bit;
                if slot >= self.count {
                    return None;
                }
                *word |= 1 << bit;
                return Some(slot as u16);
            }
        }
        None
    }

    /// Frees `slot`; whether it was taken.
    fn give_back(&mut self, slot: u16) -> bool {
        let (word, bit) = (usize::from(slot) / 64, slot % 64);
        match self.busy.get_mut(word) {
            Some(word) if *word & (1 << bit) != 0 => {
                *word &= !(1 << bit);
                true
            }
            _ => false,
        }
    }

    /// How many are taken.
    fn in_flight(&self) -> usize {
        self.busy.iter().map(|word| word.count_ones() as usize).sum()
    }

    /// How many are free.
    fn free(&self) -> usize {
        self.count - self.in_flight()
    }

    /// Frees them all.
    fn clear(&mut self) {
        self.busy = [0; CONTEXTS / 64];
    }
}

/// Checks a command before it takes a slot: its descriptor block, and its
/// data's runs against `max_segments`.
fn check_request(cdb: &[u8], data: &[(u64, u32)], max_segments: usize) -> Result<(), Error> {
    if cdb.is_empty() || cdb.len() > CDB_SIZE || data.len() > max_segments {
        return Err(Error::BadRequest);
    }
    Ok(())
}

/// The transport a controller uses.
enum Transport {
    Mfi(mfi::Mfi),
    Fusion(fusion::Fusion),
}

/// A MegaRAID controller, set up: its firmware operational and given its
/// queues.
pub struct MegaRaid {
    transport: Transport,
}

impl MegaRaid {
    /// Brings the firmware of the controller with PCI device ID `device`,
    /// whose registers are `regs`, to its ready state and gives it its
    /// queues, from `alloc`, which returns that many zeroed pages of DMA
    /// memory below 4 GiB. `msix` says whether the host will take its
    /// interrupts through MSI-X (vector 0). `wait` sleeps the milliseconds
    /// it is given: the firmware may take minutes to start. Its interrupts
    /// stay off until [`MegaRaid::enable_interrupts`].
    pub fn new(
        regs: Mmio,
        device: u16,
        msix: bool,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
        wait: &mut dyn FnMut(u32),
    ) -> Result<MegaRaid, Error> {
        let family = Family::of(device).ok_or(Error::Device("not a MegaRAID controller"))?;
        let regs = regs::Registers::new(regs, family)?;
        let transport = if family.fusion() {
            Transport::Fusion(fusion::Fusion::new(regs, msix, alloc, wait)?)
        } else {
            Transport::Mfi(mfi::Mfi::new(regs, alloc, wait)?)
        };
        Ok(MegaRaid { transport })
    }

    /// The controller's family.
    pub fn family(&self) -> Family {
        match &self.transport {
            Transport::Mfi(t) => t.family(),
            Transport::Fusion(t) => t.family(),
        }
    }

    /// The firmware's status now.
    pub fn status(&self) -> regs::Status {
        match &self.transport {
            Transport::Mfi(t) => t.status(),
            Transport::Fusion(t) => t.status(),
        }
    }

    /// How many commands may be in flight at once.
    pub fn commands(&self) -> usize {
        match &self.transport {
            Transport::Mfi(t) => t.commands(),
            Transport::Fusion(t) => t.commands(),
        }
    }

    /// The most runs of memory one command's data may take.
    pub fn max_segments(&self) -> usize {
        match &self.transport {
            Transport::Mfi(t) => t.max_segments(),
            Transport::Fusion(t) => t.max_segments(),
        }
    }

    /// Whether the firmware takes SYNCHRONIZE CACHE for a volume. When it
    /// does not, the volume's cache is the controller's, kept by its
    /// battery or flash, and the command is answered by the driver.
    pub fn volume_sync_cache(&self) -> bool {
        match &self.transport {
            Transport::Mfi(_) => false,
            Transport::Fusion(t) => t.sync_cache(),
        }
    }

    /// Lets the controller interrupt when a command ends.
    pub fn enable_interrupts(&self) {
        match &self.transport {
            Transport::Mfi(t) => t.enable_interrupts(),
            Transport::Fusion(t) => t.enable_interrupts(),
        }
    }

    /// Clears the interrupt the controller raised; whether it raised one
    /// (see [`regs::Registers::acknowledge`]).
    pub fn acknowledge(&self) -> bool {
        match &self.transport {
            Transport::Mfi(t) => t.acknowledge(),
            Transport::Fusion(t) => t.acknowledge(),
        }
    }

    /// How many more commands can go now.
    pub fn room(&self) -> usize {
        match &self.transport {
            Transport::Mfi(t) => t.room(),
            Transport::Fusion(t) => t.room(),
        }
    }

    /// How many commands are in flight.
    pub fn in_flight(&self) -> usize {
        self.commands() - self.room()
    }

    /// Sends the SCSI command `cdb` to logical unit `lun` of `target`,
    /// its data the runs `data` (physical address, length) going
    /// `direction`. Returns its context, which [`MegaRaid::complete`]
    /// gives back.
    pub fn submit(
        &mut self,
        target: Target,
        lun: u8,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        match &mut self.transport {
            Transport::Mfi(t) => t.submit(target, lun, cdb, data, direction),
            Transport::Fusion(t) => t.submit(target, lun, cdb, data, direction),
        }
    }

    /// Sends the SCSI command `cdb` to the physical disk `device`, whether
    /// it is a member of a volume or passed through, as the firmware's MFI
    /// pass-through carries one: health queries reach a volume's disks
    /// this way. Reads and writes of a disk go through
    /// [`MegaRaid::submit`]. Returns its context.
    pub fn submit_member(
        &mut self,
        device: u16,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        match &mut self.transport {
            Transport::Mfi(t) => t.submit_member(device, cdb, data, direction),
            Transport::Fusion(t) => t.submit_member(device, cdb, data, direction),
        }
    }

    /// Sends the management command `opcode` with `mailbox`, its data the
    /// runs `data` going `direction`. Returns its context.
    pub fn submit_dcmd(
        &mut self,
        opcode: u32,
        mailbox: &[u8; 12],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        if data.len() > dcmd_segments() {
            return Err(Error::BadRequest);
        }
        match &mut self.transport {
            Transport::Mfi(t) => t.submit_dcmd(opcode, mailbox, data, direction),
            Transport::Fusion(t) => t.submit_dcmd(opcode, mailbox, data, direction),
        }
    }

    /// The next command that ended: its context, and how.
    pub fn complete(&mut self) -> Option<(u16, Answer)> {
        match &mut self.transport {
            Transport::Mfi(t) => t.complete(),
            Transport::Fusion(t) => t.complete(),
        }
    }

    /// Sends a management command and waits for it, `timeout_ms` at most,
    /// reading the replies itself: while the controller is brought up and
    /// when it is shut down, with no other command in flight. Its
    /// interrupts must be enabled first ([`MegaRaid::enable_interrupts`]):
    /// QEMU's MFI models move the reply queue only while they are, and
    /// the host's handler leaves the replies to this call until it takes
    /// the controller over.
    pub fn run_dcmd(
        &mut self,
        opcode: u32,
        mailbox: &[u8; 12],
        data: &[(u64, u32)],
        direction: Direction,
        wait: &mut dyn FnMut(u32),
        timeout_ms: u32,
    ) -> Result<Answer, Error> {
        if self.in_flight() != 0 {
            return Err(Error::Full);
        }
        let context = self.submit_dcmd(opcode, mailbox, data, direction)?;
        let mut waited = 0;
        loop {
            while let Some((done, answer)) = self.complete() {
                if done == context {
                    // Read on to the end, so the fusion parts are told
                    // which replies the host has taken.
                    while self.complete().is_some() {}
                    return Ok(answer);
                }
            }
            if let regs::State::Fault = self.status().state() {
                return Err(Error::Fault(self.status().fault_code()));
            }
            if waited >= timeout_ms {
                return Err(Error::Device("a management command did not end"));
            }
            wait(1);
            waited += 1;
        }
    }

    /// Gives the controller up: stops its interrupts and sends its
    /// firmware back to ready, which drops every command in flight.
    pub fn reset(&mut self) {
        match &mut self.transport {
            Transport::Mfi(t) => t.reset(),
            Transport::Fusion(t) => t.reset(),
        }
    }
}

/// The most runs a management command's data may take.
pub const fn dcmd_segments() -> usize {
    4
}

/// Whether `memory` lies below 4 GiB, as memory the controller reaches
/// through 32-bit addresses must. Tests run on host memory, which may lie
/// anywhere: their simulated controllers take the high half from what
/// they were given.
fn below_4g(memory: &Dma) -> bool {
    cfg!(test) || memory.phys() + memory.len() as u64 <= 1 << 32
}
