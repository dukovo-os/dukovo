// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A simulated BMC for the tests: it answers the commands of this module
//! from the data a test puts in it, keeps the reservations the SDR and the
//! SEL ask for, and refuses what the specification refuses (a partial SDR
//! read with a reservation that is no longer the current one, a Clear SEL
//! without "CLR", parts over 16 bytes). It also records what it was
//! asked to change.

use std::collections::{BTreeMap, HashMap};
use std::io;

use super::{NETFN_APP, NETFN_CHASSIS, NETFN_SENSOR, NETFN_STORAGE, NETFN_TRANSPORT, Transport};

/// The most bytes the simulated BMC gives in one Get SDR or Read FRU Data.
const PART_MAX: usize = 16;

/// The bytes the log has room for, as Get SEL Info says.
const SEL_SPACE: u16 = 0x0F00;

/// A BMC with the data of a test in it.
pub(crate) struct Sim {
    /// The answer to Get Device ID, after the completion code.
    pub device_id: Vec<u8>,
    /// What Get System GUID answers, in the order of the wire; `None`
    /// makes the command invalid.
    pub guid: Option<[u8; 16]>,
    /// The SDR records, whole.
    pub sdr: Vec<Vec<u8>>,
    /// The times Get SDR Repository Info says it last added a record and
    /// last erased the repository.
    pub sdr_added: u32,
    pub sdr_erased: u32,
    /// Makes the last record's "next" the first record: a loop.
    pub sdr_loops: bool,
    /// Cancels the SDR reservation at this Get SDR call.
    pub cancel_reservation_after: Option<usize>,
    pub reservations: usize,
    pub largest_sdr_read: usize,
    sdr_reservation: u16,
    sdr_calls: usize,
    /// The answer to Get Sensor Reading for each sensor number, after the
    /// completion code.
    pub readings: HashMap<u8, Vec<u8>>,
    /// The constants Get Sensor Reading Factors gives for a sensor at a
    /// reading.
    pub factors: HashMap<(u8, u8), [u8; 6]>,
    /// The SEL's 16-byte records.
    pub sel: Vec<[u8; 16]>,
    /// The times Get SEL Info says the log last had an entry added and
    /// was last erased.
    pub sel_added: u32,
    pub sel_erased: u32,
    sel_reservation: u16,
    pub erase_requests: usize,
    erase_pending: Option<u8>,
    /// The FRU device's data, for device 0.
    pub fru: Vec<u8>,
    pub fru_by_words: bool,
    pub largest_fru_read: usize,
    /// The channels and their media.
    pub channels: Vec<(u8, u8)>,
    /// The LAN parameters, as Get LAN Configuration Parameters gives them
    /// after the revision byte.
    pub lan: BTreeMap<u8, Vec<u8>>,
    /// The parameters set, in order, with their data.
    pub lan_sets: Vec<(u8, Vec<u8>)>,
    pub refuse_lan_parameter: Option<u8>,
    pub no_commit_write: bool,
    /// The data of each Chassis Identify.
    pub identifies: Vec<Vec<u8>>,
    pub identify_refused: bool,
}

impl Sim {
    /// A BMC like a Dell's: firmware 3.20, IPMI 2.0, with one LAN
    /// channel and nothing logged.
    pub(crate) fn new() -> Sim {
        let mut lan = BTreeMap::new();
        lan.insert(0, vec![0x00]);
        lan.insert(3, vec![10, 1, 2, 3]);
        lan.insert(4, vec![0x01]);
        lan.insert(5, vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        lan.insert(6, vec![255, 255, 255, 0]);
        lan.insert(12, vec![10, 1, 2, 1]);
        lan.insert(20, vec![0x23, 0x81]);
        let mut guid = [0u8; 16];
        for (index, byte) in guid.iter_mut().enumerate() {
            *byte = 0xFF - u8::try_from(index).unwrap() * 0x11;
        }
        Sim {
            device_id: vec![0x20, 0x01, 0x03, 0x20, 0x02, 0x0F, 0xA2, 0x02, 0x00, 0x00, 0x01],
            guid: Some(guid),
            sdr: Vec::new(),
            sdr_added: 0,
            sdr_erased: 0,
            sdr_loops: false,
            cancel_reservation_after: None,
            reservations: 0,
            largest_sdr_read: 0,
            sdr_reservation: 0,
            sdr_calls: 0,
            readings: HashMap::new(),
            factors: HashMap::new(),
            sel: Vec::new(),
            sel_added: 0,
            sel_erased: 0,
            sel_reservation: 0,
            erase_requests: 0,
            erase_pending: None,
            fru: Vec::new(),
            fru_by_words: false,
            largest_fru_read: 0,
            channels: vec![(1, 0x04)],
            lan,
            lan_sets: Vec::new(),
            refuse_lan_parameter: None,
            no_commit_write: false,
            identifies: Vec::new(),
            identify_refused: false,
        }
    }

    fn app(&mut self, cmd: u8, data: &[u8]) -> Vec<u8> {
        match cmd {
            0x01 => ok(&self.device_id),
            0x37 => match self.guid {
                Some(guid) => ok(&guid),
                None => vec![0xC1],
            },
            0x42 => {
                let channel = data.first().copied().unwrap_or(0) & 0x0F;
                match self.channels.iter().find(|(number, _)| *number == channel) {
                    Some(&(number, medium)) => ok(&[number, medium, 0x01, 0, 0, 0, 0, 0, 0]),
                    None => vec![0xCC],
                }
            }
            _ => vec![0xC1],
        }
    }

    fn sensor(&mut self, cmd: u8, data: &[u8]) -> Vec<u8> {
        match (cmd, data) {
            (0x2D, [number]) => match self.readings.get(number) {
                Some(reply) => ok(reply),
                None => vec![0xCB],
            },
            (0x23, [number, raw]) => match self.factors.get(&(*number, *raw)) {
                Some(factors) => ok(&[[*raw].as_slice(), factors].concat()),
                None => vec![0xCD],
            },
            _ => vec![0xC1],
        }
    }

    fn storage(&mut self, cmd: u8, data: &[u8]) -> Vec<u8> {
        match cmd {
            0x20 => self.sdr_info(),
            0x22 => {
                self.reservations += 1;
                self.sdr_reservation = self.sdr_reservation.wrapping_add(1).max(1);
                ok(&self.sdr_reservation.to_le_bytes())
            }
            0x23 => self.get_sdr(data),
            0x40 => self.sel_info(),
            0x42 => {
                self.sel_reservation = self.sel_reservation.wrapping_add(1).max(1);
                ok(&self.sel_reservation.to_le_bytes())
            }
            0x43 => self.get_sel_entry(data),
            0x47 => self.clear_sel(data),
            0x10 => self.fru_info(),
            0x11 => self.read_fru(data),
            _ => vec![0xC1],
        }
    }

    fn get_sdr(&mut self, data: &[u8]) -> Vec<u8> {
        let &[res_low, res_high, id_low, id_high, offset, count] = data else { return vec![0xC7] };
        self.sdr_calls += 1;
        if self.cancel_reservation_after == Some(self.sdr_calls) {
            self.sdr_reservation = self.sdr_reservation.wrapping_add(1);
        }
        let (offset, count) = (usize::from(offset), usize::from(count));
        self.largest_sdr_read = self.largest_sdr_read.max(count);
        if count > PART_MAX {
            return vec![0xCA];
        }
        if offset != 0 && u16::from_le_bytes([res_low, res_high]) != self.sdr_reservation {
            return vec![0xC5];
        }
        let id = u16::from_le_bytes([id_low, id_high]);
        let index = if id == 0 {
            Some(0)
        } else {
            self.sdr.iter().position(|record| u16::from_le_bytes([record[0], record[1]]) == id)
        };
        let Some(index) = index.filter(|index| *index < self.sdr.len()) else { return vec![0xCB] };
        let record = &self.sdr[index];
        let Some(bytes) = record.get(offset..offset + count) else { return vec![0xC9] };
        let next = match self.sdr.get(index + 1) {
            Some(next) => u16::from_le_bytes([next[0], next[1]]),
            None if self.sdr_loops => u16::from_le_bytes([self.sdr[0][0], self.sdr[0][1]]),
            None => 0xFFFF,
        };
        ok(&[&next.to_le_bytes()[..], bytes].concat())
    }

    fn sdr_info(&self) -> Vec<u8> {
        let count = u16::try_from(self.sdr.len()).unwrap();
        let mut reply = vec![0x51];
        reply.extend_from_slice(&count.to_le_bytes());
        reply.extend_from_slice(&[0xFF, 0xFF]);
        reply.extend_from_slice(&self.sdr_added.to_le_bytes());
        reply.extend_from_slice(&self.sdr_erased.to_le_bytes());
        reply.push(0x02);
        ok(&reply)
    }

    fn sel_info(&self) -> Vec<u8> {
        let count = u16::try_from(self.sel.len()).unwrap();
        let free = SEL_SPACE - count * 16;
        let mut reply = vec![0x51];
        reply.extend_from_slice(&count.to_le_bytes());
        reply.extend_from_slice(&free.to_le_bytes());
        reply.extend_from_slice(&self.sel_added.to_le_bytes());
        reply.extend_from_slice(&self.sel_erased.to_le_bytes());
        // Get SEL Allocation Info is not supported; Reserve SEL is.
        reply.push(0x02);
        ok(&reply)
    }

    fn get_sel_entry(&self, data: &[u8]) -> Vec<u8> {
        let &[_, _, id_low, id_high, offset, count] = data else { return vec![0xC7] };
        if offset != 0 || count != 0xFF {
            return vec![0xCA];
        }
        let id = u16::from_le_bytes([id_low, id_high]);
        let index = match id {
            0 => Some(0),
            0xFFFF => self.sel.len().checked_sub(1),
            _ => self.sel.iter().position(|entry| u16::from_le_bytes([entry[0], entry[1]]) == id),
        };
        let Some(index) = index.filter(|index| *index < self.sel.len()) else { return vec![0xCB] };
        let next = match self.sel.get(index + 1) {
            Some(next) => u16::from_le_bytes([next[0], next[1]]),
            None => 0xFFFF,
        };
        ok(&[&next.to_le_bytes()[..], &self.sel[index][..]].concat())
    }

    fn clear_sel(&mut self, data: &[u8]) -> Vec<u8> {
        let &[res_low, res_high, c, l, r, action] = data else { return vec![0xC7] };
        let reservation = u16::from_le_bytes([res_low, res_high]);
        if reservation != self.sel_reservation || reservation == 0 {
            return vec![0xC5];
        }
        if [c, l, r] != *b"CLR" {
            return vec![0xCC];
        }
        match action {
            0xAA => {
                self.erase_requests += 1;
                self.erase_pending = Some(1);
                ok(&[0x00])
            }
            0x00 => match self.erase_pending {
                Some(0) => {
                    self.sel.clear();
                    self.sel_erased += 1;
                    self.erase_pending = None;
                    ok(&[0x01])
                }
                Some(left) => {
                    self.erase_pending = Some(left - 1);
                    ok(&[0x00])
                }
                None => ok(&[0x01]),
            },
            _ => vec![0xCC],
        }
    }

    fn fru_info(&self) -> Vec<u8> {
        if self.fru.is_empty() {
            return vec![0xCB];
        }
        let size = u16::try_from(self.fru.len()).unwrap().to_le_bytes();
        ok(&[size[0], size[1], u8::from(self.fru_by_words)])
    }

    fn read_fru(&mut self, data: &[u8]) -> Vec<u8> {
        let &[0, low, high, count] = data else { return vec![0xCB] };
        let unit = if self.fru_by_words { 2 } else { 1 };
        let offset = usize::from(u16::from_le_bytes([low, high])) * unit;
        let length = usize::from(count) * unit;
        self.largest_fru_read = self.largest_fru_read.max(length);
        if length > PART_MAX {
            return vec![0xCA];
        }
        match self.fru.get(offset..offset + length) {
            Some(bytes) => ok(&[&[count][..], bytes].concat()),
            None => vec![0xC9],
        }
    }

    fn transport(&mut self, cmd: u8, data: &[u8]) -> Vec<u8> {
        match (cmd, data) {
            (0x02, &[_, number, _, _]) => match self.lan.get(&number) {
                Some(bytes) => ok(&[&[0x11][..], bytes].concat()),
                None => vec![0x80],
            },
            (0x01, [_, number, value @ ..]) => {
                if self.refuse_lan_parameter == Some(*number) {
                    return vec![0xCC];
                }
                if *number == 0 && value == [2] && self.no_commit_write {
                    return vec![0x80];
                }
                self.lan_sets.push((*number, value.to_vec()));
                self.lan.insert(*number, value.to_vec());
                ok(&[])
            }
            _ => vec![0xC1],
        }
    }
}

/// A response with completion code 0 and `data`.
fn ok(data: &[u8]) -> Vec<u8> {
    [&[0u8][..], data].concat()
}

impl Transport for Sim {
    fn request(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> io::Result<Vec<u8>> {
        Ok(match netfn {
            NETFN_APP => self.app(cmd, data),
            NETFN_SENSOR => self.sensor(cmd, data),
            NETFN_STORAGE => self.storage(cmd, data),
            NETFN_TRANSPORT => self.transport(cmd, data),
            NETFN_CHASSIS if cmd == 0x04 && !self.identify_refused => {
                self.identifies.push(data.to_vec());
                ok(&[])
            }
            _ => vec![0xC1],
        })
    }
}

/// A full sensor record (table 43-1) of a temperature sensor with the
/// BMC as owner, `number` and `name`, on the air inlet. Its constants are
/// M = 5, B = -20, B exponent 1 and result exponent -1, so that a raw
/// reading `x` is `(5 x - 200) / 10` degrees; thresholds 40, 110 and 120
/// raw (0, 35 and 40 degrees) are readable for the lower critical, upper
/// non-critical and upper critical.
pub(crate) fn full_record(id: u16, number: u8, name: &str) -> Vec<u8> {
    let mut bytes = vec![0u8; 48];
    bytes[..2].copy_from_slice(&id.to_le_bytes());
    bytes[2] = 0x51;
    bytes[3] = 0x01;
    bytes[5] = 0x20;
    bytes[7] = number;
    bytes[8] = 0x37;
    bytes[9] = 1;
    bytes[11] = 0x04;
    bytes[12] = 0x01;
    bytes[13] = 0x01;
    bytes[19] = 0b01_1010;
    bytes[21] = 1;
    bytes[24] = 5;
    // B is -20: 0x3EC in 10 bits, its top two bits above the accuracy.
    bytes[26] = 0xEC;
    bytes[27] = 0xC0;
    bytes[29] = 0xF1;
    bytes[37] = 120;
    bytes[38] = 110;
    bytes[40] = 40;
    bytes[47] = 0xC0 | u8::try_from(name.len()).unwrap();
    bytes.extend_from_slice(name.as_bytes());
    bytes[4] = u8::try_from(bytes.len() - 5).unwrap();
    bytes
}

/// A compact sensor record (table 43-2) of a power supply's
/// sensor-specific states, on the power supply entity, standing for
/// `share` sensors from `number` on.
pub(crate) fn compact_record(id: u16, number: u8, name: &str, share: u8) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    bytes[..2].copy_from_slice(&id.to_le_bytes());
    bytes[2] = 0x51;
    bytes[3] = 0x02;
    bytes[5] = 0x20;
    bytes[7] = number;
    bytes[8] = 0x0A;
    bytes[9] = 1;
    bytes[12] = 0x08;
    bytes[13] = 0x6F;
    bytes[23] = share;
    bytes[31] = 0xC0 | u8::try_from(name.len()).unwrap();
    bytes.extend_from_slice(name.as_bytes());
    bytes[4] = u8::try_from(bytes.len() - 5).unwrap();
    bytes
}

/// A 16-byte system event record (table 32-1) from the BMC (generator
/// 0x20), with `event_type` as the direction bit and the event/reading
/// type code.
pub(crate) fn sel_entry(
    id: u16,
    stamp: u32,
    sensor_type: u8,
    sensor_number: u8,
    event_type: u8,
    data: [u8; 3],
) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[..2].copy_from_slice(&id.to_le_bytes());
    bytes[2] = 0x02;
    bytes[3..7].copy_from_slice(&stamp.to_le_bytes());
    bytes[7] = 0x20;
    bytes[9] = 0x04;
    bytes[10] = sensor_type;
    bytes[11] = sensor_number;
    bytes[12] = event_type;
    bytes[13..].copy_from_slice(&data);
    bytes
}

/// The FRU data of a rack server: a chassis, a board made on 2022-03-01
/// at 10:00 UTC and a product area, each with the encodings of its fields.
/// The 6-bit packing of `text`, first character in the low bits.
pub(crate) fn packed(text: &str) -> Vec<u8> {
    let mut bits = 0u32;
    let mut count = 0;
    let mut bytes = Vec::new();
    for c in text.bytes() {
        bits |= u32::from(c - 0x20) << count;
        count += 6;
        while count >= 8 {
            bytes.push(u8::try_from(bits & 0xFF).unwrap());
            bits >>= 8;
            count -= 8;
        }
    }
    if count > 0 {
        bytes.push(u8::try_from(bits).unwrap());
    }
    bytes
}

pub(crate) fn latin1_field(text: &str) -> Vec<u8> {
    let mut field = vec![0xC0 | u8::try_from(text.len()).unwrap()];
    field.extend_from_slice(text.as_bytes());
    field
}

/// A field in 6-bit ASCII; `text` is a multiple of 4 characters.
pub(crate) fn six_bit(text: &str) -> Vec<u8> {
    let bytes = packed(text);
    let mut field = vec![0x80 | u8::try_from(bytes.len()).unwrap()];
    field.extend(bytes);
    field
}

pub(crate) fn bcd_field(digits: &[u8]) -> Vec<u8> {
    let mut field = vec![0x40 | u8::try_from(digits.len()).unwrap()];
    field.extend_from_slice(digits);
    field
}

pub(crate) fn binary_field(bytes: &[u8]) -> Vec<u8> {
    let mut field = vec![u8::try_from(bytes.len()).unwrap()];
    field.extend_from_slice(bytes);
    field
}

/// An area whose length is a multiple of 8 and whose bytes add to 0.
pub(crate) fn fru_area(head: &[u8], fields: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = head.to_vec();
    for field in fields {
        bytes.extend_from_slice(field);
    }
    bytes.push(0xC1);
    while !(bytes.len() + 1).is_multiple_of(8) {
        bytes.push(0);
    }
    bytes.push(0);
    bytes[1] = u8::try_from(bytes.len() / 8).unwrap();
    let sum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    let last = bytes.len() - 1;
    bytes[last] = sum.wrapping_neg();
    bytes
}

pub(crate) fn fru_image() -> Vec<u8> {
    let chassis = fru_area(
        &[1, 0, 0x17],
        &[latin1_field("CH-PART-1"), latin1_field("CH-SERIAL"), latin1_field("rack 12")],
    );
    // 2022-03-01 10:00 UTC.
    let minutes = (1_646_128_800 - 820_454_400) / 60;
    let date = u32::try_from(minutes).unwrap().to_le_bytes();
    let board = fru_area(
        &[1, 0, 25, date[0], date[1], date[2]],
        &[
            latin1_field("Contoso"),
            six_bit("MB 4000 "),
            bcd_field(&[0x12, 0x3B, 0x45]),
            binary_field(&[0xDE, 0xAD]),
            latin1_field(""),
        ],
    );
    let product = fru_area(
        &[1, 0, 25],
        &[
            latin1_field("Contoso Systems"),
            latin1_field("Rack Server 1U"),
            latin1_field("RS-1U"),
            latin1_field("v2"),
            latin1_field("SN-0001"),
            latin1_field("ASSET-7"),
            latin1_field("file.fru"),
            latin1_field("extra one"),
        ],
    );
    let board_at = u8::try_from(1 + chassis.len() / 8).unwrap();
    let product_at = u8::try_from(1 + (chassis.len() + board.len()) / 8).unwrap();
    let mut image = vec![1, 0, 1, board_at, product_at, 0, 0, 0];
    image[7] = image[..7].iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)).wrapping_neg();
    image.extend(chassis);
    image.extend(board);
    image.extend(product);
    image
}
