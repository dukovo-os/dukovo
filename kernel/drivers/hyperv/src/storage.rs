// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The synthetic SCSI controller (storvsc): Hyper-V's and Azure's disks.
//!
//! Every packet each way is a 64-byte storage packet: an operation, flags
//! and a status, then a SCSI request block or what the operation asks.
//! The guest begins (BEGIN_INITIALIZATION), agrees on a protocol version
//! (QUERY_PROTOCOL_VERSION, newest first, until the host takes one),
//! asks the controller's properties (QUERY_PROPERTIES) and ends
//! (END_INITIALIZATION); the host answers each with COMPLETE_IO and a
//! status of 0. Then each SCSI command goes in an EXECUTE_SRB packet: the
//! target, the logical unit, the command, its direction and length, with
//! the pages its data is read into or written from named by the packet
//! (a PAGES packet with one range), and the host answers with COMPLETE_IO,
//! the request block's status, the SCSI status and, on CHECK CONDITION,
//! the sense data in the command's place.
//!
//! The kernel speaks the versions of Windows 8 and later, whose request
//! blocks carry the Windows 8 extension (the request's flags and its
//! timeout) and 20 bytes of sense data.

/// The size of a storage packet, with the Windows 8 extension.
pub const PACKET: usize = 64;

/// The operations.
pub mod operation {
    /// The host's answer to any of the guest's packets.
    pub const COMPLETE_IO: u32 = 1;
    /// The host took a disk away.
    pub const REMOVE_DEVICE: u32 = 2;
    /// A SCSI command.
    pub const EXECUTE_SRB: u32 = 3;
    /// The guest begins to set the controller up.
    pub const BEGIN_INITIALIZATION: u32 = 7;
    /// The guest is done setting it up.
    pub const END_INITIALIZATION: u32 = 8;
    /// The guest asks for a protocol version.
    pub const QUERY_PROTOCOL_VERSION: u32 = 9;
    /// The guest asks the controller's properties.
    pub const QUERY_PROPERTIES: u32 = 10;
    /// The host added or removed a disk: the bus is to be scanned again.
    pub const ENUMERATE_BUS: u32 = 11;
}

/// The protocol versions the kernel speaks, newest first: Windows 10's
/// 6.2, Windows 8.1's 6.0 and Windows 8's 5.1, as major << 8 | minor.
pub const VERSIONS: [u16; 3] = [0x0602, 0x0600, 0x0501];

/// A packet flag: the host answers it.
const REQUEST_COMPLETION: u32 = 1;

/// The bytes of sense data the host gives.
pub const SENSE: usize = 20;
/// The most bytes of a SCSI command.
pub const CDB: usize = 16;
/// The bytes of the request block, with the Windows 8 extension.
const SRB_LENGTH: u16 = 52;

/// A request block's direction: data to the device.
const WRITE: u8 = 0;
/// A request block's direction: data from the device.
const READ: u8 = 1;
/// A request block's direction: no data.
const NO_DATA: u8 = 2;

/// Request block flags (the Windows 8 extension).
const FLAG_DISABLE_SYNCH_TRANSFER: u32 = 0x08;
const FLAG_DATA_IN: u32 = 0x40;
const FLAG_DATA_OUT: u32 = 0x80;

/// A request block's status, without its two flag bits.
pub mod srb_status {
    /// Not done yet.
    pub const PENDING: u8 = 0x00;
    /// Done: the SCSI status says how it ended.
    pub const SUCCESS: u8 = 0x01;
    /// Aborted.
    pub const ABORTED: u8 = 0x02;
    /// Failed: the SCSI status says why, if it is CHECK CONDITION.
    pub const ERROR: u8 = 0x04;
    /// The host is busy.
    pub const BUSY: u8 = 0x05;
    /// The host refused the request.
    pub const INVALID_REQUEST: u8 = 0x06;
    /// No such bus.
    pub const INVALID_PATH_ID: u8 = 0x07;
    /// No device at that address.
    pub const NO_DEVICE: u8 = 0x08;
    /// It took too long.
    pub const TIMEOUT: u8 = 0x09;
    /// No device answered at that address.
    pub const SELECTION_TIMEOUT: u8 = 0x0a;
    /// The command took too long.
    pub const COMMAND_TIMEOUT: u8 = 0x0b;
    /// The bus was reset.
    pub const BUS_RESET: u8 = 0x0e;
    /// The data were not the length asked: shorter, for a command that
    /// moves less than its buffer holds.
    pub const DATA_OVERRUN: u8 = 0x12;
    /// The request was flushed.
    pub const REQUEST_FLUSHED: u8 = 0x16;
    /// No such logical unit.
    pub const INVALID_LUN: u8 = 0x20;
    /// No such target.
    pub const INVALID_TARGET_ID: u8 = 0x21;
}
/// A request block status flag: the sense data is valid.
pub const AUTOSENSE_VALID: u8 = 0x80;
/// A request block status flag: the device's queue is frozen.
const QUEUE_FROZEN: u8 = 0x40;

/// A packet with operation `operation` and nothing else.
fn packet(operation: u32) -> [u8; PACKET] {
    let mut bytes = [0u8; PACKET];
    bytes[0..4].copy_from_slice(&operation.to_le_bytes());
    bytes[4..8].copy_from_slice(&REQUEST_COMPLETION.to_le_bytes());
    bytes
}

/// BEGIN_INITIALIZATION.
pub fn begin() -> [u8; PACKET] {
    packet(operation::BEGIN_INITIALIZATION)
}

/// QUERY_PROTOCOL_VERSION for `version`.
pub fn query_version(version: u16) -> [u8; PACKET] {
    let mut bytes = packet(operation::QUERY_PROTOCOL_VERSION);
    bytes[12..14].copy_from_slice(&version.to_le_bytes());
    bytes
}

/// QUERY_PROPERTIES.
pub fn query_properties() -> [u8; PACKET] {
    packet(operation::QUERY_PROPERTIES)
}

/// END_INITIALIZATION.
pub fn end() -> [u8; PACKET] {
    packet(operation::END_INITIALIZATION)
}

/// Which way a command's data goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the device into memory.
    In,
    /// From memory to the device.
    Out,
    /// No data.
    None,
}

/// EXECUTE_SRB: command `cdb` to logical unit `lun` of `target`, moving
/// `length` bytes `direction`.
pub fn execute(target: u8, lun: u8, cdb: &[u8], direction: Direction, length: u32) -> [u8; PACKET] {
    let mut bytes = packet(operation::EXECUTE_SRB);
    let cdb = &cdb[..cdb.len().min(CDB)];
    bytes[12..14].copy_from_slice(&SRB_LENGTH.to_le_bytes());
    // Port and path 0, as the host numbers its one bus.
    bytes[18] = target;
    bytes[19] = lun;
    bytes[20] = cdb.len() as u8;
    bytes[21] = SENSE as u8;
    let (data_in, flags) = match direction {
        Direction::In => (READ, FLAG_DATA_IN),
        Direction::Out => (WRITE, FLAG_DATA_OUT),
        Direction::None => (NO_DATA, 0),
    };
    bytes[22] = data_in;
    bytes[24..28].copy_from_slice(&length.to_le_bytes());
    bytes[28..28 + cdb.len()].copy_from_slice(cdb);
    // The Windows 8 extension: the flags and a timeout of 60 seconds.
    bytes[52..56].copy_from_slice(&(flags | FLAG_DISABLE_SYNCH_TRANSFER).to_le_bytes());
    bytes[56..60].copy_from_slice(&60u32.to_le_bytes());
    bytes
}

/// What the host answered.
#[derive(Clone, Copy, Debug)]
pub struct Reply {
    /// The operation: [`operation::COMPLETE_IO`] for an answer.
    pub operation: u32,
    /// The packet's status: 0 is success.
    pub status: u32,
    /// The request block's status, without its flags.
    pub srb_status: u8,
    /// Whether [`Reply::sense`] holds the sense data.
    pub sense_valid: bool,
    /// The SCSI status.
    pub scsi_status: u8,
    /// The bytes moved.
    pub transferred: u32,
    /// The sense data, on CHECK CONDITION.
    pub sense: [u8; SENSE],
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The storage packet `data`; `None` if it is shorter than its header.
pub fn parse(data: &[u8]) -> Option<Reply> {
    if data.len() < 12 {
        return None;
    }
    let mut sense = [0u8; SENSE];
    if let Some(bytes) = data.get(28..28 + SENSE) {
        sense.copy_from_slice(bytes);
    }
    let srb = data.get(14).copied().unwrap_or(0);
    Some(Reply {
        operation: u32_at(data, 0),
        status: u32_at(data, 8),
        srb_status: srb & !(AUTOSENSE_VALID | QUEUE_FROZEN),
        sense_valid: srb & AUTOSENSE_VALID != 0,
        scsi_status: data.get(15).copied().unwrap_or(0),
        transferred: u32_at(data, 24),
        sense,
    })
}

/// The controller's properties, from the answer to QUERY_PROPERTIES.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Properties {
    /// How many more channels the controller could open.
    pub max_channels: u16,
    /// The most bytes one command may move: 0 if the host does not say.
    pub max_transfer: u32,
}

/// The properties in the answer to QUERY_PROPERTIES.
pub fn properties(data: &[u8]) -> Properties {
    let u16_at = |at: usize| data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    // The protocol version, path, target, channel count, port, flags and
    // the transfer limit.
    Properties { max_channels: u16_at(16), max_transfer: u32_at(data, 24) }
}

/// What a request block's status means for its command, besides success
/// and a SCSI status to look at.
pub fn status_name(status: u8) -> &'static str {
    match status {
        srb_status::ABORTED => "the host aborted the command",
        srb_status::ERROR => "the host failed the command",
        srb_status::BUSY => "the host is busy",
        srb_status::INVALID_REQUEST => "the host refused the command",
        srb_status::INVALID_PATH_ID | srb_status::INVALID_TARGET_ID => "no such target",
        srb_status::NO_DEVICE | srb_status::SELECTION_TIMEOUT => "no device there",
        srb_status::TIMEOUT | srb_status::COMMAND_TIMEOUT => "the command timed out",
        srb_status::BUS_RESET => "the bus was reset",
        srb_status::DATA_OVERRUN => "more data than asked",
        srb_status::REQUEST_FLUSHED => "the command was flushed",
        srb_status::INVALID_LUN => "no such logical unit",
        _ => "the host failed the command",
    }
}
