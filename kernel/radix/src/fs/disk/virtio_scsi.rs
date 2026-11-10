// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! virtio SCSI host bus adapters (`radix_virtio::scsi`), as cloud
//! machines attach their disks (ADR 0327): each disk at one of an
//! adapter's targets takes the first `sd` letter free, as a SATA disk
//! does.
//!
//! [`bind`] maps the adapter's registers, turns on bus mastering, gives
//! it one MSI-X interrupt for its request queue, and sets it up. Then it
//! looks for disks, one command at a time, polled: at each target, the
//! logical units REPORT LUNS lists (unit 0 of a target that does not take
//! it), each asked what it is and how big (`radix_scsi::probe`), as
//! Linux's SCSI scan does. An adapter's last target answers at once
//! that it has nothing there, so a scan of all 256 is quick. A CD or DVD
//! drive is noted and left alone.
//!
//! The disks share the adapter's one queue. Each disk's part of the block
//! queue ([`ScsiLun`]) is a handle on the adapter ([`Adapter`]), which
//! puts its requests on the virtqueue, as READ(10) or WRITE(10) (READ(16)
//! and WRITE(16) on a disk with more blocks than 32 bits count) or
//! SYNCHRONIZE CACHE, and notes whose each chain is. The interrupt
//! handler takes every finished command off the virtqueue, into its
//! disk's mailbox, and then collects the disks that got one, which take
//! theirs from the mailbox: a disk never takes another's completions off
//! the queue itself, which would leave that disk waiting for an interrupt
//! that already came. A disk waiting for room on the virtqueue that the
//! others took is woken when any of their commands ends. Locks: a disk's
//! lane, then its adapter.
//!
//! A command that ends with a unit attention, an abort, a busy adapter or
//! a broken path goes again (the queue's tries). An adapter has no task
//! management set up, so a command unanswered for 30 s is only logged.

use alloc::sync::Arc;
use alloc::vec::Vec;
use radix_block::{DeviceRef, Error, Info, SECTOR};
use radix_hal::Devices;
use radix_hw::Dma;
use radix_hw::pci::{Bar, Function, MsiX};
use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::{
    Absent, Cdb, Data, Found, Inquiry, Meaning, Reply, SERIAL_PAGE, Sense, Transport as Commands,
    TransportError, device_type, probe, reported_luns, unit_serial,
};
use radix_sync::{Once, SpinLock};
use radix_virtio::Transport;
use radix_virtio::scsi::{Address, Answer, Direction, Scsi, response, scsi_status};

use super::queue::{self, Controller, Disk, Op, Outcome, Request, SLOTS, Volume};
use super::{Entry, Ident, Name, SERIAL, Size, partitions, register};
use crate::{Arch, sched};

/// The most adapters.
const MAX_ADAPTERS: usize = 8;
/// The most disks on one adapter.
const MAX_DISKS: usize = 16;
/// How much of a REPORT LUNS answer is read: its header and 63 units.
const REPORT_LENGTH: usize = 512;
/// How long a probe's command is polled for before the thread sleeps
/// between looks: an adapter in a virtual machine answers within it.
const SPIN_NS: u64 = 1_000_000;

/// How a finished command's disk and slot are kept, by its chain's head.
#[derive(Clone, Copy)]
struct Owner {
    disk: u8,
    slot: u8,
    /// It is a flush, which a disk that has no cache to flush may refuse.
    flush: bool,
}

/// An adapter's queue and what its disks have to collect.
struct Shared {
    scsi: Scsi,
    /// Whose each chain in flight is, by head.
    owner: Vec<Owner, Kmalloc>,
    /// The slots of each disk whose commands ended, not yet collected.
    ended: [u64; MAX_DISKS],
    /// How the command in each disk's slot ended, and why, for the log:
    /// at `disk * SLOTS + slot`.
    outcome: Vec<(Outcome, Option<&'static str>), Kmalloc>,
    /// Whether commands wait in the queue for the doorbell.
    held: bool,
}

impl Shared {
    /// Takes every finished command off the virtqueue into its disk's
    /// mailbox. Returns the disks that got one, a bit each.
    fn drain(&mut self) -> u32 {
        let mut mail = 0;
        while let Some((head, answer)) = self.scsi.complete() {
            let Some(&owner) = self.owner.get(usize::from(head)) else { continue };
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
}

/// A virtio SCSI adapter that its disks share.
pub struct Adapter {
    shared: SpinLock<Shared>,
}

/// A disk at a logical unit of an adapter, as the block queue drives it.
pub struct ScsiLun {
    adapter: Arc<Adapter, Kmalloc>,
    /// Which of the adapter's disks it is.
    number: u8,
    at: Address,
    /// The 512-byte sectors in one of its blocks.
    unit: u64,
    /// Whether its reads and writes take 16-byte commands.
    sixteen: bool,
}

impl Controller for ScsiLun {
    fn room(&self, _op: Op, segments: usize) -> bool {
        self.adapter.shared.lock_irq().scsi.room(segments) > 0
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
            Op::Flush => (Cdb::synchronize_cache(), Direction::ToDevice),
            Op::Discard(_) | Op::Smart(_) => return Err(Error::Unsupported),
        };
        let mut shared = self.adapter.shared.lock_irq();
        let head = shared.scsi.submit_held(self.at, cdb.bytes(), data, direction).map_err(
            |err| match err {
                radix_virtio::Error::BadRequest => Error::OutOfRange,
                _ => Error::Io,
            },
        )?;
        let flush = op == Op::Flush;
        if let Some(owner) = shared.owner.get_mut(usize::from(head)) {
            *owner = Owner { disk: self.number, slot, flush };
        }
        shared.held = more;
        if !more {
            shared.scsi.notify();
        }
        Ok(())
    }

    fn kick(&mut self) {
        let mut shared = self.adapter.shared.lock_irq();
        if shared.held {
            shared.held = false;
            shared.scsi.notify();
        }
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
    match answer.response {
        response::OK => {}
        response::ABORTED | response::RESET => {
            return (Outcome::Retry, Some("the adapter dropped the command"));
        }
        response::BUSY => return (Outcome::Retry, Some("the adapter is busy")),
        response::TRANSPORT_FAILURE => {
            return (Outcome::Retry, Some("the path to the disk failed"));
        }
        response::BAD_TARGET => return (Outcome::Failed, Some("the disk is gone")),
        _ => return (Outcome::Failed, Some("the adapter failed the command")),
    }
    let sense = match answer.status {
        scsi_status::GOOD if answer.residual == 0 => return (Outcome::Done, None),
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
type DiskSlot = Once<Arc<Disk<ScsiLun>, Kmalloc>>;
/// Each adapter's disks, in order.
static DISKS: [[DiskSlot; MAX_DISKS]; MAX_ADAPTERS] =
    [const { [const { Once::new() }; MAX_DISKS] }; MAX_ADAPTERS];

fn on_interrupt(index: usize) {
    let Some(adapter) = ADAPTERS.get(index).and_then(Once::get) else { return };
    let mail = adapter.shared.lock_irq().drain();
    if mail == 0 {
        return;
    }
    for (number, disk) in DISKS[index].iter().enumerate() {
        let Some(disk) = disk.get() else { continue };
        if mail & (1 << number) != 0 {
            disk.collect(0);
        } else {
            // Room on the queue the others shared.
            disk.wake_room();
        }
    }
}

/// Carries a scan's commands to one target of an adapter, one at a time,
/// polled, through a page of DMA memory: before the disks exist.
struct Scan<'a> {
    scsi: &'a mut Scsi,
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
        let frames: &[(u64, u32)] = match &data {
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
        let direction = if data.into_host() { Direction::FromDevice } else { Direction::ToDevice };
        let at = Address { target: self.target, lun: u16::from(lun) };
        let head = self
            .scsi
            .submit_held(at, cdb.bytes(), frames, direction)
            .map_err(|_| TransportError::Failed("the adapter refused the command"))?;
        self.scsi.notify();
        let started = sched::now_ns();
        let deadline = started.saturating_add(u64::from(timeout_ms) * 1_000_000);
        let answer = loop {
            if let Some((done, answer)) = self.scsi.complete() {
                if done == head {
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
        match (answer.response, answer.status) {
            (response::OK, scsi_status::GOOD) => Ok(Reply::Good { residue: answer.residual }),
            (response::OK, scsi_status::CHECK_CONDITION) => {
                Ok(Reply::Failed(Sense::parse(answer.sense())))
            }
            (response::OK, scsi_status::BUSY | scsi_status::TASK_SET_FULL) => {
                Err(TransportError::Failed("the disk is busy"))
            }
            (response::OK, _) => Ok(Reply::Failed(None)),
            // Nothing at that target: it is not there.
            (response::BAD_TARGET, _) => Err(TransportError::Gone),
            (response::ABORTED | response::RESET | response::BUSY, _) => {
                Err(TransportError::Failed("the adapter dropped the command"))
            }
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

/// The logical units of `scan`'s target that REPORT LUNS lists, below
/// 256, or unit 0 if it does not answer; none if nothing is at the
/// target. Hyper-V's SCSI controller's scan uses it too (`storvsc`).
pub(super) fn units(scan: &mut impl Commands, units: &mut Vec<u8, Kmalloc>) {
    let mut answer = [0u8; REPORT_LENGTH];
    let cdb = Cdb::report_luns(REPORT_LENGTH as u32);
    match scan.command(0, &cdb, Data::In(&mut answer), 30_000) {
        Ok(Reply::Good { .. }) => {
            for lun in reported_luns(&answer) {
                if units.push_within_capacity(lun).is_err() {
                    break;
                }
            }
        }
        Err(TransportError::Gone) => {}
        _ => {
            let _ = units.push_within_capacity(0);
        }
    }
}

/// Looks for the disks behind `scsi` of the adapter at `source`, up to
/// [`MAX_DISKS`] into `found`. Fails if the adapter stopped answering.
/// Kept out of [`bind`]'s frame: its answer buffer is large.
#[inline(never)]
fn scan(
    scsi: &mut Scsi,
    source: &Function,
    found: &mut Vec<Found1, Kmalloc>,
) -> Result<(), &'static str> {
    let alloc = |pages: usize| <Arch as Devices>::alloc_dma(source.address(), pages, false);
    let page = alloc(1).ok_or("out of memory")?;
    let mut luns = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    luns.try_reserve_exact(64).map_err(|_| "out of memory")?;
    let last = scsi.info().max_target;
    for target in 0..=last {
        let mut commands = Scan { scsi: &mut *scsi, page: &page, target, broken: false };
        luns.clear();
        units(&mut commands, &mut luns);
        for &lun in &luns {
            let mut wait = |ms: u32| queue::delay(ms.saturating_mul(1000));
            match probe(&mut commands, lun, &mut wait) {
                Ok(disk) if found.len() < MAX_DISKS => {
                    let at = Address { target, lun: u16::from(lun) };
                    let serial = serial_number(&mut commands, lun);
                    let _ = found.push_within_capacity(Found1 { at, found: disk, serial });
                }
                Ok(_) => warn!(
                    "Disk: virtio SCSI adapter at {}: more than {MAX_DISKS} disks; target \
                     {target} LUN {lun} is not used",
                    source.address()
                ),
                Err(Absent::NotDisk(device_type::CD)) => info!(
                    "Disk: virtio SCSI adapter at {}: target {target} LUN {lun} is a CD or DVD \
                     drive, which is not used yet",
                    source.address()
                ),
                Err(Absent::NoUnit) => {}
                Err(why) => warn!(
                    "Disk: virtio SCSI adapter at {}: target {target} LUN {lun}: {why}",
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

/// The serial number unit `lun` of `scan`'s target gives on its vital
/// product data page, padded with spaces; blank if it has none.
pub(super) fn serial_number(scan: &mut impl Commands, lun: u8) -> [u8; SERIAL] {
    let mut answer = [0u8; 255];
    let cdb = Cdb::inquiry_page(SERIAL_PAGE, 255);
    match scan.command(lun, &cdb, Data::In(&mut answer), 30_000) {
        Ok(Reply::Good { .. }) => super::padded(unit_serial(&answer).as_bytes()),
        _ => [b' '; SERIAL],
    }
}

/// What a disk says it is (`inquiry`), as `/kernel/block` shows it: its
/// vendor and product as the model, its serial number (`serial`), and
/// its revision as the firmware's.
pub(super) fn ident(inquiry: &Inquiry, serial: [u8; SERIAL]) -> Ident {
    let mut model = [b' '; 40];
    let mut at = 0;
    for word in [inquiry.vendor(), inquiry.product()] {
        for &byte in word.as_bytes() {
            if at < model.len() {
                model[at] = byte;
                at += 1;
            }
        }
        if at > 0 && at < model.len() {
            at += 1;
        }
    }
    Ident { model, serial, firmware: super::padded(inquiry.revision().as_bytes()) }
}

/// Sets up the virtio SCSI adapter `function` as adapter number `index`,
/// and its disks, counted in `disks`. Its frame holds the adapter's queue
/// before it goes on the heap: kept out of `init`'s
/// (tools/xtask/src/frames.rs allows it).
#[inline(never)]
pub fn bind(function: &Function, index: usize, disks: &mut usize) -> Result<(), &'static str> {
    if index >= MAX_ADAPTERS {
        return Err("too many adapters");
    }
    function.enable_memory_and_dma();
    let mut map_bar = |_: usize, bar: Bar| match bar {
        Bar::Memory { address, size, .. } => {
            <Arch as Devices>::map_device(address, usize::try_from(size).ok()?)
        }
        Bar::Io { .. } => None,
    };
    let transport = Transport::find(function, &mut map_bar).map_err(|_| "no modern transport")?;
    // One MSI-X entry, for the request queue. Its disks share the queue,
    // and only its interrupt handler takes their commands off it, so an
    // adapter without one is not used.
    let msix = MsiX::find(function).ok_or("it has no MSI-X")?;
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
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(function.address(), pages, false);
    let mut scsi = match Scsi::new(transport, 0, radix_virtio::queue::MAX_TABLES, &mut alloc) {
        Ok(scsi) => scsi,
        Err(err) => {
            <Arch as Devices>::free_interrupt(vector);
            warn!("Disk: virtio SCSI adapter at {}: {err}", function.address());
            return Err("the adapter refused to be set up");
        }
    };
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut found = Vec::new_in(heap);
    let scanned = found.try_reserve_exact(MAX_DISKS).map_err(|_| "out of memory");
    if let Err(why) = scanned.and_then(|()| scan(&mut scsi, function, &mut found)) {
        drop(scsi);
        <Arch as Devices>::free_interrupt(vector);
        return Err(why);
    }
    // The scan took its answers without looking again once each came:
    // a look that finds nothing asks the device to interrupt for the
    // next one.
    while scsi.complete().is_some() {}
    if found.is_empty() {
        drop(scsi);
        <Arch as Devices>::free_interrupt(vector);
        info!("Disk: the virtio SCSI adapter at {} has no disks", function.address());
        return Ok(());
    }
    let adapter = new_adapter(scsi)?;
    let mut made = Vec::new_in(heap);
    made.try_reserve_exact(found.len()).map_err(|_| "out of memory")?;
    for (number, disk) in found.iter().enumerate() {
        let (letter, name) = super::take_sd_name().ok_or("too many SATA and SCSI disks")?;
        match add(function, (index, number), &adapter, disk, vector, &name) {
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

/// The adapter that drives `scsi`, on the heap, with room for every
/// disk's commands. Kept out of [`bind`]'s frame.
#[inline(never)]
fn new_adapter(scsi: Scsi) -> Result<Arc<Adapter, Kmalloc>, &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut owner = Vec::new_in(heap);
    let size = usize::from(radix_virtio::queue::MAX_SIZE);
    owner.try_reserve_exact(size).map_err(|_| "out of memory")?;
    for _ in 0..size {
        let _ = owner.push_within_capacity(Owner { disk: 0, slot: 0, flush: false });
    }
    let mut outcome = Vec::new_in(heap);
    outcome.try_reserve_exact(MAX_DISKS * SLOTS).map_err(|_| "out of memory")?;
    for _ in 0..MAX_DISKS * SLOTS {
        let _ = outcome.push_within_capacity((Outcome::Failed, None));
    }
    let shared = Shared { scsi, owner, ended: [0; MAX_DISKS], outcome, held: false };
    Arc::try_new_in(Adapter { shared: SpinLock::new(shared) }, heap).map_err(|_| "out of memory")
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
    vector: u32,
    name: &Name,
) -> Result<Info, &'static str> {
    let Found1 { at, found, .. } = *disk;
    let capacity = found.capacity;
    let unit = u64::from(capacity.block_length) / SECTOR;
    let max_segments = adapter.shared.lock_irq().scsi.info().max_segments as usize;
    let info = Info {
        sectors: capacity.blocks.saturating_mul(unit),
        logical_block: capacity.block_length,
        block_size: capacity.physical_block(),
        read_only: found.read_only,
        max_segments,
        discard_sectors: 0,
        // A disk that does not say spins, as Linux takes it.
        rotational: true,
    };
    let controller = ScsiLun {
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
        "Disk: {name} is a SCSI disk of {} on the virtio SCSI adapter at {source}, target {} LUN \
         {}: {} {}, {} bytes a block{}{}{}, MSI-X vector {vector:#x}",
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
fn publish(disk: &Arc<Disk<ScsiLun>, Kmalloc>, ident: Ident, name: &Name, info: Info) {
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
