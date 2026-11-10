// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PCI IDE for the Radix kernel (Phase 11g, ADR 0244): the ATA disks on an
//! IDE controller's two channels, and the SATA disks of a chipset whose
//! firmware sets its SATA controller to IDE mode, which then shows them
//! the same way.
//!
//! A channel has two devices, the master (0) and the slave (1), which
//! share its registers: a command block of eight ports and a control
//! port, in I/O space, at the legacy addresses (0x1f0 and 0x3f6, 0x170
//! and 0x376) or in the function's BARs 0 to 3 in native mode. Commands
//! go one at a time: the driver picks a device, writes the command's
//! address and count, and the command. Data moves by bus-master DMA (the
//! *Programming Interface for Bus Master IDE Controller*, revision 1.0):
//! eight more ports for each channel in BAR 4 take the address of a table
//! of physical region descriptors (PRDs), each a buffer below 4 GiB of at
//! most 64 KiB that does not cross a 64 KiB line, and a start bit.
//!
//! The commands are ATA's (ACS-4): IDENTIFY DEVICE, in PIO, then READ DMA
//! EXT, WRITE DMA EXT and FLUSH CACHE EXT, with 48-bit addresses, or READ
//! DMA, WRITE DMA and FLUSH CACHE with 28-bit ones on a disk from before
//! ATA-6; what IDENTIFY says is read by `radix_ahci::ata`, which AHCI's
//! disks share.
//!
//! A CompactFlash card in an IDE adapter (ADR 0446) is such a disk, and
//! says so in IDENTIFY. Its data move by PIO, through the data register:
//! a passive adapter need not wire the DMA lines, and many cards have no
//! DMA in True IDE mode. [`Channel::start_pio`] sends READ SECTORS or
//! WRITE SECTORS, and [`Channel::poll_pio`] moves each sector the card is
//! ready for between the data register and a buffer.
//! A packet (ATAPI) device, an optical drive, answers IDENTIFY PACKET
//! DEVICE, and takes SCSI commands in PACKET commands (ACS-4, 7.18, and
//! the PACKET protocol of ATA8-APT): after the command, the device asks
//! for the packet, the command descriptor block, through the data
//! register, and then moves its data there by PIO, as many bytes at a
//! time as it says in the byte count registers, or by bus-master DMA
//! ([`Channel::start_packet`], [`Channel::poll_packet`]). The two ends
//! of a channel share it, one command at a time, whichever kind.
//!
//! The channel's interrupt is off (nIEN): the kernel routes neither the
//! legacy IRQs 14 and 15 nor INTx, so a command is done when the device
//! is no longer busy and, for DMA, the bus master has stopped. The crate
//! is a compartment with no `unsafe` code; its tests run it against a
//! simulated channel and disks.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use radix_ahci::ata::{self, Identity, PacketIdentity, Taskfile, Unsupported, command};
use radix_hw::{Dma, IoPorts, device_barrier};

/// The PCI class and subclass of an IDE controller; its programming
/// interface says how each channel is reached ([`interface`]).
pub const PCI_CLASS: (u8, u8) = (0x01, 0x01);
/// The BAR of the bus-master registers, in I/O space.
pub const BUS_MASTER_BAR: usize = 4;
/// The legacy ports of the channels' command blocks and control ports,
/// for those not in native mode.
pub const LEGACY_PORTS: [(u16, u16); 2] = [(0x1f0, 0x3f6), (0x170, 0x376)];
/// The ports of a command block.
pub const COMMAND_PORTS: u16 = 8;
/// The ports of a channel's bus-master registers.
pub const BUS_MASTER_PORTS: u16 = 8;
/// The most PRDs a command takes.
pub const MAX_PRDS: usize = 256;

/// The bits of the programming interface.
pub mod interface {
    /// The primary channel is in native mode: its ports are BARs 0 and 1.
    pub const PRIMARY_NATIVE: u8 = 1 << 0;
    /// The secondary channel is in native mode: BARs 2 and 3.
    pub const SECONDARY_NATIVE: u8 = 1 << 2;
    /// The controller has bus-master DMA.
    pub const BUS_MASTER: u8 = 1 << 7;
}

/// The command block's registers.
mod reg {
    pub const DATA: u16 = 0;
    pub const ERROR: u16 = 1;
    /// Written, the error register's port is the features register.
    pub const FEATURES: u16 = 1;
    pub const COUNT: u16 = 2;
    pub const LBA0: u16 = 3;
    pub const LBA1: u16 = 4;
    pub const LBA2: u16 = 5;
    pub const DEVICE: u16 = 6;
    pub const STATUS: u16 = 7;
    pub const COMMAND: u16 = 7;
    /// The control port: the alternate status, read without clearing
    /// the interrupt; written, the device control register.
    pub const ALT_STATUS: u16 = 0;
    pub const DEVICE_CONTROL: u16 = 0;
    /// Device control: interrupts off, a software reset.
    pub const NIEN: u8 = 1 << 1;
    pub const SRST: u8 = 1 << 2;
    /// Device register: the slave.
    pub const SLAVE: u8 = 1 << 4;
    /// Status: a fault.
    pub const DF: u8 = 1 << 5;
}

/// A channel's bus-master registers.
mod bm {
    pub const COMMAND: u16 = 0;
    pub const STATUS: u16 = 2;
    pub const PRD: u16 = 4;
    /// Command: start, and the direction (set: into memory).
    pub const START: u8 = 1 << 0;
    pub const INTO_MEMORY: u8 = 1 << 3;
    /// Status: moving, an error, the device's interrupt, and the drives
    /// the firmware set up for DMA.
    pub const ACTIVE: u8 = 1 << 0;
    pub const ERROR: u8 = 1 << 1;
    pub const INTERRUPT: u8 = 1 << 2;
    pub const DMA_CAPABLE: u8 = 3 << 5;
}

/// A PRD's last-entry bit.
const PRD_LAST: u32 = 1 << 31;
/// The signatures a packet device leaves in LBA mid and high, parallel
/// and Serial ATA.
const ATAPI: [(u8, u8); 2] = [(0x14, 0xeb), (0x69, 0x96)];
/// The most sectors [`Channel::poll_pio`] moves in one call: each is 256
/// reads or writes of the data register, and its caller may hold a lock
/// with interrupts off meanwhile.
pub const PIO_BURST: u32 = 8;
/// How long a reset or IDENTIFY may take, and how often the driver
/// looks meanwhile, in microseconds.
const RESET_US: u32 = 2_000_000;
const IDENTIFY_US: u32 = 3_000_000;
const POLL_US: u32 = 100;

/// Whether IDENTIFY DEVICE's `words` are a Serial ATA device's: its
/// Serial ATA capabilities (word 76) are neither 0 nor all ones, or its
/// transport (word 222, bits 15 to 12) is Serial (ACS-4, 7.13.6.37 and
/// 7.13.6.53).
fn serial(words: &[u16; 256]) -> bool {
    let capabilities = words[76];
    (capabilities != 0 && capabilities != 0xffff) || words[222] >> 12 == 1
}

/// Whether `status` says a command failed: an error (a packet device's
/// CHK) or a device fault.
fn status_failed(status: u8) -> bool {
    status & (ata::STATUS_ERR | reg::DF) != 0
}

/// What is on one end of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// Nothing answers.
    Absent,
    /// An ATA disk, as it says it is.
    Ata {
        /// What it says of itself.
        identity: Identity,
        /// Whether it is a Serial ATA disk, which its words 76 (Serial
        /// ATA capabilities) or 222 (its transport) say: then the
        /// controller is a SATA controller set to IDE mode.
        serial: bool,
    },
    /// A packet device (an optical drive), as it says it is in IDENTIFY
    /// PACKET DEVICE.
    Atapi(PacketIdentity),
    /// An ATA device the driver does not drive.
    Unsupported(Unsupported),
}

/// What a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Reads sectors into memory.
    Read,
    /// Writes sectors from memory.
    Write,
    /// Has the disk write its cache out.
    Flush,
}

/// How a command could not be done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The request does not fit: too many sectors or buffers, or a
    /// buffer that does not make whole sectors.
    BadRequest,
    /// Another command is in flight on the channel.
    Busy,
    /// The device said it failed.
    Device {
        /// Its status register.
        status: u8,
        /// Its error register.
        error: u8,
    },
    /// The bus master could not reach memory.
    Bus,
    /// A packet device did not follow the PACKET protocol: it asked for
    /// what the command did not have; the channel needs a reset.
    Protocol,
}

/// The command in flight on a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Flight {
    drive: u8,
    dma: bool,
    /// A packet command's progress; `None` for a disk's command.
    packet: Option<Packet>,
    /// A disk's PIO command's progress.
    pio: Option<Pio>,
}

/// A disk's command whose data move by PIO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pio {
    /// Whether its data come from the disk.
    into_host: bool,
    /// The sectors still to move.
    left: u32,
    /// The bytes moved.
    moved: u32,
}

/// A packet command in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Packet {
    /// The command descriptor block, `length` bytes of it.
    bytes: [u8; 16],
    length: u8,
    /// Whether the device has taken it.
    sent: bool,
    /// Whether its data go by DMA, and which way.
    into_host: bool,
    /// The bytes its data phases have moved by PIO.
    moved: u32,
}

/// What a packet command moves, and how.
#[derive(Debug)]
pub enum Transfer<'a> {
    /// Nothing.
    None,
    /// By PIO, from the device into the slice: at most its length, the
    /// rest the device sends read and dropped.
    In(&'a mut [u8]),
    /// By PIO, the slice's bytes to the device.
    Out(&'a [u8]),
    /// By bus-master DMA, into memory (`into_host`) or from it, through
    /// the buffers: physical address below 4 GiB, and length.
    Dma {
        /// Whether the data come from the device.
        into_host: bool,
        /// The buffers.
        buffers: &'a [(u32, u32)],
    },
}

impl Transfer<'_> {
    /// How many bytes it moves.
    pub fn len(&self) -> u64 {
        match self {
            Transfer::None => 0,
            Transfer::In(bytes) => bytes.len() as u64,
            Transfer::Out(bytes) => bytes.len() as u64,
            Transfer::Dma { buffers, .. } => buffers.iter().map(|&(_, len)| u64::from(len)).sum(),
        }
    }

    /// Whether it moves nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The interrupt reason, in the count register while a packet command
/// asks for something (ATA8-APT): it wants the command (CoD), and data
/// go to the host (I/O).
mod reason {
    pub const COD: u8 = 1 << 0;
    pub const IO: u8 = 1 << 1;
}

/// An IDE channel and its two devices, reached through its command block,
/// its control port and its bus-master registers.
pub struct Channel<P: IoPorts> {
    command: P,
    control: P,
    bus_master: P,
    /// The PRD table: a page below 4 GiB.
    prds: Dma,
    flight: Option<Flight>,
    selected: Option<u8>,
    /// Each device's logical sector, in bytes, as IDENTIFY said.
    sector: [u32; 2],
    /// Whether each device takes 48-bit addresses, as IDENTIFY said.
    lba48: [bool; 2],
    /// Whether each device's data move by PIO: a CompactFlash card's.
    pio: [bool; 2],
}

impl<P: IoPorts> Channel<P> {
    /// The channel of these ports, with its PRD table from `alloc` (a
    /// page below 4 GiB); `None` without memory or with too few ports.
    pub fn new(
        command: P,
        control: P,
        bus_master: P,
        alloc: &mut dyn FnMut(usize, bool) -> Option<Dma>,
    ) -> Option<Channel<P>> {
        if command.len() < COMMAND_PORTS
            || control.is_empty()
            || bus_master.len() < BUS_MASTER_PORTS
        {
            return None;
        }
        let prds = alloc(1, true)?;
        Some(Channel {
            command,
            control,
            bus_master,
            prds,
            flight: None,
            selected: None,
            sector: [512; 2],
            lba48: [true; 2],
            pio: [false; 2],
        })
    }

    /// Whether anything answers on the channel: a floating bus reads all
    /// ones.
    pub fn present(&self) -> bool {
        self.control.read8(reg::ALT_STATUS) != 0xff
    }

    /// Resets both devices (a software reset), with interrupts off, and
    /// waits for the master to be ready; whether it was in time. Whatever
    /// was in flight is lost.
    pub fn reset(&mut self, delay: &mut dyn FnMut(u32)) -> bool {
        self.stop_dma();
        self.flight = None;
        self.selected = None;
        self.control.write8(reg::DEVICE_CONTROL, reg::NIEN | reg::SRST);
        delay(5);
        self.control.write8(reg::DEVICE_CONTROL, reg::NIEN);
        delay(2000);
        self.wait_not_busy(RESET_US, delay)
    }

    fn status(&self) -> u8 {
        self.control.read8(reg::ALT_STATUS)
    }

    fn wait_not_busy(&self, limit_us: u32, delay: &mut dyn FnMut(u32)) -> bool {
        let mut waited = 0;
        loop {
            let status = self.status();
            if status & ata::STATUS_BSY == 0 || status == 0xff {
                return status != 0xff;
            }
            if waited >= limit_us {
                return false;
            }
            delay(POLL_US);
            waited += POLL_US;
        }
    }

    /// Picks `drive`, and gives it 400 ns to answer.
    fn select(&mut self, drive: u8, lba: bool) {
        let mut device = if drive == 1 { reg::SLAVE } else { 0 } | 0xa0;
        if lba {
            device |= ata::DEVICE_LBA;
        }
        self.command.write8(reg::DEVICE, device);
        if self.selected != Some(drive) {
            for _ in 0..5 {
                self.status();
            }
            self.selected = Some(drive);
        }
    }

    /// Finds what `drive` is: IDENTIFY DEVICE, by PIO.
    pub fn identify(&mut self, drive: u8, delay: &mut dyn FnMut(u32)) -> Device {
        if self.flight.is_some() || drive > 1 {
            return Device::Absent;
        }
        self.select(drive, false);
        if self.status() == 0xff || !self.wait_not_busy(RESET_US, delay) {
            return Device::Absent;
        }
        let signature = (self.command.read8(reg::LBA1), self.command.read8(reg::LBA2));
        if ATAPI.contains(&signature) {
            return self.identify_packet(delay);
        }
        let words = match self.identify_words(command::IDENTIFY_DEVICE, delay) {
            Ok(words) => words,
            // A packet device aborts IDENTIFY DEVICE, and shows its
            // signature.
            Err(true) => {
                let signature = (self.command.read8(reg::LBA1), self.command.read8(reg::LBA2));
                if !ATAPI.contains(&signature) {
                    return Device::Absent;
                }
                return self.identify_packet(delay);
            }
            Err(false) => return Device::Absent,
        };
        match ata::identify(&words) {
            Ok(identity) => {
                let end = usize::from(drive);
                self.sector[end] = identity.logical_sector;
                self.lba48[end] = identity.lba48;
                self.pio[end] = identity.cfa;
                Device::Ata { identity, serial: serial(&words) }
            }
            Err(why) => Device::Unsupported(why),
        }
    }

    /// Asks the packet device picked IDENTIFY PACKET DEVICE, by PIO.
    fn identify_packet(&mut self, delay: &mut dyn FnMut(u32)) -> Device {
        match self.identify_words(command::IDENTIFY_PACKET_DEVICE, delay) {
            Ok(words) => match ata::identify_packet(&words) {
                Ok(identity) => Device::Atapi(identity),
                Err(why) => Device::Unsupported(why),
            },
            Err(_) => Device::Absent,
        }
    }

    /// Runs `code`, IDENTIFY DEVICE or IDENTIFY PACKET DEVICE, on the
    /// device picked, and reads its 256 words by PIO; `Err(true)` if the
    /// device refused it, `Err(false)` if nothing answered.
    fn identify_words(&mut self, code: u8, delay: &mut dyn FnMut(u32)) -> Result<[u16; 256], bool> {
        for register in [reg::FEATURES, reg::COUNT, reg::LBA0, reg::LBA1, reg::LBA2] {
            self.command.write8(register, 0);
        }
        self.command.write8(reg::COMMAND, code);
        if self.status() == 0 {
            return Err(false);
        }
        let mut waited = 0;
        loop {
            let status = self.status();
            if status & ata::STATUS_BSY == 0 {
                if status & ata::STATUS_ERR != 0 {
                    return Err(true);
                }
                if status & ata::STATUS_DRQ != 0 {
                    break;
                }
                if status == 0 {
                    return Err(false);
                }
            }
            if waited >= IDENTIFY_US {
                return Err(false);
            }
            delay(POLL_US);
            waited += POLL_US;
        }
        let mut words = [0u16; 256];
        for word in &mut words {
            *word = self.command.read16(reg::DATA);
        }
        // Reading the status ends the command.
        self.command.read8(reg::STATUS);
        Ok(words)
    }

    /// Whether a command is in flight.
    pub fn busy(&self) -> bool {
        self.flight.is_some()
    }

    /// Whether `drive`'s data move by PIO ([`Channel::start_pio`]) rather
    /// than by DMA ([`Channel::start`]).
    pub fn pio(&self, drive: u8) -> bool {
        self.pio.get(usize::from(drive)).copied().unwrap_or(false)
    }

    /// Whether a read or write of `count` sectors from `lba` fits what
    /// `drive` takes: up to 65,536 sectors a command below 2^48 with
    /// 48-bit addresses, up to 256 below 2^28 with 28-bit ones.
    fn fits(&self, drive: u8, lba: u64, count: u32) -> bool {
        let (most, end) =
            if self.lba48[usize::from(drive)] { (65_536, 1 << 48) } else { (256, 1 << 28) };
        count > 0 && count <= most && lba + u64::from(count) <= end
    }

    /// Writes the address and count of a read or write of `count`
    /// sectors from `lba`, on `drive`, which is picked: 48-bit, the high
    /// bytes first, or 28-bit, the top four bits in the device register.
    /// A count of the most a command takes is written as 0.
    fn write_address(&mut self, drive: u8, lba: u64, count: u32) {
        let [count_low, count_high] = ((count & 0xffff) as u16).to_le_bytes();
        let lba = lba.to_le_bytes();
        if self.lba48[usize::from(drive)] {
            for (register, value) in [
                (reg::COUNT, count_high),
                (reg::LBA0, lba[3]),
                (reg::LBA1, lba[4]),
                (reg::LBA2, lba[5]),
            ] {
                self.command.write8(register, value);
            }
        } else {
            let device = if drive == 1 { reg::SLAVE } else { 0 } | 0xa0 | ata::DEVICE_LBA;
            self.command.write8(reg::DEVICE, device | lba[3] & 0x0f);
        }
        for (register, value) in
            [(reg::COUNT, count_low), (reg::LBA0, lba[0]), (reg::LBA1, lba[1]), (reg::LBA2, lba[2])]
        {
            self.command.write8(register, value);
        }
    }

    /// Starts `op` on `drive`: for a read or write, of `count` of its
    /// logical sectors from `lba`, into or out of `buffers` (physical
    /// address below 4 GiB, and length), which hold exactly them.
    pub fn start(
        &mut self,
        drive: u8,
        op: Op,
        lba: u64,
        count: u32,
        buffers: &[(u32, u32)],
    ) -> Result<(), Error> {
        if self.flight.is_some() {
            return Err(Error::Busy);
        }
        if drive > 1 || (op != Op::Flush && self.pio(drive)) {
            return Err(Error::BadRequest);
        }
        let dma = op != Op::Flush;
        if dma {
            if !self.fits(drive, lba, count) {
                return Err(Error::BadRequest);
            }
            let bytes = u64::from(count) * u64::from(self.sector[usize::from(drive)]);
            if self.write_prds(buffers)? != bytes {
                return Err(Error::BadRequest);
            }
        }
        self.select(drive, true);
        if self.status() & (ata::STATUS_BSY | ata::STATUS_DRQ) != 0 {
            return Err(Error::Busy);
        }
        let lba48 = self.lba48[usize::from(drive)];
        let code = match (op, lba48) {
            (Op::Read, true) => command::READ_DMA_EXT,
            (Op::Write, true) => command::WRITE_DMA_EXT,
            (Op::Flush, true) => command::FLUSH_CACHE_EXT,
            (Op::Read, false) => command::READ_DMA,
            (Op::Write, false) => command::WRITE_DMA,
            (Op::Flush, false) => command::FLUSH_CACHE,
        };
        if dma {
            self.bus_master.write32(bm::PRD, self.prds.phys() as u32);
            let status = self.bus_master.read8(bm::STATUS) & bm::DMA_CAPABLE;
            self.bus_master.write8(bm::STATUS, status | bm::ERROR | bm::INTERRUPT);
            let direction = if op == Op::Read { bm::INTO_MEMORY } else { 0 };
            self.bus_master.write8(bm::COMMAND, direction);
            self.write_address(drive, lba, count);
        }
        device_barrier();
        self.command.write8(reg::COMMAND, code);
        if dma {
            let direction = if op == Op::Read { bm::INTO_MEMORY } else { 0 };
            self.bus_master.write8(bm::COMMAND, direction | bm::START);
        }
        self.flight = Some(Flight { drive, dma, packet: None, pio: None });
        Ok(())
    }

    /// Starts a read or write by PIO on `drive`, of `count` of its
    /// logical sectors from `lba`: READ SECTORS or WRITE SECTORS (their
    /// EXT forms with 48-bit addresses). [`Channel::poll_pio`] then moves
    /// them, as the drive is ready for each.
    pub fn start_pio(&mut self, drive: u8, op: Op, lba: u64, count: u32) -> Result<(), Error> {
        if self.flight.is_some() {
            return Err(Error::Busy);
        }
        if drive > 1 || op == Op::Flush || !self.fits(drive, lba, count) {
            return Err(Error::BadRequest);
        }
        self.select(drive, true);
        if self.status() & (ata::STATUS_BSY | ata::STATUS_DRQ) != 0 {
            return Err(Error::Busy);
        }
        let code = match (op == Op::Read, self.lba48[usize::from(drive)]) {
            (true, true) => command::READ_SECTORS_EXT,
            (false, true) => command::WRITE_SECTORS_EXT,
            (true, false) => command::READ_SECTORS,
            (false, false) => command::WRITE_SECTORS,
        };
        self.write_address(drive, lba, count);
        self.command.write8(reg::COMMAND, code);
        let pio = Pio { into_host: op == Op::Read, left: count, moved: 0 };
        self.flight = Some(Flight { drive, dma: false, packet: None, pio: Some(pio) });
        Ok(())
    }

    /// The PIO command in flight, moved on as far as the drive is ready,
    /// by at most [`PIO_BURST`] sectors: each sector it is ready for read
    /// into `buffer`, or written from it, from its start (ACS-4's PIO
    /// data-in and data-out protocols, a sector to each DRQ block). `None`
    /// while it runs; once done, how it ended.
    pub fn poll_pio(&mut self, buffer: &Dma) -> Option<Result<(), Error>> {
        let mut flight = self.flight?;
        let mut pio = flight.pio?;
        let sector = self.sector[usize::from(flight.drive)] as usize;
        let mut confused = false;
        for burst in 0.. {
            if burst == PIO_BURST {
                return None;
            }
            let status = self.status();
            if status & ata::STATUS_BSY != 0 {
                return None;
            }
            if status_failed(status) {
                break;
            }
            if status & ata::STATUS_DRQ == 0 {
                if pio.left == 0 {
                    break;
                }
                // Between sectors: the drive has not yet said it is busy.
                return None;
            }
            if pio.left == 0 || (pio.moved as usize + sector) > buffer.len() {
                // It asks to move more than the command has.
                confused = true;
                break;
            }
            let mut block = [0u8; 512];
            for part in (0..sector).step_by(block.len()) {
                let at = pio.moved as usize + part;
                if pio.into_host {
                    for pair in block.as_chunks_mut::<2>().0 {
                        *pair = self.command.read16(reg::DATA).to_le_bytes();
                    }
                    buffer.write(at, &block);
                } else {
                    buffer.read(at, &mut block);
                    for pair in block.as_chunks::<2>().0 {
                        self.command.write16(reg::DATA, u16::from_le_bytes(*pair));
                    }
                }
            }
            pio.moved += sector as u32;
            pio.left -= 1;
            flight.pio = Some(pio);
            self.flight = Some(flight);
            // The drive sets BSY within 400 ns of the block's last word:
            // the alternate status, read once, gives it them.
            self.status();
        }
        self.flight = None;
        // Reading the status ends the command.
        let status = self.command.read8(reg::STATUS);
        let error = self.command.read8(reg::ERROR);
        if status_failed(status) {
            return Some(Err(Error::Device { status, error }));
        }
        if confused || pio.left != 0 {
            return Some(Err(Error::Protocol));
        }
        Some(Ok(()))
    }

    /// Starts the packet command `bytes`, a SCSI command descriptor block
    /// of at most 16 bytes, on the packet device `drive`, moving
    /// `transfer`; with `dmadir`, a DMA command that moves data to the
    /// host says so (DMADIR), for a device that asks for it. The device
    /// asks for the packet and moves the data as [`Channel::poll_packet`]
    /// finds it ready.
    pub fn start_packet(
        &mut self,
        drive: u8,
        bytes: &[u8],
        transfer: &Transfer<'_>,
        dmadir: bool,
    ) -> Result<(), Error> {
        if self.flight.is_some() {
            return Err(Error::Busy);
        }
        if drive > 1 || bytes.is_empty() || bytes.len() > 16 {
            return Err(Error::BadRequest);
        }
        let total = u32::try_from(transfer.len()).map_err(|_| Error::BadRequest)?;
        let (dma, into_host) = match *transfer {
            Transfer::Dma { into_host, buffers } => {
                if self.write_prds(buffers)? != u64::from(total) {
                    return Err(Error::BadRequest);
                }
                (true, into_host)
            }
            Transfer::In(_) => (false, true),
            Transfer::None | Transfer::Out(_) => (false, false),
        };
        self.select(drive, false);
        if self.status() & (ata::STATUS_BSY | ata::STATUS_DRQ) != 0 {
            return Err(Error::Busy);
        }
        if dma {
            self.bus_master.write32(bm::PRD, self.prds.phys() as u32);
            let status = self.bus_master.read8(bm::STATUS) & bm::DMA_CAPABLE;
            self.bus_master.write8(bm::STATUS, status | bm::ERROR | bm::INTERRUPT);
            let direction = if into_host { bm::INTO_MEMORY } else { 0 };
            self.bus_master.write8(bm::COMMAND, direction);
        }
        // The byte count limit goes in LBA mid and high, for PIO.
        let taskfile = Taskfile::packet(dma, dmadir && into_host, total);
        let [limit_low, limit_high, ..] = (taskfile.lba >> 8).to_le_bytes();
        for (register, value) in [
            (reg::FEATURES, taskfile.features as u8),
            (reg::COUNT, 0),
            (reg::LBA0, 0),
            (reg::LBA1, limit_low),
            (reg::LBA2, limit_high),
        ] {
            self.command.write8(register, value);
        }
        device_barrier();
        self.command.write8(reg::COMMAND, command::PACKET);
        let mut packet =
            Packet { bytes: [0; 16], length: bytes.len() as u8, sent: false, into_host, moved: 0 };
        packet.bytes[..bytes.len()].copy_from_slice(bytes);
        self.flight = Some(Flight { drive, dma, packet: Some(packet), pio: None });
        Ok(())
    }

    /// The packet command in flight, moved on as far as the device is
    /// ready: the packet sent when it asks for it, and data moved by PIO
    /// into or out of `transfer`, the one it was started with. `None`
    /// while it runs; once done, the bytes moved (all of them, for DMA),
    /// or how it failed: the device's error register then holds the
    /// sense key in its upper four bits.
    pub fn poll_packet(&mut self, transfer: &mut Transfer<'_>) -> Option<Result<u32, Error>> {
        let mut flight = self.flight?;
        let mut packet = flight.packet?;
        let mut confused = false;
        loop {
            let status = self.status();
            if status & ata::STATUS_BSY != 0 {
                return None;
            }
            if status & ata::STATUS_DRQ == 0 {
                break;
            }
            let why = self.command.read8(reg::COUNT);
            let count = u32::from(self.command.read8(reg::LBA1))
                | u32::from(self.command.read8(reg::LBA2)) << 8;
            if why & reason::COD != 0 && why & reason::IO == 0 && !packet.sent {
                // It asks for the packet: the padding goes too, to the
                // length the device takes, and DMA starts after it.
                let length = usize::from(packet.length).max(12).next_multiple_of(2);
                for pair in packet.bytes[..length].as_chunks::<2>().0 {
                    self.command.write16(reg::DATA, u16::from_le_bytes(*pair));
                }
                packet.sent = true;
                if flight.dma {
                    let direction = if packet.into_host { bm::INTO_MEMORY } else { 0 };
                    device_barrier();
                    self.bus_master.write8(bm::COMMAND, direction | bm::START);
                }
            } else if flight.dma || !packet.sent || why & reason::COD != 0 || count == 0 {
                // Data by PIO on a DMA command or before the packet, the
                // packet asked for twice, or a phase of no bytes: the
                // device does not follow the protocol, and the command
                // goes no further.
                confused = true;
                break;
            } else {
                packet.moved +=
                    self.move_data(transfer, packet.moved, count, why & reason::IO != 0);
            }
            flight.packet = Some(packet);
            self.flight = Some(flight);
        }
        let bus = if flight.dma { self.bus_master.read8(bm::STATUS) } else { 0 };
        let failed = confused || status_failed(self.status()) || bus & bm::ERROR != 0;
        if flight.dma && bus & bm::ACTIVE != 0 && !failed {
            return None;
        }
        device_barrier();
        self.flight = None;
        // Reading the status ends the command.
        let status = self.command.read8(reg::STATUS);
        let error = self.command.read8(reg::ERROR);
        if flight.dma {
            self.stop_dma();
        }
        if bus & bm::ERROR != 0 {
            return Some(Err(Error::Bus));
        }
        if confused || !packet.sent {
            return Some(Err(Error::Protocol));
        }
        if status_failed(status) {
            return Some(Err(Error::Device { status, error }));
        }
        Some(Ok(if flight.dma { transfer.len() as u32 } else { packet.moved }))
    }

    /// One PIO data phase of `count` bytes, from `at` in `transfer`: into
    /// it when `to_host`. Bytes beyond it are read and dropped, or sent
    /// as zeros. How many bytes of it moved.
    fn move_data(
        &mut self,
        transfer: &mut Transfer<'_>,
        at: u32,
        count: u32,
        to_host: bool,
    ) -> u32 {
        let words = count.div_ceil(2);
        let at = at as usize;
        let mut moved = 0u32;
        for i in 0..words as usize {
            let offset = at + 2 * i;
            match transfer {
                Transfer::In(bytes) if to_host => {
                    let [low, high] = self.command.read16(reg::DATA).to_le_bytes();
                    for (k, byte) in [low, high].into_iter().enumerate() {
                        if let Some(slot) = bytes.get_mut(offset + k) {
                            *slot = byte;
                            moved += 1;
                        }
                    }
                }
                Transfer::Out(bytes) if !to_host => {
                    let low = bytes.get(offset).copied().unwrap_or(0);
                    let high = bytes.get(offset + 1).copied().unwrap_or(0);
                    moved += u32::from(offset < bytes.len()) + u32::from(offset + 1 < bytes.len());
                    self.command.write16(reg::DATA, u16::from_le_bytes([low, high]));
                }
                _ if to_host => {
                    self.command.read16(reg::DATA);
                }
                _ => self.command.write16(reg::DATA, 0),
            }
        }
        // An odd count's last word holds one byte.
        moved.min(count)
    }

    /// Fills the PRD table with `buffers`, which hold the command's
    /// sectors; how many bytes they hold.
    fn write_prds(&mut self, buffers: &[(u32, u32)]) -> Result<u64, Error> {
        let mut total = 0u64;
        let mut entries = 0;
        for &(start, len) in buffers {
            let (mut at, mut left) = (u64::from(start), u64::from(len));
            if at + left > 1 << 32 {
                return Err(Error::BadRequest);
            }
            while left > 0 {
                // Not across a 64 KiB line.
                let room = 0x1_0000 - (at & 0xffff);
                let part = left.min(room);
                if entries == MAX_PRDS {
                    return Err(Error::BadRequest);
                }
                let size = (part & 0xffff) as u32;
                self.prds.write32(8 * entries, at as u32);
                self.prds.write32(8 * entries + 4, size);
                entries += 1;
                at += part;
                left -= part;
                total += part;
            }
        }
        if entries == 0 {
            return Err(Error::BadRequest);
        }
        let last = 8 * (entries - 1) + 4;
        self.prds.write32(last, self.prds.read32(last) | PRD_LAST);
        Ok(total)
    }

    /// The command in flight, if it is done: how it ended. `None` while
    /// it runs, or when none is.
    pub fn poll(&mut self) -> Option<Result<(), Error>> {
        let flight =
            self.flight.filter(|flight| flight.packet.is_none() && flight.pio.is_none())?;
        let status = self.status();
        if status & ata::STATUS_BSY != 0 {
            return None;
        }
        let bus = if flight.dma { self.bus_master.read8(bm::STATUS) } else { 0 };
        let failed = status_failed(status) || bus & bm::ERROR != 0;
        if flight.dma && bus & bm::ACTIVE != 0 && !failed {
            return None;
        }
        device_barrier();
        self.flight = None;
        // Reading the status ends the command.
        let status = self.command.read8(reg::STATUS);
        let error = self.command.read8(reg::ERROR);
        if flight.dma {
            self.stop_dma();
        }
        if bus & bm::ERROR != 0 {
            return Some(Err(Error::Bus));
        }
        if status_failed(status) {
            return Some(Err(Error::Device { status, error }));
        }
        Some(Ok(()))
    }

    /// Whether the command in flight is a packet command.
    pub fn packet_in_flight(&self) -> bool {
        self.flight.is_some_and(|flight| flight.packet.is_some())
    }

    /// The drive of the command in flight.
    pub fn in_flight(&self) -> Option<u8> {
        self.flight.map(|flight| flight.drive)
    }

    fn stop_dma(&self) {
        let command = self.bus_master.read8(bm::COMMAND);
        self.bus_master.write8(bm::COMMAND, command & !bm::START);
        let status = self.bus_master.read8(bm::STATUS) & bm::DMA_CAPABLE;
        self.bus_master.write8(bm::STATUS, status | bm::ERROR | bm::INTERRUPT);
    }
}
