// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The fusion interface, of every MegaRAID part since the SAS2208.
//!
//! The host keeps an array of 256-byte request frames, one for each
//! command by its *SMID* (1 and up; frame 0 is never used), and posts a
//! command with an 8-byte request descriptor: its type, its SMID and a
//! device handle, written to the inbound queue port in two halves. The
//! firmware answers on a ring of 8-byte reply descriptors in host memory,
//! each the SMID of a command that ended, whose status it wrote into the
//! command's frame. The host sets each descriptor it reads back to all
//! ones and tells the firmware how far it read through a reply index
//! register.
//!
//! Requests are MPI2 SCSI IO requests with a RAID context in place of
//! part of the SCSI fields:
//!
//! - a volume's reads and writes go as *LD I/O* requests to the
//!   firmware, which maps them onto its disks itself (the drivers of
//!   other systems map them themselves from the firmware's RAID map, a
//!   faster path this driver leaves for later);
//! - a volume's other commands go as SCSI IO requests;
//! - a pass-through disk's commands go to the firmware too, marked as a
//!   *system* disk's;
//! - a management command goes as an MFI DCMD frame, which a
//!   pass-through request points to with a chain element.
//!
//! The firmware is given its queues by an MPI2 IOC INIT request that an
//! MFI INIT frame carries, posted as an MFA descriptor and polled.
//!
//! Each command has a 512-byte chain frame of its own, which holds the
//! rest of a scatter-gather list too long for its request frame (eight
//! entries; seven and a chain element when longer), or its DCMD frame, and
//! 96 bytes for its sense data.

use radix_hw::{Dma, device_barrier};

use crate::frame;
use crate::regs::{Registers, Status, doorbell, reg};
use crate::{
    Answer, CONTEXTS, Direction, Error, Family, MAX_SEGMENTS, PAGE, SENSE_SIZE, Slots, Target,
    check_request,
};

/// The size of a request frame.
const REQUEST: usize = 256;
/// The room for each command's chain frame.
const CHAIN: usize = 512;
/// The room for each command's sense data.
const SENSE_STRIDE: usize = 128;
/// The length the firmware is told a DCMD frame has.
const DCMD_FRAME: u32 = 256;
/// The size of an IEEE scatter-gather entry.
const SGE: usize = 16;
/// The entries a request frame holds.
const MAIN_ENTRIES: usize = (REQUEST - req::SGL) / SGE;
/// How long the IOC INIT may take, in milliseconds.
const INIT_TIMEOUT_MS: u32 = 180_000;
/// How many replies are read before the firmware is told, at most.
const REPLY_BATCH: u32 = 32;
/// The timeout given a pass-through disk's command, in seconds.
const DISK_TIMEOUT_S: u16 = 30;

/// A request frame's fields, by their offsets.
mod req {
    pub const DEV_HANDLE: usize = 0x00;
    /// Where the chain element is, in 16-byte units.
    pub const CHAIN_OFFSET: usize = 0x02;
    pub const FUNCTION: usize = 0x03;
    pub const SENSE_LOW: usize = 0x0c;
    pub const SGL_FLAGS: usize = 0x10;
    pub const SENSE_LEN: usize = 0x12;
    /// Where the list starts, in 4-byte units.
    pub const SGL_OFFSET: usize = 0x14;
    pub const DATA_LENGTH: usize = 0x1c;
    pub const IO_FLAGS: usize = 0x24;
    pub const LUN: usize = 0x34;
    pub const CONTROL: usize = 0x3c;
    pub const CDB: usize = 0x40;
    /// The RAID context.
    pub const CONTEXT: usize = 0x60;
    pub const SGL: usize = 0x80;
}

/// The RAID context's fields, by their offsets in it. Ventura and later
/// lay out a few anew (*g35*).
mod ctx {
    /// The request's type (low four bits) and segment count (high four).
    pub const TYPE_NSEG: usize = 0;
    pub const TIMEOUT: usize = 2;
    pub const LOCK_FLAGS: usize = 4;
    /// Ventura: the routing flags, where the lock flags were.
    pub const ROUTING: usize = 4;
    pub const TARGET: usize = 6;
    pub const EX_STATUS: usize = 22;
    pub const STATUS: usize = 23;
    pub const RAID_FLAGS: usize = 24;
    pub const NUM_SGE: usize = 25;
    pub const NUM_SGE_EXT: usize = 30;
    /// Ventura: the segment count, twelve bits.
    pub const NUM_SGE_G35: usize = 28;
}

/// Request functions.
mod function {
    pub const IOC_INIT: u8 = 0x02;
    pub const PASS_THROUGH: u8 = 0xf0;
    pub const LD_IO: u8 = 0xf1;
}

/// Request descriptor types.
mod descriptor {
    pub const SCSI_IO: u8 = 0;
    pub const MFA: u8 = 1;
    pub const NO_LOCK: u8 = 2;
    pub const LD_IO: u8 = 7;
}

/// IEEE scatter-gather entry flags.
mod sge {
    pub const CHAIN: u8 = 0x80;
    pub const END_OF_LIST: u8 = 0x40;
    /// The address is the host's (in a chain element, on the SAS2208).
    pub const HOST_ADDRESS: u8 = 0x03;
}

/// Where things are in the state memory.
mod state {
    /// The MFI INIT frame that carries the IOC INIT request.
    pub const INIT: usize = 0;
    /// The IOC INIT request.
    pub const IOC_INIT: usize = 1024;
    /// How much each slot's command asked to move, 8 bytes apiece.
    pub const ASKED: usize = 2048;
}

/// The IOC INIT request's fields, by their offsets.
mod ioc {
    pub const WHO_INIT: usize = 0x00;
    pub const FUNCTION: usize = 0x03;
    pub const MSG_VERSION: usize = 0x0c;
    pub const HEADER_VERSION: usize = 0x0e;
    pub const HOST_PAGE_SIZE: usize = 0x16;
    pub const HOST_MSIX_VECTORS: usize = 0x17;
    pub const REQUEST_FRAME_SIZE: usize = 0x1a;
    pub const REPLY_DEPTH: usize = 0x1c;
    pub const SENSE_HIGH: usize = 0x20;
    pub const REQUEST_FRAMES: usize = 0x28;
    pub const REPLY_QUEUE: usize = 0x30;
    /// The size of the request.
    pub const SIZE: u32 = 0x48;
}

/// Bits of the second scratch pad.
mod pad2 {
    /// The chain frame size, in units of 128 bytes (or 512).
    pub const CHAIN_SIZE: u32 = 0x3e0;
    /// The chain frame size is in units of 512 bytes.
    pub const CHAIN_UNITS_512: u32 = 0x40_0000;
    /// The firmware takes SYNCHRONIZE CACHE for a volume.
    pub const SYNC_CACHE: u32 = 0x0100_0000;
    /// The SAS2208's interrupt vectors, less one.
    pub const VECTORS_THUNDERBOLT: u32 = 0x1f;
    /// Later parts' interrupt vectors, less one.
    pub const VECTORS: u32 = 0x003f_c000;
}

/// A fusion controller, operational.
pub struct Fusion {
    regs: Registers,
    /// The request frames, SMID 0 to the command count.
    requests: Dma,
    /// Each command's chain frame, [`CHAIN`] bytes apiece.
    chains: Dma,
    /// Each command's sense data, [`SENSE_STRIDE`] bytes apiece.
    sense: Dma,
    /// The reply descriptors.
    replies: Dma,
    /// The IOC INIT and the lengths asked (see [`state`]).
    state: Dma,
    depth: u32,
    /// The next reply descriptor to read.
    next: u32,
    /// How many were read since the firmware was last told.
    unreported: u32,
    /// The register the reply index is written to.
    reply_index: usize,
    max_segments: usize,
    sync_cache: bool,
    slots: Slots,
}

impl Fusion {
    /// Brings the firmware to ready and gives it its queues. `msix`
    /// says whether the host takes its interrupts through MSI-X vector 0.
    pub fn new(
        regs: Registers,
        msix: bool,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Fusion, Error> {
        let family = regs.family();
        let status = regs.make_ready(wait)?;
        // The firmware counts one command more than it takes.
        let fw_commands = usize::from(status.max_commands()).saturating_sub(1);
        if fw_commands == 0 {
            return Err(Error::Device("its firmware takes no commands"));
        }
        let commands = CONTEXTS.min(fw_commands);
        // Twice the commands and one, in sixteens.
        let depth = 2 * (commands + 1).div_ceil(16) * 16;
        let pad2 = regs.scratch_pad_2();
        let unit = if pad2 & pad2::CHAIN_UNITS_512 != 0 { 512 } else { 128 };
        let fw_chain = ((pad2 & pad2::CHAIN_SIZE) >> 5) as usize * unit;
        let chain_entries = fw_chain.min(CHAIN) / SGE;
        let max_segments = if chain_entries >= 2 {
            MAX_SEGMENTS.min(MAIN_ENTRIES - 1 + chain_entries)
        } else {
            MAX_SEGMENTS.min(MAIN_ENTRIES)
        };
        // With more vectors than one register's eight (sixteen on
        // Ventura and later), the firmware takes the reply index of
        // vector 0 at the first of the combined registers.
        let vectors = match family {
            Family::Thunderbolt => (pad2 & pad2::VECTORS_THUNDERBOLT) + 1,
            _ => ((pad2 & pad2::VECTORS) >> 14) + 1,
        };
        let combined = match family {
            Family::Invader => vectors > 8,
            Family::Ventura | Family::Aero => vectors > 16,
            _ => false,
        };
        let reply_index =
            if msix && combined { reg::REPLY_INDEX_COMBINED } else { reg::REPLY_INDEX };
        regs.disable_interrupts();
        let mut pages = |bytes: usize| alloc(bytes.div_ceil(PAGE)).ok_or(Error::OutOfMemory);
        let requests = pages((commands + 1) * REQUEST)?;
        let chains = pages(commands * CHAIN)?;
        let sense = pages(commands * SENSE_STRIDE)?;
        let replies = pages(depth * 8)?;
        let state = pages(state::ASKED + CONTEXTS * 8)?;
        // The sense buffers' addresses share their high half, which the
        // IOC INIT gives once.
        let last = sense.phys() + sense.len() as u64 - 1;
        if sense.phys() >> 32 != last >> 32 {
            return Err(Error::Device("its sense buffers cross a 4 GiB line"));
        }
        for memory in [&requests, &chains, &sense, &state] {
            memory.zero();
        }
        for n in 0..depth {
            replies.write64(n * 8, u64::MAX);
        }
        // The IOC INIT request.
        let at = state::IOC_INIT;
        state.write8(at + ioc::WHO_INIT, 4);
        state.write8(at + ioc::FUNCTION, function::IOC_INIT);
        state.write16(at + ioc::MSG_VERSION, 0x0200);
        state.write16(at + ioc::HEADER_VERSION, 0x1000);
        state.write8(at + ioc::HOST_PAGE_SIZE, 12);
        state.write8(at + ioc::HOST_MSIX_VECTORS, u8::from(msix));
        state.write16(at + ioc::REQUEST_FRAME_SIZE, (REQUEST / 4) as u16);
        state.write16(at + ioc::REPLY_DEPTH, depth as u16);
        state.write32(at + ioc::SENSE_HIGH, (sense.phys() >> 32) as u32);
        state.write64(at + ioc::REQUEST_FRAMES, requests.phys());
        state.write64(at + ioc::REPLY_QUEUE, replies.phys());
        // The MFI INIT frame that carries it, with what the driver can
        // do: more than eight interrupt vectors' registers (on parts
        // after the SAS2208), and up to 255 volumes.
        let init = state::INIT;
        frame::write_header(
            &state,
            init,
            frame::cmd::INIT,
            0,
            0,
            0,
            frame::flag::DONT_POST,
            ioc::SIZE,
        );
        let mut capabilities = 1 << 3;
        if family != Family::Thunderbolt {
            capabilities |= 1 << 1;
        }
        state.write32(init + frame::init::CAPABILITIES, capabilities);
        let request = state.phys() + at as u64;
        state.write32(init + frame::init::QUEUE_INFO_LOW, request as u32);
        state.write32(init + frame::init::QUEUE_INFO_HIGH, (request >> 32) as u32);
        let fusion = Fusion {
            regs,
            requests,
            chains,
            sense,
            replies,
            state,
            depth: depth as u32,
            next: 0,
            unreported: 0,
            reply_index,
            max_segments,
            sync_cache: pad2 & pad2::SYNC_CACHE != 0,
            slots: Slots::new(commands),
        };
        let phys = fusion.state.phys() + init as u64;
        fusion.fire((phys as u32 & !0xff) | u32::from(descriptor::MFA) << 1, (phys >> 32) as u32);
        let mut waited = 0;
        loop {
            device_barrier();
            let (status, _) = frame::read_status(&fusion.state, init);
            if status == frame::status::OK {
                return Ok(fusion);
            }
            if status != frame::PENDING {
                return Err(Error::Device("its firmware refused its queues"));
            }
            if waited >= INIT_TIMEOUT_MS {
                return Err(Error::Device("its firmware did not take its queues"));
            }
            wait(1);
            waited += 1;
        }
    }

    /// The controller's family.
    pub fn family(&self) -> Family {
        self.regs.family()
    }

    /// The firmware's status now.
    pub fn status(&self) -> Status {
        self.regs.status()
    }

    /// How many commands may be in flight at once.
    pub fn commands(&self) -> usize {
        self.slots.count
    }

    /// The most runs of memory one command's data may take.
    pub fn max_segments(&self) -> usize {
        self.max_segments
    }

    /// Whether the firmware takes SYNCHRONIZE CACHE for a volume.
    pub fn sync_cache(&self) -> bool {
        self.sync_cache
    }

    /// Lets the controller interrupt.
    pub fn enable_interrupts(&self) {
        self.regs.enable_interrupts();
    }

    /// Clears the interrupt the controller raised; whether it raised one.
    pub fn acknowledge(&self) -> bool {
        self.regs.acknowledge()
    }

    /// How many more commands can go now.
    pub fn room(&self) -> usize {
        self.slots.free()
    }

    /// Sends a SCSI command; returns its slot (its SMID less one).
    pub fn submit(
        &mut self,
        target: Target,
        lun: u8,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        check_request(cdb, data, self.max_segments)?;
        let slot = self.slots.take().ok_or(Error::Full)?;
        let smid = slot + 1;
        let at = usize::from(smid) * REQUEST;
        let frame = &self.requests;
        frame.write(at, &[0; REQUEST]);
        let length = data.iter().map(|&(_, len)| len).sum::<u32>();
        frame.write8(at + req::FUNCTION, function::LD_IO);
        let sense = self.sense.phys() + (usize::from(slot) * SENSE_STRIDE) as u64;
        frame.write32(at + req::SENSE_LOW, sense as u32);
        frame.write16(at + req::SGL_FLAGS, 0x02);
        frame.write8(at + req::SENSE_LEN, SENSE_SIZE as u8);
        frame.write8(at + req::SGL_OFFSET, (req::SGL / 4) as u8);
        frame.write32(at + req::DATA_LENGTH, length);
        frame.write16(at + req::IO_FLAGS, cdb.len() as u16);
        // A single-level logical unit number, in the second byte.
        frame.write8(at + req::LUN + 1, lun);
        let control = match direction {
            Direction::None => 0,
            Direction::FromDevice => 0x0200_0000,
            Direction::ToDevice => 0x0100_0000,
        };
        frame.write32(at + req::CONTROL, control);
        frame.write(at + req::CDB, cdb);
        self.write_sgl(slot, data);
        let family = self.family();
        let c = at + req::CONTEXT;
        let (kind, handle) = match target {
            Target::Volume(id) if is_read_write(cdb[0]) => {
                frame.write16(at + req::DEV_HANDLE, id);
                frame.write16(c + ctx::TARGET, id);
                // The firmware's own path: a CUDA request of one segment,
                // in order (its sequence numbers on).
                match family {
                    Family::Thunderbolt => (descriptor::LD_IO, 0),
                    Family::Invader => {
                        frame.write8(c + ctx::TYPE_NSEG, 0x12);
                        frame.write8(c + ctx::LOCK_FLAGS, 0x08);
                        (descriptor::NO_LOCK, 0)
                    }
                    _ => {
                        frame.write8(c + ctx::TYPE_NSEG, 0x12);
                        frame.write16(c + ctx::ROUTING, 1 << 4);
                        (descriptor::LD_IO, 0)
                    }
                }
            }
            Target::Volume(id) => {
                frame.write16(at + req::DEV_HANDLE, id);
                frame.write16(c + ctx::TARGET, id);
                (descriptor::SCSI_IO, 0)
            }
            Target::Disk(id) => {
                // A system disk's, through the firmware: no device handle.
                frame.write16(at + req::DEV_HANDLE, 0xffff);
                frame.write16(c + ctx::TARGET, id);
                frame.write8(c + ctx::RAID_FLAGS, 1 << 4);
                frame.write16(c + ctx::TIMEOUT, DISK_TIMEOUT_S);
                (descriptor::SCSI_IO, 0xffff)
            }
        };
        let segments = data.len() as u16;
        if family.ventura() {
            frame.write16(c + ctx::NUM_SGE_G35, segments & 0xfff);
        } else {
            frame.write8(c + ctx::NUM_SGE, segments as u8);
            frame.write8(c + ctx::NUM_SGE_EXT, (segments >> 8) as u8);
        }
        self.note_asked(slot, u64::from(length));
        self.post(kind, smid, handle);
        Ok(slot)
    }

    /// Sends a management command; returns its slot.
    pub fn submit_dcmd(
        &mut self,
        opcode: u32,
        mailbox: &[u8; 12],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        let slot = self.slots.take().ok_or(Error::Full)?;
        let smid = slot + 1;
        let mfi = usize::from(slot) * CHAIN;
        let to_device = direction == Direction::ToDevice;
        frame::write_dcmd(&self.chains, mfi, u32::from(smid), opcode, mailbox, data, to_device);
        self.wrap_mfi(smid, mfi, DCMD_FRAME);
        let length = data.iter().map(|&(_, len)| u64::from(len)).sum::<u64>();
        self.note_asked(slot, length);
        self.post(descriptor::SCSI_IO, smid, 0);
        Ok(slot)
    }

    /// Sends the SCSI command `cdb` to the physical disk `device` inside an
    /// MFI pass-through, as the firmware takes one for any of its disks,
    /// in a volume or not; returns its slot. Its sense data come back in
    /// the slot's sense buffer, as an LD I/O request's do.
    pub fn submit_member(
        &mut self,
        device: u16,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        check_request(cdb, data, self.max_segments)?;
        // An MFI frame names its disk in a byte.
        let target = u8::try_from(device).map_err(|_| Error::BadRequest)?;
        let slot = self.slots.take().ok_or(Error::Full)?;
        let smid = slot + 1;
        let mfi = usize::from(slot) * CHAIN;
        let sense = self.sense.phys() + (usize::from(slot) * SENSE_STRIDE) as u64;
        let to_device = direction == Direction::ToDevice;
        frame::write_pass(
            &self.chains,
            mfi,
            u32::from(smid),
            false,
            target,
            0,
            cdb,
            data,
            to_device,
            sense,
            false,
        );
        let size = frame::pass_size(data.len(), false).max(DCMD_FRAME as usize);
        self.wrap_mfi(smid, mfi, size as u32);
        let length = data.iter().map(|&(_, len)| u64::from(len)).sum::<u64>();
        self.note_asked(slot, length);
        self.post(descriptor::SCSI_IO, smid, 0);
        Ok(slot)
    }

    /// Writes the request of `smid` as a pass-through of the MFI frame at
    /// `mfi` in the chain memory, `size` bytes long.
    fn wrap_mfi(&self, smid: u16, mfi: usize, size: u32) {
        let at = usize::from(smid) * REQUEST;
        let frame = &self.requests;
        frame.write(at, &[0; REQUEST]);
        frame.write8(at + req::FUNCTION, function::PASS_THROUGH);
        frame.write8(at + req::SGL_OFFSET, (req::SGL / 4) as u8);
        frame.write8(at + req::CHAIN_OFFSET, (req::SGL / 16) as u8);
        let chain = at + req::SGL;
        frame.write64(chain, self.chains.phys() + mfi as u64);
        frame.write32(chain + 8, size);
        frame.write8(chain + 15, sge::CHAIN | sge::HOST_ADDRESS);
    }

    /// The next command that ended: its slot, and how.
    pub fn complete(&mut self) -> Option<(u16, Answer)> {
        loop {
            device_barrier();
            let at = self.next as usize * 8;
            let word = self.replies.read64(at);
            let (low, high) = (word as u32, (word >> 32) as u32);
            if low == u32::MAX || high == u32::MAX || low & 0x0f == 0x0f {
                self.report();
                return None;
            }
            self.replies.write64(at, u64::MAX);
            self.next = (self.next + 1) % self.depth;
            self.unreported += 1;
            if self.unreported >= REPLY_BATCH {
                self.report();
            }
            let smid = (low >> 16) as u16;
            let Some(slot) = smid.checked_sub(1) else { continue };
            if !self.slots.give_back(slot) {
                continue;
            }
            let frame = usize::from(smid) * REQUEST;
            let asked = self.state.read64(state::ASKED + usize::from(slot) * 8);
            if self.requests.read8(frame + req::FUNCTION) == function::PASS_THROUGH {
                // A DCMD's SCSI status stays 0; a member command's is the
                // disk's, with its sense data in the slot's buffer.
                let mfi = usize::from(slot) * CHAIN;
                let (status, scsi_status) = frame::read_status(&self.chains, mfi);
                let moved = if status == frame::status::OK { asked } else { 0 };
                let mut answer = Answer::new(status, scsi_status, asked, moved);
                let sense_len = usize::from(self.chains.read8(mfi + frame::header::SENSE_LEN));
                answer.take_sense(&self.sense, usize::from(slot) * SENSE_STRIDE, sense_len);
                return Some((slot, answer));
            }
            let c = frame + req::CONTEXT;
            let status = self.requests.read8(c + ctx::STATUS);
            let scsi_status = self.requests.read8(c + ctx::EX_STATUS);
            // The firmware leaves in the frame's length what it moved.
            let length = u64::from(self.requests.read32(frame + req::DATA_LENGTH));
            let moved = if status == frame::status::OK { length.min(asked) } else { 0 };
            let mut answer = Answer::new(status, scsi_status, asked, moved);
            answer.take_sense(&self.sense, usize::from(slot) * SENSE_STRIDE, SENSE_SIZE);
            return Some((slot, answer));
        }
    }

    /// Stops the controller and drops every command in flight.
    pub fn reset(&mut self) {
        self.regs.disable_interrupts();
        self.regs.ring(doorbell::RESET);
        self.slots.clear();
    }

    /// Writes the list of `data` for the command in `slot`: in its
    /// request frame, and on in its chain frame when longer.
    fn write_sgl(&self, slot: u16, data: &[(u64, u32)]) {
        let at = usize::from(slot + 1) * REQUEST;
        let end_flag = if self.family() == Family::Thunderbolt { 0 } else { sge::END_OF_LIST };
        let put = |memory: &Dma, entry: usize, (phys, len): (u64, u32), last: bool| {
            memory.write64(entry, phys);
            memory.write32(entry + 8, len);
            memory.write8(entry + 15, if last { end_flag } else { 0 });
        };
        if data.len() <= MAIN_ENTRIES {
            for (n, &run) in data.iter().enumerate() {
                put(&self.requests, at + req::SGL + n * SGE, run, n + 1 == data.len());
            }
            return;
        }
        let (main, rest) = data.split_at(MAIN_ENTRIES - 1);
        for (n, &run) in main.iter().enumerate() {
            put(&self.requests, at + req::SGL + n * SGE, run, false);
        }
        let chain_at = usize::from(slot) * CHAIN;
        for (n, &run) in rest.iter().enumerate() {
            put(&self.chains, chain_at + n * SGE, run, n + 1 == rest.len());
        }
        let element = at + req::SGL + (MAIN_ENTRIES - 1) * SGE;
        self.requests.write64(element, self.chains.phys() + chain_at as u64);
        self.requests.write32(element + 8, (rest.len() * SGE) as u32);
        let flags = match self.family() {
            Family::Thunderbolt => sge::CHAIN | sge::HOST_ADDRESS,
            _ => sge::CHAIN,
        };
        self.requests.write8(element + 15, flags);
        self.requests.write8(at + req::CHAIN_OFFSET, ((element - at) / 16) as u8);
    }

    /// Notes how much the command in `slot` asked to move.
    fn note_asked(&self, slot: u16, length: u64) {
        self.state.write64(state::ASKED + usize::from(slot) * 8, length);
    }

    /// Posts the request of `smid` with a descriptor of type `kind` and
    /// device handle `handle`.
    fn post(&self, kind: u8, smid: u16, handle: u16) {
        let low = u32::from(kind) << 1 | u32::from(smid) << 16;
        let high = u32::from(handle) << 16;
        self.fire(low, high);
    }

    /// Writes a request descriptor, its low half first.
    fn fire(&self, low: u32, high: u32) {
        device_barrier();
        self.regs.write32(reg::INBOUND_LOW, low);
        self.regs.write32(reg::INBOUND_HIGH, high);
    }

    /// Tells the firmware how far the replies were read.
    fn report(&mut self) {
        if self.unreported != 0 {
            device_barrier();
            self.regs.write32(self.reply_index, self.next);
            self.unreported = 0;
        }
    }
}

/// Whether a command whose operation code is `opcode` is a read or a
/// write, which go to a volume as LD I/O.
pub fn is_read_write(opcode: u8) -> bool {
    // READ and WRITE (6), (10), (12) and (16).
    matches!(opcode, 0x08 | 0x0a | 0x28 | 0x2a | 0xa8 | 0xaa | 0x88 | 0x8a)
}

/// A DCMD frame with the most entries fits a command's chain frame, and
/// the sense data their room.
const _: () = assert!(frame::dcmd::SGL + crate::dcmd_segments() * frame::SGE64 <= CHAIN);
const _: () = assert!(SENSE_SIZE <= SENSE_STRIDE);
const _: () = assert!((MAX_SEGMENTS - (MAIN_ENTRIES - 1)) * SGE <= CHAIN);
