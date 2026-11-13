// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One port of an AHCI controller and the disk on it (AHCI 1.3.1,
//! sections 3.3, 4 and 5).
//!
//! A port's memory is one block of DMA memory: the command list (32 slot
//! headers of 32 bytes), the area the controller copies the disk's frames
//! into, a command table per slot (the command's register frame and up to
//! [`MAX_SEGMENTS`] buffer entries), and a 512-byte buffer of the port's
//! own, for the data of IDENTIFY DEVICE, READ LOG EXT and TRIM, and a
//! [`PACKET_BUFFER_SIZE`]-byte one for the data of an optical drive's
//! smaller commands.
//!
//! Reads and writes go out as NCQ commands when the disk queues: a slot's
//! number is its command's tag, its bit is set in the port's SActive
//! register and then in Command Issue, and the disk finishes queued
//! commands in any order, clearing their SActive bits. Other commands
//! (flushes, TRIM, SMART, and every command to a disk that does not queue) go out
//! alone: they wait until nothing is in flight, and nothing goes out
//! while they are. A finished command's bits are clear in both registers.
//!
//! An error stops the port ([`Port::stopped`]): the controller stops
//! processing commands and the driver hands out nothing until
//! [`Port::recover`] has restarted it, which waits and so runs outside
//! the interrupt handler. The port's interrupts are off meanwhile: an
//! error the link reports stays in its status until recovery clears it.
//! Recovery learns which command failed where it can (for a queued
//! command, from the disk's NCQ Command Error log, which a disk also needs
//! read before it takes another command). That one fails if the disk
//! could not read or find its sectors ([`ata::ERROR_MEDIA`]); the others,
//! and that one after any other error, are handed back to be issued again
//! ([`Outcome::Retry`]).
//!
//! An optical drive on a port (an ATAPI device, ADR 0431) takes SCSI
//! commands in ATA packets, one at a time, on slot 0. The thread that runs
//! the drive sends its own through [`Packets`], a [`Transport`] that waits
//! for each in the kernel's way, woken by the port's interrupt
//! ([`crate::Hba::quiet`]). A disc's reads go out without waiting
//! ([`Port::send_packet`]) from the thread that reads, and the port's
//! interrupt looks for their end ([`Port::packet_ended`]); the drive's
//! thread then has a failed one answered ([`Port::packet_reply`]), as
//! Linux's libata does (ADR 0434). The port remembers the interrupts it
//! last turned on, so that a read sent while they are on writes one
//! register: each is a trap to the hypervisor in a virtual machine.

use radix_hw::{Dma, Mmio, device_barrier};
use radix_scsi::{Cdb, Data, Reply, SENSE_LENGTH, Sense, Transport, TransportError};

use crate::ata::{self, Identity, PacketIdentity, Taskfile};
use crate::{Absent, Error, hba, wait_for};

/// Port registers, from the port's base (AHCI 1.3.1, 3.3).
mod reg {
    pub const CLB: usize = 0x00;
    pub const CLBU: usize = 0x04;
    pub const FB: usize = 0x08;
    pub const FBU: usize = 0x0c;
    pub const IS: usize = 0x10;
    pub const IE: usize = 0x14;
    pub const CMD: usize = 0x18;
    pub const TFD: usize = 0x20;
    pub const SIG: usize = 0x24;
    pub const SSTS: usize = 0x28;
    pub const SCTL: usize = 0x2c;
    pub const SERR: usize = 0x30;
    pub const SACT: usize = 0x34;
    pub const CI: usize = 0x38;
}

/// PxCMD bits.
mod cmd {
    /// Start: the controller processes the command list.
    pub const ST: u32 = 1 << 0;
    /// Spin up the device.
    pub const SUD: u32 = 1 << 1;
    /// Power on the device.
    pub const POD: u32 = 1 << 2;
    /// Command list override: clears a busy bit that stays set.
    pub const CLO: u32 = 1 << 3;
    /// FIS receive enable.
    pub const FRE: u32 = 1 << 4;
    /// FIS receive running.
    pub const FR: u32 = 1 << 14;
    /// Command list running.
    pub const CR: u32 = 1 << 15;
    /// Where the slot the controller works on (CCS) starts.
    pub const CCS_SHIFT: u32 = 8;
}

/// PxIS and PxIE bits.
mod irq {
    /// A device-to-host register frame came: a command not queued is done.
    pub const DHRS: u32 = 1 << 0;
    /// A PIO setup frame came: a PIO command moved its data.
    pub const PSS: u32 = 1 << 1;
    /// A set device bits frame came: queued commands are done.
    pub const SDBS: u32 = 1 << 3;
    /// A frame of an unknown type came.
    pub const UFS: u32 = 1 << 4;
    /// The link connected or disconnected.
    pub const PCS: u32 = 1 << 6;
    /// The link's PhyRdy signal changed.
    pub const PRCS: u32 = 1 << 22;
    /// The disk sent more data than the buffers hold.
    pub const OFS: u32 = 1 << 24;
    /// An error on the link that the controller recovered from.
    pub const INFS: u32 = 1 << 26;
    /// An error on the link that the controller did not recover from.
    pub const IFS: u32 = 1 << 27;
    /// A data error reaching memory.
    pub const HBDS: u32 = 1 << 28;
    /// A fatal error reaching memory.
    pub const HBFS: u32 = 1 << 29;
    /// The disk reported an error in its status register.
    pub const TFES: u32 = 1 << 30;

    /// What stops the port until it recovers.
    pub const STOPS: u32 = UFS | PCS | PRCS | OFS | IFS | HBDS | HBFS | TFES;
    /// Which of those need the link reset: the link or the controller
    /// failed, not the command.
    pub const FATAL: u32 = UFS | PCS | PRCS | IFS | HBFS;
    /// What raises the port's interrupt.
    pub const ENABLED: u32 = DHRS | PSS | SDBS | INFS | STOPS;
}

/// PxSSTS's DET field: a device is there and the link is up.
const DET_UP: u32 = 3;
/// A device is there, and the link not up yet.
const DET_PRESENT: u32 = 1;
/// PxSCTL with transitions to the link's power-saving states off
/// (IPM = 3), and DET as given: 1 resets the link (COMRESET), 0 lets it
/// come up.
const SCTL_NO_POWER_SAVING: u32 = 3 << 8;

/// Signatures the disk sends after a link reset (PxSIG).
const SIG_ATA: u32 = 0x0000_0101;
const SIG_ATAPI: u32 = 0xeb14_0101;
const SIG_PORT_MULTIPLIER: u32 = 0x9669_0101;

/// The most buffers one request takes.
pub const MAX_SEGMENTS: usize = 32;

/// Where things are in the port's memory, and how big it is.
const COMMAND_LIST: usize = 0;
const HEADER_SIZE: usize = 32;
const RECEIVED_FIS: usize = 1024;
const TABLES: usize = 1280;
const TABLE_PRDT: usize = 0x80;
const PRD_SIZE: usize = 16;
const TABLE_SIZE: usize = TABLE_PRDT + MAX_SEGMENTS * PRD_SIZE;
const BUFFER: usize = TABLES + 32 * TABLE_SIZE;
const BUFFER_SIZE: usize = 512;
const PACKET_BUFFER: usize = BUFFER + BUFFER_SIZE;
/// The most bytes an optical drive's command moves through the port's
/// own buffer: a command whose data are in the caller's memory
/// ([`Data::In`] and [`Data::Out`]), a TOC or a configuration.
pub const PACKET_BUFFER_SIZE: usize = 2048;
const MEMORY: usize = PACKET_BUFFER + PACKET_BUFFER_SIZE;
/// Pages of DMA memory a port takes.
pub const MEMORY_PAGES: usize = MEMORY.div_ceil(4096);

/// Command header bits (5.5.1): the frame's length in words, and whether
/// data go to the device.
const HEADER_CFL: u32 = (ata::FIS_H2D_LEN / 4) as u32;
const HEADER_WRITE: u32 = 1 << 6;
/// Command header bit: the command is a packet, whose command descriptor
/// block is in the table at [`TABLE_ACMD`].
const HEADER_ATAPI: u32 = 1 << 5;
/// Where a packet's command descriptor block is in a command table.
const TABLE_ACMD: usize = 0x40;
/// The command header's word where the controller counts the bytes moved
/// (PRDBC).
const HEADER_PRDBC: usize = 4;
/// Where the controller copies the last Register FIS the device sent in
/// the received-frame area (4.2.1), and that frame's type: a packet
/// command's DMA ends with one, its status register in byte 2.
const RECEIVED_D2H: usize = RECEIVED_FIS + 0x40;
const FIS_TYPE_D2H: u8 = 0x34;

/// How long a link reset is held: at least a millisecond (10.4.2).
pub const COMRESET_US: u32 = 1_000;
/// How often a link is looked at while it comes up, how long one with no
/// device must stay down to count as empty, and how long a device that
/// is there may take to bring its link up.
const LINK_POLL_US: u32 = 5_000;
const LINK_EMPTY_US: u32 = 100_000;
const LINK_LIMIT_US: u32 = 1_000_000;
/// How long the command engine and frame receiving take to stop (10.1.2).
const STOP_US: u32 = 500_000;
/// How long a disk may stay busy after its link comes up: a hard disk
/// spins up meanwhile.
const READY_US: u32 = 10_000_000;
/// How long a command the driver waits for may take, and how often it
/// looks.
const COMMAND_US: u32 = 5_000_000;
const COMMAND_POLL_US: u32 = 100;
const POLL_US: u32 = 1_000;
/// How long a packet command is first waited for, and the longest wait
/// between looks after: each wait doubles, so that a command answered at
/// once is seen at once and one that spins a disc up costs few looks.
/// The port's interrupt ends a wait early.
const PACKET_FIRST_US: u32 = 10;
const PACKET_LONGEST_US: u32 = 4_000;

/// What a request does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Reads sectors into the buffers.
    Read,
    /// Writes the buffers to sectors.
    Write,
    /// Writes the buffers to sectors, through the disk's write cache:
    /// durable when done.
    WriteThrough,
    /// Makes every completed write durable.
    Flush,
    /// Trims this many sectors: the disk may forget what they hold.
    Trim(u64),
    /// SMART with this feature code ([`ata::smart`]) and this LBA low
    /// register: reads a 512-byte page of the disk's SMART record into
    /// the one buffer, or with [`ata::smart::EXECUTE_OFFLINE`] and no
    /// buffer starts or aborts a self-test. Only for a disk whose SMART is
    /// on ([`Identity::smart`]).
    Smart(u8, u8),
}

/// How a request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It worked.
    Done,
    /// The disk failed it.
    Failed,
    /// Another command's error, or the link's, stopped it: issue it again.
    Retry,
}

/// What the disk on a port is and does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// Its capacity in its logical sectors, which requests count in.
    pub sectors: u64,
    /// Its logical sector size in bytes: 512, or 4096 on a disk with
    /// 4 KiB sectors.
    pub sector_size: u32,
    /// Its physical sector size in bytes, the smallest unit it writes
    /// without reading first.
    pub block_size: u32,
    /// How many commands the driver keeps in flight: the disk's queue
    /// depth and the controller's slots, or 1 when it does not queue.
    pub depth: u8,
    /// Whether reads and writes go out as queued (NCQ) commands.
    pub queued: bool,
    /// Whether it has a write cache that is on, for [`Op::Flush`] to empty.
    pub write_cache: bool,
    /// Whether it writes through its cache itself ([`Op::WriteThrough`]);
    /// without, the driver's caller writes and flushes.
    pub fua: bool,
    /// The most sectors one [`Op::Trim`] covers, or 0 when the disk does
    /// not trim.
    pub trim_sectors: u64,
    /// Whether it spins, if it says.
    pub rotational: Option<bool>,
}

/// A port with a disk on it, set up and running.
pub struct Port {
    regs: Mmio,
    /// Only `None` while the port is dropped.
    memory: Option<Dma>,
    cap: u32,
    identity: Option<Identity>,
    /// What an optical drive on the port says it is.
    packet: Option<PacketIdentity>,
    /// The interrupts the port raises, as this last set them; `None`
    /// while unknown. A disc's read finds them as it wants them and
    /// writes only the command issue register.
    interrupts: Option<u32>,
    /// Whether the packet command on the port went with its interrupt
    /// off, to be polled for in memory ([`Port::packet_over`]).
    polled: bool,
    info: Info,
    /// Slots in flight.
    busy: u32,
    /// Whether the one in flight is a command that goes out alone.
    alone: bool,
    /// Finished slots not yet handed out, by how they ended.
    done: u32,
    failed: u32,
    retry: u32,
    /// The interrupt status that stopped the port, until it recovers.
    stopped: Option<u32>,
    /// Whether the disk is gone.
    dead: bool,
    errors: u32,
    soft_errors: u32,
}

impl Port {
    /// Takes over a port whose registers are `regs`, on a controller with
    /// capabilities `cap`: stops it, gives it memory and turns on frame
    /// receiving, ready for its link to be reset.
    pub(crate) fn prepare(
        regs: Mmio,
        cap: u32,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Port, Absent> {
        let memory = alloc(MEMORY_PAGES).ok_or(Absent::NoMemory)?;
        let wide = cap & hba::CAP_S64A != 0;
        if memory.len() < MEMORY || (!wide && memory.phys() + MEMORY as u64 > 1 << 32) {
            return Err(Absent::NoMemory);
        }
        let mut port = Port {
            regs,
            memory: Some(memory),
            cap,
            identity: None,
            packet: None,
            interrupts: None,
            polled: false,
            info: Info {
                sectors: 0,
                sector_size: 512,
                block_size: 512,
                depth: 1,
                queued: false,
                write_cache: false,
                fua: false,
                trim_sectors: 0,
                rotational: None,
            },
            busy: 0,
            alone: false,
            done: 0,
            failed: 0,
            retry: 0,
            stopped: None,
            dead: false,
            errors: 0,
            soft_errors: 0,
        };
        port.set_interrupts(0);
        if !port.halt(delay) {
            return Err(Absent::Stuck);
        }
        let list = port.mem().phys() + COMMAND_LIST as u64;
        let fis = port.mem().phys() + RECEIVED_FIS as u64;
        port.regs.write32(reg::CLB, list as u32);
        port.regs.write32(reg::CLBU, (list >> 32) as u32);
        port.regs.write32(reg::FB, fis as u32);
        port.regs.write32(reg::FBU, (fis >> 32) as u32);
        port.regs.write32(reg::SERR, u32::MAX);
        port.regs.write32(reg::IS, u32::MAX);
        let command = port.regs.read32(reg::CMD);
        port.regs.write32(reg::CMD, command | cmd::FRE | cmd::SUD | cmd::POD);
        Ok(port)
    }

    fn mem(&self) -> &Dma {
        // Only `drop` takes the memory away.
        self.memory.as_ref().expect("a port's memory is there until it is dropped")
    }

    /// Stops the command engine and frame receiving and waits for both;
    /// false if they do not stop.
    fn halt(&mut self, delay: &mut dyn FnMut(u32)) -> bool {
        self.stop_engine(delay) && {
            let command = self.regs.read32(reg::CMD);
            if command & cmd::FRE != 0 {
                self.regs.write32(reg::CMD, command & !cmd::FRE);
            }
            wait_for(delay, STOP_US, POLL_US, || self.regs.read32(reg::CMD) & cmd::FR == 0)
        }
    }

    /// Stops the command engine and waits until it has; false if it does
    /// not. The controller clears Command Issue and SActive.
    fn stop_engine(&mut self, delay: &mut dyn FnMut(u32)) -> bool {
        let command = self.regs.read32(reg::CMD);
        if command & cmd::ST != 0 {
            self.regs.write32(reg::CMD, command & !cmd::ST);
        }
        wait_for(delay, STOP_US, POLL_US, || self.regs.read32(reg::CMD) & cmd::CR == 0)
    }

    /// Starts resetting the link (COMRESET); [`Port::end_link_reset`]
    /// ends it, at least [`COMRESET_US`] later.
    pub(crate) fn begin_link_reset(&mut self) {
        self.regs.write32(reg::SCTL, SCTL_NO_POWER_SAVING | 1);
    }

    /// Lets the link come up again.
    pub(crate) fn end_link_reset(&mut self) {
        self.regs.write32(reg::SCTL, SCTL_NO_POWER_SAVING);
    }

    /// The link's DET field.
    fn link(&self) -> u32 {
        self.regs.read32(reg::SSTS) & 0xf
    }

    /// Readies the disk on a port whose link is up: waits until it is not
    /// busy, checks that it is an ATA disk or an optical drive, starts the
    /// command engine and asks the device what it is. The port's
    /// interrupts are on afterwards. Its buffers stay out of the frame of
    /// [`crate::Hba::probe`].
    #[inline(never)]
    pub(crate) fn start(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Absent> {
        // The link reset left its mark in both (PxSERR.DIAG.X, and PxIS.PCS
        // with it).
        self.regs.write32(reg::SERR, u32::MAX);
        self.regs.write32(reg::IS, u32::MAX);
        if !self.wait_ready(delay) {
            return Err(Absent::NotReady);
        }
        match self.regs.read32(reg::SIG) {
            SIG_ATA => {}
            SIG_ATAPI => return self.start_packet(delay),
            SIG_PORT_MULTIPLIER => return Err(Absent::PortMultiplier),
            other => return Err(Absent::Unknown(other)),
        }
        if !self.start_engine() {
            return Err(Absent::Stuck);
        }
        let identity = self.identify(delay).ok_or(Absent::NoIdentity)?;
        let identity = ata::identify(&identity).map_err(Absent::Unsupported)?;
        // Its commands are the 48-bit ones.
        if !identity.lba48 {
            return Err(Absent::Unsupported(ata::Unsupported::No48Bit));
        }
        let slots = ((self.cap >> 8) & 0x1f) as u8 + 1;
        let queued = self.cap & hba::CAP_SNCQ != 0 && identity.queue_depth > 1;
        let trim_entries = (BUFFER_SIZE / ata::TRIM_ENTRY) as u64;
        self.info = Info {
            sectors: identity.sectors,
            sector_size: identity.logical_sector,
            block_size: identity.physical_sector,
            depth: if queued { slots.min(identity.queue_depth) } else { 1 },
            queued,
            write_cache: identity.write_cache,
            fua: identity.fua,
            trim_sectors: if identity.trim_blocks > 0 {
                trim_entries * ata::TRIM_RANGE_MAX
            } else {
                0
            },
            rotational: identity.rotational,
        };
        self.identity = Some(identity);
        self.regs.write32(reg::IS, u32::MAX);
        self.set_interrupts(irq::ENABLED);
        Ok(())
    }

    /// Readies the optical drive on the port: asks it what it is (IDENTIFY
    /// PACKET DEVICE), and turns its interrupts on.
    fn start_packet(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Absent> {
        if !self.start_engine() {
            return Err(Absent::Stuck);
        }
        let mut bytes = [0u8; BUFFER_SIZE];
        self.command_waited(Taskfile::identify_packet(), delay, &mut bytes)
            .ok_or(Absent::NoIdentity)?;
        let identity = ata::identify_packet(&ata::words(&bytes)).map_err(Absent::Unsupported)?;
        self.packet = Some(identity);
        // Its interrupts come on with each command (`Port::packet`), once
        // its thread is there to wait for them.
        self.regs.write32(reg::IS, u32::MAX);
        self.set_interrupts(0);
        Ok(())
    }

    /// Waits until the disk is neither busy nor waiting to move data;
    /// false if it stays so.
    fn wait_ready(&self, delay: &mut dyn FnMut(u32)) -> bool {
        wait_for(delay, READY_US, POLL_US, || {
            u32::from(self.status()) & u32::from(ata::STATUS_BSY | ata::STATUS_DRQ) == 0
        })
    }

    /// The disk's status register, as the port last saw it.
    fn status(&self) -> u8 {
        self.regs.read32(reg::TFD) as u8
    }

    /// Starts the command engine, which needs frame receiving on and the
    /// engine stopped; false if it is not.
    fn start_engine(&mut self) -> bool {
        let command = self.regs.read32(reg::CMD);
        if command & cmd::FRE == 0 || command & cmd::CR != 0 {
            return false;
        }
        self.regs.write32(reg::CMD, command | cmd::ST);
        true
    }

    /// IDENTIFY DEVICE, waited for: the 256 words, or `None` if the disk
    /// fails it or does not answer.
    fn identify(&mut self, delay: &mut dyn FnMut(u32)) -> Option<[u16; 256]> {
        let mut bytes = [0u8; BUFFER_SIZE];
        self.command_waited(Taskfile::identify(), delay, &mut bytes)?;
        Some(ata::words(&bytes))
    }

    /// Carries out a command that moves at most one 512-byte block into
    /// the port's buffer, alone, on slot 0, and waits for it; copies what
    /// the disk sent into `out`. Nothing else may be in flight.
    fn command_waited(
        &mut self,
        taskfile: Taskfile,
        delay: &mut dyn FnMut(u32),
        out: &mut [u8; BUFFER_SIZE],
    ) -> Option<()> {
        let buffer = self.mem().phys() + BUFFER as u64;
        self.fill(0, taskfile, &[(buffer, BUFFER_SIZE as u32)], false);
        device_barrier();
        self.regs.write32(reg::CI, 1);
        let mut failed = false;
        let finished = wait_for(delay, COMMAND_US, COMMAND_POLL_US, || {
            failed = self.regs.read32(reg::IS) & irq::STOPS != 0;
            failed || self.regs.read32(reg::CI) & 1 == 0
        });
        self.regs.write32(reg::IS, u32::MAX);
        if !finished || failed || self.status() & ata::STATUS_ERR != 0 {
            return None;
        }
        device_barrier();
        self.mem().read(BUFFER, out);
        Some(())
    }

    /// Fills in slot `slot`: its command table's register frame and
    /// buffer list, and its header.
    fn fill(&self, slot: usize, taskfile: Taskfile, data: &[(u64, u32)], write: bool) {
        let memory = self.mem();
        let table = TABLES + slot * TABLE_SIZE;
        let fis = taskfile.fis();
        memory.write(table, &fis);
        for (i, &(phys, len)) in data.iter().enumerate() {
            let entry = table + TABLE_PRDT + i * PRD_SIZE;
            memory.write64(entry, phys);
            memory.write32(entry + 8, 0);
            // The byte count less one; the top bit, an interrupt when the
            // entry is done, stays clear.
            memory.write32(entry + 12, len - 1);
        }
        let header = COMMAND_LIST + slot * HEADER_SIZE;
        let flags = HEADER_CFL | if write { HEADER_WRITE } else { 0 };
        memory.write32(header, flags | (data.len() as u32) << 16);
        memory.write32(header + 4, 0);
        memory.write64(header + 8, memory.phys() + table as u64);
    }

    /// What the disk is and does.
    pub fn info(&self) -> Info {
        self.info
    }

    /// What the disk says of itself.
    pub fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }

    /// What the optical drive on the port says of itself, if it has one
    /// (an ATAPI device), which takes commands through [`Packets`]
    /// instead of requests.
    pub fn packet_identity(&self) -> Option<&PacketIdentity> {
        self.packet.as_ref()
    }

    /// Whether `op` with `segments` buffers would find room now. A command
    /// that goes out alone finds it only while nothing is in flight, and
    /// a queued one only while no command that goes out alone is.
    pub fn room(&self, op: Op) -> bool {
        if self.dead || self.stopped.is_some() {
            // `submit` says why straight away.
            return true;
        }
        if self.alone(op) {
            self.busy == 0
        } else {
            !self.alone && self.busy.count_ones() < u32::from(self.info.depth)
        }
    }

    /// Whether `op` goes out as a command alone: only while nothing else
    /// is in flight.
    pub fn alone(&self, op: Op) -> bool {
        !self.info.queued || matches!(op, Op::Flush | Op::Trim(_) | Op::Smart(..))
    }

    /// Starts a request: `op` on the disk's logical sectors from `sector`,
    /// with `data` buffers (physical address, length) that together cover
    /// whole sectors, none for a flush or a trim. Returns its slot, which
    /// [`Port::complete`] gives back. The buffers must stay the request's
    /// alone until then.
    pub fn submit(&mut self, op: Op, sector: u64, data: &[(u64, u32)]) -> Result<u8, Error> {
        if self.dead {
            return Err(Error::Dead);
        }
        if self.stopped.is_some() {
            return Err(Error::Stopped);
        }
        let sectors = self.check(op, sector, data)?;
        if !self.room(op) {
            return Err(Error::Full);
        }
        let slot = (!self.busy).trailing_zeros() as usize;
        let alone = self.alone(op);
        let wide = self.cap & hba::CAP_S64A != 0;
        if !wide && data.iter().any(|&(phys, len)| phys + u64::from(len) > 1 << 32) {
            return Err(Error::Unreachable);
        }
        // A count of 0 in the register means 65,536 sectors.
        let count = sectors as u16;
        let write = !matches!(op, Op::Read);
        let fua = op == Op::WriteThrough;
        match op {
            Op::Read | Op::Write | Op::WriteThrough => {
                let taskfile = if alone {
                    Taskfile::dma(write, fua, sector, count)
                } else {
                    Taskfile::queued(write, fua, sector, count, slot as u8)
                };
                self.fill(slot, taskfile, data, write);
            }
            Op::Flush => self.fill(slot, Taskfile::flush(), &[], false),
            Op::Smart(feature, low) => {
                self.fill(slot, Taskfile::smart(feature, low), data, false);
            }
            Op::Trim(length) => {
                let memory = self.mem();
                let mut entries = [0u8; BUFFER_SIZE];
                let (mut at, mut left) = (sector, length);
                for entry in entries.as_chunks_mut::<{ ata::TRIM_ENTRY }>().0 {
                    let take = left.min(ata::TRIM_RANGE_MAX);
                    if take == 0 {
                        break;
                    }
                    *entry = ata::trim_entry(at, take);
                    at += take;
                    left -= take;
                }
                memory.write(BUFFER, &entries);
                let buffer = (memory.phys() + BUFFER as u64, BUFFER_SIZE as u32);
                self.fill(slot, Taskfile::trim(1), &[buffer], true);
            }
        }
        device_barrier();
        let bit = 1 << slot;
        if !alone {
            self.regs.write32(reg::SACT, bit);
        }
        self.regs.write32(reg::CI, bit);
        self.busy |= bit;
        self.alone = alone;
        Ok(slot as u8)
    }

    /// Checks a request; returns how many sectors it moves.
    fn check(&self, op: Op, sector: u64, data: &[(u64, u32)]) -> Result<u64, Error> {
        let fits =
            |sectors: u64| sector.checked_add(sectors).is_some_and(|end| end <= self.info.sectors);
        match op {
            Op::Read | Op::Write | Op::WriteThrough => {
                let bytes: u64 = data.iter().map(|&(_, len)| u64::from(len)).sum();
                let size = u64::from(self.info.sector_size);
                let sectors = bytes / size;
                let even = data
                    .iter()
                    .all(|&(phys, len)| len > 0 && len.is_multiple_of(2) && phys.is_multiple_of(2));
                if data.is_empty()
                    || data.len() > MAX_SEGMENTS
                    || !even
                    || !bytes.is_multiple_of(size)
                    || !(1..=65_536).contains(&sectors)
                    || !fits(sectors)
                {
                    return Err(Error::BadRequest);
                }
                Ok(sectors)
            }
            Op::Flush => {
                if data.is_empty() {
                    Ok(0)
                } else {
                    Err(Error::BadRequest)
                }
            }
            Op::Smart(feature, _) => {
                let on = self.identity.as_ref().and_then(|id| id.smart) == Some(true);
                match data {
                    [] if on && feature == ata::smart::EXECUTE_OFFLINE => Ok(0),
                    &[(phys, len)]
                        if on
                            && feature != ata::smart::EXECUTE_OFFLINE
                            && len == BUFFER_SIZE as u32
                            && phys.is_multiple_of(2) =>
                    {
                        Ok(0)
                    }
                    _ => Err(Error::BadRequest),
                }
            }
            Op::Trim(sectors) => {
                if !data.is_empty()
                    || sectors == 0
                    || sectors > self.info.trim_sectors
                    || !fits(sectors)
                {
                    return Err(Error::BadRequest);
                }
                Ok(sectors)
            }
        }
    }

    /// The next finished request: its slot and how it ended. Runs in the
    /// interrupt handler, or in threads that poll. Clears the port's
    /// interrupt status first, so a command that finishes later raises a
    /// new interrupt.
    pub fn complete(&mut self) -> Option<(u8, Outcome)> {
        if self.done | self.failed | self.retry == 0 && self.stopped.is_none() && !self.dead {
            self.collect();
        }
        for (set, outcome) in [
            (&mut self.done, Outcome::Done),
            (&mut self.failed, Outcome::Failed),
            (&mut self.retry, Outcome::Retry),
        ] {
            if *set != 0 {
                let slot = set.trailing_zeros();
                *set &= !(1 << slot);
                return Some((slot as u8, outcome));
            }
        }
        None
    }

    /// Takes in the slots that finished since it last looked, or the
    /// error that stopped the port.
    fn collect(&mut self) {
        let status = self.regs.read32(reg::IS);
        if status != 0 {
            self.regs.write32(reg::IS, status);
        }
        if status & irq::STOPS != 0 {
            self.set_interrupts(0);
            self.stopped = Some(status);
            return;
        }
        if status & irq::INFS != 0 {
            self.soft_errors = self.soft_errors.wrapping_add(1);
            self.regs.write32(reg::SERR, u32::MAX);
        }
        let active = self.regs.read32(reg::CI) | self.regs.read32(reg::SACT);
        let finished = self.busy & !active;
        if finished != 0 {
            device_barrier();
            self.busy &= !finished;
            self.done |= finished;
            if self.busy == 0 {
                self.alone = false;
            }
        }
    }

    /// Whether an error stopped the port, which then needs
    /// [`Port::recover`].
    pub fn stopped(&self) -> bool {
        self.stopped.is_some()
    }

    /// The port's interrupt status when an error stopped it, until it
    /// recovers: for the log.
    pub fn stop_status(&self) -> Option<u32> {
        self.stopped
    }

    /// Whether the disk is gone: every request fails.
    pub fn dead(&self) -> bool {
        self.dead
    }

    /// How many errors stopped the port, and how many the link recovered
    /// from by itself.
    pub fn errors(&self) -> (u32, u32) {
        (self.errors, self.soft_errors)
    }

    /// Restarts a port an error stopped: stops the command engine, clears
    /// the error, clears a stuck busy bit or resets the link when it must,
    /// and starts the engine again. The command the disk failed is handed
    /// out as [`Outcome::Failed`] when the driver can tell which it was
    /// and the disk could not read or find its sectors, and every other
    /// one in flight as [`Outcome::Retry`]. If the port
    /// cannot be restarted, the disk is dead, and every request in flight
    /// fails. `delay` waits the given number of microseconds.
    pub fn recover(&mut self, delay: &mut dyn FnMut(u32)) {
        let Some(status) = self.stopped.take() else { return };
        self.recover_from(status, delay);
        self.interrupts_on();
    }

    /// Turns the port's interrupts back on after recovery, unless the
    /// disk is gone.
    fn interrupts_on(&mut self) {
        if !self.dead {
            self.regs.write32(reg::IS, u32::MAX);
            self.set_interrupts(irq::ENABLED);
        }
    }

    fn recover_from(&mut self, status: u32, delay: &mut dyn FnMut(u32)) {
        self.errors = self.errors.wrapping_add(1);
        let in_flight = self.busy;
        let queued = self.info.queued && !self.alone && in_flight != 0;
        let current = (self.regs.read32(reg::CMD) >> cmd::CCS_SHIFT) as u8 & 0x1f;
        let error = (self.regs.read32(reg::TFD) >> 8) as u8;
        self.busy = 0;
        self.alone = false;
        if !self.restart(status & irq::FATAL != 0, delay) {
            self.dead = true;
            self.failed |= in_flight;
            return;
        }
        // Only a disk's own error names a command; an error of the link
        // or of the controller has every command issued again.
        let culprit = if status & irq::TFES == 0 {
            None
        } else if queued {
            // The disk takes no command until it has given its NCQ error
            // log; without the log, a link reset clears its error.
            match self.ncq_error(delay) {
                Some(named) => named,
                None => {
                    if !self.restart(true, delay) {
                        self.dead = true;
                        self.failed |= in_flight;
                        return;
                    }
                    None
                }
            }
        } else {
            Some((current, error))
        };
        // The command the disk names fails if it could not read or find
        // its sectors, as it would again; after any other error it goes
        // again with the rest (as often as the queue lets it).
        match culprit {
            Some((slot, error))
                if in_flight & (1 << slot) != 0 && error & ata::ERROR_MEDIA != 0 =>
            {
                self.failed |= 1 << slot;
                self.retry |= in_flight & !(1 << slot);
            }
            _ => self.retry |= in_flight,
        }
    }

    /// Resets the link of a port whose requests stopped being answered:
    /// every request in flight is handed out to be issued again, or failed
    /// if the disk does not come back, or a different disk does.
    pub fn reset(&mut self, delay: &mut dyn FnMut(u32)) {
        self.set_interrupts(0);
        let in_flight = self.busy;
        self.busy = 0;
        self.alone = false;
        self.stopped = None;
        self.errors = self.errors.wrapping_add(1);
        let same = self.restart(true, delay) && {
            let before = self.identity.map(|id| (id.serial, id.sectors));
            let words = self.identify(delay);
            let after =
                words.and_then(|w| ata::identify(&w).ok()).map(|id| (id.serial, id.sectors));
            before.is_some() && before == after
        };
        if same {
            self.retry |= in_flight;
        } else {
            self.dead = true;
            self.failed |= in_flight;
        }
        self.interrupts_on();
    }

    /// Stops the engine, clears what the error left, and starts it again;
    /// with `link`, or when the disk stays busy, resets the link first.
    /// False if the port does not come back.
    fn restart(&mut self, link: bool, delay: &mut dyn FnMut(u32)) -> bool {
        let stopped = self.stop_engine(delay);
        self.regs.write32(reg::SERR, u32::MAX);
        self.regs.write32(reg::IS, u32::MAX);
        let busy = u32::from(self.status()) & u32::from(ata::STATUS_BSY | ata::STATUS_DRQ) != 0;
        let mut relink = link || !stopped;
        if busy && !relink {
            relink = !self.override_busy(delay);
        }
        if relink {
            if !stopped && !self.stop_engine(delay) {
                return false;
            }
            self.begin_link_reset();
            delay(COMRESET_US);
            self.end_link_reset();
            let mut since = 0;
            let up = wait_for(delay, LINK_LIMIT_US, LINK_POLL_US, || {
                since += 1;
                self.link() == DET_UP && since > 1
            });
            self.regs.write32(reg::SERR, u32::MAX);
            if !up || !self.wait_ready(delay) {
                return false;
            }
            self.regs.write32(reg::IS, u32::MAX);
        }
        self.start_engine()
    }

    /// Clears a busy bit the disk left set (command list override); false
    /// if the controller cannot or the bit stays.
    fn override_busy(&mut self, delay: &mut dyn FnMut(u32)) -> bool {
        if self.cap & hba::CAP_SCLO == 0 {
            return false;
        }
        let command = self.regs.read32(reg::CMD);
        self.regs.write32(reg::CMD, command | cmd::CLO);
        wait_for(delay, STOP_US, POLL_US, || self.regs.read32(reg::CMD) & cmd::CLO == 0)
            && u32::from(self.status()) & u32::from(ata::STATUS_BSY | ata::STATUS_DRQ) == 0
    }

    /// Reads the disk's NCQ Command Error log: `Some` with the tag and
    /// error register of the command it names, if it names one, or `None`
    /// if the log cannot be read.
    fn ncq_error(&mut self, delay: &mut dyn FnMut(u32)) -> Option<Option<(u8, u8)>> {
        let mut page = [0u8; BUFFER_SIZE];
        self.command_waited(Taskfile::read_log(ata::LOG_NCQ_ERROR), delay, &mut page)?;
        Some(ata::ncq_error(&page))
    }
}

/// How a packet command [`Port::send_packet`] started stands once it is
/// over ([`Port::packet_ended`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketEnd {
    /// It is done, and the controller says this many bytes moved.
    Done(u32),
    /// An error stopped the port, with this interrupt status. The port's
    /// interrupt is off until [`Port::packet_reply`] restarts it.
    Stopped(u32),
}

impl Port {
    /// Starts `cdb` on the optical drive on the port, with `data`, alone,
    /// on slot 0, and turns the port's interrupt on. [`Port::packet_ended`]
    /// says when it is over, and [`Port::packet_reply`] how it went. Data
    /// in or out of the caller's memory go through the port's own buffer,
    /// by PIO; data in buffers it reaches go by DMA if the drive has it
    /// and the length is a multiple of 16 bytes (Linux's libata's rule:
    /// many drives fail DMA of less), else by PIO. Nothing goes to a
    /// drive that is gone ([`TransportError::Gone`]), nor a command longer
    /// than the drive's packets, nor buffers the controller cannot take.
    /// Without `interrupt`, a command that moves its data by DMA goes with
    /// the port's interrupt off, for its sender to poll for its end in
    /// memory ([`Port::packet_over`]); any other turns it on.
    pub fn send_packet(
        &mut self,
        cdb: &Cdb,
        data: &Data<'_>,
        interrupt: bool,
    ) -> Result<(), TransportError> {
        let identity = self.packet.ok_or(TransportError::Failed("it is not an optical drive"))?;
        if self.dead {
            return Err(TransportError::Gone);
        }
        let command = cdb.bytes();
        if command.len() > usize::from(identity.packet_length) {
            return Err(TransportError::Failed("a command longer than the drive's packets"));
        }
        let total = data.len();
        let into_host = data.into_host();
        let own = [(self.mem().phys() + PACKET_BUFFER as u64, total as u32)];
        let (frames, dma): (&[(u64, u32)], bool) = match data {
            Data::None => (&[], false),
            Data::In(bytes) if bytes.len() <= PACKET_BUFFER_SIZE => (&own, false),
            Data::Out(bytes) if bytes.len() <= PACKET_BUFFER_SIZE => {
                self.mem().write(PACKET_BUFFER, bytes);
                (&own, false)
            }
            Data::In(_) | Data::Out(_) => {
                return Err(TransportError::Failed("more data than the port's buffer holds"));
            }
            Data::Frames { frames, .. } => {
                let even = frames
                    .iter()
                    .all(|&(phys, len)| len > 0 && len.is_multiple_of(2) && phys.is_multiple_of(2));
                let wide = self.cap & hba::CAP_S64A != 0;
                let reached = |&(phys, len): &(u64, u32)| wide || phys + u64::from(len) <= 1 << 32;
                if frames.is_empty() || frames.len() > MAX_SEGMENTS || !even {
                    return Err(TransportError::Failed("a malformed request"));
                }
                if !frames.iter().all(reached) {
                    return Err(TransportError::Failed("a buffer the controller cannot reach"));
                }
                (frames, identity.dma && total.is_multiple_of(16))
            }
        };
        let taskfile = Taskfile::packet(dma, identity.dmadir && into_host, total as u32);
        self.fill(0, taskfile, frames, total > 0 && !into_host);
        let memory = self.mem();
        let mut acmd = [0u8; 16];
        acmd[..command.len()].copy_from_slice(command);
        memory.write(TABLES + TABLE_ACMD, &acmd);
        let header = COMMAND_LIST;
        memory.write32(header, memory.read32(header) | HEADER_ATAPI);
        // The device's last answer forgotten, so that its next shows.
        memory.write32(RECEIVED_D2H, 0);
        device_barrier();
        // A DMA transfer ends with a Register FIS, which can be polled
        // for; a PIO transfer's last status may come in a PIO Setup FIS.
        self.polled = !interrupt && dma;
        // Each register written is a trap to the hypervisor in a virtual
        // machine: a read sent as the last read left the interrupt writes
        // only the command issue register (ADR 0434).
        let wanted = if self.polled { 0 } else { irq::ENABLED };
        if self.interrupts != Some(wanted) {
            if wanted != 0 {
                // What the port said while its interrupt was off was
                // looked at already, or is old.
                self.regs.write32(reg::IS, u32::MAX);
            }
            self.set_interrupts(wanted);
        }
        self.regs.write32(reg::CI, 1);
        Ok(())
    }

    /// Whether the packet command [`Port::send_packet`] started is over,
    /// as cheaply as can be told: one sent with the interrupt off from
    /// what the controller wrote to memory, with one register read to see
    /// that it let go of the command; one sent with the interrupt on from
    /// the registers ([`Port::packet_ended`]) once its interrupt came
    /// (`interrupted`), or once memory shows its end. `None` while it
    /// runs.
    pub fn packet_over(&mut self, interrupted: bool) -> Option<PacketEnd> {
        // The frame's type and the device's status, in its first word.
        let frame = self.mem().read32(RECEIVED_D2H);
        let answered = frame as u8 == FIS_TYPE_D2H;
        if self.polled && self.interrupts == Some(0) {
            if !answered {
                return None;
            }
            if (frame >> 16) as u8 & ata::STATUS_ERR != 0 {
                // The error and how the port stopped are in the registers.
                return self.packet_ended();
            }
            // The controller may copy the frame before it clears the
            // command's bit.
            if self.regs.read32(reg::CI) & 1 != 0 {
                return None;
            }
            device_barrier();
            return Some(PacketEnd::Done(self.mem().read32(COMMAND_LIST + HEADER_PRDBC)));
        }
        if interrupted || (self.polled && answered) {
            return self.packet_ended();
        }
        None
    }

    /// Turns the port's interrupt on for the packet command sent with it
    /// off ([`Port::send_packet`]), which a thread is about to sleep for.
    /// Its status is left as it is, so that an end or an error already
    /// there raises the interrupt at once.
    pub fn interrupt_at_end(&mut self) {
        if self.interrupts != Some(irq::ENABLED) {
            self.set_interrupts(irq::ENABLED);
        }
    }

    /// Whether the port has an interrupt status set: something it raised
    /// its interrupt for that no one has looked at yet.
    pub fn status_set(&self) -> bool {
        self.regs.read32(reg::IS) != 0
    }

    /// Turns the port's interrupt off until its next packet command, so
    /// that its status stays for a thread to read without the interrupt
    /// coming again meanwhile.
    pub fn quiet(&mut self) {
        self.set_interrupts(0);
    }

    /// Forgets whether the port's interrupt is on, after the controller's
    /// interrupt handler may have turned it off with [`crate::Hba::quiet`]
    /// while the port was out of reach: the next packet command turns it
    /// on again.
    pub fn forget_interrupts(&mut self) {
        self.interrupts = None;
    }

    /// Sets the interrupts the port raises, and remembers them.
    fn set_interrupts(&mut self, mask: u32) {
        self.regs.write32(reg::IE, mask);
        self.interrupts = Some(mask);
    }

    /// Whether the packet command [`Port::send_packet`] started is over,
    /// without waiting: `None` while it runs. Runs in the port's interrupt
    /// handler as well as in threads. Clears the port's interrupt status,
    /// so that what happens next raises a new interrupt; an error that
    /// stopped the port turns its interrupt off.
    pub fn packet_ended(&mut self) -> Option<PacketEnd> {
        let status = self.regs.read32(reg::IS);
        if status != 0 {
            self.regs.write32(reg::IS, status);
        }
        if status & irq::STOPS != 0 {
            self.set_interrupts(0);
            return Some(PacketEnd::Stopped(status));
        }
        if self.regs.read32(reg::CI) & 1 != 0 {
            return None;
        }
        device_barrier();
        Some(PacketEnd::Done(self.mem().read32(COMMAND_LIST + HEADER_PRDBC)))
    }

    /// How the packet command [`Port::send_packet`] started with `total`
    /// bytes went, now that it ended so: a short transfer says its
    /// residue. After an error, restarts the port: the drive's own error
    /// leaves the link up, and its sense is asked for (REQUEST SENSE), as
    /// Linux's libata asks; a drive that does not answer that has the
    /// sense key of its error register. Any other error resets the link.
    /// Fails with [`TransportError::Gone`] once the port does not come
    /// back. `wait` waits the given number of microseconds, or until the
    /// port's interrupt, whichever comes first.
    pub fn packet_reply(
        &mut self,
        end: PacketEnd,
        total: u32,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Reply, TransportError> {
        match end {
            PacketEnd::Done(moved) => Ok(Reply::Good { residue: total - moved.min(total) }),
            PacketEnd::Stopped(status) => match self.restart_after(status, wait)? {
                Some(error) => Ok(Reply::Failed(Some(self.sense(error, wait)))),
                None => Err(TransportError::Failed("the link failed")),
            },
        }
    }

    /// Resets the link of a port whose packet command went unanswered:
    /// what the command's reply is then.
    pub fn packet_timed_out(&mut self, wait: &mut dyn FnMut(u32)) -> TransportError {
        self.set_interrupts(0);
        self.errors = self.errors.wrapping_add(1);
        if !self.restart(true, wait) {
            self.dead = true;
            return TransportError::Gone;
        }
        TransportError::Failed("it did not answer in time; the link was reset")
    }

    /// Restarts the port after the error `status` stopped it: the drive's
    /// error register when the drive failed the command itself, `None`
    /// when the link or the controller failed, and the link was reset.
    fn restart_after(
        &mut self,
        status: u32,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Option<u8>, TransportError> {
        let error = (self.regs.read32(reg::TFD) >> 8) as u8;
        let own_error = status & irq::TFES != 0 && status & irq::FATAL == 0;
        self.errors = self.errors.wrapping_add(1);
        if !self.restart(!own_error, wait) {
            self.dead = true;
            return Err(TransportError::Gone);
        }
        Ok(own_error.then_some(error))
    }

    /// Waits up to `timeout_us` with `wait` for the packet command
    /// [`Port::send_packet`] started to end, waiting longer each time it
    /// looks, up to 4 ms; resets the link if it does not.
    fn wait_packet(
        &mut self,
        timeout_us: u32,
        wait: &mut dyn FnMut(u32),
    ) -> Result<PacketEnd, TransportError> {
        let (mut waited, mut step) = (0u32, PACKET_FIRST_US);
        loop {
            if let Some(end) = self.packet_ended() {
                return Ok(end);
            }
            if waited >= timeout_us {
                return Err(self.packet_timed_out(wait));
            }
            wait(step);
            waited = waited.saturating_add(step);
            step = step.saturating_mul(2).min(PACKET_LONGEST_US);
        }
    }

    /// Carries out `cdb` with `data` as [`Port::send_packet`] has it,
    /// waiting up to `timeout_us` for it with `wait`, and copies what came
    /// into the port's own buffer to `data`'s.
    fn packet(
        &mut self,
        cdb: &Cdb,
        data: &mut Data<'_>,
        timeout_us: u32,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Reply, TransportError> {
        // The interrupt handler may have quieted the port after the last
        // command a thread waited for, behind its back.
        self.forget_interrupts();
        self.send_packet(cdb, data, true)?;
        let total = data.len() as u32;
        let end = self.wait_packet(timeout_us, wait)?;
        if let (PacketEnd::Done(moved), Data::In(bytes)) = (end, data) {
            let moved = moved.min(total) as usize;
            self.mem().read(PACKET_BUFFER, &mut bytes[..moved]);
        }
        self.packet_reply(end, total, wait)
    }

    /// Why the drive failed a command whose error register is `error`:
    /// its answer to REQUEST SENSE, or the sense key of the register if
    /// it does not answer that.
    fn sense(&mut self, error: u8, wait: &mut dyn FnMut(u32)) -> Sense {
        let fallback = Sense { key: error >> 4, asc: 0, ascq: 0 };
        let mut bytes = [0u8; SENSE_LENGTH as usize];
        let cdb = Cdb::request_sense(SENSE_LENGTH);
        self.forget_interrupts();
        let sent = self.send_packet(&cdb, &Data::In(&mut bytes), true);
        match sent.and_then(|()| self.wait_packet(SENSE_US, wait)) {
            Ok(PacketEnd::Done(moved)) => {
                let moved = (moved as usize).min(bytes.len());
                self.mem().read(PACKET_BUFFER, &mut bytes[..moved]);
                Sense::parse(&bytes[..moved]).unwrap_or(fallback)
            }
            // Not asked again: the port is restarted, and the key said.
            Ok(PacketEnd::Stopped(status)) => {
                let _ = self.restart_after(status, wait);
                fallback
            }
            Err(_) => fallback,
        }
    }
}

/// How long REQUEST SENSE after a failed command may take.
const SENSE_US: u32 = 10_000_000;

/// The optical drive on a port, as the [`Transport`] that carries its
/// SCSI commands (MMC-6; [`radix_scsi::mmc`]) in ATA packets, one at a
/// time, from the thread that runs the drive. A command the drive fails is
/// followed by REQUEST SENSE, whose answer is the reply's sense
/// ([`Port::packet_reply`]). Its one logical unit is 0.
pub struct Packets<'a> {
    port: &'a mut Port,
    wait: &'a mut dyn FnMut(u32),
}

impl<'a> Packets<'a> {
    /// The commands of the optical drive on `port`, which wait with
    /// `wait`: it waits the given number of microseconds, or until the
    /// port's interrupt, whichever comes first.
    pub fn new(port: &'a mut Port, wait: &'a mut dyn FnMut(u32)) -> Packets<'a> {
        Packets { port, wait }
    }
}

impl Transport for Packets<'_> {
    fn command(
        &mut self,
        lun: u8,
        cdb: &Cdb,
        mut data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<Reply, TransportError> {
        if lun != 0 {
            let none = Sense { key: radix_scsi::key::ILLEGAL_REQUEST, asc: 0x25, ascq: 0 };
            return Ok(Reply::Failed(Some(none)));
        }
        self.port.packet(cdb, &mut data, timeout_ms.saturating_mul(1_000), self.wait)
    }
}

/// Stops the port before its memory goes, so that the controller writes
/// to none of it after; if it does not stop, the memory is never given
/// back.
impl Drop for Port {
    fn drop(&mut self) {
        self.set_interrupts(0);
        // Without `delay`, it looks as many times as it would wait in
        // milliseconds; a port that has not stopped by then keeps its
        // memory.
        let mut spin = |_: u32| core::hint::spin_loop();
        let halted = self.halt(&mut spin);
        if let Some(memory) = self.memory.take()
            && !halted
        {
            core::mem::forget(memory);
        }
    }
}

/// The links of the ports being set up, while they come up after their
/// reset.
pub(crate) struct Links {
    /// The ports still coming up, a bit each.
    waiting: u32,
    up: u32,
    present: u32,
    last: [u32; 32],
    since: [u32; 32],
}

impl Links {
    pub(crate) fn new(ports: &[Option<Port>]) -> Links {
        let mut waiting = 0;
        for (index, port) in ports.iter().enumerate() {
            if port.is_some() {
                waiting |= 1 << index;
            }
        }
        Links { waiting, up: 0, present: 0, last: [0; 32], since: [0; 32] }
    }

    /// Waits until every link is up, or has stayed down long enough to be
    /// empty, or has had all the time a link gets.
    pub(crate) fn settle(&mut self, ports: &[Option<Port>], delay: &mut dyn FnMut(u32)) {
        let mut now = 0;
        while self.waiting != 0 {
            for (index, port) in ports.iter().enumerate() {
                let Some(port) = port else { continue };
                if self.waiting & (1 << index) == 0 {
                    continue;
                }
                let det = port.link();
                if det != self.last[index] || now == 0 {
                    self.last[index] = det;
                    self.since[index] = now;
                }
                let steady = now - self.since[index];
                if det != 0 {
                    self.present |= 1 << index;
                }
                // Up once it reads so twice; empty once it has read no
                // device for a while; one that is there but not up gets
                // until the limit.
                let settled = (det == DET_UP && steady >= LINK_POLL_US)
                    || (det != DET_PRESENT && det != DET_UP && steady >= LINK_EMPTY_US)
                    || now >= LINK_LIMIT_US;
                if settled {
                    self.waiting &= !(1 << index);
                    if det == DET_UP {
                        self.up |= 1 << index;
                    }
                }
            }
            if self.waiting != 0 {
                delay(LINK_POLL_US);
                now += LINK_POLL_US;
            }
        }
    }

    /// Whether port `index`'s link came up.
    pub(crate) fn up(&self, index: usize) -> bool {
        self.up & (1 << index) != 0
    }

    /// Whether a device showed on port `index` at all.
    pub(crate) fn present(&self, index: usize) -> bool {
        self.present & (1 << index) != 0
    }
}
