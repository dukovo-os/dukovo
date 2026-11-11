// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MMC commands a program sends an optical drive to record a disc
//! (ADR 0449), through radix-block(2)'s MMC_COMMAND: which of them pass,
//! which way each one's data goes, and which right each needs.
//!
//! burn(1) writes CDs, DVDs and Blu-ray discs as MMC-6 lays it out: it
//! asks the drive what disc it holds and how it is written, sets the
//! write parameters, blanks or formats a rewritable disc, writes, and
//! closes the track and the session. Each step is a command the kernel
//! carries for it as it is, but only one of those [`passed`] lists: no
//! command that reads a disc's data past the block layer, changes the
//! drive's firmware, or saves its settings, and no MODE SELECT but of the
//! write parameters page ([`mode_select_allowed`]). A command's data goes
//! the way MMC-6 says it does, not as the program says, and those that
//! change the disc need the write right.
//!
//! Start at [`passed`].

use crate::op;

/// The opcodes of the recording commands [`op`] does not name.
pub mod opcode {
    /// READ FORMAT CAPACITIES: the formats a rewritable disc takes.
    pub const READ_FORMAT_CAPACITIES: u8 = 0x23;
    /// READ TRACK INFORMATION: a track's state, start and next writable
    /// address.
    pub const READ_TRACK_INFORMATION: u8 = 0x52;
    /// RESERVE TRACK: sets aside room for a track of a size.
    pub const RESERVE_TRACK: u8 = 0x53;
    /// MODE SELECT(10): sets a page of the drive's parameters.
    pub const MODE_SELECT_10: u8 = 0x55;
    /// CLOSE TRACK/SESSION: closes a track, a session, or the disc.
    pub const CLOSE_TRACK_SESSION: u8 = 0x5b;
    /// READ BUFFER CAPACITY: how full the drive's buffer is.
    pub const READ_BUFFER_CAPACITY: u8 = 0x5c;
    /// SEND CUE SHEET: a disc-at-once CD's layout, before it is written.
    pub const SEND_CUE_SHEET: u8 = 0x5d;
    /// BLANK: erases a rewritable CD or DVD-RW.
    pub const BLANK: u8 = 0xa1;
    /// WRITE(12).
    pub const WRITE_12: u8 = 0xaa;
    /// GET PERFORMANCE: the speeds the drive writes the disc at.
    pub const GET_PERFORMANCE: u8 = 0xac;
    /// SET STREAMING: the speed to write a DVD or Blu-ray disc at.
    pub const SET_STREAMING: u8 = 0xb6;
    /// SET CD SPEED: the speed to write a CD at.
    pub const SET_CD_SPEED: u8 = 0xbb;
}

/// The write parameters mode page, the only one MODE SELECT sets.
pub const WRITE_PARAMETERS_PAGE: u8 = 0x05;

/// Which way a passed command's data goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// It moves none.
    None,
    /// From the drive.
    FromDrive,
    /// To the drive.
    ToDrive,
}

/// A command MMC_COMMAND passes: how long its block is, which way its data
/// goes, and whether it changes the disc or the drive's state, which
/// needs the write right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passed {
    /// The bytes of its command block: 6, 10 or 12.
    pub length: usize,
    /// Which way its data goes.
    pub direction: Direction,
    /// Whether it changes the disc, the drive's speed or its write
    /// parameters.
    pub writes: bool,
}

/// The commands passed, by opcode: which way their data goes, and
/// whether they write.
const PASSED: [(u8, Direction, bool); 23] = [
    (op::TEST_UNIT_READY, Direction::None, false),
    // The sense of the last command, with BLANK's and FORMAT UNIT's
    // progress.
    (op::REQUEST_SENSE, Direction::FromDrive, false),
    (op::INQUIRY, Direction::FromDrive, false),
    (opcode::READ_FORMAT_CAPACITIES, Direction::FromDrive, false),
    (op::READ_CAPACITY_10, Direction::FromDrive, false),
    (op::READ_TOC, Direction::FromDrive, false),
    (op::GET_CONFIGURATION, Direction::FromDrive, false),
    (op::READ_DISC_INFORMATION, Direction::FromDrive, false),
    (opcode::READ_TRACK_INFORMATION, Direction::FromDrive, false),
    (op::MODE_SENSE_10, Direction::FromDrive, false),
    (opcode::READ_BUFFER_CAPACITY, Direction::FromDrive, false),
    (opcode::GET_PERFORMANCE, Direction::FromDrive, false),
    (op::FORMAT_UNIT, Direction::ToDrive, true),
    (op::WRITE_10, Direction::ToDrive, true),
    (op::SYNCHRONIZE_CACHE_10, Direction::None, true),
    (opcode::RESERVE_TRACK, Direction::None, true),
    (opcode::MODE_SELECT_10, Direction::ToDrive, true),
    (opcode::CLOSE_TRACK_SESSION, Direction::None, true),
    (opcode::SEND_CUE_SHEET, Direction::ToDrive, true),
    (opcode::BLANK, Direction::None, true),
    (opcode::WRITE_12, Direction::ToDrive, true),
    (opcode::SET_STREAMING, Direction::ToDrive, true),
    (opcode::SET_CD_SPEED, Direction::None, true),
    // GET EVENT STATUS NOTIFICATION is the kernel's own: an event a
    // program took would not reach the drive's thread, as the eject
    // button's must. PREVENT ALLOW MEDIUM REMOVAL and START STOP UNIT are
    // the kernel's too: it keeps the tray shut while a disc is burned,
    // and EJECT lets it go before it opens.
];

/// The command `cdb` begins, if MMC_COMMAND passes it: one of its
/// opcodes, and for MODE SELECT(10), one that saves no page (its SP bit
/// clear). Its
/// length follows its opcode's group, as SPC lays out: 6 bytes for
/// 00h-1Fh, 10 for 20h-5Fh, 12 for A0h-BFh.
pub fn passed(cdb: &[u8]) -> Option<Passed> {
    let opcode = *cdb.first()?;
    let &(_, direction, writes) = PASSED.iter().find(|(code, _, _)| *code == opcode)?;
    let length = match opcode {
        0x00..=0x1f => 6,
        0x20..=0x5f => 10,
        0xa0..=0xbf => 12,
        _ => return None,
    };
    if cdb.len() < length {
        return None;
    }
    if opcode == opcode::MODE_SELECT_10 && cdb[1] & 0x01 != 0 {
        return None;
    }
    Some(Passed { length, direction, writes })
}

/// Whether the parameter list of a MODE SELECT(10), `data`, sets only
/// the write parameters page ([`WRITE_PARAMETERS_PAGE`]): an 8-byte
/// header, its block descriptors, and that page, whole.
pub fn mode_select_allowed(data: &[u8]) -> bool {
    let Some(header) = data.get(..8) else { return false };
    let descriptors = usize::from(u16::from_be_bytes([header[6], header[7]]));
    let Some(page) = data.get(8 + descriptors..) else { return false };
    match page {
        [code, length, ..] => {
            code & 0x3f == WRITE_PARAMETERS_PAGE && page.len() == 2 + usize::from(*length)
        }
        _ => false,
    }
}
