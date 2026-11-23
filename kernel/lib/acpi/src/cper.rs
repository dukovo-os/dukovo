// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hardware error records in the Common Platform Error Record format: the
//! Generic Error Status Block that firmware fills for a GHES source or the
//! BERT (ACPI Specification 6.5, section 18.3.2.7), the Generic Error Data
//! Entries inside it, and the decoders of the sections the kernel reads
//! (UEFI Specification 2.10, appendix N).
//!
//! The bytes come from firmware and are not trusted: every length in the
//! block (data length, raw data offset and length, each entry's error data
//! length) is checked against the bytes given before it is used, and a
//! block that lies about one yields a [`CperError`], never a panic. At most
//! [`MAX_ENTRIES`] entries are walked. A decoded field is `Some` only when
//! the section's validation bits say it is valid.
//!
//! The tables that point at these blocks are in [`hest`] and [`bert`].
//!
//! [`hest`]: crate::hest
//! [`bert`]: crate::bert

use core::fmt;

use crate::{u16_at, u32_at, u64_at};

/// Most entries the walk of one block returns.
pub const MAX_ENTRIES: usize = 64;

/// Bytes in the Generic Error Status Block's header.
pub const BLOCK_HEADER_LEN: usize = 20;

/// The bytes the block whose header starts `header` takes, its entries
/// and its raw data: what to read of a block before
/// [`ErrorStatusBlock::parse`]. `None` when the header is short or its
/// lengths overflow; the length is not bounded here, so the caller bounds
/// it by the space the block may take.
pub fn block_len(header: &[u8]) -> Option<usize> {
    if header.len() < BLOCK_HEADER_LEN {
        return None;
    }
    // Raw data offset at 4, raw data length at 8, data length at 12.
    let raw_offset = u32_at(header, 4)? as usize;
    let raw_length = u32_at(header, 8)? as usize;
    let data_end = BLOCK_HEADER_LEN.checked_add(u32_at(header, 12)? as usize)?;
    if raw_length == 0 {
        return Some(data_end);
    }
    Some(data_end.max(raw_offset.checked_add(raw_length)?))
}

/// The first entry of a block of which only `bytes`, its start, could be
/// read: for the report of an error that stops the machine, made where
/// the whole block cannot be copied. `None` when the header is short or
/// the block counts no entries; an error when the entry does not fit in
/// `bytes` or in the block's data length.
pub fn first_entry(bytes: &[u8]) -> Option<Result<ErrorEntry<'_>, CperError>> {
    let status = u32_at(bytes, 0)?;
    if (status >> 4) & 0x3ff == 0 {
        return None;
    }
    let data_length = u32_at(bytes, 12)? as usize;
    let data = bytes.get(BLOCK_HEADER_LEN..)?;
    let data = &data[..data.len().min(data_length)];
    Some(ErrorEntry::split(data).map(|(entry, _)| entry))
}
/// Bytes in a Generic Error Data Entry's header before revision 0x300.
const ENTRY_HEADER_LEN: usize = 64;
/// Bytes in the header from revision 0x300 on, which adds a time stamp.
const ENTRY_HEADER_LEN_TIMESTAMPED: usize = 72;
/// The first entry revision whose header carries a time stamp.
const TIMESTAMP_REVISION: u16 = 0x300;

/// Why a block or a section could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CperError {
    /// Fewer bytes than the 20-byte block header.
    BlockTooShort,
    /// The block's data length is larger than the bytes after the header.
    DataLength,
    /// The raw data's offset and length reach past the block.
    RawData,
    /// An entry's header runs past the block's data.
    EntryHeader,
    /// An entry's error data length runs past the block's data.
    EntryDataLength,
    /// The block counts more entries than [`MAX_ENTRIES`].
    TooManyEntries,
    /// A section has fewer bytes than its type's fixed size.
    SectionTooShort,
}

impl fmt::Display for CperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CperError::BlockTooShort => "error status block shorter than its header",
            CperError::DataLength => "error status block's data length runs past the block",
            CperError::RawData => "error status block's raw data runs past the block",
            CperError::EntryHeader => "error data entry's header runs past the block's data",
            CperError::EntryDataLength => "error data entry's length runs past the block's data",
            CperError::TooManyEntries => "error status block counts too many entries",
            CperError::SectionTooShort => "error section shorter than its type's size",
        })
    }
}

/// A globally unique identifier, as stored: the first three fields
/// little-endian, then eight bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid([u8; 16]);

impl Guid {
    /// A GUID from its text form's fields: `a-b-c-d` where `d` is the
    /// last two groups, 8 bytes in text order.
    pub const fn new(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
        let a = a.to_le_bytes();
        let b = b.to_le_bytes();
        let c = c.to_le_bytes();
        Guid([
            a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5],
            d[6], d[7],
        ])
    }

    /// A GUID from its 16 stored bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Guid {
        Guid(bytes)
    }

    /// The 16 stored bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    fn from_slice(bytes: &[u8]) -> Option<Guid> {
        Some(Guid(bytes.get(..16)?.try_into().ok()?))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15],
        )
    }
}

/// Section type: Platform Memory Error (UEFI 2.10, table N.?
/// "Memory Error Section").
pub const SECTION_MEMORY: Guid =
    Guid::new(0xa5bc_1114, 0x6f64, 0x4ede, [0xb8, 0x63, 0x3e, 0x83, 0xed, 0x7c, 0x83, 0xb1]);
/// Section type: Memory Error Section 2. Known by name and not decoded: its
/// layout is not the first section's, firmware written for Linux, which
/// does not decode it either, reports memory errors in the first, and it
/// waits for firmware that uses it (docs/deferred.md).
pub const SECTION_MEMORY_2: Guid =
    Guid::new(0x61ec_04fc, 0x48e6, 0xd813, [0x25, 0xc9, 0x8d, 0xaa, 0x44, 0x75, 0x0b, 0x12]);
/// Section type: PCI Express Error.
pub const SECTION_PCIE: Guid =
    Guid::new(0xd995_e954, 0xbbc1, 0x430f, [0xad, 0x91, 0xb4, 0x4d, 0xcb, 0x3c, 0x6f, 0x35]);
/// Section type: Processor Generic Error.
pub const SECTION_PROCESSOR_GENERIC: Guid =
    Guid::new(0x9876_ccad, 0x47b4, 0x4bdb, [0xb6, 0x5e, 0x16, 0xf1, 0x93, 0xc4, 0xf3, 0xdb]);
/// Section type: IA32/X64 Processor Error.
pub const SECTION_PROCESSOR_X86: Guid =
    Guid::new(0xdc3e_a0b0, 0xa144, 0x4797, [0xb9, 0x5b, 0x53, 0xfa, 0x24, 0x2b, 0x6e, 0x1d]);

/// How bad an error is (the Error Severity field of a block and of each
/// entry; UEFI 2.10, table N.? "Error Severity").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Severity(pub u32);

impl Severity {
    /// The error was recovered from, or can be.
    pub const RECOVERABLE: Severity = Severity(0);
    /// The error cannot be recovered from.
    pub const FATAL: Severity = Severity(1);
    /// The hardware corrected the error.
    pub const CORRECTED: Severity = Severity(2);
    /// Not an error: information only.
    pub const INFORMATIONAL: Severity = Severity(3);
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.write_str("recoverable"),
            1 => f.write_str("fatal"),
            2 => f.write_str("corrected"),
            3 => f.write_str("informational"),
            other => write!(f, "unknown severity {other}"),
        }
    }
}

/// A Generic Error Status Block: the header and the bounds of its entries
/// and its raw data, all checked against the bytes given.
#[derive(Clone, Copy, Debug)]
pub struct ErrorStatusBlock<'a> {
    /// The Block Status field.
    pub block_status: u32,
    /// Where the raw data starts, in bytes from the start of the block.
    pub raw_data_offset: u32,
    /// The raw data's length in bytes.
    pub raw_data_length: u32,
    /// The length in bytes of the entries after the header.
    pub data_length: u32,
    /// The worst severity of the block's errors.
    pub severity: Severity,
    entries: &'a [u8],
    raw: &'a [u8],
}

impl<'a> ErrorStatusBlock<'a> {
    /// Reads a block from `bytes`, the block as the firmware left it.
    ///
    /// # Errors
    ///
    /// The header must fit, and the data length and the raw data must lie
    /// within `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<ErrorStatusBlock<'a>, CperError> {
        // ACPI 6.5, "Generic Error Status Block": block status at 0, raw
        // data offset at 4, raw data length at 8, data length at 12, error
        // severity at 16.
        if bytes.len() < BLOCK_HEADER_LEN {
            return Err(CperError::BlockTooShort);
        }
        let field = |at| u32_at(bytes, at).ok_or(CperError::BlockTooShort);
        let (raw_data_offset, raw_data_length) = (field(4)?, field(8)?);
        let data_length = field(12)?;
        let entries =
            bytes[BLOCK_HEADER_LEN..].get(..data_length as usize).ok_or(CperError::DataLength)?;
        let raw = if raw_data_length == 0 {
            &[]
        } else {
            let start = raw_data_offset as usize;
            let end = start.checked_add(raw_data_length as usize).ok_or(CperError::RawData)?;
            bytes.get(start..end).ok_or(CperError::RawData)?
        };
        Ok(ErrorStatusBlock {
            block_status: field(0)?,
            raw_data_offset,
            raw_data_length,
            data_length,
            severity: Severity(field(16)?),
            entries,
            raw,
        })
    }

    /// An uncorrectable error is recorded (Block Status bit 0).
    pub fn uncorrectable_valid(&self) -> bool {
        self.block_status & 1 != 0
    }

    /// A correctable error is recorded (bit 1).
    pub fn correctable_valid(&self) -> bool {
        self.block_status & (1 << 1) != 0
    }

    /// More than one uncorrectable error happened (bit 2).
    pub fn multiple_uncorrectable(&self) -> bool {
        self.block_status & (1 << 2) != 0
    }

    /// More than one correctable error happened (bit 3).
    pub fn multiple_correctable(&self) -> bool {
        self.block_status & (1 << 3) != 0
    }

    /// How many entries the block says it holds (bits 13:4).
    pub fn entry_count(&self) -> u16 {
        ((self.block_status >> 4) & 0x3ff) as u16
    }

    /// The raw data, which is empty when the block has none.
    pub fn raw_data(&self) -> &'a [u8] {
        self.raw
    }

    /// The entries, as many as the block counts and at most
    /// [`MAX_ENTRIES`]. An entry that cannot be read is the last item, an
    /// error; so is a count above [`MAX_ENTRIES`], after the first
    /// [`MAX_ENTRIES`] entries.
    pub fn entries(&self) -> Entries<'a> {
        let count = usize::from(self.entry_count());
        Entries {
            rest: self.entries,
            remaining: count.min(MAX_ENTRIES),
            over_the_most: count > MAX_ENTRIES,
        }
    }
}

/// The walk of a block's Generic Error Data Entries.
#[derive(Clone, Debug)]
pub struct Entries<'a> {
    rest: &'a [u8],
    remaining: usize,
    over_the_most: bool,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Result<ErrorEntry<'a>, CperError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            if self.over_the_most {
                self.over_the_most = false;
                return Some(Err(CperError::TooManyEntries));
            }
            return None;
        }
        self.remaining -= 1;
        match ErrorEntry::split(self.rest) {
            Ok((entry, rest)) => {
                self.rest = rest;
                Some(Ok(entry))
            }
            Err(error) => {
                self.remaining = 0;
                self.over_the_most = false;
                Some(Err(error))
            }
        }
    }
}

/// A Generic Error Data Entry: one section of the record and what says
/// what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorEntry<'a> {
    /// The section's type.
    pub section_type: Guid,
    /// The error's severity.
    pub severity: Severity,
    /// The entry's revision: 0x300 is 3.0.
    pub revision: u16,
    /// The Validation Bits: FRU Id (bit 0), FRU text (bit 1), time stamp
    /// (bit 2).
    pub validation_bits: u8,
    /// The entry's flags.
    pub flags: u8,
    /// The identifier of the field replaceable unit, if valid.
    pub fru_id: Option<Guid>,
    /// The text that names the field replaceable unit, if valid.
    pub fru_text: Option<[u8; 20]>,
    /// The time stamp, in the format of UEFI 2.10 table N.?
    /// "Timestamp", present from revision 0x300 on; see
    /// [`ErrorEntry::timestamp_valid`].
    pub timestamp: Option<u64>,
    /// The section's bytes.
    pub data: &'a [u8],
}

impl<'a> ErrorEntry<'a> {
    /// The time stamp's validation bit is set.
    pub fn timestamp_valid(&self) -> bool {
        self.validation_bits & (1 << 2) != 0
    }

    /// Reads the entry at the start of `bytes` and returns it with the
    /// bytes after it.
    fn split(bytes: &'a [u8]) -> Result<(ErrorEntry<'a>, &'a [u8]), CperError> {
        // ACPI 6.5, "Generic Error Data Entry": section type at 0,
        // severity at 16, revision at 20, validation bits at 22, flags at
        // 23, error data length at 24, FRU id at 28, FRU text at 44 (20
        // bytes), and from revision 0x300 the time stamp at 64.
        if bytes.len() < ENTRY_HEADER_LEN {
            return Err(CperError::EntryHeader);
        }
        let revision = u16_at(bytes, 20).ok_or(CperError::EntryHeader)?;
        let timestamped = revision >= TIMESTAMP_REVISION;
        let header_len = if timestamped { ENTRY_HEADER_LEN_TIMESTAMPED } else { ENTRY_HEADER_LEN };
        if bytes.len() < header_len {
            return Err(CperError::EntryHeader);
        }
        let length = u32_at(bytes, 24).ok_or(CperError::EntryHeader)? as usize;
        let end = header_len.checked_add(length).ok_or(CperError::EntryDataLength)?;
        let (this, rest) = bytes.split_at_checked(end).ok_or(CperError::EntryDataLength)?;
        let validation_bits = this[22];
        let entry = ErrorEntry {
            section_type: Guid::from_slice(this).ok_or(CperError::EntryHeader)?,
            severity: Severity(u32_at(this, 16).ok_or(CperError::EntryHeader)?),
            revision,
            validation_bits,
            flags: this[23],
            fru_id: if validation_bits & 1 != 0 { Guid::from_slice(&this[28..]) } else { None },
            fru_text: if validation_bits & 2 != 0 { this[44..64].try_into().ok() } else { None },
            timestamp: if timestamped { u64_at(this, 64) } else { None },
            data: &this[header_len..],
        };
        Ok((entry, rest))
    }

    /// Decodes the section by its type; a type this module does not know is
    /// returned undecoded.
    ///
    /// # Errors
    ///
    /// A known type whose data is shorter than the type's fixed size.
    pub fn section(&self) -> Result<Section<'a>, CperError> {
        let data = self.data;
        Ok(match self.section_type {
            SECTION_MEMORY => Section::Memory(MemoryError::parse(data)?),
            SECTION_PCIE => Section::Pcie(PcieError::parse(data)?),
            SECTION_PROCESSOR_GENERIC => Section::Processor(ProcessorError::parse(data)?),
            SECTION_PROCESSOR_X86 => Section::ProcessorX86(X86ProcessorError::parse(data)?),
            guid => Section::Unknown { guid, severity: self.severity, data },
        })
    }
}

/// A decoded section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section<'a> {
    /// A platform memory error.
    Memory(MemoryError),
    /// A PCI Express error.
    Pcie(PcieError<'a>),
    /// A processor generic error.
    Processor(ProcessorError),
    /// An IA32/X64 processor error.
    ProcessorX86(X86ProcessorError),
    /// Any other section type, as it came.
    Unknown {
        /// The section's type.
        guid: Guid,
        /// The entry's severity.
        severity: Severity,
        /// The section's bytes.
        data: &'a [u8],
    },
}

/// Whether validation bit `bit` is set.
fn valid(bits: u64, bit: u32) -> bool {
    (bits >> bit) & 1 != 0
}

/// `value` when validation bit `bit` is set.
fn when<T>(bits: u64, bit: u32, value: Option<T>) -> Option<T> {
    if valid(bits, bit) { value } else { None }
}

/// A platform memory error (UEFI 2.10, appendix N, "Memory Error
/// Section"): where the error is and what kind it is. Each field is `Some`
/// only when its validation bit is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryError {
    /// The memory controller's error status register.
    pub error_status: Option<u64>,
    /// The physical address of the error.
    pub physical_address: Option<u64>,
    /// The bits of the address that are valid.
    pub physical_address_mask: Option<u64>,
    /// The node.
    pub node: Option<u16>,
    /// The card.
    pub card: Option<u16>,
    /// The module or rank.
    pub module: Option<u16>,
    /// The bank, in all its 16 bits.
    pub bank: Option<u16>,
    /// The bank address, bits 7:0 of the bank field, when bit 20 says so.
    pub bank_address: Option<u8>,
    /// The bank group, bits 15:8 of the bank field, when bit 19 says so.
    pub bank_group: Option<u8>,
    /// The device.
    pub device: Option<u16>,
    /// The row.
    pub row: Option<u16>,
    /// Row bits 17:16, from the Extended field.
    pub extended_row_bits: Option<u8>,
    /// The column.
    pub column: Option<u16>,
    /// The bit position.
    pub bit_position: Option<u16>,
    /// The requestor's identifier.
    pub requestor_id: Option<u64>,
    /// The responder's identifier.
    pub responder_id: Option<u64>,
    /// The target's identifier.
    pub target_id: Option<u64>,
    /// The memory error type: see [`memory_error_type_name`].
    pub error_type: Option<u8>,
    /// The rank number.
    pub rank: Option<u16>,
    /// The card's SMBIOS type 16 handle.
    pub card_handle: Option<u16>,
    /// The module's SMBIOS type 17 handle.
    pub module_handle: Option<u16>,
    /// The chip identification, bits 7:5 of the Extended field.
    pub chip_id: Option<u8>,
}

impl MemoryError {
    /// Bytes in the fixed part of the original section, before the rank
    /// number and the handles were added.
    const MIN_LEN: usize = 72;

    /// The row with its bits 17:16, when the section has them.
    pub fn full_row(&self) -> Option<u32> {
        let row = u32::from(self.row?);
        Some(row | u32::from(self.extended_row_bits.unwrap_or(0)) << 16)
    }

    fn parse(d: &[u8]) -> Result<MemoryError, CperError> {
        // Section 1, UEFI 2.10 N.2.5: validation bits at 0, error status
        // 8, physical address 16, mask 24, node 32, card 34, module 36,
        // bank 38, device 40, row 42, column 44, bit position 46,
        // requestor 48, responder 56, target 64, error type 72, extended
        // 73, rank 74, card handle 76, module handle 78. Validation bits:
        // 0 status, 1 address, 2 mask, 3 node, 4 card, 5 module, 6 bank,
        // 7 device, 8 row, 9 column, 10 bit position, 11 requestor, 12
        // responder, 13 target, 14 type, 15 rank, 16 card handle, 17
        // module handle, 18 extended row bits, 19 bank group, 20 bank
        // address, 21 chip identification.
        if d.len() < Self::MIN_LEN {
            return Err(CperError::SectionTooShort);
        }
        let bits = u64_at(d, 0).ok_or(CperError::SectionTooShort)?;
        let u64f = |bit, at| when(bits, bit, u64_at(d, at));
        let u16f = |bit, at| when(bits, bit, u16_at(d, at));
        let u8f = |bit, at| when(bits, bit, d.get(at).copied());
        let extended = d.get(73).copied().unwrap_or(0);
        let bank = u16f(6, 38);
        Ok(MemoryError {
            error_status: u64f(0, 8),
            physical_address: u64f(1, 16),
            physical_address_mask: u64f(2, 24),
            node: u16f(3, 32),
            card: u16f(4, 34),
            module: u16f(5, 36),
            bank,
            bank_address: when(bits, 20, u16_at(d, 38)).map(|bank| (bank & 0xff) as u8),
            bank_group: when(bits, 19, u16_at(d, 38)).map(|bank| (bank >> 8) as u8),
            device: u16f(7, 40),
            row: u16f(8, 42),
            extended_row_bits: when(bits, 18, Some(extended & 3)),
            column: u16f(9, 44),
            bit_position: u16f(10, 46),
            requestor_id: u64f(11, 48),
            responder_id: u64f(12, 56),
            target_id: u64f(13, 64),
            error_type: u8f(14, 72),
            rank: u16f(15, 74),
            card_handle: u16f(16, 76),
            module_handle: u16f(17, 78),
            chip_id: when(bits, 21, Some(extended >> 5)),
        })
    }
}

/// The name of a memory error type (UEFI 2.10, table N.? "Memory Error
/// Type").
pub fn memory_error_type_name(kind: u8) -> &'static str {
    match kind {
        0 => "unknown",
        1 => "no error",
        2 => "single-bit ECC",
        3 => "multi-bit ECC",
        4 => "single-symbol chipkill ECC",
        5 => "multi-symbol chipkill ECC",
        6 => "master abort",
        7 => "target abort",
        8 => "parity",
        9 => "watchdog timeout",
        10 => "invalid address",
        11 => "mirror broken",
        12 => "memory sparing",
        13 => "scrub corrected",
        14 => "scrub uncorrected",
        15 => "physical memory map-out",
        _ => "unknown type",
    }
}

/// Writes ", name value" after a first "name value" for each field a line
/// has.
struct Fields<'a, 'b> {
    out: &'a mut fmt::Formatter<'b>,
    separator: &'static str,
}

impl Fields<'_, '_> {
    fn put(&mut self, label: &str, value: impl fmt::Display) -> fmt::Result {
        write!(self.out, "{}{label} {value}", self.separator)?;
        self.separator = ", ";
        Ok(())
    }
}

/// A number shown in hexadecimal.
struct Hex(u64);

impl fmt::Display for Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory error")?;
        let mut w = Fields { out: f, separator: ": " };
        if let Some(kind) = self.error_type {
            w.put("type", memory_error_type_name(kind))?;
        }
        if let Some(status) = self.error_status {
            w.put("status", Hex(status))?;
        }
        if let Some(address) = self.physical_address {
            w.put("address", Hex(address))?;
        }
        if let Some(mask) = self.physical_address_mask {
            w.put("address mask", Hex(mask))?;
        }
        if let Some(node) = self.node {
            w.put("node", node)?;
        }
        if let Some(card) = self.card {
            w.put("card", card)?;
        }
        if let Some(module) = self.module {
            w.put("module", module)?;
        }
        if let Some(rank) = self.rank {
            w.put("rank", rank)?;
        }
        if let Some(bank) = self.bank {
            w.put("bank", bank)?;
        }
        if let Some(device) = self.device {
            w.put("device", device)?;
        }
        if let Some(chip) = self.chip_id {
            w.put("chip", chip)?;
        }
        if let Some(row) = self.full_row() {
            w.put("row", row)?;
        }
        if let Some(column) = self.column {
            w.put("column", column)?;
        }
        if let Some(bit) = self.bit_position {
            w.put("bit", bit)?;
        }
        if let Some(id) = self.requestor_id {
            w.put("requestor", Hex(id))?;
        }
        if let Some(id) = self.responder_id {
            w.put("responder", Hex(id))?;
        }
        if let Some(id) = self.target_id {
            w.put("target", Hex(id))?;
        }
        if let Some(handle) = self.card_handle {
            w.put("card handle", Hex(u64::from(handle)))?;
        }
        if let Some(handle) = self.module_handle {
            w.put("module handle", Hex(u64::from(handle)))?;
        }
        Ok(())
    }
}

/// The PCI function a PCI Express error section names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcieDeviceId {
    /// The vendor ID.
    pub vendor_id: u16,
    /// The device ID.
    pub device_id: u16,
    /// The class code's three bytes, as stored.
    pub class_code: [u8; 3],
    /// The function number.
    pub function: u8,
    /// The device number.
    pub device: u8,
    /// The PCI segment.
    pub segment: u16,
    /// The primary bus number.
    pub primary_bus: u8,
    /// The secondary bus number (a bridge's).
    pub secondary_bus: u8,
}

/// A PCI Express error (UEFI 2.10, appendix N, "PCI Express Error
/// Section"). Each field is `Some` only when its validation bit is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcieError<'a> {
    /// The device or port type from the PCI Express capability: 4 is a
    /// root port, 0 an endpoint.
    pub port_type: Option<u32>,
    /// The PCI Express version: (major, minor).
    pub version: Option<(u8, u8)>,
    /// The function's command register.
    pub command: Option<u16>,
    /// The function's status register.
    pub status: Option<u16>,
    /// The function that reported the error.
    pub device_id: Option<PcieDeviceId>,
    /// The device serial number.
    pub serial_number: Option<u64>,
    /// A bridge's secondary status register.
    pub bridge_secondary_status: Option<u16>,
    /// A bridge's control register.
    pub bridge_control: Option<u16>,
    /// The function's PCI Express capability structure.
    pub capability: Option<&'a [u8; 60]>,
    /// The function's AER capability structure.
    pub aer: Option<&'a [u8; 96]>,
}

impl<'a> PcieError<'a> {
    /// Bytes in the section.
    const LEN: usize = 208;

    fn parse(d: &'a [u8]) -> Result<PcieError<'a>, CperError> {
        // UEFI 2.10 N.2.7: validation bits at 0 (0 port type, 1 version, 2
        // command and status, 3 device ID, 4 serial number, 5 bridge
        // control and status, 6 capability structure, 7 AER info), port
        // type 8, version 12 (minor, major), command 16, status 18, 4
        // reserved bytes, device ID at 24 (16 bytes), serial number 40,
        // bridge secondary status 48, bridge control 50, capability
        // structure 52 (60 bytes), AER info 112 (96 bytes).
        if d.len() < Self::LEN {
            return Err(CperError::SectionTooShort);
        }
        let bits = u64_at(d, 0).ok_or(CperError::SectionTooShort)?;
        let device_id = when(bits, 3, Some(())).and_then(|()| Self::device_id_at(d, 24));
        Ok(PcieError {
            port_type: when(bits, 0, u32_at(d, 8)),
            version: when(bits, 1, Some((d[13], d[12]))),
            command: when(bits, 2, u16_at(d, 16)),
            status: when(bits, 2, u16_at(d, 18)),
            device_id,
            serial_number: when(bits, 4, u64_at(d, 40)),
            bridge_secondary_status: when(bits, 5, u16_at(d, 48)),
            bridge_control: when(bits, 5, u16_at(d, 50)),
            capability: when(bits, 6, d.get(52..112).and_then(|c| c.try_into().ok())),
            aer: when(bits, 7, d.get(112..208).and_then(|c| c.try_into().ok())),
        })
    }

    /// The Device ID structure: vendor at 0, device 2, class code 4 (3
    /// bytes), function 7, device 8, segment 9, primary bus 11, secondary
    /// bus 12, then 3 reserved bytes.
    fn device_id_at(d: &[u8], at: usize) -> Option<PcieDeviceId> {
        let id = d.get(at..at + 16)?;
        Some(PcieDeviceId {
            vendor_id: u16_at(id, 0)?,
            device_id: u16_at(id, 2)?,
            class_code: id.get(4..7)?.try_into().ok()?,
            function: id[7],
            device: id[8],
            segment: u16_at(id, 9)?,
            primary_bus: id[11],
            secondary_bus: id[12],
        })
    }

    fn aer_dword(&self, offset: usize) -> Option<u32> {
        u32_at(self.aer?, offset)
    }

    /// The Uncorrectable Error Status register (offset 0x04 of the AER
    /// capability), when the section carries AER bytes.
    pub fn uncorrectable_status(&self) -> Option<u32> {
        self.aer_dword(0x04)
    }

    /// The Uncorrectable Error Severity register (offset 0x0C).
    pub fn uncorrectable_severity(&self) -> Option<u32> {
        self.aer_dword(0x0c)
    }

    /// The Correctable Error Status register (offset 0x10).
    pub fn correctable_status(&self) -> Option<u32> {
        self.aer_dword(0x10)
    }

    /// The four dwords of the Header Log (offset 0x1C).
    pub fn header_log(&self) -> Option<[u32; 4]> {
        Some([
            self.aer_dword(0x1c)?,
            self.aer_dword(0x20)?,
            self.aer_dword(0x24)?,
            self.aer_dword(0x28)?,
        ])
    }
}

/// The name of a PCI Express device or port type, as in the PCI Express
/// capability's Device/Port Type field.
pub fn pcie_port_type_name(kind: u32) -> &'static str {
    match kind {
        0 => "endpoint",
        1 => "legacy endpoint",
        4 => "root port",
        5 => "upstream switch port",
        6 => "downstream switch port",
        7 => "PCI Express to PCI bridge",
        8 => "PCI to PCI Express bridge",
        9 => "root complex integrated endpoint",
        10 => "root complex event collector",
        _ => "unknown type",
    }
}

impl fmt::Display for PcieError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PCIe error")?;
        let mut w = Fields { out: f, separator: ": " };
        if let Some(id) = self.device_id {
            w.put(
                "device",
                format_args!(
                    "{:04x}:{:02x}:{:02x}.{} ({:04x}:{:04x})",
                    id.segment, id.primary_bus, id.device, id.function, id.vendor_id, id.device_id
                ),
            )?;
        }
        if let Some(kind) = self.port_type {
            w.put("port", pcie_port_type_name(kind))?;
        }
        if let Some(status) = self.uncorrectable_status() {
            w.put("uncorrectable status", Hex(u64::from(status)))?;
        }
        if let Some(severity) = self.uncorrectable_severity() {
            w.put("uncorrectable severity", Hex(u64::from(severity)))?;
        }
        if let Some(status) = self.correctable_status() {
            w.put("correctable status", Hex(u64::from(status)))?;
        }
        Ok(())
    }
}

/// A processor generic error (UEFI 2.10, appendix N, "Processor Generic
/// Error Section"). Each field is `Some` only when its validation bit is
/// set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorError {
    /// The processor type: 0 IA-32 or x86-64, 1 Itanium, 2 Arm.
    pub processor_type: Option<u8>,
    /// The instruction set: 0 IA-32, 1 Itanium, 2 x86-64, 3 Arm A32, 4
    /// Arm A64.
    pub isa: Option<u8>,
    /// The error type bit set: 0 unknown, 1 cache, 2 TLB, 4 bus, 8
    /// micro-architectural.
    pub error_type: Option<u8>,
    /// The operation: 0 generic, 1 data read, 2 data write, 3 instruction
    /// execution.
    pub operation: Option<u8>,
    /// The flags: restartable (bit 0), precise IP (bit 1), overflow (bit
    /// 2), corrected (bit 3).
    pub flags: Option<u8>,
    /// The cache or TLB level.
    pub level: Option<u8>,
    /// The CPU version information (the CPUID signature on x86).
    pub cpu_version: Option<u64>,
    /// The processor's identifier: its local APIC ID on x86.
    pub apic_id: Option<u64>,
    /// The address the operation targeted.
    pub target_address: Option<u64>,
    /// The requestor's identifier.
    pub requestor_id: Option<u64>,
    /// The responder's identifier.
    pub responder_id: Option<u64>,
    /// The instruction pointer at the error.
    pub instruction_pointer: Option<u64>,
}

impl ProcessorError {
    /// Bytes in the section.
    const LEN: usize = 192;

    fn parse(d: &[u8]) -> Result<ProcessorError, CperError> {
        // UEFI 2.10 N.2.4.1: validation bits at 0 (0 type, 1 ISA, 2 error
        // type, 3 operation, 4 flags, 5 level, 6 version, 7 brand string,
        // 8 processor ID, 9 target, 10 requestor, 11 responder, 12
        // instruction pointer), type 8, ISA 9, error type 10, operation 11,
        // flags 12, level 13, 2 reserved bytes, version 16, brand string
        // 24 (128 bytes), processor ID 152, target 160, requestor 168,
        // responder 176, instruction pointer 184.
        if d.len() < Self::LEN {
            return Err(CperError::SectionTooShort);
        }
        let bits = u64_at(d, 0).ok_or(CperError::SectionTooShort)?;
        let byte = |bit, at| when(bits, bit, d.get(at).copied());
        let quad = |bit, at| when(bits, bit, u64_at(d, at));
        Ok(ProcessorError {
            processor_type: byte(0, 8),
            isa: byte(1, 9),
            error_type: byte(2, 10),
            operation: byte(3, 11),
            flags: byte(4, 12),
            level: byte(5, 13),
            cpu_version: quad(6, 16),
            apic_id: quad(8, 152),
            target_address: quad(9, 160),
            requestor_id: quad(10, 168),
            responder_id: quad(11, 176),
            instruction_pointer: quad(12, 184),
        })
    }
}

/// The name of a processor generic error's type bit set.
fn processor_error_type_name(kind: u8) -> &'static str {
    match kind {
        0 => "unknown",
        1 => "cache",
        2 => "TLB",
        4 => "bus",
        8 => "micro-architectural",
        _ => "several",
    }
}

impl fmt::Display for ProcessorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("processor error")?;
        let mut w = Fields { out: f, separator: ": " };
        if let Some(kind) = self.error_type {
            w.put("type", processor_error_type_name(kind))?;
        }
        if let Some(level) = self.level {
            w.put("level", level)?;
        }
        if let Some(operation) = self.operation {
            let name = match operation {
                0 => "generic",
                1 => "data read",
                2 => "data write",
                3 => "instruction execution",
                _ => "unknown",
            };
            w.put("operation", name)?;
        }
        if let Some(flags) = self.flags {
            if flags & (1 << 3) != 0 {
                w.put("corrected", "yes")?;
            }
            if flags & 1 != 0 {
                w.put("restartable", "yes")?;
            }
        }
        if let Some(id) = self.apic_id {
            w.put("APIC ID", id)?;
        }
        if let Some(address) = self.target_address {
            w.put("address", Hex(address))?;
        }
        if let Some(ip) = self.instruction_pointer {
            w.put("instruction", Hex(ip))?;
        }
        Ok(())
    }
}

/// An IA32/X64 processor error (UEFI 2.10, appendix N, "IA32/X64
/// Processor Error Section"): which processor, and how many structures
/// follow that describe the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X86ProcessorError {
    /// The local APIC ID.
    pub local_apic_id: Option<u64>,
    /// The CPUID information: eax, ebx, ecx, edx of leaf 1 and the other
    /// registers the format lists, 48 bytes as stored.
    pub cpuid: Option<[u8; 48]>,
    /// How many Processor Error Information Structures follow.
    pub error_info_count: u8,
    /// How many Processor Context Information Structures follow those.
    pub context_count: u8,
}

impl X86ProcessorError {
    /// Bytes in the fixed part.
    const LEN: usize = 64;

    fn parse(d: &[u8]) -> Result<X86ProcessorError, CperError> {
        // UEFI 2.10 N.2.4.2: validation bits at 0 (0 local APIC ID, 1
        // CPUID info, 7:2 error information structure count, 13:8 context
        // structure count), local APIC ID 8, CPUID info 16 (48 bytes).
        if d.len() < Self::LEN {
            return Err(CperError::SectionTooShort);
        }
        let bits = u64_at(d, 0).ok_or(CperError::SectionTooShort)?;
        Ok(X86ProcessorError {
            local_apic_id: when(bits, 0, u64_at(d, 8)),
            cpuid: when(bits, 1, d.get(16..64).and_then(|c| c.try_into().ok())),
            error_info_count: ((bits >> 2) & 0x3f) as u8,
            context_count: ((bits >> 8) & 0x3f) as u8,
        })
    }
}

impl fmt::Display for X86ProcessorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("x86 processor error")?;
        let mut w = Fields { out: f, separator: ": " };
        if let Some(id) = self.local_apic_id {
            w.put("APIC ID", id)?;
        }
        if let Some(cpuid) = self.cpuid {
            // CPUID leaf 1's EAX: the family, model and stepping.
            let signature = u32::from_le_bytes([cpuid[0], cpuid[1], cpuid[2], cpuid[3]]);
            w.put("signature", Hex(u64::from(signature)))?;
        }
        w.put("error structures", self.error_info_count)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// A Generic Error Data Entry (ACPI 6.5, "Generic Error Data
    /// Entry"): section type at 0, severity at 16, revision at 20,
    /// validation bits at 22, flags at 23, data length at 24, FRU id at
    /// 28, FRU text at 44, and from revision 0x300 the time stamp at 64.
    fn entry(guid: Guid, severity: u32, revision: u16, validation: u8, data: &[u8]) -> Vec<u8> {
        let header = if revision >= 0x300 { 72 } else { 64 };
        let mut e = vec![0u8; header];
        e[..16].copy_from_slice(guid.as_bytes());
        put32(&mut e, 16, severity);
        put16(&mut e, 20, revision);
        e[22] = validation;
        e[23] = 1;
        put32(&mut e, 24, data.len() as u32);
        e.extend_from_slice(data);
        e
    }

    /// A Generic Error Status Block (ACPI 6.5, "Generic Error Status
    /// Block"): block status at 0, raw data offset at 4, raw data length
    /// at 8, data length at 12, severity at 16, the entries from 20 and
    /// the raw data after them.
    fn block(status: u32, severity: u32, entries: &[Vec<u8>], raw: &[u8]) -> Vec<u8> {
        let data: Vec<u8> = entries.concat();
        let mut b = vec![0u8; 20];
        put32(&mut b, 0, status);
        put32(&mut b, 4, if raw.is_empty() { 0 } else { 20 + data.len() as u32 });
        put32(&mut b, 8, raw.len() as u32);
        put32(&mut b, 12, data.len() as u32);
        put32(&mut b, 16, severity);
        b.extend_from_slice(&data);
        b.extend_from_slice(raw);
        b
    }

    /// The status for a block of `count` entries with the uncorrectable
    /// bit (bit 0) set; the count is bits 13:4.
    fn status(count: u32) -> u32 {
        1 | count << 4
    }

    /// The Memory Error Section: see [`MemoryError::parse`] for the
    /// offsets. The values are distinct so a swapped field shows.
    fn memory_section(bits: u64) -> Vec<u8> {
        let mut s = vec![0u8; 80];
        put64(&mut s, 0, bits);
        put64(&mut s, 8, 0x0400);
        put64(&mut s, 16, 0x1_2345_6000);
        put64(&mut s, 24, 0xffff_ffff_f000);
        put16(&mut s, 32, 1);
        put16(&mut s, 34, 2);
        put16(&mut s, 36, 3);
        put16(&mut s, 38, 0x0102);
        put16(&mut s, 40, 5);
        put16(&mut s, 42, 0x1234);
        put16(&mut s, 44, 0x56);
        put16(&mut s, 46, 7);
        put64(&mut s, 48, 0xaa);
        put64(&mut s, 56, 0xbb);
        put64(&mut s, 64, 0xcc);
        s[72] = 3;
        // Extended: bits 1:0 row bits 17:16 (1), bits 7:5 chip id (5).
        s[73] = 0b1010_0001;
        put16(&mut s, 74, 4);
        put16(&mut s, 76, 0x30);
        put16(&mut s, 78, 0x31);
        s
    }

    /// Validation bits 0 to 15: everything up to the rank number.
    const MEMORY_BITS_NO_HANDLES: u64 = 0xffff;

    /// A PCI Express Error Section: see [`PcieError::parse`] for the
    /// offsets.
    fn pcie_section(bits: u64) -> Vec<u8> {
        let mut s = vec![0u8; 208];
        put64(&mut s, 0, bits);
        put32(&mut s, 8, 4);
        s[12] = 1; // minor
        s[13] = 2; // major
        put16(&mut s, 16, 0x0007);
        put16(&mut s, 18, 0x0010);
        // Device ID: vendor, device, class code, function, device,
        // segment, primary bus, secondary bus.
        put16(&mut s, 24, 0x8086);
        put16(&mut s, 26, 0x1533);
        s[28..31].copy_from_slice(&[0x00, 0x00, 0x02]);
        s[31] = 1;
        s[32] = 2;
        put16(&mut s, 33, 3);
        s[35] = 0x40;
        s[36] = 0x41;
        put64(&mut s, 40, 0x1122_3344_5566_7788);
        put16(&mut s, 48, 0x0a0a);
        put16(&mut s, 50, 0x0b0b);
        for (i, byte) in s[52..112].iter_mut().enumerate() {
            *byte = i as u8;
        }
        // The AER capability structure from 112: uncorrectable status at
        // +0x04, mask +0x08, severity +0x0C, correctable status +0x10,
        // header log at +0x1C.
        put32(&mut s, 112 + 0x04, 0x0010_0000);
        put32(&mut s, 112 + 0x0c, 0x0046_2030);
        put32(&mut s, 112 + 0x10, 0x0000_0001);
        for (i, word) in [0xa0a0_a0a0u32, 0xb1b1_b1b1, 0xc2c2_c2c2, 0xd3d3_d3d3].iter().enumerate()
        {
            put32(&mut s, 112 + 0x1c + 4 * i, *word);
        }
        s
    }

    fn processor_section(bits: u64) -> Vec<u8> {
        // UEFI 2.10 N.2.4.1 offsets: see ProcessorError::parse.
        let mut s = vec![0u8; 192];
        put64(&mut s, 0, bits);
        s[8] = 0;
        s[9] = 2;
        s[10] = 4;
        s[11] = 1;
        s[12] = 0b1001;
        s[13] = 2;
        put64(&mut s, 16, 0x000a_0652);
        put64(&mut s, 152, 7);
        put64(&mut s, 160, 0xdead_0000);
        put64(&mut s, 168, 0x11);
        put64(&mut s, 176, 0x22);
        put64(&mut s, 184, 0xffff_ffff_8100_0000);
        s
    }

    fn x86_section(bits: u64) -> Vec<u8> {
        // UEFI 2.10 N.2.4.2: local APIC id at 8, CPUID info at 16.
        let mut s = vec![0u8; 64];
        put64(&mut s, 0, bits);
        put64(&mut s, 8, 9);
        for (i, byte) in s[16..64].iter_mut().enumerate() {
            *byte = 0x80 + i as u8;
        }
        s
    }

    fn entries_of(bytes: &[u8]) -> Vec<Result<ErrorEntry<'_>, CperError>> {
        ErrorStatusBlock::parse(bytes).unwrap().entries().collect()
    }

    #[test]
    fn guids_are_shown_in_the_standard_form() {
        assert_eq!(SECTION_MEMORY.to_string(), "A5BC1114-6F64-4EDE-B863-3E83ED7C83B1");
        assert_eq!(SECTION_MEMORY_2.to_string(), "61EC04FC-48E6-D813-25C9-8DAA44750B12");
        assert_eq!(SECTION_PCIE.to_string(), "D995E954-BBC1-430F-AD91-B44DCB3C6F35");
        assert_eq!(SECTION_PROCESSOR_GENERIC.to_string(), "9876CCAD-47B4-4BDB-B65E-16F193C4F3DB");
        assert_eq!(SECTION_PROCESSOR_X86.to_string(), "DC3EA0B0-A144-4797-B95B-53FA242B6E1D");
        // The first three fields are stored little-endian.
        assert_eq!(&SECTION_MEMORY.as_bytes()[..4], &[0x14, 0x11, 0xbc, 0xa5]);
    }

    #[test]
    fn severities_read_in_plain_words() {
        assert_eq!(Severity::RECOVERABLE.to_string(), "recoverable");
        assert_eq!(Severity::FATAL.to_string(), "fatal");
        assert_eq!(Severity::CORRECTED.to_string(), "corrected");
        assert_eq!(Severity::INFORMATIONAL.to_string(), "informational");
        assert_eq!(Severity(9).to_string(), "unknown severity 9");
    }

    #[test]
    fn reads_the_block_header() {
        let section = memory_section(0);
        let bytes = block(
            1 | 1 << 1 | 1 << 2 | 1 << 3 | 1 << 4,
            1,
            &[entry(SECTION_MEMORY, 1, 0x201, 0, &section)],
            &[1, 2, 3],
        );
        let block = ErrorStatusBlock::parse(&bytes).unwrap();
        assert!(block.uncorrectable_valid());
        assert!(block.correctable_valid());
        assert!(block.multiple_uncorrectable());
        assert!(block.multiple_correctable());
        assert_eq!(block.entry_count(), 1);
        assert_eq!(block.severity, Severity::FATAL);
        assert_eq!(block.data_length as usize, 64 + 80);
        assert_eq!(block.raw_data(), [1, 2, 3]);

        let quiet = block_with_status(0);
        let block = ErrorStatusBlock::parse(&quiet).unwrap();
        assert!(!block.uncorrectable_valid() && !block.correctable_valid());
        assert!(!block.multiple_uncorrectable() && !block.multiple_correctable());
        assert_eq!(block.entry_count(), 0);
        assert!(block.raw_data().is_empty());
        assert_eq!(block.entries().count(), 0);
    }

    fn block_with_status(status: u32) -> Vec<u8> {
        block(status, 3, &[], &[])
    }

    #[test]
    fn decodes_a_memory_error_with_a_module_handle() {
        let section = memory_section(0x3f_ffff);
        let bytes = block(status(1), 1, &[entry(SECTION_MEMORY, 1, 0x201, 0, &section)], &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 1);
        let entry = entries[0].unwrap();
        assert_eq!(entry.section_type, SECTION_MEMORY);
        assert_eq!(entry.severity, Severity::FATAL);
        assert_eq!(entry.revision, 0x201);
        assert_eq!(entry.timestamp, None);
        let Section::Memory(m) = entry.section().unwrap() else { panic!("not memory") };
        assert_eq!(m.error_status, Some(0x0400));
        assert_eq!(m.physical_address, Some(0x1_2345_6000));
        assert_eq!(m.physical_address_mask, Some(0xffff_ffff_f000));
        assert_eq!((m.node, m.card, m.module), (Some(1), Some(2), Some(3)));
        assert_eq!(m.bank, Some(0x0102));
        assert_eq!((m.bank_group, m.bank_address), (Some(1), Some(2)));
        assert_eq!(
            (m.device, m.row, m.column, m.bit_position),
            (Some(5), Some(0x1234), Some(0x56), Some(7))
        );
        assert_eq!(m.extended_row_bits, Some(1));
        assert_eq!(m.full_row(), Some(0x1_1234));
        assert_eq!(
            (m.requestor_id, m.responder_id, m.target_id),
            (Some(0xaa), Some(0xbb), Some(0xcc))
        );
        assert_eq!(m.error_type, Some(3));
        assert_eq!(m.rank, Some(4));
        assert_eq!(m.card_handle, Some(0x30));
        assert_eq!(m.module_handle, Some(0x31));
        assert_eq!(m.chip_id, Some(5));
        assert_eq!(
            m.to_string(),
            "memory error: type multi-bit ECC, status 0x400, address 0x123456000, address mask \
             0xfffffffff000, node 1, card 2, module 3, rank 4, bank 258, device 5, chip 5, row \
             70196, column 86, bit 7, requestor 0xaa, responder 0xbb, target 0xcc, card handle \
             0x30, module handle 0x31"
        );
    }

    #[test]
    fn a_memory_error_without_handles_leaves_them_out() {
        // The bytes hold handles, but validation bits 16 and 17 are clear.
        let section = memory_section(MEMORY_BITS_NO_HANDLES);
        let bytes = block(status(1), 2, &[entry(SECTION_MEMORY, 2, 0x201, 0, &section)], &[]);
        let first = entries_of(&bytes)[0].unwrap();
        let Section::Memory(m) = first.section().unwrap() else { panic!("not memory") };
        assert_eq!(m.card_handle, None);
        assert_eq!(m.module_handle, None);
        assert_eq!(m.rank, Some(4));
        assert_eq!(m.bank, Some(0x0102));
        assert_eq!(
            (m.bank_group, m.bank_address, m.extended_row_bits, m.chip_id),
            (None, None, None, None)
        );
        assert_eq!(m.full_row(), Some(0x1234));
        assert!(!m.to_string().contains("handle"));

        let nothing = memory_section(0);
        let Section::Memory(m) = ErrorEntry {
            section_type: SECTION_MEMORY,
            severity: Severity::CORRECTED,
            revision: 0x201,
            validation_bits: 0,
            flags: 0,
            fru_id: None,
            fru_text: None,
            timestamp: None,
            data: &nothing,
        }
        .section()
        .unwrap() else {
            panic!("not memory")
        };
        assert_eq!(m.physical_address, None);
        assert_eq!(m.to_string(), "memory error");
    }

    #[test]
    fn memory_error_section_2_is_named_and_not_decoded() {
        let section = memory_section(0x3f_ffff);
        let bytes = block(status(1), 2, &[entry(SECTION_MEMORY_2, 2, 0x300, 0, &section)], &[]);
        let first = entries_of(&bytes)[0].unwrap();
        let Section::Unknown { guid, .. } = first.section().unwrap() else { panic!("decoded") };
        assert_eq!(guid, SECTION_MEMORY_2);
    }

    #[test]
    fn decodes_a_pcie_error_with_aer_bytes() {
        let section = pcie_section(0xff);
        let fru = [0x5a; 16];
        let mut e = entry(SECTION_PCIE, 0, 0x201, 0b11, &section);
        e[28..44].copy_from_slice(&fru);
        e[44..52].copy_from_slice(b"slot 3\0\0");
        let bytes = block(status(1), 0, &[e], &[]);
        let entry = entries_of(&bytes)[0].unwrap();
        assert_eq!(entry.fru_id, Some(Guid::from_bytes(fru)));
        assert_eq!(&entry.fru_text.unwrap()[..6], b"slot 3");
        let Section::Pcie(p) = entry.section().unwrap() else { panic!("not pcie") };
        assert_eq!(p.port_type, Some(4));
        assert_eq!(p.version, Some((2, 1)));
        assert_eq!((p.command, p.status), (Some(7), Some(0x10)));
        let id = p.device_id.unwrap();
        assert_eq!((id.vendor_id, id.device_id), (0x8086, 0x1533));
        assert_eq!(id.class_code, [0, 0, 2]);
        assert_eq!((id.function, id.device, id.segment), (1, 2, 3));
        assert_eq!((id.primary_bus, id.secondary_bus), (0x40, 0x41));
        assert_eq!(p.serial_number, Some(0x1122_3344_5566_7788));
        assert_eq!((p.bridge_secondary_status, p.bridge_control), (Some(0x0a0a), Some(0x0b0b)));
        let capability = p.capability.unwrap();
        assert_eq!((capability[0], capability[59]), (0, 59));
        assert_eq!(p.aer.unwrap().len(), 96);
        assert_eq!(p.uncorrectable_status(), Some(0x0010_0000));
        assert_eq!(p.uncorrectable_severity(), Some(0x0046_2030));
        assert_eq!(p.correctable_status(), Some(1));
        assert_eq!(p.header_log(), Some([0xa0a0_a0a0, 0xb1b1_b1b1, 0xc2c2_c2c2, 0xd3d3_d3d3]));
        assert_eq!(
            p.to_string(),
            "PCIe error: device 0003:40:02.1 (8086:1533), port root port, uncorrectable status \
             0x100000, uncorrectable severity 0x462030, correctable status 0x1"
        );
    }

    #[test]
    fn a_pcie_error_without_aer_bytes_has_no_accessors() {
        let section = pcie_section(0b0000_1001);
        let bytes = block(status(1), 2, &[entry(SECTION_PCIE, 2, 0x201, 0, &section)], &[]);
        let Section::Pcie(p) = entries_of(&bytes)[0].unwrap().section().unwrap() else {
            panic!("not pcie")
        };
        assert_eq!(p.port_type, Some(4));
        assert!(p.device_id.is_some());
        assert_eq!((p.command, p.version, p.capability, p.aer), (None, None, None, None));
        assert_eq!(p.uncorrectable_status(), None);
        assert_eq!(p.header_log(), None);
        assert_eq!(p.to_string(), "PCIe error: device 0003:40:02.1 (8086:1533), port root port");
    }

    #[test]
    fn decodes_a_processor_generic_error() {
        let section = processor_section(0x1f7f);
        let bytes =
            block(status(1), 2, &[entry(SECTION_PROCESSOR_GENERIC, 2, 0x201, 0, &section)], &[]);
        let Section::Processor(p) = entries_of(&bytes)[0].unwrap().section().unwrap() else {
            panic!("not a processor error")
        };
        assert_eq!((p.processor_type, p.isa, p.error_type), (Some(0), Some(2), Some(4)));
        assert_eq!((p.operation, p.flags, p.level), (Some(1), Some(0b1001), Some(2)));
        assert_eq!(p.cpu_version, Some(0x000a_0652));
        assert_eq!(p.apic_id, Some(7));
        assert_eq!(p.target_address, Some(0xdead_0000));
        assert_eq!((p.requestor_id, p.responder_id), (Some(0x11), Some(0x22)));
        assert_eq!(p.instruction_pointer, Some(0xffff_ffff_8100_0000));

        let section = processor_section(1 << 8);
        let bytes =
            block(status(1), 2, &[entry(SECTION_PROCESSOR_GENERIC, 2, 0x201, 0, &section)], &[]);
        let Section::Processor(p) = entries_of(&bytes)[0].unwrap().section().unwrap() else {
            panic!("not a processor error")
        };
        assert_eq!(p.apic_id, Some(7));
        assert_eq!((p.processor_type, p.cpu_version, p.instruction_pointer), (None, None, None));
    }

    #[test]
    fn decodes_an_x86_processor_error() {
        // Local APIC id valid, CPUID valid, 3 error structures, 2 contexts.
        let section = x86_section(1 | 1 << 1 | 3 << 2 | 2 << 8);
        let bytes =
            block(status(1), 1, &[entry(SECTION_PROCESSOR_X86, 1, 0x201, 0, &section)], &[]);
        let Section::ProcessorX86(p) = entries_of(&bytes)[0].unwrap().section().unwrap() else {
            panic!("not an x86 processor error")
        };
        assert_eq!(p.local_apic_id, Some(9));
        assert_eq!(p.cpuid.unwrap()[0], 0x80);
        assert_eq!(p.cpuid.unwrap()[47], 0x80 + 47);
        assert_eq!((p.error_info_count, p.context_count), (3, 2));

        let section = x86_section(0);
        let bytes =
            block(status(1), 1, &[entry(SECTION_PROCESSOR_X86, 1, 0x201, 0, &section)], &[]);
        let Section::ProcessorX86(p) = entries_of(&bytes)[0].unwrap().section().unwrap() else {
            panic!("not an x86 processor error")
        };
        assert_eq!(
            (p.local_apic_id, p.cpuid, p.error_info_count, p.context_count),
            (None, None, 0, 0)
        );
    }

    #[test]
    fn an_unknown_section_comes_back_undecoded() {
        let other = Guid::new(0x1234_5678, 0x9abc, 0xdef0, [1, 2, 3, 4, 5, 6, 7, 8]);
        let bytes = block(status(1), 3, &[entry(other, 3, 0x201, 0, &[9, 8, 7])], &[]);
        let entry = entries_of(&bytes)[0].unwrap();
        assert_eq!(
            entry.section(),
            Ok(Section::Unknown {
                guid: other,
                severity: Severity::INFORMATIONAL,
                data: &[9, 8, 7]
            })
        );
        assert_eq!(other.to_string(), "12345678-9ABC-DEF0-0102-030405060708");
    }

    #[test]
    fn a_revision_3_entry_has_a_time_stamp_and_a_longer_header() {
        let section = memory_section(0x3f_ffff);
        let mut e = entry(SECTION_MEMORY, 2, 0x300, 0b100, &section);
        // The time stamp is at 64, after the 64-byte header of earlier
        // revisions.
        put64(&mut e, 64, 0x2026_1123_0405_0600);
        let plain = entry(SECTION_MEMORY, 2, 0x201, 0, &section);
        assert_eq!(e.len(), plain.len() + 8);
        let bytes = block(status(2), 2, &[e, plain], &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 2);
        let stamped = entries[0].unwrap();
        assert_eq!(stamped.timestamp, Some(0x2026_1123_0405_0600));
        assert!(stamped.timestamp_valid());
        assert_eq!(stamped.data.len(), 80);
        assert!(matches!(stamped.section(), Ok(Section::Memory(_))));
        let second = entries[1].unwrap();
        assert_eq!(second.timestamp, None);
        assert!(!second.timestamp_valid());
        assert_eq!(second.data.len(), 80);
    }

    #[test]
    fn a_block_that_lies_about_its_lengths_is_refused() {
        assert_eq!(ErrorStatusBlock::parse(&[0; 19]).unwrap_err(), CperError::BlockTooShort);

        let section = memory_section(0);
        let good = block(status(1), 1, &[entry(SECTION_MEMORY, 1, 0x201, 0, &section)], &[1, 2]);

        // The data length reaches past the block.
        let mut lie = good.clone();
        put32(&mut lie, 12, 0x7fff_ffff);
        assert_eq!(ErrorStatusBlock::parse(&lie).unwrap_err(), CperError::DataLength);
        put32(&mut lie, 12, u32::MAX);
        assert_eq!(ErrorStatusBlock::parse(&lie).unwrap_err(), CperError::DataLength);

        // The raw data offset or length reaches past the block.
        let mut lie = good.clone();
        put32(&mut lie, 8, 3);
        assert_eq!(ErrorStatusBlock::parse(&lie).unwrap_err(), CperError::RawData);
        let mut lie = good.clone();
        put32(&mut lie, 4, 0x1000);
        assert_eq!(ErrorStatusBlock::parse(&lie).unwrap_err(), CperError::RawData);
        let mut lie = good;
        put32(&mut lie, 4, u32::MAX);
        put32(&mut lie, 8, u32::MAX);
        assert_eq!(ErrorStatusBlock::parse(&lie).unwrap_err(), CperError::RawData);
    }

    #[test]
    fn an_entry_that_lies_about_its_length_stops_the_walk() {
        let section = memory_section(0);
        let mut e = entry(SECTION_MEMORY, 1, 0x201, 0, &section);
        put32(&mut e, 24, 0x7fff_ffff);
        let bytes = block(status(2), 1, &[entry(SECTION_MEMORY, 1, 0x201, 0, &section), e], &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_ok());
        assert_eq!(entries[1], Err(CperError::EntryDataLength));

        let mut e = entry(SECTION_MEMORY, 1, 0x201, 0, &section);
        put32(&mut e, 24, u32::MAX);
        let bytes = block(status(1), 1, &[e], &[]);
        assert_eq!(entries_of(&bytes), [Err(CperError::EntryDataLength)]);
    }

    #[test]
    fn an_entry_header_that_does_not_fit_stops_the_walk() {
        // The block's data is shorter than a header.
        let bytes = block(status(1), 1, &[vec![0u8; 30]], &[]);
        assert_eq!(entries_of(&bytes), [Err(CperError::EntryHeader)]);

        // A revision 0x300 entry whose data stops inside the 72-byte
        // header.
        let mut e = entry(SECTION_MEMORY, 1, 0x300, 0, &[]);
        e.truncate(68);
        let bytes = block(status(1), 1, &[e], &[]);
        assert_eq!(entries_of(&bytes), [Err(CperError::EntryHeader)]);
    }

    #[test]
    fn an_entry_count_that_lies_is_refused() {
        let section = memory_section(0);
        let e = entry(SECTION_MEMORY, 1, 0x201, 0, &section);

        // The block counts three entries and holds one.
        let bytes = block(status(3), 1, std::slice::from_ref(&e), &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_ok());
        assert_eq!(entries[1], Err(CperError::EntryHeader));

        // The block counts 100 entries and holds 100: only 64 are walked,
        // and the walk says it stopped.
        let many = vec![e.clone(); 100];
        let bytes = block(status(100), 1, &many, &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), MAX_ENTRIES + 1);
        assert!(entries[..MAX_ENTRIES].iter().all(Result::is_ok));
        assert_eq!(entries[MAX_ENTRIES], Err(CperError::TooManyEntries));

        // The block counts the most it can, 1023, and holds one.
        let bytes = block(status(1023), 1, std::slice::from_ref(&e), &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1], Err(CperError::EntryHeader));

        // Exactly 64 is fine.
        let bytes = block(status(64), 1, &vec![e; 64], &[]);
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 64);
        assert!(entries.iter().all(Result::is_ok));
    }

    #[test]
    fn a_section_shorter_than_its_type_is_refused() {
        let bytes = block(
            status(4),
            1,
            &[
                entry(SECTION_MEMORY, 1, 0x201, 0, &[0; 71]),
                entry(SECTION_PCIE, 1, 0x201, 0, &[0; 207]),
                entry(SECTION_PROCESSOR_GENERIC, 1, 0x201, 0, &[0; 191]),
                entry(SECTION_PROCESSOR_X86, 1, 0x201, 0, &[0; 63]),
            ],
            &[],
        );
        let entries = entries_of(&bytes);
        assert_eq!(entries.len(), 4);
        for entry in entries {
            assert_eq!(entry.unwrap().section(), Err(CperError::SectionTooShort));
        }
    }

    #[test]
    fn errors_read_in_plain_words() {
        assert_eq!(
            CperError::DataLength.to_string(),
            "error status block's data length runs past the block"
        );
        assert_eq!(
            CperError::TooManyEntries.to_string(),
            "error status block counts too many entries"
        );
    }

    #[test]
    fn processor_errors_read_in_words() {
        let section = processor_section(0x1f7f);
        let bytes =
            block(status(1), 2, &[entry(SECTION_PROCESSOR_GENERIC, 2, 0x201, 0, &section)], &[]);
        let section = entries_of(&bytes)[0].unwrap().section().unwrap();
        let Section::Processor(p) = section else { panic!("not a processor error") };
        assert_eq!(
            p.to_string(),
            "processor error: type bus, level 2, operation data read, corrected yes, \
             restartable yes, APIC ID 7, address 0xdead0000, instruction 0xffffffff81000000"
        );

        let section = x86_section(1 | 1 << 1 | 3 << 2 | 2 << 8);
        let bytes =
            block(status(1), 1, &[entry(SECTION_PROCESSOR_X86, 1, 0x201, 0, &section)], &[]);
        let Section::ProcessorX86(p) = entries_of(&bytes)[0].unwrap().section().unwrap() else {
            panic!("not an x86 processor error")
        };
        assert_eq!(
            p.to_string(),
            "x86 processor error: APIC ID 9, signature 0x83828180, error structures 3"
        );
    }

    #[test]
    fn the_length_of_a_block_counts_its_entries_and_its_raw_data() {
        let section = memory_section(MEMORY_BITS_NO_HANDLES);
        let one = entry(SECTION_MEMORY, 2, 0x201, 0, &section);
        let bytes = block(status(1), 2, core::slice::from_ref(&one), &[]);
        assert_eq!(block_len(&bytes), Some(20 + one.len()));
        let bytes = block(status(1), 2, core::slice::from_ref(&one), &[7; 12]);
        assert_eq!(block_len(&bytes), Some(20 + one.len() + 12));
        assert_eq!(block_len(&bytes[..19]), None);
        // Raw data before the entries: the entries' end is the length.
        let mut bytes = block(status(1), 2, &[one], &[]);
        put32(&mut bytes, 4, 0);
        put32(&mut bytes, 8, 4);
        assert_eq!(block_len(&bytes), Some(bytes.len()));
        // Lengths at the top of their range cannot overflow.
        put32(&mut bytes, 4, u32::MAX);
        put32(&mut bytes, 8, u32::MAX);
        assert_eq!(block_len(&bytes), Some(u32::MAX as usize * 2));
    }

    #[test]
    fn the_first_entry_is_read_from_the_start_of_a_block() {
        let section = memory_section(MEMORY_BITS_NO_HANDLES);
        let first = entry(SECTION_MEMORY, 1, 0x201, 0, &section);
        let second = entry(SECTION_PROCESSOR_X86, 1, 0x201, 0, &x86_section(0));
        let bytes = block(status(2), 1, &[first.clone(), second], &[]);
        // Only the first entry's bytes were read.
        let start = &bytes[..20 + first.len()];
        let entry = first_entry(start).unwrap().unwrap();
        assert_eq!(entry.section_type, SECTION_MEMORY);
        let Section::Memory(m) = entry.section().unwrap() else { panic!("not a memory error") };
        assert_eq!(m.physical_address, Some(0x1_2345_6000));
        // Too little of it read: an error, not a panic.
        let start = &bytes[..20 + first.len() - 1];
        assert_eq!(first_entry(start).unwrap(), Err(CperError::EntryDataLength));
        // A data length shorter than the entry bounds it too.
        let mut short = bytes.clone();
        put32(&mut short, 12, 30);
        assert_eq!(first_entry(&short).unwrap(), Err(CperError::EntryHeader));
        // No entries counted, or a short header: nothing.
        assert!(first_entry(&block(0, 1, &[], &[])).is_none());
        assert!(first_entry(&bytes[..10]).is_none());
    }
}
