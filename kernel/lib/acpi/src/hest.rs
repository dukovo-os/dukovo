// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Hardware Error Source Table (signature `HEST`), which lists the
//! sources of hardware errors the firmware knows of and how the operating
//! system learns of each: machine check banks, NMIs, PCI Express AER
//! capabilities, and the Generic Hardware Error Sources (GHES) through
//! which firmware hands over error records (ACPI Specification 6.5,
//! section 18.3.2).
//!
//! The table is a count followed by variable-length structures, each
//! starting with its 16-bit type. A structure's length follows from its
//! type (and, for the machine check types, from the number of banks it
//! lists), so a type this parser does not know ends the walk: the
//! structures before it are returned and the walk's last item is an
//! [`HestError`]. A structure that runs past the end of the table, or a
//! count that promises more structures than the table holds, ends the walk
//! the same way. The decoder of the error records themselves is [`cper`].
//!
//! [`cper`]: crate::cper

use crate::{Error, GenericAddress, Sdt, u16_at, u32_at, u64_at};

/// The source is handled by firmware first (FIRMWARE_FIRST, bit 0).
const FLAG_FIRMWARE_FIRST: u8 = 1 << 0;
/// The flags of an AER source apply to every device (GLOBAL, bit 1).
const FLAG_GLOBAL: u8 = 1 << 1;
/// The firmware assists the operating system's machine check handling
/// (GHES_ASSIST, bit 2).
const FLAG_GHES_ASSIST: u8 = 1 << 2;

/// Bytes in the Notification Structure (ACPI 6.5, table 18.x).
const NOTIFICATION_LEN: usize = 28;
/// Bytes in an IA-32 machine check bank structure.
const BANK_LEN: usize = 28;

/// Why the walk of the structures ended before the table's count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HestError {
    /// A structure of a type this parser does not know. Its length is
    /// unknown, so nothing after it can be read.
    UnknownType(u16),
    /// A structure of this type is longer than what is left of the table.
    Truncated(u16),
    /// The count promises another structure, but the table has no bytes
    /// for its type.
    Missing,
}

impl core::fmt::Display for HestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HestError::UnknownType(kind) => write!(f, "unknown error source type {kind}"),
            HestError::Truncated(kind) => {
                write!(f, "error source of type {kind} runs past the end of the table")
            }
            HestError::Missing => f.write_str("fewer error sources than the table counts"),
        }
    }
}

/// How the operating system is told of an error (the Notification
/// Structure of a GHES, a corrected machine check or a deferred machine
/// check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notification {
    /// The notification type: one of the associated constants.
    pub kind: u8,
    /// The structure's length in bytes, 28.
    pub length: u8,
    /// Which of the polling and threshold fields below the firmware lets
    /// the operating system write (Configuration Write Enable, bits 0 to
    /// 5: type, poll interval, switch value, switch window, error value,
    /// error window).
    pub config_write_enable: u16,
    /// Milliseconds between polls of a polled source.
    pub poll_interval: u32,
    /// The interrupt vector, GSIV or SCI-related value, by type.
    pub vector: u32,
    /// Events after which to switch from interrupts to polling.
    pub switch_to_polling_threshold_value: u32,
    /// Milliseconds over which those events are counted.
    pub switch_to_polling_threshold_window: u32,
    /// Errors after which the source is taken out of use.
    pub error_threshold_value: u32,
    /// Milliseconds over which those errors are counted.
    pub error_threshold_window: u32,
}

impl Notification {
    /// Polled: the operating system reads the status at the poll interval.
    pub const POLLED: u8 = 0;
    /// An external interrupt.
    pub const EXTERNAL_INTERRUPT: u8 = 1;
    /// A local interrupt.
    pub const LOCAL_INTERRUPT: u8 = 2;
    /// The System Control Interrupt.
    pub const SCI: u8 = 3;
    /// A non-maskable interrupt.
    pub const NMI: u8 = 4;
    /// A corrected machine check interrupt (CMCI).
    pub const CMCI: u8 = 5;
    /// A machine check exception.
    pub const MCE: u8 = 6;
    /// A GPIO-signal.
    pub const GPIO: u8 = 7;
    /// A synchronous external abort (Arm).
    pub const SEA: u8 = 8;
    /// A system error interrupt (Arm).
    pub const SEI: u8 = 9;
    /// A global system interrupt vector.
    pub const GSIV: u8 = 10;
    /// A software delegated exception (Arm).
    pub const SDEI: u8 = 11;

    /// Parses the 28-byte structure.
    pub fn parse(bytes: &[u8]) -> Option<Notification> {
        // ACPI 6.5 table "Hardware Error Notification Structure": type,
        // length, write enable, then six 32-bit fields.
        Some(Notification {
            kind: *bytes.first()?,
            length: *bytes.get(1)?,
            config_write_enable: u16_at(bytes, 2)?,
            poll_interval: u32_at(bytes, 4)?,
            vector: u32_at(bytes, 8)?,
            switch_to_polling_threshold_value: u32_at(bytes, 12)?,
            switch_to_polling_threshold_window: u32_at(bytes, 16)?,
            error_threshold_value: u32_at(bytes, 20)?,
            error_threshold_window: u32_at(bytes, 24)?,
        })
    }
}

/// Which machine check structure a [`MachineCheck`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineCheckKind {
    /// IA-32 Architecture Machine Check Exception (type 0).
    Exception,
    /// IA-32 Architecture Corrected Machine Check (type 1).
    Corrected,
    /// IA-32 Architecture Deferred Machine Check (type 11).
    Deferred,
}

/// An IA-32 machine check source: its flags and the banks that follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineCheck<'a> {
    /// Which structure type it is.
    pub kind: MachineCheckKind,
    /// The source's identifier.
    pub source_id: u16,
    /// The flags: FIRMWARE_FIRST (bit 0) and GHES_ASSIST (bit 2).
    pub flags: u8,
    /// Whether the source is enabled.
    pub enabled: bool,
    /// Error records to pre-allocate for it.
    pub records_to_preallocate: u32,
    /// The most sections one record may hold.
    pub max_sections_per_record: u32,
    /// How firmware notifies the operating system (types 1 and 11 only).
    pub notification: Option<Notification>,
    /// How many bank structures follow.
    pub bank_count: u8,
    banks: &'a [u8],
}

impl<'a> MachineCheck<'a> {
    /// The firmware owns the banks and handles their errors first.
    pub fn firmware_first(&self) -> bool {
        self.flags & FLAG_FIRMWARE_FIRST != 0
    }

    /// The firmware assists the operating system's handling.
    pub fn ghes_assist(&self) -> bool {
        self.flags & FLAG_GHES_ASSIST != 0
    }

    /// The numbers of the machine check banks the structure lists.
    pub fn bank_numbers(&self) -> impl Iterator<Item = u8> + 'a {
        self.banks.as_chunks::<BANK_LEN>().0.iter().map(|bank| bank[0])
    }
}

/// An IA-32 NMI source (type 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nmi {
    /// The source's identifier.
    pub source_id: u16,
    /// Error records to pre-allocate for it.
    pub records_to_preallocate: u32,
    /// The most sections one record may hold.
    pub max_sections_per_record: u32,
    /// The longest raw data a record may carry, in bytes.
    pub max_raw_data_length: u32,
}

/// Which PCI Express structure an [`Aer`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AerKind {
    /// PCI Express Root Port AER (type 6).
    RootPort,
    /// PCI Express Device AER (type 7).
    Device,
    /// PCI Express Bridge AER (type 8).
    Bridge,
}

/// The secondary-bus fields a bridge adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecondaryAer {
    /// Secondary Uncorrectable Error Mask.
    pub uncorrectable_mask: u32,
    /// Secondary Uncorrectable Error Severity.
    pub uncorrectable_severity: u32,
    /// Secondary Advanced Error Capabilities and Control.
    pub capabilities_control: u32,
}

/// A PCI Express AER source: the function it applies to and the AER
/// register values the firmware wants set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aer {
    /// Which structure type it is.
    pub kind: AerKind,
    /// The source's identifier.
    pub source_id: u16,
    /// The flags: FIRMWARE_FIRST (bit 0) and GLOBAL (bit 1).
    pub flags: u8,
    /// Whether the source is enabled.
    pub enabled: bool,
    /// Error records to pre-allocate for it.
    pub records_to_preallocate: u32,
    /// The most sections one record may hold.
    pub max_sections_per_record: u32,
    /// The Bus field: bits 7:0 the bus number, bits 23:8 the segment.
    pub bus: u32,
    /// The device number.
    pub device: u16,
    /// The function number.
    pub function: u16,
    /// The Device Control register value to set.
    pub device_control: u16,
    /// The Uncorrectable Error Mask value to set.
    pub uncorrectable_mask: u32,
    /// The Uncorrectable Error Severity value to set.
    pub uncorrectable_severity: u32,
    /// The Correctable Error Mask value to set.
    pub correctable_mask: u32,
    /// The Advanced Error Capabilities and Control value to set.
    pub capabilities_control: u32,
    /// The Root Error Command value to set (root ports only).
    pub root_error_command: Option<u32>,
    /// The secondary-bus fields (bridges only).
    pub secondary: Option<SecondaryAer>,
}

impl Aer {
    /// The firmware handles this source's errors first.
    pub fn firmware_first(&self) -> bool {
        self.flags & FLAG_FIRMWARE_FIRST != 0
    }

    /// The structure applies to every function, not only the one it names;
    /// its bus, device and function are then to be ignored.
    pub fn global(&self) -> bool {
        self.flags & FLAG_GLOBAL != 0
    }

    /// The PCI bus number the structure names.
    pub fn bus_number(&self) -> u8 {
        (self.bus & 0xff) as u8
    }

    /// The PCI segment the structure names.
    pub fn segment(&self) -> u16 {
        ((self.bus >> 8) & 0xffff) as u16
    }
}

/// The Read Ack Register of a GHESv2 and the masks that go with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadAck {
    /// The register the operating system writes to say it has read the
    /// error status block.
    pub register: GenericAddress,
    /// The mask the register's value is ANDed with before the write.
    pub preserve: u64,
    /// The value ORed in before the write.
    pub write: u64,
}

/// A Generic Hardware Error Source (type 9) or its version 2 (type 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ghes {
    /// The source's identifier.
    pub source_id: u16,
    /// The source this one is related to, or 0xffff for none.
    pub related_source_id: u16,
    /// The flags (all reserved in the specification's text for this type,
    /// kept as given).
    pub flags: u8,
    /// Whether the source is enabled.
    pub enabled: bool,
    /// Error records to pre-allocate for it.
    pub records_to_preallocate: u32,
    /// The most sections one record may hold.
    pub max_sections_per_record: u32,
    /// The longest raw data a record may carry, in bytes.
    pub max_raw_data_length: u32,
    /// The register that holds the physical address of the error status
    /// block.
    pub error_status_address: GenericAddress,
    /// How firmware notifies the operating system.
    pub notification: Notification,
    /// The length of the error status block in bytes.
    pub error_status_block_length: u32,
    /// The read acknowledgement register (version 2 only).
    pub read_ack: Option<ReadAck>,
}

/// One error source structure of the HEST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorSource<'a> {
    /// Machine check exception, corrected machine check or deferred
    /// machine check (types 0, 1, 11).
    MachineCheck(MachineCheck<'a>),
    /// NMI (type 2).
    Nmi(Nmi),
    /// PCI Express AER (types 6, 7, 8).
    Aer(Aer),
    /// GHES or GHESv2 (types 9, 10).
    Ghes(Ghes),
}

/// The parsed HEST; [`Hest::sources`] walks its structures.
#[derive(Clone, Copy, Debug)]
pub struct Hest<'a> {
    /// How many error source structures the table says it holds.
    pub source_count: u32,
    structures: &'a [u8],
}

impl<'a> Hest<'a> {
    /// Parses a `HEST` table's count; the structures are read by
    /// [`Hest::sources`].
    pub fn parse(sdt: &Sdt<'a>) -> Result<Hest<'a>, Error> {
        if &sdt.signature != b"HEST" {
            return Err(Error::BadSignature);
        }
        let body = sdt.body();
        Ok(Hest {
            source_count: u32_at(body, 0).ok_or(Error::TooShort)?,
            structures: body.get(4..).ok_or(Error::TooShort)?,
        })
    }

    /// The error sources in table order. A structure that cannot be read
    /// is the last item, an error; the sources before it are good.
    pub fn sources(&self) -> Sources<'a> {
        Sources { rest: self.structures, remaining: self.source_count }
    }
}

/// The walk of a HEST's structures.
#[derive(Clone, Debug)]
pub struct Sources<'a> {
    rest: &'a [u8],
    remaining: u32,
}

impl<'a> Iterator for Sources<'a> {
    type Item = Result<ErrorSource<'a>, HestError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let item = next_source(self.rest);
        match item {
            Ok((_, rest)) => self.rest = rest,
            Err(_) => self.remaining = 0,
        }
        Some(item.map(|(source, _)| source))
    }
}

/// Reads the structure at the start of `bytes` and returns it with the
/// bytes after it.
fn next_source(bytes: &[u8]) -> Result<(ErrorSource<'_>, &[u8]), HestError> {
    let kind = u16_at(bytes, 0).ok_or(HestError::Missing)?;
    let length = structure_length(kind, bytes)?;
    let (this, rest) = bytes.split_at_checked(length).ok_or(HestError::Truncated(kind))?;
    let source = parse_source(kind, this).ok_or(HestError::Truncated(kind))?;
    Ok((source, rest))
}

/// The length of a structure of `kind` that starts `bytes`.
fn structure_length(kind: u16, bytes: &[u8]) -> Result<usize, HestError> {
    // The bank count is a byte of the fixed part; a structure too short
    // to hold it is truncated.
    let banks = |offset: usize, fixed: usize| {
        let count = *bytes.get(offset).ok_or(HestError::Truncated(kind))?;
        Ok(fixed + usize::from(count) * BANK_LEN)
    };
    match kind {
        // IA-32 machine check exception: bank count at byte 32, banks
        // from byte 40 (ACPI 6.5, table "IA-32 Architecture Machine Check
        // Exception Structure").
        0 => banks(32, 40),
        // Corrected and deferred machine check: bank count at byte 44
        // after the notification structure, banks from byte 48.
        1 | 11 => banks(44, 48),
        2 => Ok(20),
        6 => Ok(48),
        7 => Ok(44),
        8 => Ok(56),
        9 => Ok(64),
        10 => Ok(92),
        _ => Err(HestError::UnknownType(kind)),
    }
}

/// Decodes a structure whose length has been checked.
fn parse_source(kind: u16, b: &[u8]) -> Option<ErrorSource<'_>> {
    match kind {
        0 | 1 | 11 => parse_machine_check(kind, b).map(ErrorSource::MachineCheck),
        2 => Some(ErrorSource::Nmi(Nmi {
            // Type 2: source id at 2, 2 reserved bytes, then the three
            // counts.
            source_id: u16_at(b, 2)?,
            records_to_preallocate: u32_at(b, 8)?,
            max_sections_per_record: u32_at(b, 12)?,
            max_raw_data_length: u32_at(b, 16)?,
        })),
        6..=8 => parse_aer(kind, b).map(ErrorSource::Aer),
        9 | 10 => parse_ghes(kind, b).map(ErrorSource::Ghes),
        _ => None,
    }
}

fn parse_machine_check(kind: u16, b: &[u8]) -> Option<MachineCheck<'_>> {
    // All three: type, source id, 2 reserved bytes, flags at 6, enabled
    // at 7, records at 8, sections at 12.
    let (kind, notification, bank_count_at, banks_at) = match kind {
        0 => (MachineCheckKind::Exception, None, 32, 40),
        1 => (
            MachineCheckKind::Corrected,
            Notification::parse(b.get(16..16 + NOTIFICATION_LEN)?),
            44,
            48,
        ),
        _ => (
            MachineCheckKind::Deferred,
            Notification::parse(b.get(16..16 + NOTIFICATION_LEN)?),
            44,
            48,
        ),
    };
    Some(MachineCheck {
        kind,
        source_id: u16_at(b, 2)?,
        flags: *b.get(6)?,
        enabled: *b.get(7)? != 0,
        records_to_preallocate: u32_at(b, 8)?,
        max_sections_per_record: u32_at(b, 12)?,
        notification,
        bank_count: *b.get(bank_count_at)?,
        banks: b.get(banks_at..)?,
    })
}

fn parse_aer(kind: u16, b: &[u8]) -> Option<Aer> {
    // Common to types 6, 7 and 8 (ACPI 6.5, "PCI Express AER Structure"
    // tables): flags at 6, enabled at 7, records at 8, sections at 12,
    // bus at 16, device at 20, function at 22, device control at 24, then
    // after 2 reserved bytes the four AER values from 28.
    let (kind, root_error_command, secondary) = match kind {
        6 => (AerKind::RootPort, Some(u32_at(b, 44)?), None),
        7 => (AerKind::Device, None, None),
        _ => (
            AerKind::Bridge,
            None,
            Some(SecondaryAer {
                uncorrectable_mask: u32_at(b, 44)?,
                uncorrectable_severity: u32_at(b, 48)?,
                capabilities_control: u32_at(b, 52)?,
            }),
        ),
    };
    Some(Aer {
        kind,
        source_id: u16_at(b, 2)?,
        flags: *b.get(6)?,
        enabled: *b.get(7)? != 0,
        records_to_preallocate: u32_at(b, 8)?,
        max_sections_per_record: u32_at(b, 12)?,
        bus: u32_at(b, 16)?,
        device: u16_at(b, 20)?,
        function: u16_at(b, 22)?,
        device_control: u16_at(b, 24)?,
        uncorrectable_mask: u32_at(b, 28)?,
        uncorrectable_severity: u32_at(b, 32)?,
        correctable_mask: u32_at(b, 36)?,
        capabilities_control: u32_at(b, 40)?,
        root_error_command,
        secondary,
    })
}

fn parse_ghes(kind: u16, b: &[u8]) -> Option<Ghes> {
    // ACPI 6.5, "Generic Hardware Error Source Structure": source id at 2,
    // related source id at 4, flags at 6, enabled at 7, records at 8,
    // sections at 12, raw data length at 16, error status address (12
    // bytes) at 20, notification (28 bytes) at 32, block length at 60. The
    // version 2 structure adds the read ack register at 64 and its two
    // masks at 76 and 84.
    let read_ack = if kind == 10 {
        Some(ReadAck {
            register: GenericAddress::parse(b.get(64..76)?)?,
            preserve: u64_at(b, 76)?,
            write: u64_at(b, 84)?,
        })
    } else {
        None
    };
    Some(Ghes {
        source_id: u16_at(b, 2)?,
        related_source_id: u16_at(b, 4)?,
        flags: *b.get(6)?,
        enabled: *b.get(7)? != 0,
        records_to_preallocate: u32_at(b, 8)?,
        max_sections_per_record: u32_at(b, 12)?,
        max_raw_data_length: u32_at(b, 16)?,
        error_status_address: GenericAddress::parse(b.get(20..32)?)?,
        notification: Notification::parse(b.get(32..32 + NOTIFICATION_LEN)?)?,
        error_status_block_length: u32_at(b, 60)?,
        read_ack,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::table;

    fn put16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// The structure header every type shares: type, source id, flags at
    /// 6 and enabled at 7.
    fn structure(kind: u16, length: usize, source_id: u16, flags: u8) -> Vec<u8> {
        let mut s = vec![0u8; length];
        put16(&mut s, 0, kind);
        put16(&mut s, 2, source_id);
        s[6] = flags;
        s[7] = 1;
        put32(&mut s, 8, 4);
        put32(&mut s, 12, 2);
        s
    }

    /// An IA-32 machine check bank structure (28 bytes): the bank number
    /// is its first byte.
    fn bank(number: u8) -> Vec<u8> {
        let mut b = vec![0u8; BANK_LEN];
        b[0] = number;
        b[1] = 1;
        b
    }

    /// The Notification Structure, 28 bytes.
    fn notification(kind: u8, poll_interval: u32, vector: u32) -> Vec<u8> {
        let mut n = vec![0u8; NOTIFICATION_LEN];
        n[0] = kind;
        n[1] = 28;
        put16(&mut n, 2, 0x3f);
        put32(&mut n, 4, poll_interval);
        put32(&mut n, 8, vector);
        put32(&mut n, 12, 5);
        put32(&mut n, 16, 1000);
        put32(&mut n, 20, 10);
        put32(&mut n, 24, 60_000);
        n
    }

    /// Type 0: banks counted at 32, 40 bytes plus 28 for each.
    fn machine_check_exception(banks: &[u8]) -> Vec<u8> {
        let mut s = structure(0, 40, 1, 1);
        put64(&mut s, 16, 0x0106);
        put64(&mut s, 24, 0xff);
        s[32] = banks.len() as u8;
        for &number in banks {
            s.extend_from_slice(&bank(number));
        }
        s
    }

    /// Types 1 and 11: notification at 16, bank count at 44, 48 bytes plus
    /// 28 for each bank.
    fn machine_check_with_notification(kind: u16, banks: &[u8]) -> Vec<u8> {
        let mut s = structure(kind, 48, 2, 1 | 4);
        s[16..44].copy_from_slice(&notification(Notification::CMCI, 0, 0));
        s[44] = banks.len() as u8;
        for &number in banks {
            s.extend_from_slice(&bank(number));
        }
        s
    }

    /// Type 2: 20 bytes, raw data length at 16.
    fn nmi() -> Vec<u8> {
        let mut s = structure(2, 20, 3, 0);
        put32(&mut s, 16, 4096);
        s
    }

    /// Types 6, 7 and 8 with their common fields: bus at 16 (segment in
    /// bits 23:8), device at 20, function at 22, device control at 24,
    /// masks from 28.
    fn aer(kind: u16, length: usize) -> Vec<u8> {
        let mut s = structure(kind, length, 6, 1);
        put32(&mut s, 16, (3 << 8) | 5);
        put16(&mut s, 20, 3);
        put16(&mut s, 22, 1);
        put16(&mut s, 24, 0x0f);
        put32(&mut s, 28, 0x1111);
        put32(&mut s, 32, 0x2222);
        put32(&mut s, 36, 0x3333);
        put32(&mut s, 40, 0x4444);
        s
    }

    /// Type 9: status address at 20, notification at 32, block length at
    /// 60.
    fn ghes() -> Vec<u8> {
        let mut s = structure(9, 64, 7, 0);
        put16(&mut s, 4, 0xffff);
        put32(&mut s, 16, 0x1000);
        // Generic Address Structure: system memory, 64 bits, at 0x9000.
        s[20] = 0;
        s[21] = 64;
        s[23] = 4;
        put64(&mut s, 24, 0x9000);
        s[32..60].copy_from_slice(&notification(Notification::POLLED, 1000, 0));
        put32(&mut s, 60, 0x1000);
        s
    }

    /// Type 10: the type 9 layout plus the read ack register at 64 and
    /// its masks at 76 and 84.
    fn ghes_v2() -> Vec<u8> {
        let mut s = ghes();
        put16(&mut s, 0, 10);
        put16(&mut s, 2, 8);
        s[32..60].copy_from_slice(&notification(Notification::GSIV, 0, 33));
        s.resize(92, 0);
        s[64] = 0;
        s[65] = 64;
        s[67] = 4;
        put64(&mut s, 68, 0xa000);
        put64(&mut s, 76, 0xffff_ffff_ffff_fffe);
        put64(&mut s, 84, 1);
        s
    }

    fn hest(count: u32, structures: &[Vec<u8>]) -> Vec<u8> {
        let mut body = count.to_le_bytes().to_vec();
        for s in structures {
            body.extend_from_slice(s);
        }
        table(*b"HEST", 1, &body)
    }

    fn walk(bytes: &[u8]) -> Vec<Result<ErrorSource<'_>, HestError>> {
        let sdt = Sdt::parse(0, bytes).unwrap();
        // The sources borrow the table's bytes, not the Sdt value.
        let hest = Hest::parse(&sdt).unwrap();
        hest.sources().collect()
    }

    #[test]
    fn walks_one_structure_of_each_type() {
        let bytes = hest(
            9,
            &[
                machine_check_exception(&[0, 1, 4]),
                machine_check_with_notification(1, &[2, 3]),
                nmi(),
                aer(6, 48),
                aer(7, 44),
                aer(8, 56),
                ghes(),
                ghes_v2(),
                machine_check_with_notification(11, &[5]),
            ],
        );
        let sources = walk(&bytes);
        assert_eq!(sources.len(), 9);

        let Ok(ErrorSource::MachineCheck(mce)) = &sources[0] else { panic!("type 0") };
        assert_eq!(mce.kind, MachineCheckKind::Exception);
        assert_eq!(mce.source_id, 1);
        assert!(mce.firmware_first());
        assert!(!mce.ghes_assist());
        assert!(mce.enabled);
        assert_eq!(mce.bank_count, 3);
        assert_eq!(mce.bank_numbers().collect::<Vec<_>>(), [0, 1, 4]);
        assert_eq!(mce.notification, None);

        let Ok(ErrorSource::MachineCheck(cmc)) = &sources[1] else { panic!("type 1") };
        assert_eq!(cmc.kind, MachineCheckKind::Corrected);
        assert!(cmc.firmware_first() && cmc.ghes_assist());
        assert_eq!(cmc.bank_numbers().collect::<Vec<_>>(), [2, 3]);
        assert_eq!(cmc.notification.unwrap().kind, Notification::CMCI);
        assert_eq!(cmc.records_to_preallocate, 4);
        assert_eq!(cmc.max_sections_per_record, 2);

        let Ok(ErrorSource::Nmi(nmi)) = &sources[2] else { panic!("type 2") };
        assert_eq!(nmi.source_id, 3);
        assert_eq!(nmi.max_raw_data_length, 4096);

        let Ok(ErrorSource::Aer(root)) = &sources[3] else { panic!("type 6") };
        assert_eq!(root.kind, AerKind::RootPort);
        assert_eq!(root.root_error_command, Some(0));
        assert_eq!(root.secondary, None);
        assert!(root.firmware_first() && !root.global());

        let Ok(ErrorSource::Aer(device)) = &sources[4] else { panic!("type 7") };
        assert_eq!(device.kind, AerKind::Device);
        assert_eq!((device.segment(), device.bus_number()), (3, 5));
        assert_eq!((device.device, device.function), (3, 1));
        assert_eq!(device.device_control, 0x0f);
        assert_eq!(device.uncorrectable_mask, 0x1111);
        assert_eq!(device.uncorrectable_severity, 0x2222);
        assert_eq!(device.correctable_mask, 0x3333);
        assert_eq!(device.capabilities_control, 0x4444);

        let Ok(ErrorSource::Aer(bridge)) = &sources[5] else { panic!("type 8") };
        assert_eq!(bridge.kind, AerKind::Bridge);
        assert_eq!(
            bridge.secondary,
            Some(SecondaryAer {
                uncorrectable_mask: 0,
                uncorrectable_severity: 0,
                capabilities_control: 0,
            })
        );

        let Ok(ErrorSource::Ghes(plain)) = &sources[6] else { panic!("type 9") };
        assert_eq!(plain.source_id, 7);
        assert_eq!(plain.related_source_id, 0xffff);
        assert_eq!(plain.records_to_preallocate, 4);
        assert_eq!(plain.max_sections_per_record, 2);
        assert_eq!(plain.max_raw_data_length, 0x1000);
        assert_eq!(plain.error_status_address.address, 0x9000);
        assert_eq!(plain.error_status_address.bit_width, 64);
        assert_eq!(plain.notification.kind, Notification::POLLED);
        assert_eq!(plain.notification.poll_interval, 1000);
        assert_eq!(plain.notification.config_write_enable, 0x3f);
        assert_eq!(plain.notification.switch_to_polling_threshold_value, 5);
        assert_eq!(plain.notification.switch_to_polling_threshold_window, 1000);
        assert_eq!(plain.notification.error_threshold_value, 10);
        assert_eq!(plain.notification.error_threshold_window, 60_000);
        assert_eq!(plain.error_status_block_length, 0x1000);
        assert_eq!(plain.read_ack, None);

        let Ok(ErrorSource::Ghes(v2)) = &sources[7] else { panic!("type 10") };
        assert_eq!(v2.source_id, 8);
        assert_eq!(v2.notification.kind, Notification::GSIV);
        assert_eq!(v2.notification.vector, 33);
        let ack = v2.read_ack.unwrap();
        assert_eq!(ack.register.address, 0xa000);
        assert_eq!(ack.preserve, 0xffff_ffff_ffff_fffe);
        assert_eq!(ack.write, 1);

        let Ok(ErrorSource::MachineCheck(deferred)) = &sources[8] else { panic!("type 11") };
        assert_eq!(deferred.kind, MachineCheckKind::Deferred);
        assert_eq!(deferred.bank_numbers().collect::<Vec<_>>(), [5]);
    }

    #[test]
    fn an_unknown_type_ends_the_walk_after_what_came_before() {
        let bytes = hest(3, &[nmi(), vec![0x77, 0, 8, 0, 0, 0, 0, 0], nmi()]);
        let sources = walk(&bytes);
        assert_eq!(sources.len(), 2);
        assert!(matches!(sources[0], Ok(ErrorSource::Nmi(_))));
        assert_eq!(sources[1], Err(HestError::UnknownType(0x77)));
    }

    #[test]
    fn a_truncated_structure_ends_the_walk() {
        let mut ghes = ghes();
        ghes.truncate(40);
        let bytes = hest(2, &[nmi(), ghes]);
        let sources = walk(&bytes);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[1], Err(HestError::Truncated(9)));

        // A machine check whose banks do not fit.
        let mut mce = machine_check_exception(&[0, 1]);
        mce.truncate(40 + 28 + 10);
        let bytes = hest(1, &[mce]);
        assert_eq!(walk(&bytes), [Err(HestError::Truncated(0))]);

        // A machine check cut before its bank count.
        let mut short = machine_check_exception(&[]);
        short.truncate(20);
        let bytes = hest(1, &[short]);
        assert_eq!(walk(&bytes), [Err(HestError::Truncated(0))]);
    }

    #[test]
    fn a_count_larger_than_the_table_ends_the_walk() {
        let bytes = hest(3, &[nmi()]);
        let sources = walk(&bytes);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[1], Err(HestError::Missing));
    }

    #[test]
    fn an_empty_table_has_no_sources() {
        let bytes = hest(0, &[]);
        assert!(walk(&bytes).is_empty());
        let wrong = table(*b"BERT", 1, &[0; 8]);
        let sdt = Sdt::parse(0, &wrong).unwrap();
        assert_eq!(Hest::parse(&sdt).unwrap_err(), Error::BadSignature);
        let short = table(*b"HEST", 1, &[0; 2]);
        let sdt = Sdt::parse(0, &short).unwrap();
        assert_eq!(Hest::parse(&sdt).unwrap_err(), Error::TooShort);
    }

    #[test]
    fn errors_read_in_plain_words() {
        assert_eq!(HestError::UnknownType(7).to_string(), "unknown error source type 7");
        assert_eq!(
            HestError::Truncated(9).to_string(),
            "error source of type 9 runs past the end of the table"
        );
        assert_eq!(HestError::Missing.to_string(), "fewer error sources than the table counts");
    }
}
