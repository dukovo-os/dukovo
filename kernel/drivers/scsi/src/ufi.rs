// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB floppy drives (ADR 0436): what the *USB Mass Storage Class UFI
//! Command Specification, Revision 1.0* adds to the commands of SPC and
//! SBC for a drive of 3.5-inch disks.
//!
//! A UFI drive takes a subset of SCSI's commands, each in a 12-byte
//! command block, which the transport pads a shorter one to
//! (`radix_usb::storage::cbi`). Two matter here:
//!
//! - It has no MODE SENSE(6): whether its disk is write-protected comes
//!   from MODE SENSE(10)'s header, which [`crate::medium`] asks for when
//!   MODE SENSE(6) fails ([`crate::write_protected_10`]).
//! - It formats a disk a track at a time with FORMAT UNIT, the
//!   track and side in the command and the disk's size in a format
//!   descriptor: [`format_track`].

use crate::probe::{Asked, ask_out};
use crate::{Absent, Cdb, op};

/// FORMAT UNIT's parameter list: a defect list header and one format
/// descriptor.
pub const FORMAT_PARAMETERS: usize = 12;
/// How long a track's format may take: two turns of the disk, a seek and
/// the drive's check of what it wrote, with room to spare.
pub const FORMAT_TIMEOUT_MS: u32 = 30_000;

/// FORMAT UNIT's command block for a track of a UFI drive's disk:
/// FmtData, as a parameter list follows, a defect list format of 7, and
/// `track`, the cylinder. The parameter list is [`format_parameters`].
pub fn format_unit(track: u8) -> Cdb {
    let length = FORMAT_PARAMETERS as u16;
    let [high, low] = length.to_be_bytes();
    Cdb::new(&[op::FORMAT_UNIT, 0x17, track, 0, 0, 0, 0, high, low, 0, 0, 0])
}

/// FORMAT UNIT's parameter list for one track of a disk of `blocks`
/// blocks of `block_length` bytes: the defect list header, its format
/// options valid (FOV), one track (SingleTrack) on side `side`, and 8
/// bytes of descriptor; then the format descriptor, the disk's blocks and
/// their length, which says which of the drive's formats to write.
pub fn format_parameters(side: u8, blocks: u32, block_length: u32) -> [u8; FORMAT_PARAMETERS] {
    const FOV: u8 = 0x80;
    const SINGLE_TRACK: u8 = 0x10;
    let mut bytes = [0u8; FORMAT_PARAMETERS];
    bytes[1] = FOV | SINGLE_TRACK | (side & 1);
    bytes[3] = 8;
    bytes[4..8].copy_from_slice(&blocks.to_be_bytes());
    bytes[9..12].copy_from_slice(&block_length.to_be_bytes()[1..]);
    bytes
}

/// Formats track `track` on side `side` of the disk in the UFI drive at
/// logical unit `lun`, as a disk of `blocks` blocks of 512 bytes: 1440 a
/// 720 KB disk, 2880 a 1.44 MB one. What the track held is gone. Fails
/// with [`Absent::Failed`] when the drive refuses: no disk in it
/// ([`crate::Meaning::NoMedium`]), a write-protected one, or a size its
/// disk cannot take.
pub fn format_track<T: crate::Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    (track, side): (u8, u8),
    blocks: u32,
) -> Result<(), Absent> {
    let parameters = format_parameters(side, blocks, 512);
    let cdb = format_unit(track);
    match ask_out(transport, lun, &cdb, &parameters, FORMAT_TIMEOUT_MS)? {
        Asked::Came(_) => Ok(()),
        Asked::Failed(sense) => Err(Absent::Failed("FORMAT UNIT", sense)),
    }
}
