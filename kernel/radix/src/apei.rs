// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The hardware errors the firmware reports (ACPI's APEI, ADR 0662).
//!
//! A server's firmware often handles its memory and PCIe errors first,
//! decodes them with what it knows of the board, and leaves a record for
//! the operating system in a Generic Error Status Block, one block for
//! each of the error sources its HEST lists (GHES). This module reads
//! them:
//!
//! - At start, the BERT: the errors that ended the last boot, each logged
//!   as "Hardware error before this boot" and recorded in `/kernel/ras`.
//! - The firmware is told the kernel reads its records (`_OSC`, as Linux
//!   asks it), and the machine-check banks the HEST says the firmware
//!   handles first are left to it ([`crate::mce::firmware_first`]), so
//!   their corrected errors are not taken from under it nor counted twice.
//! - Each enabled GHES is read by the `apei` thread: a polled one at its
//!   interval (100 ms at least), one the firmware signals by SCI when the
//!   hardware error device (`PNP0C33`) is notified, and one it signals by
//!   NMI from the copy [`on_nmi`] made. A source signalled any other way
//!   (an interrupt the kernel does not route yet) is read every second, as
//!   is an SCI source on a machine with no hardware error device.
//!
//! Reading a block: when its status is set, copy it (bounded by the length
//! the HEST gives it), write zero to its status, and for a version 2
//! source acknowledge it (`(read & preserve) | write`), so the firmware
//! can report the next error; then decode each entry, record and count it
//! ([`crate::ras`]), log it (a corrected one within the log's limit), and
//! take a page of memory with an error that was not corrected out of use
//! ([`radix_mm::Frames::retire`]), or one whose corrected errors passed
//! the firmware's threshold. A fatal error panics, with its first entry
//! decoded, as Linux does.
//!
//! The NMI handler takes no lock and allocates nothing: each NMI source
//! has a buffer allocated at start, and its state ([`IDLE`], [`BUSY`],
//! [`PENDING`]) decides who reads the block, so the handler and the thread
//! never read one block at once and the handler never waits.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use alloc::vec::Vec;

use radix_acpi::bert::Bert;
use radix_acpi::cper::{
    self, CperError, ErrorEntry, ErrorStatusBlock, MemoryError, PcieDeviceId, SECTION_MEMORY_2,
    Section,
};
use radix_acpi::hest::{ErrorSource, Ghes, Hest, MachineCheckKind, Notification};
use radix_acpi::{GenericAddress, Sdt};
use radix_hal::{Aml, Devices, Platform};
use radix_hw::Mmio;
use radix_klog::{TextBuf, error, info, warn};
use radix_mca::record::{Kind, Log, Severity, Source as Reporter};
use radix_mm::{FRAMES, Flags, HEAP, Kmalloc, Retired};
use radix_sync::Once;
use radix_uacpi::Node;

use crate::Arch;
use crate::dmi;
use crate::ras::{self, Report};
use crate::sched::{self, Options, Policy, ThreadRef};

/// The most bytes of a block read: the HEST's length past it is cut.
const MAX_BLOCK: usize = 64 * 1024;

/// The most bytes of the BERT's region read.
const MAX_BOOT_REGION: usize = 1024 * 1024;

/// The shortest interval a polled source is read at.
const MIN_POLL_NS: u64 = 100_000_000;

/// How often a source the kernel is not signalled for is read, and the
/// NMI handler's copies collected.
const FALLBACK_NS: u64 = 1_000_000_000;

/// The most bytes of a fatal error's record the NMI handler reads for its
/// panic: the header and the first entry of any section it decodes.
const FATAL_PREFIX: usize = 512;

/// `_OSC`'s UUID for the WHEA (APEI) capabilities,
/// ED855E0C-6C90-47BF-A62A-26DE0FC5AD5C, in the order of its bytes.
const WHEA: [u8; 16] = [
    0x0c, 0x5e, 0x85, 0xed, 0x90, 0x6c, 0xbf, 0x47, 0xa6, 0x2a, 0x26, 0xde, 0x0f, 0xc5, 0xad, 0x5c,
];

/// The platform-wide `_OSC`'s UUID, 0811B06E-4A27-44F9-8D60-3CBBC22E7B48.
const PLATFORM: [u8; 16] = [
    0x6e, 0xb0, 0x11, 0x08, 0x27, 0x4a, 0xf9, 0x44, 0x8d, 0x60, 0x3c, 0xbb, 0xc2, 0x2e, 0x7b, 0x48,
];

/// The platform-wide `_OSC`'s APEI support bit (ACPI 6.5, table 6.13).
const APEI_SUPPORT: u32 = 1 << 4;

/// `_OSC`'s first return dword: failure, unknown UUID, unknown revision
/// and capabilities masked (bits 1 to 4).
const OSC_ERRORS: u32 = 0x1e;

// A source's state, which says who may read its block.

/// No one reads the block, and the NMI copy is free.
const IDLE: u8 = 0;
/// The thread or the NMI handler reads the block.
const BUSY: u8 = 1;
/// The NMI handler copied a record the thread has not taken yet.
const PENDING: u8 = 2;

/// Whether the firmware's error sources are read.
static ON: AtomicBool = AtomicBool::new(false);
/// The GHES sources read.
static SOURCES: Once<Vec<Source, Kmalloc>> = Once::new();
/// The thread, for the hardware error device's notify to wake.
static THREAD: Once<ThreadRef> = Once::new();
/// The hardware error device was notified since the thread last looked.
static NOTIFIED: AtomicBool = AtomicBool::new(false);

/// Whether the firmware's error sources are read: the HEST lists one the
/// kernel can read.
pub fn on() -> bool {
    ON.load(Ordering::Acquire)
}

/// Maps `len` bytes of the firmware's memory at `phys` for the kernel's
/// life.
#[allow(unsafe_code)]
fn map(phys: u64, len: usize) -> Option<Mmio> {
    let virt = Arch::map_firmware(phys, len)?;
    // SAFETY: `map_firmware` mapped `phys..phys + len` readable and
    // writable at `virt`, and nothing unmaps it: it is the firmware's
    // error registers or records, which nothing else in the kernel uses
    // as memory. Whether it is cached only changes how fast it is read.
    Some(unsafe { Mmio::new(virt, len) })
}

/// A register a Generic Address Structure names: in memory or an I/O port.
struct Register {
    place: Place,
    /// The bytes of each access: 1, 2, 4 or 8.
    width: u8,
    /// The register's first bit within the access.
    shift: u8,
}

enum Place {
    Memory(Mmio),
    Port(u16),
}

impl Register {
    /// Maps the register `gas` names, or says why it cannot be read.
    fn map(gas: &GenericAddress) -> Result<Register, &'static str> {
        // ACPI 6.5, "Generic Address Structure": access size 1 byte to 4
        // qword, or 0 for one the register's width gives.
        let width: u8 = match gas.access_size {
            1..=4 => 1 << (gas.access_size - 1),
            0 => match gas.bit_width {
                8 => 1,
                16 => 2,
                32 => 4,
                64 => 8,
                _ => return Err("a register of no access size"),
            },
            _ => return Err("an access size ACPI does not have"),
        };
        let bits = u32::from(width) * 8;
        if u32::from(gas.bit_offset) >= bits
            || u32::from(gas.bit_offset) + u32::from(gas.bit_width) > bits
        {
            return Err("a register wider than its access");
        }
        let place = match gas.space {
            GenericAddress::MEMORY => {
                if !gas.address.is_multiple_of(u64::from(width)) {
                    return Err("a register at an unaligned address");
                }
                Place::Memory(map(gas.address, usize::from(width)).ok_or("unmappable memory")?)
            }
            GenericAddress::IO if width <= 4 => {
                Place::Port(u16::try_from(gas.address).map_err(|_| "a port past 0xffff")?)
            }
            GenericAddress::IO => return Err("a 64-bit port"),
            _ => return Err("an address space other than memory and ports"),
        };
        Ok(Register { place, width, shift: gas.bit_offset })
    }

    /// Reads the whole access.
    fn read(&self) -> u64 {
        match &self.place {
            Place::Memory(mmio) => match self.width {
                1 => u64::from(mmio.read8(0)),
                2 => u64::from(mmio.read16(0)),
                4 => u64::from(mmio.read32(0)),
                _ => mmio.read64(0),
            },
            Place::Port(port) => u64::from(Arch::read_port(*port, self.width)),
        }
    }

    /// Writes the whole access; bits past its width are dropped.
    fn write(&self, value: u64) {
        match &self.place {
            Place::Memory(mmio) => match self.width {
                1 => mmio.write8(0, value as u8),
                2 => mmio.write16(0, value as u16),
                4 => mmio.write32(0, value as u32),
                _ => mmio.write64(0, value),
            },
            Place::Port(port) => Arch::write_port(*port, self.width, value as u32),
        }
    }
}

/// A GHESv2's Read Ack Register and its masks.
struct Ack {
    register: Register,
    preserve: u64,
    write: u64,
}

impl Ack {
    /// Tells the firmware its block was read: ACPI 6.5 18.3.2.8,
    /// `(read & preserve) | write`, both masks at the register's bits.
    fn acknowledge(&self) {
        let shift = self.register.shift;
        let value = self.register.read();
        self.register.write((value & (self.preserve << shift)) | (self.write << shift));
    }
}

/// The firmware's memory where a record is: a GHES block or the BERT's
/// region, mapped for the kernel's life.
struct Region {
    mmio: Mmio,
    /// Its physical address, by which the firmware names it.
    phys: u64,
}

impl Region {
    fn map(phys: u64, len: usize) -> Option<Region> {
        Some(Region { mmio: map(phys, len)?, phys })
    }

    /// The Block Status of the block at `offset`.
    fn status(&self, offset: usize) -> u32 {
        let mut bytes = [0; 4];
        self.copy(offset, &mut bytes);
        u32::from_le_bytes(bytes)
    }

    /// Copies `out.len()` bytes from `offset`, eight at a time where they
    /// are aligned. The mapping keeps the address's place within its page,
    /// so `phys` says the alignment.
    fn copy(&self, offset: usize, out: &mut [u8]) {
        let mut at = 0;
        while at < out.len() {
            let here = offset + at;
            let aligned = (self.phys as usize).wrapping_add(here).is_multiple_of(8);
            if aligned && out.len() - at >= 8 {
                out[at..at + 8].copy_from_slice(&self.mmio.read64(here).to_le_bytes());
                at += 8;
            } else {
                out[at] = self.mmio.read8(here);
                at += 1;
            }
        }
    }

    /// The bytes the block at `offset` takes, as its header says, at most
    /// `most`; `None` when the header says nothing that can be read.
    fn block_len(&self, offset: usize, most: usize) -> Option<usize> {
        let mut header = [0; cper::BLOCK_HEADER_LEN];
        self.copy(offset, &mut header);
        Some(cper::block_len(&header)?.min(most))
    }

    /// Writes zero to the Block Status of the block at `offset`: the block
    /// is free for the firmware's next record.
    fn clear(&self, offset: usize) {
        if (self.phys as usize).wrapping_add(offset).is_multiple_of(4) {
            self.mmio.write32(offset, 0);
        } else {
            for byte in 0..4 {
                self.mmio.write8(offset + byte, 0);
            }
        }
    }
}

/// How the firmware tells the kernel a source has a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum How {
    Polled,
    Sci,
    Nmi,
    /// Another notification, which the kernel does not route yet.
    Other(u8),
}

/// One GHES the kernel reads.
struct Source {
    id: u16,
    how: How,
    /// Nanoseconds between reads; 0 for a source read only when the
    /// firmware says.
    every_ns: u64,
    /// When it is read next; only the thread uses it.
    next_ns: AtomicU64,
    /// The register that holds the block's physical address.
    address: Register,
    ack: Option<Ack>,
    /// The bytes of the block read, at most [`MAX_BLOCK`].
    len: usize,
    /// The block, mapped by the thread when the address register first
    /// names one.
    block: Once<Region>,
    /// Who reads the block: [`IDLE`], [`BUSY`] or [`PENDING`].
    state: AtomicU8,
    /// An NMI source's copy of its last record, eight bytes a word, and
    /// its length; empty for the others.
    copy: Vec<AtomicU64, Kmalloc>,
    copied: AtomicUsize,
    /// The block moved, or could not be mapped, and the log said so.
    said: AtomicBool,
}

impl Source {
    /// The source `ghes` describes, or why it cannot be read.
    fn new(ghes: &Ghes, sci_heard: bool, heap: Kmalloc) -> Result<Source, &'static str> {
        let given = ghes.error_status_block_length as usize;
        if given < cper::BLOCK_HEADER_LEN {
            return Err("its error status block is shorter than a header");
        }
        let len = given.min(MAX_BLOCK);
        let address = Register::map(&ghes.error_status_address)?;
        let ack = match &ghes.read_ack {
            Some(read_ack) => Some(Ack {
                register: Register::map(&read_ack.register)?,
                preserve: read_ack.preserve,
                write: read_ack.write,
            }),
            None => None,
        };
        let notification = &ghes.notification;
        let (how, every_ns) = match notification.kind {
            Notification::POLLED => {
                let ms = u64::from(notification.poll_interval);
                let ns = if ms == 0 { FALLBACK_NS } else { (ms * 1_000_000).max(MIN_POLL_NS) };
                (How::Polled, ns)
            }
            Notification::SCI => (How::Sci, if sci_heard { 0 } else { FALLBACK_NS }),
            Notification::NMI => (How::Nmi, FALLBACK_NS),
            other => (How::Other(other), FALLBACK_NS),
        };
        let mut copy = Vec::new_in(heap);
        if how == How::Nmi {
            let words = len.div_ceil(8);
            copy.try_reserve_exact(words).map_err(|_| "no memory for its copy")?;
            for _ in 0..words {
                let _ = copy.push_within_capacity(AtomicU64::new(0));
            }
        }
        Ok(Source {
            id: ghes.source_id,
            how,
            every_ns,
            next_ns: AtomicU64::new(0),
            address,
            ack,
            len,
            block: Once::new(),
            state: AtomicU8::new(IDLE),
            copy,
            copied: AtomicUsize::new(0),
            said: AtomicBool::new(false),
        })
    }

    /// The block the address register names now, mapped; `None` when it
    /// names none (no record), or one that cannot be read. The thread's
    /// alone: it maps.
    fn block(&self) -> Option<&Region> {
        let phys = self.address.read();
        if phys == 0 {
            return None;
        }
        if self.block.get().is_none() {
            let Some(region) = Region::map(phys, self.len) else {
                if !self.said.swap(true, Ordering::Relaxed) {
                    warn!("APEI: error source {}'s block at {phys:#x} cannot be mapped", self.id);
                }
                return None;
            };
            // Only this thread sets it.
            let _ = self.block.set(region);
        }
        let block = self.block.get()?;
        if block.phys != phys {
            if !self.said.swap(true, Ordering::Relaxed) {
                warn!(
                    "APEI: error source {} moved its block from {:#x} to {phys:#x}; its records \
                     there are not read",
                    self.id, block.phys
                );
            }
            return None;
        }
        Some(block)
    }

    /// Takes the block's record into `out` and frees the block for the
    /// next: copies it, clears its status, acknowledges it. Returns the
    /// record's length, or `None` when the block holds none or the NMI
    /// handler is reading it.
    fn take(&self, block: &Region, out: &mut [u8]) -> Option<usize> {
        if self.state.compare_exchange(IDLE, BUSY, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return None;
        }
        let taken = if block.status(0) == 0 {
            None
        } else {
            // A header whose lengths overflow is read alone: the record
            // then says it cannot be read.
            let len = block.block_len(0, self.len.min(out.len())).unwrap_or(cper::BLOCK_HEADER_LEN);
            block.copy(0, &mut out[..len]);
            block.clear(0);
            if let Some(ack) = &self.ack {
                ack.acknowledge();
            }
            Some(len)
        };
        self.state.store(IDLE, Ordering::Release);
        taken
    }

    /// The thread: reads the NMI handler's copy, if it left one, and then
    /// the block.
    fn read(&self, scratch: &mut [u8]) {
        if self.state.load(Ordering::Acquire) == PENDING {
            let len = self.copied.load(Ordering::Relaxed).min(scratch.len());
            for (chunk, word) in scratch[..len].chunks_mut(8).zip(&self.copy) {
                chunk.copy_from_slice(&word.load(Ordering::Relaxed).to_le_bytes()[..chunk.len()]);
            }
            self.state.store(IDLE, Ordering::Release);
            report(Origin::Firmware(self.id), &scratch[..len]);
        }
        let Some(block) = self.block() else { return };
        if let Some(len) = self.take(block, scratch) {
            report(Origin::Firmware(self.id), &scratch[..len]);
        }
    }

    /// In the NMI handler: if the block holds a record, copies it for the
    /// thread and frees the block, or panics if it is fatal. Returns
    /// whether the block held one.
    fn read_in_nmi(&self) -> bool {
        let Some(block) = self.block.get() else { return false };
        if self.address.read() != block.phys || block.status(0) == 0 {
            return false;
        }
        if self.state.compare_exchange(IDLE, BUSY, Ordering::Acquire, Ordering::Relaxed).is_err() {
            // The thread is reading the block, or has not taken the last
            // copy: it reads this record soon.
            return true;
        }
        let len = block.block_len(0, self.len).unwrap_or(cper::BLOCK_HEADER_LEN);
        let mut start = [0; FATAL_PREFIX];
        let prefix = len.min(FATAL_PREFIX);
        block.copy(0, &mut start[..prefix]);
        let severity =
            cper::Severity(u32::from_le_bytes([start[16], start[17], start[18], start[19]]));
        if severity == cper::Severity::FATAL {
            block.clear(0);
            if let Some(ack) = &self.ack {
                ack.acknowledge();
            }
            fatal(Origin::Firmware(self.id), &start[..prefix]);
        }
        let mut at = 0;
        for word in &self.copy {
            if at >= len {
                break;
            }
            let mut bytes = [0; 8];
            let n = (len - at).min(8);
            block.copy(at, &mut bytes[..n]);
            word.store(u64::from_le_bytes(bytes), Ordering::Relaxed);
            at += 8;
        }
        self.copied.store(len.min(self.copy.len() * 8), Ordering::Relaxed);
        block.clear(0);
        if let Some(ack) = &self.ack {
            ack.acknowledge();
        }
        self.state.store(PENDING, Ordering::Release);
        true
    }
}

/// The NMI handler's part ([`radix_hal::Platform::set_nmi_handler`]):
/// reads the NMI sources' blocks. Takes no lock and allocates nothing.
fn on_nmi() -> bool {
    let Some(sources) = SOURCES.get() else { return false };
    let mut ours = false;
    for source in sources.iter().filter(|source| source.how == How::Nmi) {
        ours |= source.read_in_nmi();
    }
    ours
}

/// The hardware error device's notify, in the notify thread.
fn notified(_: Node, _: u64) {
    NOTIFIED.store(true, Ordering::Release);
    if let Some(thread) = THREAD.get() {
        sched::wake(thread);
    }
}

/// Where a record came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// A GHES, by its id.
    Firmware(u16),
    /// The BERT: before this boot.
    LastBoot,
}

impl Origin {
    fn reporter(self) -> Reporter {
        match self {
            Origin::Firmware(id) => Reporter::Firmware(id),
            Origin::LastBoot => Reporter::LastBoot,
        }
    }

    /// What the log's limit counts its errors by when they name no part.
    fn key(self) -> u64 {
        match self {
            Origin::Firmware(id) => ras::KEY_FIRMWARE | u64::from(id),
            Origin::LastBoot => ras::KEY_FIRMWARE | 0x1_0000,
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Firmware(id) => write!(f, "the firmware's error source {id}"),
            Origin::LastBoot => f.write_str("the firmware's boot error record"),
        }
    }
}

/// The record's severity, in the record's terms; one UEFI does not name
/// is taken as not corrected.
fn severity_of(severity: cper::Severity) -> Severity {
    match severity {
        cper::Severity::CORRECTED => Severity::Corrected,
        cper::Severity::INFORMATIONAL => Severity::Informational,
        cper::Severity::FATAL => Severity::Fatal,
        _ => Severity::Recoverable,
    }
}

/// A Generic Error Data Entry's FRU text, when it is printable.
fn fru_text(text: &[u8; 20]) -> Option<&str> {
    let end = text.iter().position(|&byte| byte == 0).unwrap_or(text.len());
    let text = &text[..end];
    if !text.iter().all(|byte| (0x20..0x7f).contains(byte)) {
        return None;
    }
    let text = core::str::from_utf8(text).ok()?.trim();
    (!text.is_empty()).then_some(text)
}

/// The part an error was in, by the name on the board where it is known.
enum Part<'a> {
    Memory(&'a MemoryError, Option<&'a str>),
    Pcie(Option<PcieDeviceId>),
    Processor(Option<u64>),
    Other(Option<&'a str>),
}

impl fmt::Display for Part<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Part::Memory(memory, fru) => {
                if let Some(handle) = memory.module_handle {
                    return match dmi::dimm_by_handle(handle).and_then(|dimm| dimm.locator) {
                        Some(locator) => write!(f, "{locator}"),
                        None => write!(f, "memory device {handle:#06x}"),
                    };
                }
                if let Some(fru) = fru {
                    return f.write_str(fru);
                }
                f.write_str("memory")?;
                for (name, value) in
                    [("node", memory.node), ("card", memory.card), ("module", memory.module)]
                {
                    if let Some(value) = value {
                        write!(f, " {name} {value}")?;
                    }
                }
                Ok(())
            }
            Part::Pcie(Some(id)) => write!(
                f,
                "PCIe {:04x}:{:02x}:{:02x}.{}",
                id.segment, id.primary_bus, id.device, id.function
            ),
            Part::Pcie(None) => f.write_str("a PCIe device"),
            Part::Processor(Some(apic)) => write!(f, "the processor of APIC ID {apic}"),
            Part::Processor(None) => f.write_str("a processor"),
            Part::Other(Some(fru)) => f.write_str(fru),
            Part::Other(None) => f.write_str("the platform"),
        }
    }
}

/// What an entry said.
enum What<'a> {
    Section(&'a Section<'a>),
    Unreadable(CperError),
    Empty,
}

impl fmt::Display for What<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            What::Section(Section::Memory(memory)) => write!(f, "{memory}"),
            What::Section(Section::Pcie(pcie)) => write!(f, "{pcie}"),
            What::Section(Section::Processor(processor)) => write!(f, "{processor}"),
            What::Section(Section::ProcessorX86(processor)) => write!(f, "{processor}"),
            What::Section(Section::Unknown { guid, data, .. }) if *guid == SECTION_MEMORY_2 => {
                write!(
                    f,
                    "memory error, in the second section format ({} bytes, not decoded)",
                    data.len()
                )
            }
            What::Section(Section::Unknown { guid, data, .. }) => {
                write!(
                    f,
                    "an error in a section of type {guid} ({} bytes, not decoded)",
                    data.len()
                )
            }
            What::Unreadable(error) => write!(f, "a record that cannot be read: {error}"),
            What::Empty => f.write_str("a record with no entries"),
        }
    }
}

/// Writes what `entry` says: "{part}: {what}".
fn describe(out: &mut dyn Write, entry: &ErrorEntry<'_>) -> fmt::Result {
    let fru = entry.fru_text.as_ref().and_then(fru_text);
    match entry.section() {
        Ok(section) => {
            let part = part_of(&section, fru);
            write!(out, "{part}: {}", What::Section(&section))
        }
        Err(error) => write!(out, "{}: {}", Part::Other(fru), What::Unreadable(error)),
    }
}

/// The part a section names.
fn part_of<'a>(section: &'a Section<'a>, fru: Option<&'a str>) -> Part<'a> {
    match section {
        Section::Memory(memory) => Part::Memory(memory, fru),
        Section::Pcie(pcie) => Part::Pcie(pcie.device_id),
        Section::Processor(processor) => Part::Processor(processor.apic_id),
        Section::ProcessorX86(processor) => Part::Processor(processor.local_apic_id),
        Section::Unknown { .. } => Part::Other(fru),
    }
}

/// Panics for a fatal error, with what the first entry of `bytes`, the
/// start of its record, says. Takes no lock and allocates nothing.
fn fatal(origin: Origin, bytes: &[u8]) -> ! {
    let mut detail = TextBuf::<256>::new();
    let _ = match cper::first_entry(bytes) {
        Some(Ok(entry)) => describe(&mut detail, &entry),
        Some(Err(error)) => write!(detail, "{}", What::Unreadable(error)),
        None => write!(detail, "{}", What::Empty),
    };
    panic!("Hardware error: a fatal error, reported by {origin}: {}", detail.as_str());
}

/// Records, counts and logs the errors of the record `bytes`, takes the
/// memory pages they name out of use, and panics for a fatal one (but for
/// one from before this boot).
fn report(origin: Origin, bytes: &[u8]) {
    let header_severity = bytes.get(16..20).map_or(cper::Severity::RECOVERABLE, |b| {
        cper::Severity(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    });
    match ErrorStatusBlock::parse(bytes) {
        Ok(block) => {
            let mut any = false;
            for item in block.entries() {
                any = true;
                match item {
                    Ok(entry) => one(origin, &entry),
                    Err(error) => {
                        unreadable(origin, severity_of(block.severity), &What::Unreadable(error));
                    }
                }
            }
            if !any {
                unreadable(origin, severity_of(block.severity), &What::Empty);
            }
        }
        Err(error) => unreadable(origin, severity_of(header_severity), &What::Unreadable(error)),
    }
    if header_severity == cper::Severity::FATAL && origin != Origin::LastBoot {
        fatal(origin, bytes);
    }
}

/// Records and logs an entry that cannot be read, or a record with none.
fn unreadable(origin: Origin, severity: Severity, what: &What<'_>) {
    let part = Part::Other(None);
    let report = Report {
        source: origin.reporter(),
        severity,
        kind: Kind::Other,
        count: 1,
        dimm: None,
        part: &part,
        what,
        key: origin.key(),
    };
    let log = ras::record(&report);
    log_line(origin, severity, log, &part, what);
}

/// Records, counts and logs one entry, and retires the page it names.
fn one(origin: Origin, entry: &ErrorEntry<'_>) {
    let severity = severity_of(entry.severity);
    let fru = entry.fru_text.as_ref().and_then(fru_text);
    let section = match entry.section() {
        Ok(section) => section,
        Err(error) => {
            unreadable(origin, severity, &What::Unreadable(error));
            return;
        }
    };
    let part = part_of(&section, fru);
    let what = What::Section(&section);
    let (kind, dimm, key, page) = match &section {
        Section::Memory(memory) => (
            Kind::Memory,
            memory.module_handle,
            origin.key(),
            memory.physical_address.map(|address| address & !0xfff),
        ),
        Section::Unknown { guid, .. } if *guid == SECTION_MEMORY_2 => {
            (Kind::Memory, None, origin.key(), None)
        }
        Section::Pcie(pcie) => {
            let key = pcie.device_id.map_or(origin.key(), |id| {
                ras::KEY_PCIE
                    | u64::from(id.segment) << 16
                    | u64::from(id.primary_bus) << 8
                    | u64::from(id.device & 0x1f) << 3
                    | u64::from(id.function & 7)
            });
            (Kind::Pcie, None, key, None)
        }
        Section::Processor(_) | Section::ProcessorX86(_) => {
            (Kind::Processor, None, origin.key(), None)
        }
        Section::Unknown { .. } => (Kind::Other, None, origin.key(), None),
    };
    let report = Report {
        source: origin.reporter(),
        severity,
        kind,
        count: 1,
        dimm,
        part: &part,
        what: &what,
        key,
    };
    let log = ras::record(&report);
    log_line(origin, severity, log, &part, &what);
    // UEFI 2.10 N.2.2, the section descriptor's flags: bit 3, the error
    // threshold was exceeded.
    let threshold = entry.flags & (1 << 3) != 0;
    let retire = severity == Severity::Recoverable || (severity.corrected() && threshold);
    if let Some(page) = page.filter(|_| retire && origin != Origin::LastBoot) {
        retire_page(page);
    }
}

/// The error's line in the log: one from before this boot as a warning,
/// a corrected one as a warning within the log's limit, one for
/// information as such, and any other as an error.
fn log_line(origin: Origin, severity: Severity, log: Log, part: &Part<'_>, what: &What<'_>) {
    if origin == Origin::LastBoot {
        warn!("Hardware error before this boot: {severity}, {part}: {what}");
        return;
    }
    let Log::Line { left_out } = log else { return };
    if left_out > 0 {
        warn!(
            "Hardware error: {left_out} more errors of {part} in the last minute were left out \
             of the log (/kernel/ras/errors has them)"
        );
    }
    match severity {
        Severity::Informational => {
            info!("Hardware error: {severity}, from {origin}, {part}: {what}");
        }
        Severity::Corrected => warn!("Hardware error: {severity}, from {origin}, {part}: {what}"),
        Severity::Recoverable | Severity::Fatal => {
            error!("Hardware error: {severity}, from {origin}, {part}: {what}");
        }
    }
}

/// Takes the page of memory at `page`, which an error names, out of use.
fn retire_page(page: u64) {
    match FRAMES.retire(page) {
        Retired::Now => warn!("Hardware error: the page at {page:#x} is out of use for good"),
        Retired::WhenFreed => {
            warn!("Hardware error: the page at {page:#x} leaves use for good once freed");
        }
        Retired::NotManaged => {}
        Retired::TooMany => {
            error!("Hardware error: the page at {page:#x} cannot be retired: too many are");
        }
    }
}

/// Reads the BERT's region: the errors that ended the last boot.
fn read_boot_errors() {
    let Some(table) = Arch::firmware_table(b"BERT") else { return };
    let bert = match Sdt::parse(0, table).and_then(|sdt| Bert::parse(&sdt)) {
        Ok(bert) => bert,
        Err(err) => {
            warn!("APEI: the BERT cannot be read: {err:?}");
            return;
        }
    };
    let len = bert.region_length as usize;
    if len < cper::BLOCK_HEADER_LEN || bert.region_address == 0 {
        return;
    }
    if len > MAX_BOOT_REGION {
        warn!("APEI: the boot error region is {len} bytes; the first {MAX_BOOT_REGION} are read");
    }
    let len = len.min(MAX_BOOT_REGION);
    let Some(region) = Region::map(bert.region_address, len) else {
        warn!("APEI: the boot error region at {:#x} cannot be mapped", bert.region_address);
        return;
    };
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut offset = 0;
    let mut records = 0;
    // As Linux reads it: block after block, until one with no status or
    // one that does not fit.
    while len - offset >= cper::BLOCK_HEADER_LEN && records < cper::MAX_ENTRIES {
        if region.status(offset) == 0 {
            break;
        }
        let Some(block_len) = region.block_len(offset, usize::MAX) else { break };
        if block_len > len - offset || block_len < cper::BLOCK_HEADER_LEN {
            warn!("APEI: a boot error record of {block_len} bytes does not fit its region");
            break;
        }
        let Ok(mut scratch) = radix_mm::bytes::zeroed(block_len, heap) else {
            error!("APEI: no memory to read the boot error record");
            break;
        };
        region.copy(offset, &mut scratch);
        report(Origin::LastBoot, &scratch);
        records += 1;
        offset += block_len;
    }
}

/// Tells the firmware the kernel reads its error records, as Linux asks
/// it: some firmware fills its GHES blocks only once told.
fn tell_firmware() {
    if !crate::acpi::running() {
        return;
    }
    let Some(sb) = radix_uacpi::find_node("\\_SB") else { return };
    // The WHEA capabilities, a query: Linux's request.
    let mut whea = [1, 1, 0];
    if sb.osc(&WHEA, 1, &mut whea).is_ok() && whea[0] & OSC_ERRORS == 0 {
        info!("APEI: the firmware is told the kernel reads its error records");
    }
    // The platform-wide APEI bit: asked, and if granted, taken.
    let mut platform = [1, APEI_SUPPORT];
    if sb.osc(&PLATFORM, 1, &mut platform).is_ok()
        && platform[0] & OSC_ERRORS == 0
        && platform[1] & APEI_SUPPORT != 0
    {
        let mut take = [0, APEI_SUPPORT];
        if let Err(err) = sb.osc(&PLATFORM, 1, &mut take) {
            warn!("APEI: the platform's _OSC refused the APEI support it offered: {err}");
        }
    }
}

/// Listens to the hardware error devices (`PNP0C33`); returns whether
/// there was one.
fn hear_error_devices() -> bool {
    if !crate::acpi::running() {
        return false;
    }
    let mut heard = false;
    radix_uacpi::find_devices(c"PNP0C33", &mut |node| match node.on_notify(notified) {
        Ok(()) => heard = true,
        Err(err) => warn!("APEI: the hardware error device cannot be heard: {err}"),
    });
    heard
}

/// Reads the firmware's tables: the errors of the last boot, the banks
/// the firmware handles first, and the GHES sources, which the `apei`
/// thread then reads. Called once, after the machine checks start.
pub fn start() {
    read_boot_errors();
    let Some(table) = Arch::firmware_table(b"HEST") else { return };
    let hest = match Sdt::parse(0, table).and_then(|sdt| Hest::parse(&sdt)) {
        Ok(hest) => hest,
        Err(err) => {
            warn!("APEI: the HEST cannot be read: {err:?}");
            return;
        }
    };
    tell_firmware();
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut banks: Vec<u8, Kmalloc> = Vec::new_in(heap);
    let mut found: Vec<Ghes, Kmalloc> = Vec::new_in(heap);
    for item in hest.sources() {
        match item {
            // Linux's rule: a corrected machine check source the firmware
            // handles first, without asking the kernel's help.
            Ok(ErrorSource::MachineCheck(check))
                if check.kind == MachineCheckKind::Corrected
                    && check.enabled
                    && check.firmware_first()
                    && !check.ghes_assist() =>
            {
                for bank in check.bank_numbers() {
                    if banks.try_reserve(1).is_ok() {
                        let _ = banks.push_within_capacity(bank);
                    }
                }
            }
            Ok(ErrorSource::Ghes(ghes)) if ghes.enabled => {
                if found.try_reserve(1).is_ok() {
                    let _ = found.push_within_capacity(ghes);
                }
            }
            Ok(_) => {}
            Err(err) => {
                warn!(
                    "APEI: the HEST's error sources after the {} read are not: {err}",
                    found.len()
                );
            }
        }
    }
    crate::mce::firmware_first(&banks);
    let sci_heard = found.iter().any(|ghes| ghes.notification.kind == Notification::SCI)
        && hear_error_devices();
    let mut sources: Vec<Source, Kmalloc> = Vec::new_in(heap);
    if sources.try_reserve_exact(found.len()).is_err() {
        error!("APEI: no memory for the firmware's error sources");
        return;
    }
    for ghes in &found {
        match Source::new(ghes, sci_heard, heap) {
            Ok(source) => {
                let _ = sources.push_within_capacity(source);
            }
            Err(why) => warn!("APEI: error source {} is not read: {why}", ghes.source_id),
        }
    }
    if sources.is_empty() {
        return;
    }
    let count = |how: fn(How) -> bool| sources.iter().filter(|source| how(source.how)).count();
    let (polled, sci, nmi) = (
        count(|how| how == How::Polled),
        count(|how| how == How::Sci),
        count(|how| how == How::Nmi),
    );
    let other = sources.len() - polled - sci - nmi;
    for source in &sources {
        if let How::Other(kind) = source.how {
            info!(
                "APEI: error source {} is signalled by a notification of type {kind}, which is \
                 not routed yet; it is read every second",
                source.id
            );
        }
    }
    if sci > 0 && !sci_heard {
        info!("APEI: no hardware error device is heard; the SCI sources are read every second");
    }
    let Ok(sources) = SOURCES.set(sources) else { return };
    ON.store(true, Ordering::Release);
    if nmi > 0 {
        Arch::set_nmi_handler(on_nmi);
    }
    let options = Options::new().policy(Policy::Fair { nice: -5 });
    if let Err(err) = sched::spawn("apei", run, 0, options) {
        error!(
            "APEI: could not start the thread `apei`: {err}; the firmware's errors are not read"
        );
        return;
    }
    info!(
        "APEI: {} error sources read: {polled} polled, {sci} by SCI, {nmi} by NMI, {other} \
         every second",
        sources.len()
    );
}

/// The `apei` thread: reads each source when it is due, when the firmware
/// says, and once at the start for what is waiting from before.
fn run(_: usize) {
    if let Some(me) = sched::current() {
        let _ = THREAD.set(me);
    }
    let Some(sources) = SOURCES.get() else { return };
    let most = sources.iter().map(|source| source.len).max().unwrap_or(0);
    let Ok(mut scratch) = radix_mm::bytes::zeroed(most, HEAP.with(Flags::MAY_SLEEP)) else {
        error!("APEI: no memory to read the firmware's error records");
        return;
    };
    let mut first = true;
    loop {
        let now = sched::now_ns();
        let notified = NOTIFIED.swap(false, Ordering::AcqRel);
        // At most an hour asleep, though nothing is due.
        let mut deadline = now.saturating_add(3_600_000_000_000);
        for source in sources {
            let next = source.next_ns.load(Ordering::Relaxed);
            let timed = source.every_ns > 0 && now >= next;
            if first || timed || (notified && source.how == How::Sci) {
                source.read(&mut scratch);
            }
            if source.every_ns > 0 {
                let next = if timed || first { now.saturating_add(source.every_ns) } else { next };
                source.next_ns.store(next, Ordering::Relaxed);
                deadline = deadline.min(next);
            }
        }
        first = false;
        sched::block_until(deadline, "apei");
    }
}
