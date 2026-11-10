// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a floppy drive is, and the disks it reads: their geometry, the
//! data rate their tracks were written at, and the gaps between their
//! sectors; and how a track of one is formatted ([`Format::track_ids`]).

use core::fmt;

/// The bytes of a sector, the only size a PC floppy disk uses.
pub const SECTOR: usize = 512;
/// A sector's size as a command gives it: 128 << 2.
pub(crate) const SIZE_CODE: u8 = 2;
/// The byte a format fills each sector with: what DOS's `format` writes,
/// and what a newly formatted disk reads as.
pub const FILLER: u8 = 0xf6;
/// The bytes of a sector's ID in the list FORMAT TRACK takes: its
/// cylinder, head, number and size code.
pub const ID_BYTES: usize = 4;

/// The rate at which the controller reads and writes a track. The disk's
/// tracks were written at one of them, and a read at another finds no
/// sector IDs at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    /// 250 kbit/s: double-density disks (720 KB).
    K250,
    /// 300 kbit/s: double-density disks in a 5.25-inch high-density
    /// drive.
    K300,
    /// 500 kbit/s: high-density disks (1.44 MB).
    K500,
    /// 1 Mbit/s: extra-density disks (2.88 MB).
    M1,
}

impl Rate {
    /// The value of the configuration control register that selects it.
    pub(crate) fn bits(self) -> u8 {
        match self {
            Rate::K500 => 0,
            Rate::K300 => 1,
            Rate::K250 => 2,
            Rate::M1 => 3,
        }
    }

    /// Its kilobits a second.
    pub fn kbits(self) -> u32 {
        match self {
            Rate::K250 => 250,
            Rate::K300 => 300,
            Rate::K500 => 500,
            Rate::M1 => 1000,
        }
    }
}

/// A kind of disk: its geometry, data rate and gaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// What it is called: `1.44 MB`.
    pub name: &'static str,
    /// Cylinders: tracks on each side.
    pub cylinders: u8,
    /// Heads: sides.
    pub heads: u8,
    /// Sectors on each track, numbered from 1.
    pub sectors: u8,
    /// The data rate its tracks were written at.
    pub rate: Rate,
    /// The gap a read or write leaves before each sector's data.
    pub(crate) gap: u8,
    /// The gap a format leaves between sectors, longer than `gap`: the
    /// 82077AA datasheet's figure for the format, as DOS and Linux use.
    pub(crate) format_gap: u8,
    /// SPECIFY's first byte for its rate: the step rate in the high
    /// nibble and the head unload time in the low one.
    pub(crate) step: u8,
}

/// A 3.5-inch high-density disk: 80 cylinders of two heads, 18 sectors a
/// track, 1,474,560 bytes.
pub const HD_1440: Format = Format {
    name: "1.44 MB",
    cylinders: 80,
    heads: 2,
    sectors: 18,
    rate: Rate::K500,
    gap: 0x1b,
    format_gap: 0x6c,
    step: 0xcf,
};

/// A 3.5-inch double-density disk: 80 cylinders of two heads, 9 sectors a
/// track, 737,280 bytes.
pub const DD_720: Format = Format {
    name: "720 KB",
    cylinders: 80,
    heads: 2,
    sectors: 9,
    rate: Rate::K250,
    gap: 0x2a,
    format_gap: 0x50,
    step: 0xdf,
};

/// Where a sector is on a disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// Its cylinder, from 0.
    pub cylinder: u8,
    /// Its head, 0 or 1.
    pub head: u8,
    /// Its number on the track, from 1.
    pub sector: u8,
}

impl Format {
    /// Its sectors.
    pub fn total(&self) -> u32 {
        u32::from(self.cylinders) * u32::from(self.heads) * u32::from(self.sectors)
    }

    /// Its bytes.
    pub fn bytes(&self) -> u64 {
        u64::from(self.total()) * SECTOR as u64
    }

    /// The sectors of a cylinder.
    pub fn cylinder_sectors(&self) -> u32 {
        u32::from(self.heads) * u32::from(self.sectors)
    }

    /// Where the sector `number` (from 0, in the order DOS and Linux
    /// number them: a track, then the other head's, then the next
    /// cylinder) is, or `None` past the end.
    pub fn place(&self, number: u32) -> Option<Place> {
        if number >= self.total() {
            return None;
        }
        let track = number / u32::from(self.sectors);
        Some(Place {
            cylinder: (track / u32::from(self.heads)) as u8,
            head: (track % u32::from(self.heads)) as u8,
            sector: (number % u32::from(self.sectors)) as u8 + 1,
        })
    }

    /// How many sectors from the sector `number` to the end of its
    /// cylinder: as many as one command moves.
    pub fn left_in_cylinder(&self, number: u32) -> u32 {
        let cylinder = self.cylinder_sectors();
        cylinder - number % cylinder
    }

    /// The bytes of the sector IDs that FORMAT TRACK writes on track
    /// `cylinder`, `head`: [`ID_BYTES`] for each sector, numbered from 1 in
    /// order around the track, as DOS lays a track out. Returns how many
    /// bytes it wrote into `out`; 0 if the track is not on the disk or
    /// `out` is too small.
    pub fn track_ids(&self, cylinder: u8, head: u8, out: &mut [u8]) -> usize {
        let len = usize::from(self.sectors) * ID_BYTES;
        if cylinder >= self.cylinders || head >= self.heads || out.len() < len {
            return 0;
        }
        let (ids, _) = out.as_chunks_mut::<ID_BYTES>();
        for (number, id) in (1..=self.sectors).zip(ids) {
            *id = [cylinder, head, number, SIZE_CODE];
        }
        len
    }
}

/// A kind of floppy drive, as the firmware's `_FDI` or the CMOS names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    /// 5.25-inch, 360 KB.
    Inch525Dd,
    /// 5.25-inch, 1.2 MB.
    Inch525Hd,
    /// 3.5-inch, 720 KB.
    Inch35Dd,
    /// 3.5-inch, 1.44 MB: the PC's drive since 1987.
    Inch35Hd,
    /// 3.5-inch, 2.88 MB.
    Inch35Ed,
}

impl DriveKind {
    /// The kind of drive number `number` means in `_FDI`'s second element
    /// and in the CMOS's register 0x10: 1 to 5.
    pub fn from_number(number: u64) -> Option<DriveKind> {
        Some(match number {
            1 => DriveKind::Inch525Dd,
            2 => DriveKind::Inch525Hd,
            3 => DriveKind::Inch35Dd,
            4 => DriveKind::Inch35Hd,
            5 => DriveKind::Inch35Ed,
            _ => return None,
        })
    }

    /// The disks it reads that the driver knows, in the order they are
    /// tried: the densest first. A 2.88 MB drive reads 1.44 MB and 720 KB
    /// disks as a 1.44 MB one does; its own disks, written with
    /// perpendicular recording, are not read yet, nor are 5.25-inch
    /// disks.
    pub fn formats(self) -> &'static [Format] {
        match self {
            DriveKind::Inch35Hd | DriveKind::Inch35Ed => &[HD_1440, DD_720],
            DriveKind::Inch35Dd => &[DD_720],
            DriveKind::Inch525Dd | DriveKind::Inch525Hd => &[],
        }
    }
}

impl fmt::Display for DriveKind {
    /// As the log says what a drive is: `a 3.5-inch 1.44 MB floppy drive`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DriveKind::Inch525Dd => "a 5.25-inch 360 KB floppy drive",
            DriveKind::Inch525Hd => "a 5.25-inch 1.2 MB floppy drive",
            DriveKind::Inch35Dd => "a 3.5-inch 720 KB floppy drive",
            DriveKind::Inch35Hd => "a 3.5-inch 1.44 MB floppy drive",
            DriveKind::Inch35Ed => "a 3.5-inch 2.88 MB floppy drive",
        })
    }
}

/// The geometry a FAT boot sector, `sector`, says its disk has: its
/// sectors a track and heads, from its BIOS parameter block. `None` when
/// the sector holds none, as a disk with no FAT filesystem, or says
/// nothing of a floppy disk's tracks: more than two heads is a hard
/// disk's geometry, such as the 63 sectors and 255 heads that many tools
/// write on every volume they make, the disk's whole size included.
pub fn boot_geometry(sector: &[u8; SECTOR]) -> Option<(u16, u16)> {
    let word = |at: usize| u16::from_le_bytes([sector[at], sector[at + 1]]);
    // A boot sector starts with a jump over the parameter block.
    let jumps = matches!(sector[0], 0xeb | 0xe9);
    let (bytes, sectors, heads) = (word(11), word(24), word(26));
    let valid =
        jumps && bytes == SECTOR as u16 && (1..=63).contains(&sectors) && (1..=2).contains(&heads);
    valid.then_some((sectors, heads))
}

/// What an ACPI floppy disk controller's `_CRS` gives: its ports, its
/// interrupt and its DMA channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resources {
    /// The first of its eight ports: 0x3f0 for the first controller.
    pub base: u16,
    /// Its ISA interrupt, if it names one.
    pub irq: Option<u8>,
    /// Its ISA DMA channel, if it names one.
    pub dma: Option<u8>,
}

/// The resources in a `_CRS` resource template (ACPI 6.5, 6.4): the base
/// of its first I/O descriptor, rounded down to eight ports (firmware
/// often leaves out the first two, which only PS/2 machines have), and
/// its first IRQ and DMA descriptors. `None` with no I/O descriptor.
pub fn resources(template: &[u8]) -> Option<Resources> {
    let mut found = Resources { base: 0, irq: None, dma: None };
    let mut base = None;
    let mut at = 0;
    while at < template.len() {
        let tag = template[at];
        if tag & 0x80 != 0 {
            // A large descriptor: a 16-bit length follows its tag.
            let len = u16::from_le_bytes([*template.get(at + 1)?, *template.get(at + 2)?]);
            at += 3 + usize::from(len);
            continue;
        }
        let (kind, len) = (tag >> 3, usize::from(tag & 7));
        let body = template.get(at + 1..at + 1 + len)?;
        match kind {
            // IRQ: a mask of the interrupts it may use, the lowest first.
            0x04 if len >= 2 && found.irq.is_none() => {
                let mask = u16::from_le_bytes([body[0], body[1]]);
                found.irq = (mask != 0).then(|| mask.trailing_zeros() as u8);
            }
            // DMA: a mask of the channels.
            0x05 if len >= 1 && found.dma.is_none() => {
                found.dma = (body[0] != 0).then(|| body[0].trailing_zeros() as u8);
            }
            // I/O port: information, minimum base, maximum base,
            // alignment, length.
            0x08 if len >= 7 && base.is_none() => {
                base = Some(u16::from_le_bytes([body[1], body[2]]));
            }
            // Fixed I/O port: base, length.
            0x09 if len >= 3 && base.is_none() => {
                base = Some(u16::from_le_bytes([body[0], body[1]]) & 0x3ff);
            }
            // The end tag.
            0x0f => break,
            _ => {}
        }
        at += 1 + len;
    }
    found.base = base? & !7;
    Some(found)
}
