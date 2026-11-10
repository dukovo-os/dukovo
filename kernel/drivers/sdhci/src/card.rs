// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an SD memory card says of itself (the *SD Specifications Part 1,
//! Physical Layer Simplified Specification*, 5.2 and 5.3): its CID, which
//! names it, and its CSD, which gives its capacity, as the controller
//! hands them over in a 136-bit response with the CRC left out; and the
//! speed it was set to run at ([`Speed`], 4.3.10 and 4.3.11). An MMC's or
//! eMMC's are laid out a little differently (JEDEC's *Embedded
//! MultiMediaCard (e•MMC) Electrical Standard 5.1*, JESD84-B51, 7.2 and
//! 7.3): [`Cid::parse_mmc`] and [`Csd::parse_mmc`].

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
    /// OID: two characters naming the OEM or the application; an MMC's
    /// is one byte, in the second.
    pub oem: [u8; 2],
    /// PNM: the product's name, five characters (an MMC's six).
    pub name: [u8; 6],
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
            name: [byte(103), byte(95), byte(87), byte(79), byte(71), 0],
            revision: byte(63),
            serial: bits(long, 55, 24),
            year: 2000 + bits(long, 19, 12) as u16,
            month: bits(long, 11, 8) as u8,
        }
    }

    /// An MMC's CID in `long` (JESD84-B51, 7.2), its year counted from
    /// 2013 for a device of `rev` 5 or later (EXT_CSD_REV), from 1997
    /// before.
    pub fn parse_mmc(long: &Long, rev: u8) -> Cid {
        let byte = |high: u32| bits(long, high, high - 7) as u8;
        let year = bits(long, 11, 8) as u16;
        Cid {
            maker: byte(127),
            oem: [0, byte(111)],
            name: [byte(103), byte(95), byte(87), byte(79), byte(71), byte(63)],
            revision: byte(55),
            serial: bits(long, 47, 16),
            year: if rev >= 5 && year < 13 { 2013 + year } else { 1997 + year },
            month: bits(long, 15, 12) as u8,
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

    /// An MMC's CSD in `long` (JESD84-B51, 7.3): the version of the
    /// standard it follows (SPEC_VERS, 4 and later having an EXT_CSD),
    /// and its 512-byte blocks as its C_SIZE gives them, which a device
    /// over 2 GB, addressed in sectors, gives in EXT_CSD's SEC_COUNT
    /// instead.
    pub fn parse_mmc(long: &Long) -> (u8, u64) {
        let size = u64::from(bits(long, 73, 62)) + 1;
        let multiplier = bits(long, 49, 47) + 2;
        let length = bits(long, 83, 80);
        (bits(long, 125, 122) as u8, (size << multiplier << length) / 512)
    }
}

/// The bus speed mode a card runs in: a default and a High Speed one at
/// 3.3 V, and UHS-I's at 1.8 V (Physical Layer, 4.3.10 and 4.3.11); an
/// MMC's High Speed and HS200 (JESD84-B51, 5.3).
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
    /// An MMC's High Speed: up to 52 MHz, HS_TIMING 1.
    MmcHigh,
    /// An eMMC's HS200: up to 200 MHz at 1.8 V, HS_TIMING 2, tuned.
    Hs200,
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
            Speed::MmcHigh => "High Speed",
            Speed::Hs200 => "HS200",
        }
    }

    /// The most its clock runs at, in kHz.
    pub fn khz(self) -> u32 {
        match self {
            Speed::Default | Speed::Sdr12 => 25_000,
            Speed::High | Speed::Sdr25 | Speed::Ddr50 => 50_000,
            Speed::Sdr50 => 100_000,
            Speed::Sdr104 => 208_000,
            Speed::MmcHigh => 52_000,
            Speed::Hs200 => 200_000,
        }
    }
}

/// Which kind of memory a card is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// An SD memory card.
    Sd,
    /// A MultiMediaCard in a slot it can be taken out of.
    Mmc,
    /// An eMMC: a MultiMediaCard soldered to the board.
    Emmc,
}

/// A card the driver has set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// SD, MMC or eMMC.
    pub family: Family,
    /// The relative address it was given (CMD3).
    pub rca: u16,
    /// It is an SDHC or SDXC card, or an MMC over 2 GB, addressed in
    /// blocks; a standard capacity card is addressed in bytes.
    pub high_capacity: bool,
    /// Its 512-byte blocks.
    pub blocks: u64,
    /// What it says it is.
    pub cid: Cid,
    /// The bus speed mode it runs in.
    pub speed: Speed,
    /// The clock it runs at, in kHz.
    pub khz: u32,
    /// Its bus's width in bits: 1, 4 or 8 (an MMC's).
    pub width: u8,
    /// Its write-protect switch is set (an SD card's: an MMC has none).
    pub read_only: bool,
}

impl Card {
    /// What kind of card it is: SDSC, SDHC (up to 32 GB) or SDXC, MMC or
    /// eMMC.
    pub fn kind(&self) -> &'static str {
        let big = self.blocks > 32 * 1_000_000_000 / 512;
        match (self.family, self.high_capacity, big) {
            (Family::Mmc, ..) => "MMC",
            (Family::Emmc, ..) => "eMMC",
            (Family::Sd, false, _) => "SD",
            (Family::Sd, true, false) => "SDHC",
            (Family::Sd, true, true) => "SDXC",
        }
    }

    /// Its product's name, without the spaces and NULs around it.
    pub fn name(&self) -> &str {
        let name = core::str::from_utf8(&self.cid.name).unwrap_or("");
        name.trim_matches(|c: char| c == ' ' || c == '\0')
    }
}
