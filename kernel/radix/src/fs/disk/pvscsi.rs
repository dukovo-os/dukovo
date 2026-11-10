// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMware's paravirtual SCSI adapter (`radix_pvscsi`), as VMware's
//! hypervisors attach a machine's disks when asked for their fastest
//! controller (ADR 0424): each disk at one of its targets takes the first
//! `sd` letter free, as a SATA disk does.
//!
//! [`bind`] maps the adapter's registers, turns on bus mastering, gives
//! it one interrupt (MSI-X entry 0, or MSI), resets it and gives it its
//! rings. Then it looks for disks, one command at a time, polled, with the
//! adapter's interrupts off, as the virtio SCSI adapter's scan does: at
//! each target, the logical units REPORT LUNS lists, each asked what it is
//! and how big (`radix_scsi::probe`). A target with nothing there times
//! the selection out at once.
//!
//! The disks share the adapter's rings. Each disk's part of the block
//! queue ([`PvscsiLun`]) is a handle on the adapter ([`Adapter`]), which
//! puts its requests on the request ring as READ(10) or WRITE(10)
//! (16 on a disk with more blocks than 32 bits count) or SYNCHRONIZE
//! CACHE, and notes whose each context is. The interrupt handler takes
//! every completion off the completion ring into its disk's mailbox, and
//! then collects the disks that got one, as the virtio SCSI adapter's
//! does. Disks the host adds or removes while the machine runs are logged
//! from the message ring, and not seen until the next boot.
//!
//! A command that ends busy, aborted, reset or short goes again (the
//! queue's tries).

use alloc::sync::Arc;
use alloc::vec::Vec;
use radix_block::{DeviceRef, Error, Info, SECTOR};
use radix_hal::Devices;
use radix_hw::Dma;
use radix_hw::pci::{Bar, Function, Msi, MsiX};
use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_pvscsi::{
    Address, Answer, CONTEXTS, Direction, MAX_SEGMENTS, Message, Pvscsi, REGISTERS, host_status,
    is_read_write, scsi_status,
};
use radix_scsi::{
    Absent, Cdb, Data, Found, Meaning, Reply, Sense, Transport as Commands, TransportError,
    device_type, probe,
};
use radix_sync::{Once, SpinLock};

use super::queue::{self, Controller, Disk, Op, Outcome, Request, SLOTS, Volume};
use super::virtio_scsi::{ident, serial_number, units};
use super::{Entry, Name, SERIAL, Size, partitions, register};
use crate::{Arch, sched};

/// The most adapters.
const MAX_ADAPTERS: usize = 8;
/// The most disks on one adapter.
const MAX_DISKS: usize = 16;
/// How long a probe's command is polled for before the thread sleeps
/// between looks: an adapter in a virtual machine answers within it.
const SPIN_NS: u64 = 1_000_000;

/// Whose a context in flight is.
#[derive(Clone, Copy)]
struct Owner {
    disk: u8,
    slot: u8,
    /// It is a flush, which a disk that has no cache to flush may refuse.
    flush: bool,
}

/// An adapter's rings and what its disks have to collect.
struct Shared {
    pvscsi: Pvscsi,
    /// Whose each context in flight is.
    owner: Vec<Owner, Kmalloc>,
    /// The slots of each disk whose commands ended, not yet collected.
    ended: [u64; MAX_DISKS],
    /// How the command in each disk's slot ended, and why, for the log:
    /// at `disk * SLOTS + slot`.
    outcome: Vec<(Outcome, Option<&'static str>), Kmalloc>,
    /// The doorbells owed to commands held back: reads and writes, others.
    held: (bool, bool),
}

impl Shared {
    /// Takes every completion off the ring into its disk's mailbox. Returns
    /// the disks that got one, a bit each.
    fn drain(&mut self) -> u32 {
        let mut mail = 0;
        while let Some((context, answer)) = self.pvscsi.complete() {
            let Some(&owner) = self.owner.get(usize::from(context)) else { continue };
            let at = usize::from(owner.disk) * SLOTS + usize::from(owner.slot);
            if let Some(outcome) = self.outcome.get_mut(at) {
                *outcome = judge(&answer, owner.flush);
                self.ended[usize::from(owner.disk)] |= 1 << owner.slot;
                mail |= 1 << owner.disk;
            }
        }
        mail
    }

    /// The next of disk `disk`'s commands that ended: its slot, and how.
    fn take(&mut self, disk: u8) -> Option<(u8, Outcome)> {
        let ended = self.ended.get_mut(usize::from(disk))?;
        if *ended == 0 {
            return None;
        }
        let slot = ended.trailing_zeros() as u8;
        *ended &= *ended - 1;
        let (outcome, _) = self.outcome[usize::from(disk) * SLOTS + usize::from(slot)];
        Some((slot, outcome))
    }

    /// Rings the doorbells owed.
    fn ring(&mut self) {
        let (read_write, other) = core::mem::take(&mut self.held);
        if read_write {
            self.pvscsi.kick(true);
        }
        if other {
            self.pvscsi.kick(false);
        }
    }
}

/// A PVSCSI adapter that its disks share.
pub struct Adapter {
    shared: SpinLock<Shared>,
    /// Where it is, for the log.
    place: radix_hw::pci::Address,
}

/// A disk at a logical unit of an adapter, as the block queue drives it.
pub struct PvscsiLun {
    adapter: Arc<Adapter, Kmalloc>,
    /// Which of the adapter's disks it is.
    number: u8,
    at: Address,
    /// The 512-byte sectors in one of its blocks.
    unit: u64,
    /// Whether its reads and writes take 16-byte commands.
    sixteen: bool,
}

impl Controller for PvscsiLun {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        self.adapter.shared.lock_irq().pvscsi.room() > 0
    }

    fn submit(&mut self, slot: u8, request: &Request<'_>, more: bool) -> Result<(), Error> {
        let Request { op, sector, data, .. } = *request;
        let (cdb, direction) = match op {
            Op::Read | Op::Write | Op::WriteThrough => {
                let sectors = data.iter().map(|&(_, len)| u64::from(len)).sum::<u64>() / SECTOR;
                if !sector.is_multiple_of(self.unit) || !sectors.is_multiple_of(self.unit) {
                    return Err(Error::OutOfRange);
                }
                let (lba, blocks) = (sector / self.unit, sectors / self.unit);
                let blocks = u32::try_from(blocks).map_err(|_| Error::OutOfRange)?;
                // Never asked to write through: the disk does not say it does.
                let (cdb, direction) = if op == Op::Read {
                    (Cdb::read(lba, blocks, self.sixteen), Direction::FromDevice)
                } else {
                    (Cdb::write(lba, blocks, self.sixteen), Direction::ToDevice)
                };
                (cdb.ok_or(Error::OutOfRange)?, direction)
            }
            Op::Flush => (Cdb::synchronize_cache(), Direction::None),
            Op::Discard(_) | Op::Smart(_) => return Err(Error::Unsupported),
        };
        let bytes = cdb.bytes();
        let mut shared = self.adapter.shared.lock_irq();
        let context =
            shared.pvscsi.submit(self.at, bytes, data, direction).map_err(|err| match err {
                radix_pvscsi::Error::BadRequest => Error::OutOfRange,
                _ => Error::Io,
            })?;
        let flush = op == Op::Flush;
        if let Some(owner) = shared.owner.get_mut(usize::from(context)) {
            *owner = Owner { disk: self.number, slot, flush };
        }
        if is_read_write(bytes[0]) {
            shared.held.0 = true;
        } else {
            shared.held.1 = true;
        }
        if !more {
            shared.ring();
        }
        Ok(())
    }

    fn kick(&mut self) {
        self.adapter.shared.lock_irq().ring();
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        self.adapter.shared.lock_irq().take(self.number)
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        let at = usize::from(self.number) * SLOTS + usize::from(slot);
        self.adapter.shared.lock_irq().outcome.get(at).and_then(|&(_, why)| why)
    }
}

/// How a command that ended with `answer` ends its request, and why for
/// the log: a `flush` the disk does not take did not need doing.
fn judge(answer: &Answer, flush: bool) -> (Outcome, Option<&'static str>) {
    match answer.host_status {
        host_status::SUCCESS
        | host_status::LINKED_COMMAND_COMPLETED
        | host_status::LINKED_COMMAND_COMPLETED_WITH_FLAG => {}
        host_status::DATA_UNDERRUN => return (Outcome::Retry, Some("it moved less than asked")),
        host_status::ABORT_QUEUE | host_status::ADAPTER_TIMEOUT => {
            return (Outcome::Retry, Some("the adapter dropped the command"));
        }
        host_status::SENT_RESET
        | host_status::RECEIVED_RESET
        | host_status::BUS_RESET
        | host_status::NO_RESPONSE => {
            return (Outcome::Retry, Some("the path to the disk was reset"));
        }
        host_status::SELECTION_TIMEOUT => return (Outcome::Failed, Some("the disk is gone")),
        _ => return (Outcome::Failed, Some("the adapter failed the command")),
    }
    let sense = match answer.scsi_status {
        scsi_status::GOOD if answer.moved >= answer.asked => return (Outcome::Done, None),
        scsi_status::GOOD => return (Outcome::Retry, Some("it moved less than asked")),
        scsi_status::BUSY | scsi_status::TASK_SET_FULL => {
            return (Outcome::Retry, Some("the disk is busy"));
        }
        scsi_status::CHECK_CONDITION => Sense::parse(answer.sense()),
        _ => return (Outcome::Failed, Some("the disk ended the command with another status")),
    };
    let Some(sense) = sense else {
        return (Outcome::Retry, Some("it failed the command, and did not say why"));
    };
    match sense.meaning() {
        Meaning::Fine => (Outcome::Done, None),
        Meaning::Unsupported if flush => (Outcome::Done, None),
        Meaning::Attention | Meaning::Aborted => (Outcome::Retry, Some(sense.key_name())),
        Meaning::BecomingReady => (Outcome::Retry, Some("it is becoming ready")),
        Meaning::WriteProtected => (Outcome::Failed, Some("it is write-protected")),
        _ => (Outcome::Failed, Some(sense.key_name())),
    }
}

/// The adapters, for their interrupt handlers: set once each, after
/// their disks.
static ADAPTERS: [Once<Arc<Adapter, Kmalloc>>; MAX_ADAPTERS] =
    [const { Once::new() }; MAX_ADAPTERS];
/// A disk of an adapter, once it is set up.
type DiskSlot = Once<Arc<Disk<PvscsiLun>, Kmalloc>>;
/// Each adapter's disks, in order.
static DISKS: [[DiskSlot; MAX_DISKS]; MAX_ADAPTERS] =
    [const { [const { Once::new() }; MAX_DISKS] }; MAX_ADAPTERS];

fn on_interrupt(index: usize) {
    let Some(adapter) = ADAPTERS.get(index).and_then(Once::get) else { return };
    let mail = {
        let mut shared = adapter.shared.lock_irq();
        shared.pvscsi.acknowledge();
        while let Some(message) = shared.pvscsi.message() {
            note(adapter.place, message);
        }
        shared.drain()
    };
    if mail == 0 {
        return;
    }
    for (number, disk) in DISKS[index].iter().enumerate() {
        let Some(disk) = disk.get() else { continue };
        if mail & (1 << number) != 0 {
            disk.collect(0);
        } else {
            // Room on the rings the others shared.
            disk.wake_room();
        }
    }
}

/// Logs what a message from the adapter at `place` says.
fn note(place: radix_hw::pci::Address, message: Message) {
    match message {
        Message::Added(at) => info!(
            "Disk: the host added a disk at target {} LUN {} of the VMware PVSCSI adapter at \
             {place}; not seen until the next boot",
            at.target, at.lun
        ),
        Message::Removed(at) => info!(
            "Disk: the host removed the disk at target {} LUN {} of the VMware PVSCSI adapter at \
             {place}",
            at.target, at.lun
        ),
        Message::Other(_) => {}
    }
}

/// Carries a scan's commands to one target of an adapter, one at a time,
/// polled, through a page of DMA memory: before the disks exist.
struct Scan<'a> {
    pvscsi: &'a mut Pvscsi,
    page: &'a Dma,
    target: u8,
    /// A command went unanswered: the adapter is given up.
    broken: bool,
}

impl Commands for Scan<'_> {
    fn command(
        &mut self,
        lun: u8,
        cdb: &Cdb,
        data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<Reply, TransportError> {
        if self.broken {
            return Err(TransportError::Failed("the adapter stopped answering"));
        }
        let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let page = [(self.page.phys(), length)];
        let runs: &[(u64, u32)] = match &data {
            Data::None => &[],
            Data::Frames { frames, .. } => frames,
            Data::In(_) | Data::Out(_) if data.len() > self.page.len() => {
                return Err(TransportError::Failed("its answer does not fit a page"));
            }
            Data::Out(bytes) => {
                self.page.write(0, bytes);
                &page
            }
            Data::In(_) => &page,
        };
        let direction = match &data {
            Data::None => Direction::None,
            _ if data.into_host() => Direction::FromDevice,
            _ => Direction::ToDevice,
        };
        let at = Address { target: self.target, lun };
        let bytes = cdb.bytes();
        let context = self
            .pvscsi
            .submit(at, bytes, runs, direction)
            .map_err(|_| TransportError::Failed("the adapter refused the command"))?;
        self.pvscsi.kick(is_read_write(bytes[0]));
        let started = sched::now_ns();
        let deadline = started.saturating_add(u64::from(timeout_ms) * 1_000_000);
        let answer = loop {
            if let Some((done, answer)) = self.pvscsi.complete() {
                if done == context {
                    break answer;
                }
                continue;
            }
            let now = sched::now_ns();
            if now >= deadline {
                self.broken = true;
                return Err(TransportError::Failed("the adapter did not answer"));
            }
            if now < started + SPIN_NS {
                core::hint::spin_loop();
            } else {
                queue::delay(100);
            }
        };
        if let Data::In(bytes) = data {
            self.page.read(0, bytes);
        }
        let residue = u32::try_from(answer.asked.saturating_sub(answer.moved)).unwrap_or(u32::MAX);
        match (answer.host_status, answer.scsi_status) {
            (host_status::SUCCESS | host_status::DATA_UNDERRUN, scsi_status::GOOD) => {
                Ok(Reply::Good { residue })
            }
            (host_status::SUCCESS, scsi_status::CHECK_CONDITION) => {
                Ok(Reply::Failed(Sense::parse(answer.sense())))
            }
            (host_status::SUCCESS, scsi_status::BUSY | scsi_status::TASK_SET_FULL) => {
                Err(TransportError::Failed("the disk is busy"))
            }
            (host_status::SUCCESS, _) => Ok(Reply::Failed(None)),
            // Nothing at that target: it is not there.
            (host_status::SELECTION_TIMEOUT, _) => Err(TransportError::Gone),
            _ => Err(TransportError::Failed("the adapter failed the command")),
        }
    }
}

/// A disk the scan found: where, and what it is.
#[derive(Clone, Copy)]
struct Found1 {
    at: Address,
    found: Found,
    /// Its serial number, padded with spaces; blank if it gives none.
    serial: [u8; SERIAL],
}

/// Looks for the disks behind `pvscsi` of the adapter at `source`, up to
/// [`MAX_DISKS`] into `found`. Fails if the adapter stopped answering.
/// Kept out of [`bind`]'s frame: its answer buffers are large.
#[inline(never)]
fn scan(
    pvscsi: &mut Pvscsi,
    source: &Function,
    found: &mut Vec<Found1, Kmalloc>,
) -> Result<(), &'static str> {
    let page = <Arch as Devices>::alloc_dma(source.address(), 1, false).ok_or("out of memory")?;
    let mut luns = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    luns.try_reserve_exact(64).map_err(|_| "out of memory")?;
    let targets = pvscsi.targets().min(256);
    for target in 0..targets {
        let target = target as u8;
        let mut commands = Scan { pvscsi: &mut *pvscsi, page: &page, target, broken: false };
        luns.clear();
        units(&mut commands, &mut luns);
        for &lun in &luns {
            let mut wait = |ms: u32| queue::delay(ms.saturating_mul(1000));
            match probe(&mut commands, lun, &mut wait) {
                Ok(disk) if found.len() < MAX_DISKS => {
                    let at = Address { target, lun };
                    let serial = serial_number(&mut commands, lun);
                    let _ = found.push_within_capacity(Found1 { at, found: disk, serial });
                }
                Ok(_) => warn!(
                    "Disk: VMware PVSCSI adapter at {}: more than {MAX_DISKS} disks; target \
                     {target} LUN {lun} is not used",
                    source.address()
                ),
                Err(Absent::NotDisk(device_type::CD)) => info!(
                    "Disk: VMware PVSCSI adapter at {}: target {target} LUN {lun} is a CD or DVD \
                     drive, which is not used yet",
                    source.address()
                ),
                Err(Absent::NoUnit) => {}
                Err(why) => warn!(
                    "Disk: VMware PVSCSI adapter at {}: target {target} LUN {lun}: {why}",
                    source.address()
                ),
            }
            if commands.broken {
                return Err("it stopped answering");
            }
        }
        if commands.broken {
            return Err("it stopped answering");
        }
    }
    Ok(())
}

/// The interrupt `function` raises for adapter `index`: MSI-X entry 0,
/// or else MSI. Returns its vector and which kind.
fn interrupt(function: &Function, index: usize) -> Result<(u32, &'static str), &'static str> {
    if let Some(msix) = MsiX::find(function) {
        let (bar, offset) = msix.table_location();
        let Some(Bar::Memory { address, .. }) = function.bar(bar) else {
            return Err("its MSI-X table is not in memory");
        };
        let table = <Arch as Devices>::map_device(address + offset, msix.table_len())
            .ok_or("its MSI-X table could not be mapped")?;
        let (vector, message) =
            <Arch as Devices>::allocate_interrupt(function.address(), on_interrupt, index)
                .ok_or("no interrupt vector is free")?;
        msix.enable_masked(function);
        msix.set_entry(&table, 0, message);
        msix.unmask_all(function);
        return Ok((vector, "MSI-X"));
    }
    let msi = Msi::find(function).ok_or("it has neither MSI-X nor MSI")?;
    let (vector, message) =
        <Arch as Devices>::allocate_interrupt(function.address(), on_interrupt, index)
            .ok_or("no interrupt vector is free")?;
    msi.enable(function, message);
    Ok((vector, "MSI"))
}

/// Sets up the PVSCSI adapter `function` as adapter number `index`, and
/// its disks, counted in `disks`. Kept out of `init`'s frame.
#[inline(never)]
pub fn bind(function: &Function, index: usize, disks: &mut usize) -> Result<(), &'static str> {
    if index >= MAX_ADAPTERS {
        return Err("too many adapters");
    }
    // Its registers are in its first memory BAR.
    let bar = (0..6).find_map(|n| match function.bar(n) {
        Some(Bar::Memory { address, size, .. }) if size >= REGISTERS as u64 => Some(address),
        _ => None,
    });
    let address = bar.ok_or("its registers are not in memory")?;
    let regs = <Arch as Devices>::map_device(address, REGISTERS)
        .ok_or("its registers could not be mapped")?;
    function.enable_memory_and_dma();
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(function.address(), pages, false);
    let mut pvscsi = Pvscsi::new(regs, &mut alloc).map_err(|err| {
        warn!("Disk: VMware PVSCSI adapter at {}: {err}", function.address());
        "the adapter refused to be set up"
    })?;
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut found = Vec::new_in(heap);
    let scanned = found.try_reserve_exact(MAX_DISKS).map_err(|_| "out of memory");
    if let Err(why) = scanned.and_then(|()| scan(&mut pvscsi, function, &mut found)) {
        pvscsi.reset();
        return Err(why);
    }
    if found.is_empty() {
        pvscsi.reset();
        info!("Disk: the VMware PVSCSI adapter at {} has no disks", function.address());
        return Ok(());
    }
    let (vector, kind) = match interrupt(function, index) {
        Ok(interrupt) => interrupt,
        Err(why) => {
            pvscsi.reset();
            return Err(why);
        }
    };
    pvscsi.enable_interrupts();
    let adapter = new_adapter(pvscsi, function.address())?;
    let mut made = Vec::new_in(heap);
    made.try_reserve_exact(found.len()).map_err(|_| "out of memory")?;
    for (number, disk) in found.iter().enumerate() {
        let (letter, name) = super::take_sd_name().ok_or("too many SATA and SCSI disks")?;
        match add(function, (index, number), &adapter, disk, (vector, kind), &name) {
            Ok(info) => {
                let _ = made.push_within_capacity((number, name, info));
                *disks += 1;
            }
            Err(why) => {
                super::give_back_sd_letter(letter);
                warn!("Disk: {name}: {why}");
            }
        }
    }
    // Its interrupts reach the disks from here, and their partition
    // tables are read through their queues.
    let _ = ADAPTERS[index].set(adapter);
    for (number, name, info) in &made {
        if let Some(disk) = DISKS[index][*number].get() {
            let one = &found[*number];
            publish(disk, ident(&one.found.inquiry, one.serial), name, *info);
        }
    }
    Ok(())
}

/// The adapter that drives `pvscsi`, on the heap, with room for every
/// context's owner. Kept out of [`bind`]'s frame.
#[inline(never)]
fn new_adapter(
    pvscsi: Pvscsi,
    place: radix_hw::pci::Address,
) -> Result<Arc<Adapter, Kmalloc>, &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut owner = Vec::new_in(heap);
    owner.try_reserve_exact(CONTEXTS).map_err(|_| "out of memory")?;
    for _ in 0..CONTEXTS {
        let _ = owner.push_within_capacity(Owner { disk: 0, slot: 0, flush: false });
    }
    let mut outcome = Vec::new_in(heap);
    outcome.try_reserve_exact(MAX_DISKS * SLOTS).map_err(|_| "out of memory")?;
    for _ in 0..MAX_DISKS * SLOTS {
        let _ = outcome.push_within_capacity((Outcome::Failed, None));
    }
    let shared = Shared { pvscsi, owner, ended: [0; MAX_DISKS], outcome, held: (false, false) };
    Arc::try_new_in(Adapter { shared: SpinLock::new(shared), place }, heap)
        .map_err(|_| "out of memory")
}

/// Makes the disk number `number` of adapter `index`, which `disk` says
/// is there, named `name`, and sets it in [`DISKS`]; returns what it is.
/// Kept out of [`bind`]'s frame: the disk is built on the stack before it
/// moves.
#[inline(never)]
fn add(
    function: &Function,
    (index, number): (usize, usize),
    adapter: &Arc<Adapter, Kmalloc>,
    disk: &Found1,
    (vector, kind): (u32, &'static str),
    name: &Name,
) -> Result<Info, &'static str> {
    let Found1 { at, found, .. } = *disk;
    let capacity = found.capacity;
    let unit = u64::from(capacity.block_length) / SECTOR;
    let info = Info {
        sectors: capacity.blocks.saturating_mul(unit),
        logical_block: capacity.block_length,
        block_size: capacity.physical_block(),
        read_only: found.read_only,
        max_segments: MAX_SEGMENTS,
        discard_sectors: 0,
        // A disk that does not say spins, as Linux takes it.
        rotational: true,
    };
    let controller = PvscsiLun {
        adapter: adapter.clone(),
        number: number as u8,
        at,
        unit,
        sixteen: found.sixteen,
    };
    let part = queue::part(controller).ok_or("out of memory")?;
    let once = core::iter::once(part);
    let source = function.address();
    let made = Disk::new(once, info, found.write_cache, false, true, name.clone(), source)
        .ok_or("out of memory")?;
    let made = Arc::try_new_in(made, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    let _ = DISKS[index][number].set(made);
    info!(
        "Disk: {name} is a SCSI disk of {} on the VMware PVSCSI adapter at {source}, target {} \
         LUN {}: {} {}, {} bytes a block{}{}{}, {kind} vector {vector:#x}",
        Size(info.sectors * SECTOR),
        at.target,
        at.lun,
        found.inquiry.vendor(),
        found.inquiry.product(),
        info.block_size,
        super::sector_note(&info),
        if info.read_only { ", read-only" } else { "" },
        if found.write_cache { ", write cache" } else { "" },
    );
    Ok(info)
}

/// Registers `disk`, named `name`, which says it is `ident` and is
/// `info`, with its partitions, once its adapter's interrupt reaches it.
fn publish(disk: &Arc<Disk<PvscsiLun>, Kmalloc>, ident: super::Ident, name: &Name, info: Info) {
    let Some(volume) = Volume::new(disk.clone(), 0, info) else {
        warn!("Disk: {name}: out of memory");
        return;
    };
    let Ok(device) = Arc::try_new_in(volume, HEAP.with(Flags::MAY_SLEEP)) else {
        warn!("Disk: {name}: out of memory");
        return;
    };
    let device: DeviceRef = device;
    register(Entry { name: name.clone(), device: device.clone(), place: None, ident: Some(ident) });
    partitions(name, &device);
}
