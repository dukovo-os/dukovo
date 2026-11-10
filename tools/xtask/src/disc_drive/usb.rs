// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The drive as a USB device: a high-speed mass storage device of the
//! Bulk-Only Transport (USB Mass Storage Class, Bulk-Only Transport 1.0),
//! with one interface of SCSI's transparent command set, a bulk endpoint
//! in (81h) and one out (02h).
//!
//! Its control transfers are the standard requests a host makes of any
//! device (its descriptors, its status, an endpoint's halt cleared) and
//! Bulk-Only's two: Get Max LUN, which says it has one logical unit, and
//! the reset. Its bulk transfers carry the commands: a command block
//! wrapper out, its data in or out, and a command status wrapper in.

use super::mmc::Drive;

/// pid.codes's vendor, and one of the product IDs it keeps for private
/// testing, which no product on sale has.
pub const VENDOR_ID: u16 = 0x1209;
pub const PRODUCT_ID: u16 = 0x0001;
/// The device's release, 1.00.
pub const DEVICE_VERSION: u16 = 0x0100;

/// The bulk endpoints.
pub const BULK_IN: u8 = 0x81;
pub const BULK_OUT: u8 = 0x02;
/// Their packet size at high speed, and the control endpoint's.
pub const BULK_PACKET: u16 = 512;
pub const CONTROL_PACKET: u16 = 64;

/// Its interface's class: mass storage, SCSI's transparent command set,
/// Bulk-Only.
pub const INTERFACE: [u8; 4] = [0, 0x08, 0x06, 0x50];

/// A command block wrapper's and a command status wrapper's signatures,
/// and their lengths.
const CBW_SIGNATURE: u32 = 0x4342_5355;
const CSW_SIGNATURE: u32 = 0x5342_5355;
const CBW_LENGTH: usize = 31;

/// The device's strings, from index 1: its maker, its name, and its
/// serial number, which Bulk-Only asks to be at least twelve hex digits.
const STRINGS: [&str; 3] = ["Dukovo", "Test Disc Drive", "000000000001"];

/// Where the device is in a command's three phases.
enum Phase {
    /// Waiting for a command block wrapper.
    Command,
    /// Its data to the host: `data`, of which `sent` bytes went, where the
    /// host asked for `expected`.
    DataIn { tag: u32, data: Vec<u8>, sent: usize, expected: u32, failed: bool },
    /// Its data from the host, of which `data` came, `expected` in all.
    DataOut { tag: u32, cdb: Vec<u8>, data: Vec<u8>, expected: u32 },
    /// Its status, for the host to read.
    Status { tag: u32, residue: u32, failed: bool },
}

/// The device.
pub struct Device {
    pub drive: Drive,
    configuration: u8,
    phase: Phase,
}

/// A transfer's answer: done with this data, or stalled.
pub type Answer = Result<Vec<u8>, Stall>;

/// The endpoint stalls the transfer: a request it does not take.
#[derive(Debug, PartialEq, Eq)]
pub struct Stall;

impl Device {
    pub fn new(drive: Drive) -> Device {
        Device { drive, configuration: 0, phase: Phase::Command }
    }

    /// Its configuration: 0 until the host sets it.
    pub fn configuration(&self) -> u8 {
        self.configuration
    }

    /// SET_CONFIGURATION: it has one, 1.
    pub fn set_configuration(&mut self, value: u8) -> bool {
        if value > 1 {
            return false;
        }
        self.configuration = value;
        self.phase = Phase::Command;
        true
    }

    /// A reset of the port: the device as plugged in.
    pub fn reset(&mut self) {
        self.configuration = 0;
        self.phase = Phase::Command;
    }

    /// A control transfer of `request` (by `request_type`), `value` and
    /// `index`, asking for up to `length` bytes or giving `data`.
    pub fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
    ) -> Answer {
        const GET_STATUS: u8 = 0;
        const CLEAR_FEATURE: u8 = 1;
        const SET_FEATURE: u8 = 3;
        const GET_DESCRIPTOR: u8 = 6;
        const GET_MAX_LUN: u8 = 0xfe;
        const MASS_STORAGE_RESET: u8 = 0xff;
        let mut data = match (request_type, request) {
            (0x80..=0x82, GET_STATUS) => vec![0, 0],
            (0x00..=0x02, CLEAR_FEATURE | SET_FEATURE) => Vec::new(),
            (0x80, GET_DESCRIPTOR) => {
                descriptor((value >> 8) as u8, value as u8, index).ok_or(Stall)?
            }
            (0xa1, GET_MAX_LUN) => vec![0],
            (0x21, MASS_STORAGE_RESET) => {
                self.phase = Phase::Command;
                Vec::new()
            }
            _ => return Err(Stall),
        };
        data.truncate(usize::from(length));
        Ok(data)
    }

    /// A bulk transfer out: a command block wrapper, or a command's data.
    pub fn bulk_out(&mut self, bytes: &[u8]) -> Result<(), Stall> {
        match &mut self.phase {
            Phase::Command => self.command(bytes),
            Phase::DataOut { data, expected, .. } => {
                data.extend_from_slice(bytes);
                if data.len() >= *expected as usize {
                    let Phase::DataOut { tag, cdb, data, expected } =
                        std::mem::replace(&mut self.phase, Phase::Command)
                    else {
                        unreachable!()
                    };
                    let failed = self.drive.command(&cdb, &data).is_err();
                    let residue = expected.saturating_sub(data.len() as u32);
                    self.phase = Phase::Status { tag, residue, failed };
                }
                Ok(())
            }
            // Data out where none is due.
            _ => Err(Stall),
        }
    }

    /// A bulk transfer in, of up to `length` bytes: a command's data, or
    /// its status. `None` while there is none to give: a host's transfer
    /// in waits, as a device's endpoint answers NAK, until a command
    /// comes.
    pub fn bulk_in(&mut self, length: u32) -> Option<Vec<u8>> {
        match &mut self.phase {
            Phase::Command | Phase::DataOut { .. } => None,
            Phase::DataIn { tag, data, sent, expected, failed } => {
                let left = data.len() - *sent;
                let n = left.min(length as usize);
                let chunk = data[*sent..*sent + n].to_vec();
                *sent += n;
                // A short packet, or all the host asked for, ends the
                // data.
                if n < length as usize || *sent as u32 >= *expected {
                    let residue = expected.saturating_sub(*sent as u32);
                    self.phase = Phase::Status { tag: *tag, residue, failed: *failed };
                }
                Some(chunk)
            }
            Phase::Status { tag, residue, failed } => {
                let mut csw = Vec::with_capacity(13);
                csw.extend(CSW_SIGNATURE.to_le_bytes());
                csw.extend(tag.to_le_bytes());
                csw.extend(residue.to_le_bytes());
                csw.push(u8::from(*failed));
                self.phase = Phase::Command;
                Some(csw)
            }
        }
    }

    /// A command block wrapper: the command is carried out now if its
    /// data comes from the device, once its data came if not.
    fn command(&mut self, bytes: &[u8]) -> Result<(), Stall> {
        if bytes.len() != CBW_LENGTH
            || u32::from_le_bytes(bytes[0..4].try_into().unwrap()) != CBW_SIGNATURE
        {
            return Err(Stall);
        }
        let tag = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let expected = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let into_host = bytes[12] & 0x80 != 0;
        let cdb = bytes[15..15 + usize::from(bytes[14].clamp(1, 16))].to_vec();
        self.phase = if !into_host && expected > 0 {
            Phase::DataOut { tag, cdb, data: Vec::new(), expected }
        } else {
            let (mut data, failed) = match self.drive.command(&cdb, &[]) {
                Ok(data) => (data, false),
                Err(_) => (Vec::new(), true),
            };
            data.truncate(expected as usize);
            if expected == 0 {
                Phase::Status { tag, residue: 0, failed }
            } else {
                Phase::DataIn { tag, data, sent: 0, expected, failed }
            }
        };
        Ok(())
    }
}

/// The descriptor of `kind` and `index` (a string's in language
/// `language`), as GET_DESCRIPTOR gives it; `None` for one the device does
/// not have.
pub fn descriptor(kind: u8, index: u8, language: u16) -> Option<Vec<u8>> {
    const DEVICE: u8 = 1;
    const CONFIGURATION: u8 = 2;
    const STRING: u8 = 3;
    const QUALIFIER: u8 = 6;
    let [packet0, _] = CONTROL_PACKET.to_le_bytes();
    let [packet_lo, packet_hi] = BULK_PACKET.to_le_bytes();
    let [vendor_lo, vendor_hi] = VENDOR_ID.to_le_bytes();
    let [product_lo, product_hi] = PRODUCT_ID.to_le_bytes();
    let [release_lo, release_hi] = DEVICE_VERSION.to_le_bytes();
    match (kind, index) {
        // USB 2.0, its class in its interface, one configuration; its
        // maker, name and serial number at strings 1 to 3.
        (DEVICE, 0) => Some(vec![
            18, DEVICE, 0x00, 0x02, 0, 0, 0, packet0, vendor_lo, vendor_hi, product_lo, product_hi,
            release_lo, release_hi, 1, 2, 3, 1,
        ]),
        // What it would be at full speed.
        (QUALIFIER, 0) => Some(vec![10, QUALIFIER, 0x00, 0x02, 0, 0, 0, packet0, 1, 0]),
        // Configuration 1: one interface, bus powered at 500 mA, and its
        // two bulk endpoints.
        (CONFIGURATION, 0) => {
            let [class, subclass, protocol] = [INTERFACE[1], INTERFACE[2], INTERFACE[3]];
            Some(vec![
                9,
                CONFIGURATION,
                32,
                0,
                1,
                1,
                0,
                0x80,
                250, //
                9,
                4,
                0,
                0,
                2,
                class,
                subclass,
                protocol,
                0, //
                7,
                5,
                BULK_IN,
                2,
                packet_lo,
                packet_hi,
                0, //
                7,
                5,
                BULK_OUT,
                2,
                packet_lo,
                packet_hi,
                0,
            ])
        }
        // US English alone.
        (STRING, 0) => Some(vec![4, STRING, 0x09, 0x04]),
        (STRING, n) if language == 0x0409 => {
            let text = STRINGS.get(usize::from(n) - 1)?;
            let mut string = vec![0, STRING];
            for unit in text.encode_utf16() {
                string.extend(unit.to_le_bytes());
            }
            string[0] = string.len() as u8;
            Some(string)
        }
        _ => None,
    }
}
