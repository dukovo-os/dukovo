// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PCI IDE controllers (`radix-ide`, Phase 11g, ADR 0244): the ATA disks
//! on their channels, and the SATA disks of a chipset whose firmware sets
//! its SATA controller to IDE mode. Each disk is named `sda` and on, as
//! the SATA disks on AHCI controllers are, and as Linux names them.
//!
//! [`bind`] turns on the controller's I/O ports and bus master, finds each
//! channel's ports (the legacy ones, or BARs 0 to 3 in native mode) and
//! its bus-master registers (BAR 4), and asks both ends of each channel
//! what they are. A channel takes one command at a time for its two
//! disks, so it is one lane of one queue ([`Disk`]), and each disk is a
//! volume of it, as an NVMe controller's namespaces are: the master is
//! target 0, the slave 1. The channels are polled, by the threads waiting
//! for them: the kernel routes neither the legacy IRQs 14 and 15 nor an
//! IDE controller's INTx.
//!
//! The bus master reaches only the first 4 GiB, which page caches are not
//! kept to: a request whose buffers all lie below it goes to them, and
//! any other through a bounce buffer of [`SEGMENTS`] pages below 4 GiB,
//! copied before a write and after a read.
//!
//! A CompactFlash card in an IDE adapter (ADR 0446) is a disk named as the
//! others are, whose sectors move by PIO: each request goes through the
//! bounce buffer, which the channel reads into or writes from a few
//! sectors at each poll. A card, or a disk from before ATA-6, without
//! 48-bit addresses takes 28-bit commands, of at most 256 sectors: a
//! request's [`SEGMENTS`] pages are no more.
//!
//! A SATA controller in IDE mode runs one command at a time on each
//! channel, with no queuing, hot-plug or link power management: when a
//! disk says it is Serial ATA, the log suggests setting the controller to
//! AHCI in the firmware's setup, as the installer will.
//!
//! An optical drive on a channel (ADR 0432) is run by a thread of its
//! own, as one on an AHCI port is ([`super::optical`]), which carries its
//! SCSI commands in PACKET commands ([`IdeLink`]). It shares the channel
//! with a disk on the other end: the thread borrows the channel from the
//! disks' queue between the disks' commands ([`Share`]), and while it
//! waits for it the queue starts no new one, so that neither waits for
//! long; it gives the channel back after each command and the sense of
//! one that failed.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use radix_ahci::ata::{PacketIdentity, trimmed};
use radix_block::{DeviceRef, Error, Info};
use radix_hal::{Devices, UserMode};
use radix_hw::Dma;
use radix_hw::pci::{Bar, Function, command};
use radix_ide::{Channel, Device, Op as IdeOp, Transfer, interface};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::{Cdb, Data, Reply, Sense, Transport, TransportError};
use radix_sync::SpinLock;

use super::queue::{self, Controller, Disk, Op, Outcome, Part, Request, Volume};
use super::{Entry, Ident, Name, Size, optical, partitions, register};
use crate::Arch;
use crate::ioports::IoWindow;

/// The most segments of a request, each at most a page: the bounce
/// buffer's pages.
const SEGMENTS: usize = 32;
const PAGE: usize = 4096;
/// The legacy ports are taken: by the first controller in compatibility
/// mode on each channel.
static LEGACY: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];

/// A channel, as its two ends share it: one command at a time, a disk's
/// from the disks' queue or an optical drive's from its thread.
pub(super) struct Share {
    channel: SpinLock<Channel<IoWindow>>,
    /// The end whose optical drive has borrowed the channel, plus one; 0
    /// while none has.
    holder: AtomicU8,
    /// The ends whose optical drives wait for the channel, a bit each:
    /// the disks' queue starts no new command meanwhile.
    wants: AtomicU8,
}

impl Share {
    /// Lends the channel to the optical drive at `end`, if no other has
    /// it and, as `disks_idle` says, no disk's command is in flight; if
    /// not, notes that it waits.
    fn lend(&self, end: u8, disks_idle: bool) -> bool {
        let bit = 1 << end;
        if disks_idle
            && self.holder.compare_exchange(0, end + 1, Ordering::AcqRel, Ordering::Acquire).is_ok()
        {
            self.wants.fetch_and(!bit, Ordering::AcqRel);
            return true;
        }
        self.wants.fetch_or(bit, Ordering::AcqRel);
        false
    }

    /// Whether the disks' queue may start a command: no optical drive has
    /// the channel or waits for it.
    fn free(&self) -> bool {
        self.holder.load(Ordering::Acquire) == 0 && self.wants.load(Ordering::Acquire) == 0
    }
}

/// What the channel knows of a disk on it.
#[derive(Clone, Copy, Default)]
struct Drive {
    present: bool,
    /// It has a write cache that is on, which FLUSH CACHE EXT empties.
    flush: bool,
    /// Its sectors move by PIO, through the bounce buffer: a CompactFlash
    /// card's.
    pio: bool,
}

/// The request in flight.
#[derive(Clone, Copy)]
struct Flight {
    slot: u8,
    /// A read through the bounce buffer, whose bytes go to the request's
    /// buffers, kept in `buffers`, once it is done.
    bounced_read: bool,
    /// Its sectors move by PIO.
    pio: bool,
}

/// A channel as the queue drives it.
pub struct IdeChannel {
    share: Arc<Share, Kmalloc>,
    drives: [Drive; 2],
    flight: Option<Flight>,
    /// A request done without the disk: a flush of one with no cache.
    done: Option<(u8, Outcome)>,
    /// The bounce buffer, below 4 GiB.
    bounce: Dma,
    /// The buffers of a bounced read; or the PRDs' buffers of one that
    /// goes to its own.
    buffers: Vec<(u64, u32), Kmalloc>,
    prds: Vec<(u32, u32), Kmalloc>,
    /// A page between a request's buffer and the bounce buffer.
    copy: Vec<u8, Kmalloc>,
    /// Why the last request failed, for the log.
    why: Option<(u8, &'static str)>,
    /// The channel's place, for the log: `primary` or `secondary`, and
    /// the controller's address.
    place: TextBuf<40>,
}

impl IdeChannel {
    /// Lends the channel to the optical drive at `end` if no disk's
    /// command is in flight ([`Share::lend`]). Runs under the queue's
    /// lock, as its requests start.
    fn lend(&self, end: u8) -> bool {
        self.share.lend(end, self.flight.is_none() && self.done.is_none())
    }

    /// Copies a bounced read's bytes to its buffers.
    fn copy_back(&mut self) {
        let mut at = self.bounce.phys();
        for &(phys, len) in &self.buffers {
            let part = &mut self.copy[..len as usize];
            <Arch as UserMode>::read_frames(at, part);
            <Arch as UserMode>::write_frames(phys, part);
            at += u64::from(len);
        }
    }

    /// The PRDs' buffers for `data`: themselves when all lie below 4 GiB
    /// and the drive does DMA (`!pio`), else the bounce buffer, filled for
    /// a write.
    fn place_buffers(
        &mut self,
        data: &[(u64, u32)],
        write: bool,
        pio: bool,
    ) -> Result<bool, Error> {
        self.prds.clear();
        self.buffers.clear();
        if data.len() > SEGMENTS || data.iter().any(|&(_, len)| len as usize > PAGE) {
            return Err(Error::OutOfRange);
        }
        if !pio && data.iter().all(|&(phys, len)| phys + u64::from(len) <= 1 << 32) {
            for &(phys, len) in data {
                let _ = self.prds.push_within_capacity((phys as u32, len));
            }
            return Ok(false);
        }
        let mut at = self.bounce.phys();
        for &(phys, len) in data {
            let _ = self.buffers.push_within_capacity((phys, len));
            if write {
                let part = &mut self.copy[..len as usize];
                <Arch as UserMode>::read_frames(phys, part);
                <Arch as UserMode>::write_frames(at, part);
            }
            at += u64::from(len);
        }
        let total = (at - self.bounce.phys()) as u32;
        let _ = self.prds.push_within_capacity((self.bounce.phys() as u32, total));
        Ok(true)
    }
}

impl Controller for IdeChannel {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        self.flight.is_none() && self.done.is_none() && self.share.free()
    }

    fn submit(&mut self, slot: u8, request: &Request<'_>, _more: bool) -> Result<(), Error> {
        let drive = u8::try_from(request.target).map_err(|_| Error::OutOfRange)?;
        let known = self.drives.get(usize::from(drive)).copied().unwrap_or_default();
        if !known.present {
            return Err(Error::OutOfRange);
        }
        let op = match request.op {
            Op::Read => IdeOp::Read,
            // The disk is not said to write through: the queue flushes.
            Op::Write | Op::WriteThrough => IdeOp::Write,
            Op::Flush if !known.flush => {
                self.done = Some((slot, Outcome::Done));
                return Ok(());
            }
            Op::Flush => IdeOp::Flush,
            Op::Discard(_) | Op::Smart(_) => return Err(Error::Unsupported),
        };
        // The disk counts in its own sectors, the request in 512 bytes.
        let unit = request.unit.max(1);
        let lba = request.sector / unit;
        let bytes: u64 = request.data.iter().map(|&(_, len)| u64::from(len)).sum();
        let sector = unit * radix_block::SECTOR;
        let count = u32::try_from(bytes / sector).map_err(|_| Error::OutOfRange)?;
        let pio = known.pio && op != IdeOp::Flush;
        let bounced_read = if op == IdeOp::Flush {
            self.prds.clear();
            false
        } else {
            self.place_buffers(request.data, op == IdeOp::Write, pio)? && op == IdeOp::Read
        };
        let mut channel = self.share.channel.lock();
        let started = if pio {
            channel.start_pio(drive, op, lba, count)
        } else {
            channel.start(drive, op, lba, count, &self.prds)
        };
        drop(channel);
        started.map_err(|err| match err {
            radix_ide::Error::BadRequest => Error::OutOfRange,
            _ => Error::Io,
        })?;
        self.flight = Some(Flight { slot, bounced_read, pio });
        Ok(())
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        if let Some(done) = self.done.take() {
            return Some(done);
        }
        let flight = self.flight?;
        let result = if flight.pio {
            self.share.channel.lock().poll_pio(&self.bounce)?
        } else {
            self.share.channel.lock().poll()?
        };
        self.flight = None;
        match result {
            Ok(()) => {
                if flight.bounced_read {
                    self.copy_back();
                }
                Some((flight.slot, Outcome::Done))
            }
            Err(err) => {
                let why = match err {
                    radix_ide::Error::Bus => "the bus master could not reach memory",
                    radix_ide::Error::Protocol => "the card asked to move more than was asked",
                    radix_ide::Error::Device { error, .. } if error & 1 << 6 != 0 => {
                        "the disk could not read a sector"
                    }
                    radix_ide::Error::Device { error, .. } if error & 1 << 2 != 0 => {
                        "the disk refused the command"
                    }
                    _ => "the disk failed the command",
                };
                self.why = Some((flight.slot, why));
                Some((flight.slot, Outcome::Failed))
            }
        }
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        self.why.filter(|&(failed, _)| failed == slot).map(|(_, why)| why)
    }

    fn reset(lanes: &mut [Part<IdeChannel>]) -> bool {
        for lane in lanes {
            let ready = lane.share.channel.lock().reset(&mut queue::delay);
            if let Some(flight) = lane.flight.take() {
                lane.done = Some((flight.slot, Outcome::Retry));
            }
            warn!(
                "Disk: a request on the IDE controller's {} went unanswered for 30 s; the \
                 channel was reset{}",
                lane.place,
                if ready { ", and what was in flight goes again" } else { ", and is not ready" }
            );
        }
        true
    }
}

/// The I/O ports of a BAR, or `None` if it is not in I/O space.
fn io_bar(function: &Function, index: usize) -> Option<(u16, u16)> {
    match function.bar(index)? {
        Bar::Io { port, size } => {
            let port = u16::try_from(port).ok()?;
            Some((port, u16::try_from(size).unwrap_or(u16::MAX)))
        }
        Bar::Memory { .. } => None,
    }
}

/// Sets up the IDE controller `function`, the disks on its channels,
/// counting them in `next`, and its optical drives, counting them in
/// `drives`. Kept out of the frame of `init`'s scan.
#[inline(never)]
pub fn bind(
    function: &Function,
    (next, drives): (&mut usize, &mut usize),
) -> Result<(), &'static str> {
    let (_, _, programming) = function.class();
    if programming & interface::BUS_MASTER == 0 {
        return Err("it has no bus-master DMA, which the kernel needs");
    }
    let Some((bus_master, size)) = io_bar(function, radix_ide::BUS_MASTER_BAR) else {
        return Err("its bus-master registers (BAR 4) are not in I/O space");
    };
    if size < 2 * radix_ide::BUS_MASTER_PORTS {
        return Err("its bus-master registers (BAR 4) are too few");
    }
    function.set_command(function.command() | command::IO | command::BUS_MASTER);
    let mut serial = false;
    for number in 0..2u8 {
        let native = programming
            & if number == 0 { interface::PRIMARY_NATIVE } else { interface::SECONDARY_NATIVE }
            != 0;
        let mut place = TextBuf::<40>::new();
        let which = if number == 0 { "primary" } else { "secondary" };
        let _ = write!(place, "{which} channel at {}", function.address());
        let ports = if native {
            let bar = 2 * usize::from(number);
            match (io_bar(function, bar), io_bar(function, bar + 1)) {
                // The control port is the third of its block's four.
                (Some((command, _)), Some((control, _))) => Some((command, control + 2)),
                _ => None,
            }
        } else if LEGACY[usize::from(number)].swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(radix_ide::LEGACY_PORTS[usize::from(number)])
        };
        let Some((command, control)) = ports else {
            info!("Disk: IDE {place}: its ports are not in I/O space, or taken; not used");
            continue;
        };
        let windows = (
            IoWindow::new(command, radix_ide::COMMAND_PORTS),
            IoWindow::new(control, 1),
            IoWindow::new(bus_master + u16::from(number) * 8, radix_ide::BUS_MASTER_PORTS),
        );
        match add_channel(function, windows, &place, (&mut *next, &mut *drives)) {
            Ok(found) => serial |= found,
            Err(why) => warn!("Disk: IDE {place}: {why}"),
        }
    }
    if serial {
        info!(
            "Disk: the SATA controller at {} is in IDE mode, which runs one command at a time \
             on each channel, with no queuing or hot-plug: setting it to AHCI in the \
             firmware's setup lets its disks run faster",
            function.address()
        );
    }
    Ok(())
}

/// What a channel's two ends are, and which are disks.
struct Found {
    devices: [Device; 2],
}

/// Sets up the channel of `windows` (its command block, control port and
/// bus-master registers), its disks, counted in `next`, and its optical
/// drives, counted in `drives`; whether a disk is Serial ATA.
#[inline(never)]
fn add_channel(
    function: &Function,
    (command, control, bus_master): (IoWindow, IoWindow, IoWindow),
    place: &TextBuf<40>,
    (next, drives): (&mut usize, &mut usize),
) -> Result<bool, &'static str> {
    let source = function.address();
    let mut alloc = |pages: usize, below: bool| <Arch as Devices>::alloc_dma(source, pages, below);
    let mut channel =
        Channel::new(command, control, bus_master, &mut alloc).ok_or("out of memory")?;
    if !channel.present() {
        info!("Disk: IDE {place}: empty");
        return Ok(false);
    }
    if !channel.reset(&mut queue::delay) {
        info!("Disk: IDE {place}: no device became ready after a reset");
    }
    let found = Found {
        devices: [channel.identify(0, &mut queue::delay), channel.identify(1, &mut queue::delay)],
    };
    let share = new_share(channel)?;
    let mut ends = [Drive::default(); 2];
    let mut serial = false;
    for (end, device) in found.devices.iter().enumerate() {
        let which = if end == 0 { "master" } else { "slave" };
        match device {
            Device::Absent | Device::Atapi(_) => {}
            Device::Unsupported(why) => {
                let why = radix_ahci::Absent::Unsupported(*why);
                info!("Disk: IDE {place}, {which}: {why}; not used");
            }
            Device::Ata { identity, serial: sata } => {
                let flush = identity.write_cache && identity.flush;
                ends[end] = Drive { present: true, flush, pio: identity.cfa };
                serial |= sata;
            }
        }
    }
    let disks = if ends.iter().any(|end| end.present) {
        Some(add_disks(source, &share, ends, &found, place, next)?)
    } else {
        None
    };
    for (end, device) in found.devices.iter().enumerate() {
        let Device::Atapi(identity) = device else { continue };
        let which = if end == 0 { "master" } else { "slave" };
        let link = new_link(source, share.clone(), disks.clone(), end as u8, *identity);
        let mut at = TextBuf::new();
        let _ = write!(at, "the IDE {place}, {which}");
        match link.and_then(|link| optical::add_ide(link, at, source)) {
            Ok(thread) => {
                optical::release(&thread);
                *drives += 1;
            }
            Err(why) => warn!("Disk: IDE {place}, {which}: {why}"),
        }
    }
    Ok(serial)
}

/// The channel, shared by its two ends, on the heap.
#[inline(never)]
fn new_share(channel: Channel<IoWindow>) -> Result<Arc<Share, Kmalloc>, &'static str> {
    let share = Share {
        channel: SpinLock::new(channel),
        holder: AtomicU8::new(0),
        wants: AtomicU8::new(0),
    };
    Arc::try_new_in(share, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")
}

/// Sets up the disks at the `ends` of the channel `share` that are disks,
/// as `found` says, counting them in `next`: one queue for both. Returns
/// the queue.
#[inline(never)]
fn add_disks(
    source: radix_hw::pci::Address,
    share: &Arc<Share, Kmalloc>,
    ends: [Drive; 2],
    found: &Found,
    place: &TextBuf<40>,
    next: &mut usize,
) -> Result<Arc<Disk<IdeChannel>, Kmalloc>, &'static str> {
    let bounce =
        <Arch as Devices>::alloc_dma(source, SEGMENTS, true).ok_or("out of memory below 4 GiB")?;
    let controller = new_channel(share.clone(), ends, bounce, place.clone())?;
    // Each disk's name; the queue is named after the first.
    let mut names: [Option<(u32, Name)>; 2] = [None, None];
    for (end, drive) in ends.iter().enumerate() {
        if drive.present {
            names[end] = super::take_sd_name();
        }
    }
    let give_back = |names: &[Option<(u32, Name)>; 2]| {
        for (letter, _) in names.iter().flatten() {
            super::give_back_sd_letter(*letter);
        }
    };
    if ends.iter().zip(&names).any(|(drive, name)| drive.present && name.is_none()) {
        give_back(&names);
        return Err("too many SATA and USB disks");
    }
    let first = names.iter().flatten().next().map_or_else(Name::new, |(_, name)| name.clone());
    let cache = ends.iter().any(|drive| drive.flush);
    let template = Info {
        sectors: 0,
        logical_block: 512,
        block_size: 512,
        read_only: false,
        max_segments: SEGMENTS,
        discard_sectors: 0,
        rotational: true,
    };
    let disk =
        new_disk(controller, template, cache, first, source).inspect_err(|_| give_back(&names))?;
    for (end, device) in found.devices.iter().enumerate() {
        let (Device::Ata { identity, serial }, Some((letter, name))) = (device, &names[end]) else {
            continue;
        };
        let which = if end == 0 { "master" } else { "slave" };
        if let Err(why) = add_disk(&disk, end as u32, identity, *serial, name, place, which) {
            super::give_back_sd_letter(*letter);
            warn!("Disk: IDE {place}, {which}: {why}");
        } else {
            *next += 1;
        }
    }
    Ok(disk)
}

/// The channel's part of the queue, on the heap.
#[inline(never)]
fn new_channel(
    share: Arc<Share, Kmalloc>,
    drives: [Drive; 2],
    bounce: Dma,
    place: TextBuf<40>,
) -> Result<Part<IdeChannel>, &'static str> {
    let heap = || HEAP.with(Flags::MAY_SLEEP);
    let mut buffers = Vec::new_in(heap());
    let mut prds = Vec::new_in(heap());
    let mut copy = Vec::new_in(heap());
    buffers.try_reserve_exact(SEGMENTS).map_err(|_| "out of memory")?;
    prds.try_reserve_exact(SEGMENTS).map_err(|_| "out of memory")?;
    copy.try_reserve_exact(PAGE).map_err(|_| "out of memory")?;
    while copy.len() < PAGE {
        let _ = copy.push_within_capacity(0);
    }
    let controller = IdeChannel {
        share,
        drives,
        flight: None,
        done: None,
        bounce,
        buffers,
        prds,
        copy,
        why: None,
        place,
    };
    queue::part(controller).ok_or("out of memory")
}

/// The queue of a channel, on the heap: polled.
#[inline(never)]
fn new_disk(
    controller: Part<IdeChannel>,
    template: Info,
    cache: bool,
    name: Name,
    source: radix_hw::pci::Address,
) -> Result<Arc<Disk<IdeChannel>, Kmalloc>, &'static str> {
    let once = core::iter::once(controller);
    let disk =
        Disk::new(once, template, cache, false, false, name, source).ok_or("out of memory")?;
    Arc::try_new_in(disk, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")
}

/// Sets up the disk at end `target` of the channel of `disk`, named
/// `name`.
fn add_disk(
    disk: &Arc<Disk<IdeChannel>, Kmalloc>,
    target: u32,
    identity: &radix_ahci::ata::Identity,
    serial: bool,
    name: &Name,
    place: &TextBuf<40>,
    which: &str,
) -> Result<(), &'static str> {
    // The disk counts in its own sectors, the block layer in 512 bytes.
    let unit = u64::from(identity.logical_sector) / radix_block::SECTOR;
    let info = Info {
        sectors: identity.sectors.saturating_mul(unit),
        logical_block: identity.logical_sector,
        block_size: identity.physical_sector,
        read_only: false,
        max_segments: SEGMENTS,
        discard_sectors: 0,
        // A disk that does not say spins, as Linux takes it; a card is
        // flash.
        rotational: identity.rotational.unwrap_or(!identity.cfa),
    };
    let volume = Volume::new(disk.clone(), target, info).ok_or("out of memory")?;
    let volume =
        Arc::try_new_in(volume, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    let kind = match (identity.cfa, serial) {
        (true, _) => "a CompactFlash card",
        (false, true) => "a SATA disk",
        (false, false) => "an ATA disk",
    };
    info!(
        "Disk: {name} is {kind} of {} on the IDE {place}, {which}: {}, {} bytes a block{}{}{}{}{}",
        Size(info.sectors * radix_block::SECTOR),
        trimmed(&identity.model),
        info.block_size,
        super::sector_note(&info),
        if identity.write_cache { ", write cache" } else { "" },
        if info.rotational { "" } else { ", solid state" },
        if identity.lba48 { "" } else { ", 28-bit addresses" },
        if identity.cfa { ", by PIO" } else { "" },
    );
    let device: DeviceRef = volume;
    let ident = Ident {
        model: identity.model,
        serial: super::padded(&identity.serial),
        firmware: identity.firmware,
    };
    register(Entry { name: name.clone(), device: device.clone(), place: None, ident: Some(ident) });
    partitions(name, &device);
    Ok(())
}

/// How long REQUEST SENSE after a failed command may take.
const SENSE_US: u32 = 10_000_000;
/// How long a packet command is first waited for, in microseconds; each
/// wait after doubles, to [`PACKET_LONGEST_US`].
const PACKET_FIRST_US: u32 = 10;
const PACKET_LONGEST_US: u32 = 4_000;
/// How often an optical drive's thread asks again for its channel while
/// a disk's command has it.
const BORROW_US: u32 = 500;

/// An optical drive on an IDE channel, as its thread reaches it.
pub(super) struct IdeLink {
    share: Arc<Share, Kmalloc>,
    /// The queue of the disks on the channel, if any: the channel is
    /// borrowed from it.
    disks: Option<Arc<Disk<IdeChannel>, Kmalloc>>,
    /// Which end it is: 0, the master, or 1.
    end: u8,
    identity: PacketIdentity,
    /// Below 4 GiB, for reads by DMA into memory the bus master does not
    /// reach: [`SEGMENTS`] pages.
    bounce: Dma,
    /// A read's bytes by PIO, or a page between its buffers and the
    /// bounce buffer: [`SEGMENTS`] pages.
    copy: Vec<u8, Kmalloc>,
    /// The drive stopped answering, and its channel did not come back.
    dead: bool,
}

/// The link to the optical drive at `end` of the channel `share`, whose
/// disks' queue, if any, is `disks`. Kept out of `add_channel`'s frame.
#[inline(never)]
fn new_link(
    source: radix_hw::pci::Address,
    share: Arc<Share, Kmalloc>,
    disks: Option<Arc<Disk<IdeChannel>, Kmalloc>>,
    end: u8,
    identity: PacketIdentity,
) -> Result<IdeLink, &'static str> {
    let bounce =
        <Arch as Devices>::alloc_dma(source, SEGMENTS, true).ok_or("out of memory below 4 GiB")?;
    let mut copy = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    copy.try_reserve_exact(SEGMENTS * PAGE).map_err(|_| "out of memory")?;
    while copy.len() < SEGMENTS * PAGE {
        let _ = copy.push_within_capacity(0);
    }
    Ok(IdeLink { share, disks, end, identity, bounce, copy, dead: false })
}

impl IdeLink {
    /// What the drive said of itself.
    pub(super) fn identity(&self) -> &PacketIdentity {
        &self.identity
    }

    /// Whether the drive stopped answering for good.
    pub(super) fn dead(&self) -> bool {
        self.dead
    }

    /// The most buffers of a read, each at most a page.
    pub(super) const MAX_SEGMENTS: usize = SEGMENTS;

    /// The drive's commands, which wait with `wait`.
    pub(super) fn packets<'a>(&'a mut self, wait: &'a mut dyn FnMut(u32)) -> IdePackets<'a> {
        IdePackets { link: self, wait }
    }

    /// Borrows the channel, waiting while a disk's command or the other
    /// end's has it, for at most `limit_us`.
    fn borrow(&mut self, limit_us: u32, wait: &mut dyn FnMut(u32)) -> bool {
        let mut waited = 0u32;
        loop {
            let lent = match &self.disks {
                Some(disks) => disks.controller(|c| c.lend(self.end)).unwrap_or(false),
                None => self.share.lend(self.end, true),
            };
            if lent {
                return true;
            }
            if waited >= limit_us {
                self.share.wants.fetch_and(!(1 << self.end), Ordering::AcqRel);
                if let Some(disks) = &self.disks {
                    disks.collect(0);
                }
                return false;
            }
            wait(BORROW_US);
            waited = waited.saturating_add(BORROW_US);
        }
    }

    /// Gives the channel back, and wakes the disks' requests that waited
    /// for it.
    fn give_back(&self) {
        self.share.holder.store(0, Ordering::Release);
        if let Some(disks) = &self.disks {
            disks.collect(0);
        }
    }

    /// Resets the channel, after a command the drive left unanswered or
    /// garbled; whether a device is ready after.
    fn reset(&mut self) -> bool {
        let ready = self.share.channel.lock().reset(&mut queue::delay);
        if !ready {
            self.dead = true;
        }
        ready
    }

    /// Runs the packet `bytes`, moving `transfer`, to its end, for at
    /// most `limit_us`: the bytes moved, or the error register of a
    /// command the drive failed.
    fn exchange(
        &mut self,
        bytes: &[u8],
        mut transfer: Transfer<'_>,
        limit_us: u32,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Result<u32, u8>, TransportError> {
        let dmadir = self.identity.dmadir;
        let started = self.share.channel.lock().start_packet(self.end, bytes, &transfer, dmadir);
        if started.is_err() {
            return Err(TransportError::Failed("its channel refused the command"));
        }
        let (mut waited, mut step) = (0u32, PACKET_FIRST_US);
        loop {
            let polled = self.share.channel.lock().poll_packet(&mut transfer);
            match polled {
                Some(Ok(moved)) => return Ok(Ok(moved)),
                Some(Err(radix_ide::Error::Device { error, .. })) => return Ok(Err(error)),
                Some(Err(radix_ide::Error::Bus)) => {
                    return Err(TransportError::Failed("the bus master could not reach memory"));
                }
                Some(Err(_)) => {
                    return if self.reset() {
                        Err(TransportError::Failed(
                            "it did not follow the packet protocol; the channel was reset",
                        ))
                    } else {
                        Err(TransportError::Gone)
                    };
                }
                None => {}
            }
            if waited >= limit_us {
                return if self.reset() {
                    Err(TransportError::Failed("it did not answer in time; the channel was reset"))
                } else {
                    Err(TransportError::Gone)
                };
            }
            wait(step);
            waited = waited.saturating_add(step);
            step = step.saturating_mul(2).min(PACKET_LONGEST_US);
        }
    }

    /// Runs `cdb` with `data`, by PIO, or by DMA for buffers in memory
    /// when the drive does DMA and the length is a multiple of 16 bytes
    /// (Linux's libata's rule: many drives fail DMA of less), through the
    /// bounce buffer for those the bus master does not reach.
    fn run(
        &mut self,
        cdb: &Cdb,
        data: Data<'_>,
        limit_us: u32,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Result<u32, u8>, TransportError> {
        let bytes = cdb.bytes();
        let (into_host, frames) = match data {
            Data::None => return self.exchange(bytes, Transfer::None, limit_us, wait),
            Data::In(buffer) => return self.exchange(bytes, Transfer::In(buffer), limit_us, wait),
            Data::Out(buffer) => {
                return self.exchange(bytes, Transfer::Out(buffer), limit_us, wait);
            }
            Data::Frames { into_host, frames } => (into_host, frames),
        };
        let total: usize = frames.iter().map(|&(_, len)| len as usize).sum();
        if frames.len() > SEGMENTS || total > self.copy.len() {
            return Err(TransportError::Failed("more data than a command takes"));
        }
        if self.identity.dma && total.is_multiple_of(16) {
            let reached = frames.iter().all(|&(phys, len)| phys + u64::from(len) <= 1 << 32);
            if reached {
                let mut buffers = [(0u32, 0u32); SEGMENTS];
                for (slot, &(phys, len)) in buffers.iter_mut().zip(frames) {
                    *slot = (phys as u32, len);
                }
                let transfer = Transfer::Dma { into_host, buffers: &buffers[..frames.len()] };
                return self.exchange(bytes, transfer, limit_us, wait);
            }
            // Through the bounce buffer, a page at a time.
            let at = self.bounce.phys();
            if !into_host {
                self.bounce_out(frames);
            }
            let buffers = [(at as u32, total as u32)];
            let transfer = Transfer::Dma { into_host, buffers: &buffers };
            let done = self.exchange(bytes, transfer, limit_us, wait)?;
            if into_host && done.is_ok() {
                self.bounce_in(frames);
            }
            return Ok(done);
        }
        if !into_host {
            let mut at = 0;
            for &(phys, len) in frames {
                <Arch as UserMode>::read_frames(phys, &mut self.copy[at..at + len as usize]);
                at += len as usize;
            }
        }
        // Out of the link while the command borrows it; nothing is
        // allocated in its place.
        let mut buffer =
            core::mem::replace(&mut self.copy, Vec::new_in(HEAP.with(Flags::MAY_SLEEP)));
        let transfer = if into_host {
            Transfer::In(&mut buffer[..total])
        } else {
            Transfer::Out(&buffer[..total])
        };
        let done = self.exchange(bytes, transfer, limit_us, wait);
        if into_host && let Ok(Ok(moved)) = done {
            let mut at = 0;
            for &(phys, len) in frames {
                let end = (at + len as usize).min(moved as usize);
                if end > at {
                    <Arch as UserMode>::write_frames(phys, &buffer[at..end]);
                }
                at += len as usize;
            }
        }
        self.copy = buffer;
        done
    }

    /// Copies `frames` into the bounce buffer, for a write.
    fn bounce_out(&mut self, frames: &[(u64, u32)]) {
        let mut at = self.bounce.phys();
        for &(phys, len) in frames {
            let page = &mut self.copy[..len as usize];
            <Arch as UserMode>::read_frames(phys, page);
            <Arch as UserMode>::write_frames(at, page);
            at += u64::from(len);
        }
    }

    /// Copies the bounce buffer out to `frames`, after a read.
    fn bounce_in(&mut self, frames: &[(u64, u32)]) {
        let mut at = self.bounce.phys();
        for &(phys, len) in frames {
            let page = &mut self.copy[..len as usize];
            <Arch as UserMode>::read_frames(at, page);
            <Arch as UserMode>::write_frames(phys, page);
            at += u64::from(len);
        }
    }

    /// Why the drive failed a command whose error register is `error`:
    /// what REQUEST SENSE says, or the sense key in the register's upper
    /// bits if it says nothing.
    fn sense(&mut self, error: u8, wait: &mut dyn FnMut(u32)) -> Result<Sense, TransportError> {
        let fallback = Sense { key: error >> 4, asc: 0, ascq: 0 };
        let mut bytes = [0u8; radix_scsi::SENSE_LENGTH as usize];
        let cdb = Cdb::request_sense(radix_scsi::SENSE_LENGTH);
        Ok(match self.exchange(cdb.bytes(), Transfer::In(&mut bytes), SENSE_US, wait)? {
            Ok(moved) => Sense::parse(&bytes[..moved as usize]).unwrap_or(fallback),
            Err(_) => fallback,
        })
    }
}

/// The optical drive on an IDE channel, as the [`Transport`] that carries
/// its SCSI commands (MMC-6) in PACKET commands, one at a time, on the
/// channel it borrows for each. A command the drive fails is followed by
/// REQUEST SENSE, whose answer is the reply's sense, as Linux's libata
/// gets it. Its one logical unit is 0.
pub(super) struct IdePackets<'a> {
    link: &'a mut IdeLink,
    wait: &'a mut dyn FnMut(u32),
}

impl Transport for IdePackets<'_> {
    fn command(
        &mut self,
        lun: u8,
        cdb: &Cdb,
        data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<Reply, TransportError> {
        if lun != 0 {
            let none = Sense { key: radix_scsi::key::ILLEGAL_REQUEST, asc: 0x25, ascq: 0 };
            return Ok(Reply::Failed(Some(none)));
        }
        if self.link.dead {
            return Err(TransportError::Gone);
        }
        let total = data.len() as u32;
        let limit_us = timeout_ms.saturating_mul(1_000);
        if !self.link.borrow(limit_us, self.wait) {
            return Err(TransportError::Failed("its channel stayed busy with a disk's commands"));
        }
        let reply = match self.link.run(cdb, data, limit_us, self.wait) {
            Ok(Ok(moved)) => Ok(Reply::Good { residue: total.saturating_sub(moved) }),
            Ok(Err(error)) => {
                self.link.sense(error, self.wait).map(|sense| Reply::Failed(Some(sense)))
            }
            Err(why) => Err(why),
        };
        self.link.give_back();
        reply
    }
}
