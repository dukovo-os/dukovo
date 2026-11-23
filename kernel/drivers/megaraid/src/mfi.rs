// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MFI interface, of the SAS1078, SAS2108 and SAS2008 parts and of
//! QEMU's `megasas` and `megasas-gen2`.
//!
//! The host posts a frame by writing its address, with the count of
//! 64-byte frames after the first that it spans, to the inbound queue
//! port. When the firmware is done with it, it writes the frame's status
//! into the frame and the frame's context into the reply queue, a ring of
//! 32-bit words in host memory, and moves the queue's producer index; the
//! host moves the consumer index as it reads them. The firmware is given
//! the queue with an INIT frame, whose status the host polls.
//!
//! Each command has a 512-byte frame of its own (at most eight 64-byte
//! frames, as the firmware reads them) and 96 bytes for its sense data.
//! A frame's context is its slot plus one, so a context of 0 (the INIT
//! frame's) is never taken for a command.

use radix_hw::{Dma, device_barrier};

use crate::frame::{self, header};
use crate::regs::{Registers, Status, reg};
use crate::{
    Answer, CONTEXTS, Direction, Error, Family, MAX_SEGMENTS, PAGE, SENSE_SIZE, Slots, Target,
    below_4g, check_request,
};

/// The room for each command's frame.
const FRAME: usize = 512;
/// The room for each command's sense data.
const SENSE_STRIDE: usize = 128;
/// The longest reply queue the driver gives the firmware.
const MAX_ENTRIES: u32 = 4096;
/// How much QEMU maps from a frame's address.
const QEMU_FRAME_MAP: usize = 2048;
/// How long the INIT frame may take, in milliseconds.
const INIT_TIMEOUT_MS: u32 = 60_000;

/// Where things are in the state page.
mod state {
    /// The reply queue's producer index, which the firmware moves.
    pub const PRODUCER: usize = 0;
    /// Its consumer index, which the host moves.
    pub const CONSUMER: usize = 4;
    /// The INIT frame.
    pub const INIT: usize = 64;
    /// The queue information the INIT frame points to.
    pub const QUEUE_INFO: usize = 128;
    /// How much each slot's command asked to move, 8 bytes apiece.
    pub const ASKED: usize = 1024;
}

/// An MFI controller, operational.
pub struct Mfi {
    regs: Registers,
    /// Each command's frame, [`FRAME`] bytes apiece.
    frames: Dma,
    /// Each command's sense data, [`SENSE_STRIDE`] bytes apiece.
    sense: Dma,
    /// The reply queue: `entries` contexts.
    queue: Dma,
    /// The indexes, the INIT frame and the lengths asked (see [`state`]).
    state: Dma,
    entries: u32,
    max_segments: usize,
    slots: Slots,
}

impl Mfi {
    /// Brings the firmware to ready and gives it its reply queue.
    pub fn new(
        regs: Registers,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Mfi, Error> {
        let status = regs.make_ready(wait)?;
        let fw_commands = u32::from(status.max_commands());
        if fw_commands < 2 {
            return Err(Error::Device("its firmware takes no commands"));
        }
        // QEMU wraps the reply queue at the firmware's command count, the
        // real firmware at the count it is given: give it that count.
        let entries = fw_commands.min(MAX_ENTRIES);
        let commands = CONTEXTS.min(entries as usize - 1);
        let ieee = regs.family() == Family::MfiSkinny;
        let entry = if ieee { frame::SGE_IEEE } else { frame::SGE64 };
        let fit = (FRAME - frame::pass::SGL) / entry;
        let mut max_segments = MAX_SEGMENTS.min(fit);
        if status.max_sge() != 0 {
            max_segments = max_segments.min(usize::from(status.max_sge()));
        }
        // The interrupts stay off until the queue is given, as the
        // controllers' makers do it (QEMU resets on this too, harmlessly
        // while the firmware is ready).
        regs.disable_interrupts();
        let mut pages = |bytes: usize| alloc(bytes.div_ceil(PAGE)).ok_or(Error::OutOfMemory);
        // QEMU maps 2 KiB from each frame's address, whatever its size.
        let frames = pages(commands * FRAME + QEMU_FRAME_MAP)?;
        let sense = pages(commands * SENSE_STRIDE)?;
        let queue = pages(entries as usize * 4)?;
        let state = pages(state::ASKED + CONTEXTS * 8)?;
        // The SAS1078 and SAS2108 take a frame's address in 32 bits.
        if !(below_4g(&frames) && below_4g(&state)) {
            return Err(Error::Device("its frames are not below 4 GiB"));
        }
        for memory in [&frames, &sense, &queue, &state] {
            memory.zero();
        }
        let qi = state::QUEUE_INFO;
        state.write32(qi + frame::init::QI_FLAGS, 0);
        state.write32(qi + frame::init::QI_ENTRIES, entries);
        state.write64(qi + frame::init::QI_QUEUE, queue.phys());
        state.write64(qi + frame::init::QI_PRODUCER, state.phys() + state::PRODUCER as u64);
        state.write64(qi + frame::init::QI_CONSUMER, state.phys() + state::CONSUMER as u64);
        let init = state::INIT;
        frame::write_header(
            &state,
            init,
            frame::cmd::INIT,
            0,
            0,
            0,
            frame::flag::DONT_POST,
            frame::init::QI_SIZE as u32,
        );
        let qi_phys = state.phys() + qi as u64;
        state.write32(init + frame::init::QUEUE_INFO_LOW, qi_phys as u32);
        state.write32(init + frame::init::QUEUE_INFO_HIGH, (qi_phys >> 32) as u32);
        let mfi = Mfi {
            regs,
            frames,
            sense,
            queue,
            state,
            entries,
            max_segments,
            slots: Slots::new(commands),
        };
        mfi.post(mfi.state.phys() + init as u64, frame::header::SIZE + frame::init::QI_SIZE);
        let mut waited = 0;
        loop {
            device_barrier();
            let (status, _) = frame::read_status(&mfi.state, init);
            if status == frame::status::OK {
                return Ok(mfi);
            }
            if status != frame::PENDING {
                return Err(Error::Device("its firmware refused its reply queue"));
            }
            if waited >= INIT_TIMEOUT_MS {
                return Err(Error::Device("its firmware did not take its reply queue"));
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

    /// Sends a SCSI command; returns its slot.
    pub fn submit(
        &mut self,
        target: Target,
        lun: u8,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        check_request(cdb, data, self.max_segments)?;
        let (volume, id, lun) = match target {
            Target::Volume(id) => (true, id, lun),
            // QEMU numbers a disk by its SCSI ID and LUN, `id << 8 | lun`;
            // a controller's own numbers are below 256, at LUN 0.
            Target::Disk(id) if id > 0xff => (false, id >> 8, (id & 0xff) as u8),
            Target::Disk(id) => (false, id, lun),
        };
        // A frame names its target in a byte.
        let id = u8::try_from(id).map_err(|_| Error::BadRequest)?;
        let slot = self.slots.take().ok_or(Error::Full)?;
        let at = usize::from(slot) * FRAME;
        let sense = self.sense.phys() + (usize::from(slot) * SENSE_STRIDE) as u64;
        let ieee = self.family() == Family::MfiSkinny;
        let to_device = direction == Direction::ToDevice;
        let context = u32::from(slot) + 1;
        frame::write_pass(
            &self.frames,
            at,
            context,
            volume,
            id,
            lun,
            cdb,
            data,
            to_device,
            sense,
            ieee,
        );
        self.note_asked(slot, data);
        self.post(self.frames.phys() + at as u64, frame::pass_size(data.len(), ieee));
        Ok(slot)
    }

    /// Sends the SCSI command `cdb` to the physical disk `device`, in a
    /// volume or not: an MFI controller takes the same pass frame for
    /// every disk. Returns its slot.
    pub fn submit_member(
        &mut self,
        device: u16,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        self.submit(Target::Disk(device), 0, cdb, data, direction)
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
        let at = usize::from(slot) * FRAME;
        let to_device = direction == Direction::ToDevice;
        let context = u32::from(slot) + 1;
        frame::write_dcmd(&self.frames, at, context, opcode, mailbox, data, to_device);
        self.note_asked(slot, data);
        self.post(self.frames.phys() + at as u64, frame::dcmd_size(data.len()));
        Ok(slot)
    }

    /// The next command that ended: its slot, and how.
    pub fn complete(&mut self) -> Option<(u16, Answer)> {
        loop {
            device_barrier();
            let produced = self.state.read32(state::PRODUCER);
            let consumed = self.state.read32(state::CONSUMER);
            if produced == consumed || produced >= self.entries || consumed >= self.entries {
                return None;
            }
            let context = self.queue.read32(consumed as usize * 4);
            device_barrier();
            self.state.write32(state::CONSUMER, (consumed + 1) % self.entries);
            // A context the driver did not give out is passed over.
            let Some(slot) = context.checked_sub(1).and_then(|slot| u16::try_from(slot).ok())
            else {
                continue;
            };
            if !self.slots.give_back(slot) {
                continue;
            }
            let at = usize::from(slot) * FRAME;
            let (status, scsi_status) = frame::read_status(&self.frames, at);
            let asked = self.state.read64(state::ASKED + usize::from(slot) * 8);
            let moved = if status == frame::status::OK { asked } else { 0 };
            let mut answer = Answer::new(status, scsi_status, asked, moved);
            let sense_len = usize::from(self.frames.read8(at + header::SENSE_LEN));
            answer.take_sense(&self.sense, usize::from(slot) * SENSE_STRIDE, sense_len);
            return Some((slot, answer));
        }
    }

    /// Stops the controller and drops every command in flight.
    pub fn reset(&mut self) {
        self.regs.disable_interrupts();
        self.regs.ring(crate::regs::doorbell::RESET);
        self.slots.clear();
    }

    /// Notes how much the command in `slot` asked to move.
    fn note_asked(&self, slot: u16, data: &[(u64, u32)]) {
        let length = data.iter().map(|&(_, len)| u64::from(len)).sum::<u64>();
        self.state.write64(state::ASKED + usize::from(slot) * 8, length);
    }

    /// Posts the frame at `phys`, `size` bytes of it.
    fn post(&self, phys: u64, size: usize) {
        // The 64-byte frames after the first, seven at most: the firmware
        // reads the rest of a longer list itself.
        let extra = (size.div_ceil(64).saturating_sub(1)).min(7) as u32;
        let word = (phys as u32 & !0x1f) | (extra << 1) | 1;
        device_barrier();
        match self.family() {
            Family::MfiSkinny => {
                self.regs.write32(reg::INBOUND_HIGH, (phys >> 32) as u32);
                self.regs.write32(reg::INBOUND_LOW, word);
            }
            _ => self.regs.write32(reg::INBOUND_QUEUE, word),
        }
    }
}

/// The sense data each command has room for.
const _: () = assert!(SENSE_SIZE <= SENSE_STRIDE);
/// A pass frame with the most entries fits a command's frame.
const _: () = assert!(frame::pass::SGL + MAX_SEGMENTS * frame::SGE64 <= FRAME);
