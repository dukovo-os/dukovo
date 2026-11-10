// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an SD memory card says of itself (the *SD Specifications Part 1,
//! Physical Layer Simplified Specification*, 5.2 and 5.3): its CID, which
//! names it, and its CSD, which gives its capacity, as the controller
//! hands them over in a 136-bit response with the CRC left out; and the
//! speed it was set to run at ([`Speed`], 4.3.10 and 4.3.11).

/// A 136-bit response (R2) as the controller holds it: bits 8 to 127 of
/// the register, the first word the lowest.
pub type Long = [u32; 4];

/// Bits `high` down to `low` of the register `long` holds (numbered as
/// the specification numbers them, from 127; at most 32 of them, none
/// below 8, which the controller leaves out).
pub fn bits(long: &Long, high: u32, low: u32) -> u32 {
    debug_assert!(high >= low && high - low < 32 && low >= 8 && high < 128);
    let mut value = 0u64;
    for bit in (low..=high).rev() {
        let at = bit - 8;
        let set = long[(at / 32) as usize] >> (at % 32) & 1;
        value = value << 1 | u64::from(set);
    }
    value as u32
}

/// The card's identification (5.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cid {
    /// MID: its maker, as the SD Association numbers them.
    pub maker: u8,
    /// OID: two characters naming the OEM or the application.
    pub oem: [u8; 2],
    /// PNM: the product's name, five characters.
    pub name: [u8; 5],
    /// PRV: its revision, two BCD digits.
    pub revision: u8,
    /// PSN: its serial number.
    pub serial: u32,
    /// MDT: the year it was made.
    pub year: u16,
    /// MDT: the month it was made, from 1.
    pub month: u8,
}

impl Cid {
    /// The CID in `long`.
    pub fn parse(long: &Long) -> Cid {
        let byte = |high: u32| bits(long, high, high - 7) as u8;
        Cid {
            maker: byte(127),
            oem: [byte(119), byte(111)],
            name: [byte(103), byte(95), byte(87), byte(79), byte(71)],
            revision: byte(63),
            serial: bits(long, 55, 24),
            year: 2000 + bits(long, 19, 12) as u16,
            month: bits(long, 11, 8) as u8,
        }
    }
}

/// How a card's capacity is laid out, from its CSD (5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Csd {
    /// Version 1.0, of a standard capacity card: its 512-byte blocks.
    Standard(u64),
    /// Version 2.0, of an SDHC or SDXC card: its 512-byte blocks.
    High(u64),
    /// Version 3.0, of an SDUC card, which is not driven.
    Ultra,
    /// A structure the specification does not have.
    Unknown,
}

impl Csd {
    /// The CSD in `long`.
    pub fn parse(long: &Long) -> Csd {
        match bits(long, 127, 126) {
            0 => {
                // (C_SIZE + 1) * 2^(C_SIZE_MULT + 2) blocks of
                // 2^READ_BL_LEN bytes.
                let size = u64::from(bits(long, 73, 62)) + 1;
                let multiplier = bits(long, 49, 47) + 2;
                let length = bits(long, 83, 80);
                let bytes = size << multiplier << length;
                Csd::Standard(bytes / 512)
            }
            // (C_SIZE + 1) * 512 KiB.
            1 => Csd::High((u64::from(bits(long, 69, 48)) + 1) * 1024),
            2 => Csd::Ultra,
            _ => Csd::Unknown,
        }
    }
}

/// The bus speed mode a card runs in: a default and a High Speed one at
/// 3.3 V, and UHS-I's at 1.8 V (Physical Layer, 4.3.10 and 4.3.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    /// Up to 25 MHz: every card's.
    Default,
    /// Up to 50 MHz, CMD6's function 1 of group 1, at 3.3 V.
    High,
    /// UHS-I's modes, at 1.8 V: up to 25 MHz, a card's first.
    Sdr12,
    /// Up to 50 MHz, function 1.
    Sdr25,
    /// Up to 100 MHz, function 2, tuned where the controller asks.
    Sdr50,
    /// Up to 208 MHz, function 3, always tuned.
    Sdr104,
    /// Up to 50 MHz on both edges of the clock, function 4.
    Ddr50,
}

impl Speed {
    /// Its name, as the specifications give it.
    pub fn name(self) -> &'static str {
        match self {
            Speed::Default => "default speed",
            Speed::High => "High Speed",
            Speed::Sdr12 => "UHS-I SDR12",
            Speed::Sdr25 => "UHS-I SDR25",
            Speed::Sdr50 => "UHS-I SDR50",
            Speed::Sdr104 => "UHS-I SDR104",
            Speed::Ddr50 => "UHS-I DDR50",
        }
    }

    /// The most its clock runs at, in kHz.
    pub fn khz(self) -> u32 {
        match self {
            Speed::Default | Speed::Sdr12 => 25_000,
            Speed::High | Speed::Sdr25 | Speed::Ddr50 => 50_000,
            Speed::Sdr50 => 100_000,
            Speed::Sdr104 => 208_000,
        }
    }
}

/// A card the driver has set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// The relative address it was given (CMD3).
    pub rca: u16,
    /// It is an SDHC or SDXC card, addressed in blocks; a standard
    /// capacity card is addressed in bytes.
    pub high_capacity: bool,
    /// Its 512-byte blocks.
    pub blocks: u64,
    /// What it says it is.
    pub cid: Cid,
    /// The bus speed mode it runs in.
    pub speed: Speed,
    /// The clock it runs at, in kHz.
    pub khz: u32,
    /// Its bus's width in bits: 1 or 4.
    pub width: u8,
    /// Its write-protect switch is set.
    pub read_only: bool,
}

impl Card {
    /// What kind of card it is: SDSC, SDHC (up to 32 GB) or SDXC.
    pub fn kind(&self) -> &'static str {
        match (self.high_capacity, self.blocks > 32 * 1_000_000_000 / 512) {
            (false, _) => "SD",
            (true, false) => "SDHC",
            (true, true) => "SDXC",
        }
    }

    /// Its product's name, without the spaces and NULs around it.
    pub fn name(&self) -> &str {
        let name = core::str::from_utf8(&self.cid.name).unwrap_or("");
        name.trim_matches(|c: char| c == ' ' || c == '\0')
    }
}
