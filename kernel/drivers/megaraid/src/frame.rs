// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! MFI frames: the commands of the MFI interface, which the fusion one
//! also carries for its management commands.
//!
//! A frame is a 24-byte header, then what its command needs, then a
//! scatter-gather list. MFI sends a frame as is; fusion sends a DCMD frame
//! inside a pass-through request ([`crate::fusion`]). The functions here
//! write a frame into DMA memory at an offset the transport chose.

use radix_hw::Dma;

/// The commands a frame carries (its first byte).
pub mod cmd {
    /// Give the firmware its queues.
    pub const INIT: u8 = 0x00;
    /// A SCSI command to a logical drive (a volume).
    pub const LD_SCSI_IO: u8 = 0x03;
    /// A SCSI command to a physical disk.
    pub const PD_SCSI_IO: u8 = 0x04;
    /// A management command.
    pub const DCMD: u8 = 0x05;
}

/// The header's fields, by their offsets.
pub mod header {
    /// The command.
    pub const CMD: usize = 0;
    /// The room for sense data; the firmware leaves how much it wrote.
    pub const SENSE_LEN: usize = 1;
    /// The firmware's status of the command.
    pub const CMD_STATUS: usize = 2;
    /// The target's SCSI status.
    pub const SCSI_STATUS: usize = 3;
    /// The target.
    pub const TARGET: usize = 4;
    /// The logical unit.
    pub const LUN: usize = 5;
    /// The length of the command descriptor block.
    pub const CDB_LEN: usize = 6;
    /// How many scatter-gather entries follow.
    pub const SG_COUNT: usize = 7;
    /// The host's number for the command, which the reply queue gives back.
    pub const CONTEXT: usize = 8;
    /// The context's high half, kept 0.
    pub const CONTEXT_HIGH: usize = 12;
    /// The flags ([`super::flag`]).
    pub const FLAGS: usize = 16;
    /// A timeout, unused (0).
    pub const TIMEOUT: usize = 18;
    /// How many bytes the data are.
    pub const DATA_LEN: usize = 20;
    /// The size of the header.
    pub const SIZE: usize = 24;
}

/// Bits of the header's flags.
pub mod flag {
    /// Do not put the frame's context on the reply queue: the host polls
    /// its status instead.
    pub const DONT_POST: u16 = 0x0001;
    /// The scatter-gather entries have 64-bit addresses.
    pub const SGL64: u16 = 0x0002;
    /// The sense buffer's address is 64 bits.
    pub const SENSE64: u16 = 0x0004;
    /// The data go to the device.
    pub const WRITE: u16 = 0x0008;
    /// The data come from the device.
    pub const READ: u16 = 0x0010;
    /// The scatter-gather entries are the 16-byte IEEE ones (skinny parts).
    pub const IEEE: u16 = 0x0020;
}

/// Where a DCMD frame's fields are.
pub mod dcmd {
    /// The opcode.
    pub const OPCODE: usize = 24;
    /// The 12-byte mailbox.
    pub const MAILBOX: usize = 28;
    /// The scatter-gather list.
    pub const SGL: usize = 40;
}

/// Where a SCSI pass frame's fields are.
pub mod pass {
    /// The sense buffer's address, low half.
    pub const SENSE_LOW: usize = 24;
    /// Its high half.
    pub const SENSE_HIGH: usize = 28;
    /// The command descriptor block, 16 bytes.
    pub const CDB: usize = 32;
    /// The scatter-gather list.
    pub const SGL: usize = 48;
}

/// Where the INIT frame's fields are, and its queue information's.
pub mod init {
    /// The address of the queue information.
    pub const QUEUE_INFO_LOW: usize = 24;
    /// Its high half.
    pub const QUEUE_INFO_HIGH: usize = 28;
    /// Fusion: what the driver can do (a capability word).
    pub const CAPABILITIES: usize = 4;
    /// The queue information's flags (0: 32-bit contexts).
    pub const QI_FLAGS: usize = 0;
    /// The reply queue's entries.
    pub const QI_ENTRIES: usize = 4;
    /// The reply queue's address.
    pub const QI_QUEUE: usize = 8;
    /// The producer index's address.
    pub const QI_PRODUCER: usize = 16;
    /// The consumer index's address.
    pub const QI_CONSUMER: usize = 24;
    /// The size of the queue information.
    pub const QI_SIZE: usize = 32;
}

/// The size of a 64-bit scatter-gather entry: address and length.
pub const SGE64: usize = 12;
/// The size of an IEEE entry: address, length and flags.
pub const SGE_IEEE: usize = 16;

/// A frame's status when the firmware has not answered: the host sets it
/// before posting a frame it polls.
pub const PENDING: u8 = 0xff;
/// The sense data a SCSI command may return.
pub const SENSE_SIZE: usize = 96;
/// The longest command descriptor block a frame holds.
pub const CDB_SIZE: usize = 16;

/// The command statuses the firmware writes, the ones the driver tells
/// apart.
pub mod status {
    /// Done.
    pub const OK: u8 = 0x00;
    /// The command is not known.
    pub const INVALID_CMD: u8 = 0x01;
    /// The management command is not known.
    pub const INVALID_DCMD: u8 = 0x02;
    /// A parameter is not valid.
    pub const INVALID_PARAMETER: u8 = 0x03;
    /// There is no such device.
    pub const DEVICE_NOT_FOUND: u8 = 0x0c;
    /// The volume is offline.
    pub const LD_OFFLINE: u8 = 0x33;
    /// The SCSI command ended with an error: its SCSI status says which.
    pub const SCSI_DONE_WITH_ERROR: u8 = 0x2d;
    /// The SCSI command failed.
    pub const SCSI_IO_FAILED: u8 = 0x2e;
    /// The controller is in the wrong state.
    pub const WRONG_STATE: u8 = 0x32;
    /// The configuration changed under the command.
    pub const CONFIG_SEQ_MISMATCH: u8 = 0x67;
}

/// A command status in words, for the log.
pub fn status_text(code: u8) -> &'static str {
    match code {
        status::OK => "done",
        status::INVALID_CMD => "the command is not known",
        status::INVALID_DCMD => "the management command is not known",
        status::INVALID_PARAMETER => "a parameter is not valid",
        status::DEVICE_NOT_FOUND => "there is no such device",
        0x0d => "the drive is too small",
        0x17 => "a consistency check is running",
        0x18 => "the volume is initialising",
        0x19 => "the block is beyond the volume",
        0x1c => "a rebuild is running",
        0x20 => "the controller is out of memory",
        0x21 => "the controller's hardware failed",
        status::SCSI_DONE_WITH_ERROR => "the SCSI command ended with an error",
        status::SCSI_IO_FAILED => "the SCSI command failed",
        0x2f => "a reservation conflicts",
        0x30 => "the shutdown failed",
        0x31 => "the time is not set",
        status::WRONG_STATE => "the controller is in the wrong state",
        status::LD_OFFLINE => "the volume is offline",
        0x37 => "the controller found I2C errors",
        0x38 => "the controller found PCI errors",
        0x39 => "the controller's self-test failed",
        status::CONFIG_SEQ_MISMATCH => "the configuration changed",
        PENDING => "no answer",
        _ => "an error",
    }
}

/// Writes a header at `at` of `memory`: the command, the target and
/// logical unit, the context, the flags and the data length; zeroes the
/// rest of the header and sets the status to [`PENDING`].
#[allow(clippy::too_many_arguments)]
pub fn write_header(
    memory: &Dma,
    at: usize,
    command: u8,
    target: u8,
    lun: u8,
    context: u32,
    flags: u16,
    data_len: u32,
) {
    memory.write(at, &[0; header::SIZE]);
    memory.write8(at + header::CMD, command);
    memory.write8(at + header::CMD_STATUS, PENDING);
    memory.write8(at + header::TARGET, target);
    memory.write8(at + header::LUN, lun);
    memory.write32(at + header::CONTEXT, context);
    memory.write16(at + header::FLAGS, flags);
    memory.write32(at + header::DATA_LEN, data_len);
}

/// Writes the scatter-gather list `runs` (address, length) at `at`, in
/// 64-bit entries, or in IEEE ones if `ieee`; returns its size.
pub fn write_sgl(memory: &Dma, at: usize, runs: &[(u64, u32)], ieee: bool) -> usize {
    let size = if ieee { SGE_IEEE } else { SGE64 };
    for (n, &(phys, len)) in runs.iter().enumerate() {
        let entry = at + n * size;
        // A 12-byte entry's address is only 4-byte aligned after the
        // first: it is written as two halves.
        memory.write32(entry, phys as u32);
        memory.write32(entry + 4, (phys >> 32) as u32);
        memory.write32(entry + 8, len);
        if ieee {
            memory.write32(entry + 12, 0);
        }
    }
    runs.len() * size
}

/// The direction flags of data going `to_device`, or coming from it, or
/// of none if there are none.
pub fn direction_flags(runs: &[(u64, u32)], to_device: bool) -> u16 {
    match (runs.is_empty(), to_device) {
        (true, _) => 0,
        (false, true) => flag::WRITE,
        (false, false) => flag::READ,
    }
}

/// Writes a DCMD frame at `at`: `opcode` with `mailbox`, its data the
/// runs `data` going `to_device` or from it, posted with `context`. A
/// DCMD's entries are always the 64-bit ones.
pub fn write_dcmd(
    memory: &Dma,
    at: usize,
    context: u32,
    opcode: u32,
    mailbox: &[u8; 12],
    data: &[(u64, u32)],
    to_device: bool,
) {
    let length = data.iter().map(|&(_, len)| len).sum::<u32>();
    let flags = direction_flags(data, to_device) | if data.is_empty() { 0 } else { flag::SGL64 };
    write_header(memory, at, cmd::DCMD, 0, 0, context, flags, length);
    memory.write8(at + header::SG_COUNT, data.len() as u8);
    memory.write32(at + dcmd::OPCODE, opcode);
    memory.write(at + dcmd::MAILBOX, mailbox);
    write_sgl(memory, at + dcmd::SGL, data, false);
}

/// The size of a DCMD frame with `runs` entries.
pub fn dcmd_size(runs: usize) -> usize {
    dcmd::SGL + runs * SGE64
}

/// Writes a SCSI pass frame at `at`: the command `cdb` to `target` and
/// `lun`, a volume's if `volume` and a physical disk's otherwise, its
/// data the runs `data` going `to_device` or from it, its sense data to
/// `sense`, posted with `context`; the entries are IEEE ones if `ieee`.
#[allow(clippy::too_many_arguments)]
pub fn write_pass(
    memory: &Dma,
    at: usize,
    context: u32,
    volume: bool,
    target: u8,
    lun: u8,
    cdb: &[u8],
    data: &[(u64, u32)],
    to_device: bool,
    sense: u64,
    ieee: bool,
) {
    let length = data.iter().map(|&(_, len)| len).sum::<u32>();
    let mut flags = direction_flags(data, to_device) | flag::SGL64 | flag::SENSE64;
    if ieee {
        flags |= flag::IEEE;
    }
    let command = if volume { cmd::LD_SCSI_IO } else { cmd::PD_SCSI_IO };
    write_header(memory, at, command, target, lun, context, flags, length);
    memory.write8(at + header::SENSE_LEN, SENSE_SIZE as u8);
    memory.write8(at + header::CDB_LEN, cdb.len() as u8);
    memory.write8(at + header::SG_COUNT, data.len() as u8);
    memory.write32(at + pass::SENSE_LOW, sense as u32);
    memory.write32(at + pass::SENSE_HIGH, (sense >> 32) as u32);
    let mut padded = [0; CDB_SIZE];
    padded[..cdb.len()].copy_from_slice(cdb);
    memory.write(at + pass::CDB, &padded);
    write_sgl(memory, at + pass::SGL, data, ieee);
}

/// The size of a pass frame with `runs` entries.
pub fn pass_size(runs: usize, ieee: bool) -> usize {
    pass::SGL + runs * if ieee { SGE_IEEE } else { SGE64 }
}

/// What a frame's header says once the firmware answered: the command
/// status and the SCSI status.
pub fn read_status(memory: &Dma, at: usize) -> (u8, u8) {
    (memory.read8(at + header::CMD_STATUS), memory.read8(at + header::SCSI_STATUS))
}
