// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The FRU inventory (Platform Management FRU Information Storage
//! Definition 1.0): Get FRU Inventory Area Info and Read FRU Data (IPMI
//! 2.0 section 34), the common header (section 8 of the FRU definition)
//! and the chassis (10), board (11) and product (12) info areas.
//! Internal use and multi-record areas are not read.

use super::text;
use super::{Error, NETFN_STORAGE, Result, Transport, command, le16};

/// Get FRU Inventory Area Info.
const GET_FRU_INFO: u8 = 0x10;
/// Read FRU Data.
const READ_FRU_DATA: u8 = 0x11;

/// The bytes asked for in one Read FRU Data.
const PART: usize = 16;

/// The common header is 8 bytes (section 8).
const COMMON_HEADER: usize = 8;

/// The board's manufacturing date counts minutes from 1996-01-01 00:00 UTC
/// (section 11), which is this many seconds after 1970.
const EPOCH_1996: i64 = 820_454_400;

/// The inventory of one FRU device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fru {
    pub chassis: Option<Chassis>,
    pub board: Option<Board>,
    pub product: Option<Product>,
    /// Things wrong with the inventory that were passed over: an area
    /// whose checksum is wrong.
    pub notes: Vec<String>,
}

/// The chassis info area (section 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chassis {
    pub kind: String,
    pub part_number: String,
    pub serial_number: String,
    pub custom: Vec<String>,
}

/// The board info area (section 11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Board {
    /// Seconds since 1970, or `None` when the date is unspecified (0).
    pub manufactured: Option<i64>,
    pub manufacturer: String,
    pub product_name: String,
    pub serial_number: String,
    pub part_number: String,
    pub file_id: String,
    pub custom: Vec<String>,
}

/// The product info area (section 12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Product {
    pub manufacturer: String,
    pub name: String,
    pub part_number: String,
    pub version: String,
    pub serial_number: String,
    pub asset_tag: String,
    pub file_id: String,
    pub custom: Vec<String>,
}

/// Reads the inventory of FRU device `device` (0 is the baseboard's).
///
/// # Errors
///
/// If the BMC has no such FRU, does not answer, or the data is not a FRU
/// at all: a wrong common header.
pub(crate) fn read_fru(link: &mut dyn Transport, device: u8) -> Result<Fru> {
    let reply = command(link, NETFN_STORAGE, GET_FRU_INFO, &[device])?;
    let size = usize::from(le16(&reply, 0)?);
    // Bit 0 of the third byte: the device is read by words, and offsets
    // and counts are in words.
    let by_words = reply.get(2).is_some_and(|access| access & 1 == 1);
    let mut reader = Reader { link, device, size, by_words };
    let header = reader.read(0, COMMON_HEADER)?;
    check_header(&header)?;
    let mut fru = Fru::default();
    // Offsets 1 to 5 of the header, in units of 8 bytes: internal use,
    // chassis, board, product, multi-record.
    for (name, at) in [("chassis", 2), ("board", 3), ("product", 4)] {
        let offset = usize::from(header[at]) * 8;
        if offset == 0 {
            continue;
        }
        let area = reader.read_area(offset)?;
        if area.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
            fru.notes.push(format!("the {name} area's checksum is wrong; it is not shown"));
            continue;
        }
        match at {
            2 => fru.chassis = chassis(&area),
            3 => fru.board = board(&area),
            _ => fru.product = product(&area),
        }
    }
    Ok(fru)
}

/// The FRU device's data, read in parts.
struct Reader<'a> {
    link: &'a mut dyn Transport,
    device: u8,
    size: usize,
    by_words: bool,
}

impl Reader<'_> {
    /// `length` bytes from `offset` of the device's data.
    fn read(&mut self, offset: usize, length: usize) -> Result<Vec<u8>> {
        let end = offset.checked_add(length).filter(|end| *end <= self.size);
        let Some(end) = end else {
            return Err(Error::Malformed("an area runs past the end of the FRU"));
        };
        let unit = if self.by_words { 2 } else { 1 };
        // A device read by words is read from an even offset to an even
        // end, and the bytes asked for are cut from that.
        let start = offset / unit * unit;
        let stop = end.div_ceil(unit) * unit;
        let mut bytes = Vec::with_capacity(stop - start);
        while start + bytes.len() < stop {
            let at = start + bytes.len();
            let count = PART.min(stop - at);
            let [low, high] = u16::try_from(at / unit)
                .map_err(|_| Error::Malformed("the FRU is too long"))?
                .to_le_bytes();
            let count_in_units = u8::try_from(count / unit).unwrap_or(u8::MAX);
            let reply = command(
                self.link,
                NETFN_STORAGE,
                READ_FRU_DATA,
                &[self.device, low, high, count_in_units],
            )?;
            let Some((&got, data)) = reply.split_first() else {
                return Err(Error::Malformed("a FRU read is cut short"));
            };
            if usize::from(got) * unit != data.len() || data.is_empty() {
                return Err(Error::Malformed("a FRU read gave the wrong count"));
            }
            bytes.extend_from_slice(data);
        }
        Ok(bytes[offset - start..offset - start + length].to_vec())
    }

    /// The area at `offset`: its second byte is its length in 8-byte units.
    fn read_area(&mut self, offset: usize) -> Result<Vec<u8>> {
        let head = self.read(offset, 2)?;
        let length = usize::from(head[1]) * 8;
        if length < 8 {
            return Err(Error::Malformed("an area's length is 0"));
        }
        self.read(offset, length)
    }
}

/// The common header's format version (low nibble 1) and checksum.
fn check_header(header: &[u8]) -> Result<()> {
    let sum = header.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    if header.len() < COMMON_HEADER || header[0] & 0x0F != 1 || sum != 0 {
        return Err(Error::Malformed("the FRU's common header is not valid"));
    }
    Ok(())
}

/// The strings from `start` on: the `fixed` ones every area of its kind
/// has, then the custom ones up to the 0xC1 that ends them. A fixed field
/// the area ends before is empty.
fn fields(area: &[u8], start: usize, fixed: usize) -> (Vec<String>, Vec<String>) {
    let mut strings = Vec::new();
    let mut at = start;
    while let Some((string, next)) = text::field(area, at) {
        strings.push(string);
        at = next;
    }
    strings.resize(strings.len().max(fixed), String::new());
    let custom = strings.split_off(fixed);
    (strings, custom)
}

fn chassis(area: &[u8]) -> Option<Chassis> {
    let kind = *area.get(2)?;
    let (fixed, custom) = fields(area, 3, 2);
    let [part_number, serial_number] = <[String; 2]>::try_from(fixed).ok()?;
    Some(Chassis { kind: chassis_type(kind), part_number, serial_number, custom })
}

fn board(area: &[u8]) -> Option<Board> {
    let minutes = u32::from_le_bytes([*area.get(3)?, *area.get(4)?, *area.get(5)?, 0]);
    let (fixed, custom) = fields(area, 6, 5);
    let [manufacturer, product_name, serial_number, part_number, file_id] =
        <[String; 5]>::try_from(fixed).ok()?;
    Some(Board {
        manufactured: (minutes != 0).then(|| EPOCH_1996 + i64::from(minutes) * 60),
        manufacturer,
        product_name,
        serial_number,
        part_number,
        file_id,
        custom,
    })
}

fn product(area: &[u8]) -> Option<Product> {
    area.get(2)?;
    let (fixed, custom) = fields(area, 3, 7);
    let [manufacturer, name, part_number, version, serial_number, asset_tag, file_id] =
        <[String; 7]>::try_from(fixed).ok()?;
    Some(Product {
        manufacturer,
        name,
        part_number,
        version,
        serial_number,
        asset_tag,
        file_id,
        custom,
    })
}

/// The chassis type of table 10-? of the FRU definition (the same codes
/// as SMBIOS's).
fn chassis_type(code: u8) -> String {
    match code {
        0x01 => "other",
        0x02 => "unknown",
        0x03 => "desktop",
        0x04 => "low-profile desktop",
        0x05 => "pizza box",
        0x06 => "mini tower",
        0x07 => "tower",
        0x08 => "portable",
        0x09 => "laptop",
        0x0A => "notebook",
        0x0B => "hand held",
        0x0C => "docking station",
        0x0D => "all in one",
        0x0E => "sub notebook",
        0x0F => "space-saving",
        0x10 => "lunch box",
        0x11 => "main server chassis",
        0x12 => "expansion chassis",
        0x13 => "sub chassis",
        0x14 => "bus expansion chassis",
        0x15 => "peripheral chassis",
        0x16 => "RAID chassis",
        0x17 => "rack-mount chassis",
        0x18 => "sealed-case PC",
        0x19 => "multi-system chassis",
        other => return format!("type 0x{other:02X}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::sim::{Sim, fru_area, fru_image, latin1_field};
    use super::*;

    #[test]
    fn the_areas_are_read_in_parts_and_decoded_with_each_encoding() {
        let mut bmc = Sim::new();
        bmc.fru = fru_image();
        let fru = read_fru(&mut bmc, 0).unwrap();
        assert!(fru.notes.is_empty(), "{:?}", fru.notes);
        let chassis = fru.chassis.unwrap();
        assert_eq!(chassis.kind, "rack-mount chassis");
        assert_eq!(chassis.part_number, "CH-PART-1");
        assert_eq!(chassis.serial_number, "CH-SERIAL");
        assert_eq!(chassis.custom, ["rack 12"]);
        let board = fru.board.unwrap();
        assert_eq!(board.manufactured, Some(1_646_128_800));
        assert_eq!(board.manufacturer, "Contoso");
        assert_eq!(board.product_name, "MB 4000");
        assert_eq!(board.serial_number, "123-45");
        assert_eq!(board.part_number, "DEAD");
        assert_eq!(board.file_id, "");
        let product = fru.product.unwrap();
        assert_eq!(product.manufacturer, "Contoso Systems");
        assert_eq!(product.name, "Rack Server 1U");
        assert_eq!(product.part_number, "RS-1U");
        assert_eq!(product.version, "v2");
        assert_eq!(product.serial_number, "SN-0001");
        assert_eq!(product.asset_tag, "ASSET-7");
        assert_eq!(product.file_id, "file.fru");
        assert_eq!(product.custom, ["extra one"]);
        assert!(bmc.largest_fru_read <= PART);
    }

    #[test]
    fn a_device_read_by_words_gives_the_same_inventory() {
        let mut bmc = Sim::new();
        bmc.fru = fru_image();
        bmc.fru_by_words = true;
        let words = read_fru(&mut bmc, 0).unwrap();
        bmc.fru_by_words = false;
        assert_eq!(words, read_fru(&mut bmc, 0).unwrap());
        assert!(words.board.is_some());
    }

    #[test]
    fn an_unspecified_date_is_none() {
        let fields: Vec<Vec<u8>> = ["A", "B", "C", "D", "E"].map(latin1_field).to_vec();
        let board = fru_area(&[1, 0, 0, 0, 0, 0], &fields);
        assert_eq!(super::board(&board).unwrap().manufactured, None);
    }

    #[test]
    fn an_area_with_a_wrong_checksum_is_passed_over_with_a_note() {
        let mut bmc = Sim::new();
        let mut broken = fru_image();
        broken[8 + 5] ^= 0x01;
        bmc.fru = broken;
        let fru = read_fru(&mut bmc, 0).unwrap();
        assert!(fru.chassis.is_none());
        assert!(fru.board.is_some());
        assert_eq!(fru.notes.len(), 1);
        assert!(fru.notes[0].contains("chassis"), "{:?}", fru.notes);
    }

    #[test]
    fn data_that_is_no_fru_is_refused() {
        let mut bmc = Sim::new();
        bmc.fru = vec![0xFF; 64];
        assert!(matches!(read_fru(&mut bmc, 0), Err(Error::Malformed(_))));
        // A header with no areas is a FRU with nothing in it.
        bmc.fru = vec![0x01, 0, 0, 0, 0, 0, 0, 0xFF];
        assert_eq!(read_fru(&mut bmc, 0).unwrap(), Fru::default());
    }

    #[test]
    fn an_area_that_runs_past_the_device_is_refused() {
        let mut bmc = Sim::new();
        let mut short = fru_image();
        short.truncate(40);
        bmc.fru = short;
        assert!(matches!(read_fru(&mut bmc, 0), Err(Error::Malformed(_))));
    }

    #[test]
    fn a_missing_device_is_the_bmcs_refusal() {
        let mut bmc = Sim::new();
        bmc.fru = Vec::new();
        assert!(matches!(read_fru(&mut bmc, 0), Err(Error::Completion(0xCB))));
    }
}
