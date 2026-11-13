// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hyper-V's synthetic SCSI controller (storvsc, `radix_hyperv::storage`),
//! as Generation 2 machines and Azure attach their disks (ADR 0423): each
//! disk takes the first `sd` letter free, as a SATA disk does.
//!
//! VMBus's thread opens the controller's channel and hands it to [`bind`],
//! which sets the controller up (BEGIN_INITIALIZATION, the newest protocol
//! version the host takes, QUERY_PROPERTIES, END_INITIALIZATION) and looks
//! for disks, one command at a time, polled, as the virtio SCSI adapter's
//! scan does: at targets 0 and 1, the logical units REPORT LUNS lists,
//! each asked what it is and how big (`radix_scsi::probe`).
//!
//! The disks share the channel. Each disk's part of the block queue
//! ([`StorLun`]) is a handle on the controller ([`Controller`]), which
//! writes its requests into the channel's outbound ring: READ(10) or
//! WRITE(10) (16 on a disk with more blocks than 32 bits count), or
//! SYNCHRONIZE CACHE, each in a packet that names the request's pages as
//! one range, and whose transaction number says whose it is. VMBus's
//! interrupt handler, through [`on_event`], takes every answer off the
//! inbound ring into its disk's mailbox, then has the disks that got one
//! collect it, as the virtio SCSI adapter's handler does.
//!
//! A command that ends busy, aborted, reset or short goes again (the
//! queue's tries). Disks the host adds or removes while the machine runs
//! are not seen yet: the host's ENUMERATE_BUS is logged.

use alloc::sync::Arc;
use alloc::vec::Vec;
use radix_block::{DeviceRef, Error, Info, SECTOR};
use radix_hal::Hyperv;
use radix_hw::Dma;
use radix_hyperv::ring::{self, COMPLETION, INBAND, Range, Rings};
use radix_hyperv::storage::{self, Direction, Reply as Answer, operation, srb_status};
use radix_hyperv::{Guid, PAGE};
use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::{
    Absent, Cdb, Data, Found, Meaning, Reply, Sense, Transport as Commands, TransportError,
    device_type, probe,
};
use radix_sync::{Once, SpinLock};

use super::queue::{self, Controller as Lane, Disk, Op, Outcome, Request, SLOTS, Volume};
use super::virtio_scsi::{ident, serial_number, units};
use super::{Entry, Name, SERIAL, Size, partitions, register};
use crate::hyperv::{self, Channel};
use crate::{Arch, sched};

/// The most controllers.
const MAX_CONTROLLERS: usize = 8;
/// The most disks on one controller.
const MAX_DISKS: usize = 16;
/// The targets a controller has: Hyper-V puts its disks at target 0,
/// logical units 0 to 63, and Azure its data disks too.
const TARGETS: u8 = 2;
/// The most pages one command's data spans, its first entered at an
/// offset: 256 KiB.
const MAX_PAGES: usize = 65;
/// How long a scan's command is polled for before the thread sleeps
/// between looks.
const SPIN_NS: u64 = 1_000_000;
/// How long the controller has to answer while it is set up.
const SETUP_MS: u32 = 5_000;
/// The room a command takes in the ring at most: its header, its range
/// and pages, the storage packet and the trailer.
const COMMAND_ROOM: usize = ring::HEADER + 8 + 8 + MAX_PAGES * 8 + storage::PACKET + 8;

/// A transaction number: a disk's slot's, with this bit set; the scan's
/// and the setup's are below it.
const QUEUED: u64 = 1 << 32;

/// What a command in flight asked for.
#[derive(Clone, Copy, Default)]
struct InFlight {
    /// The bytes it asked to move.
    asked: u32,
    /// It is a flush, which a disk that has no cache to flush may refuse.
    flush: bool,
}

/// A controller's channel and what its disks have to collect.
struct Shared {
    rings: Rings,
    connection: u32,
    /// Each disk's slot's command, at `disk * SLOTS + slot`.
    in_flight: Vec<InFlight, Kmalloc>,
    /// How each disk's slot's command ended, and why, for the log.
    outcome: Vec<(Outcome, Option<&'static str>), Kmalloc>,
    /// The slots of each disk whose commands ended, not yet collected.
    ended: [u64; MAX_DISKS],
    /// A command was written into an empty ring while the doorbell was
    /// held: the host is to be signalled.
    signal: bool,
}

impl Shared {
    /// Signals the host, if a command waits for it.
    fn ring(&mut self) {
        if core::mem::take(&mut self.signal) {
            hyperv::signal(self.connection);
        }
    }

    /// Takes every answer off the inbound ring into its disk's mailbox.
    /// Returns the disks that got one, a bit each.
    fn drain(&mut self) -> u32 {
        let mut mail = 0;
        let mut data = [0u8; storage::PACKET];
        while let Some((header, len, wake)) = self.rings.read(&mut data) {
            if wake {
                hyperv::signal(self.connection);
            }
            if header.kind == INBAND {
                note(&data[..len]);
                continue;
            }
            let transaction = header.transaction;
            if header.kind != COMPLETION || transaction & QUEUED == 0 {
                continue;
            }
            let (disk, slot) =
                (((transaction >> 8) & 0xff) as usize, (transaction & 0xff) as usize);
            let at = disk * SLOTS + slot;
            let (Some(asked), Some(reply)) = (self.in_flight.get(at), storage::parse(&data[..len]))
            else {
                continue;
            };
            let outcome = judge(&reply, *asked);
            if let (Some(place), Some(ended)) = (self.outcome.get_mut(at), self.ended.get_mut(disk))
            {
                *place = outcome;
                *ended |= 1 << slot;
                mail |= 1 << disk;
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

/// A packet the host sent unasked: a disk added or taken away.
fn note(data: &[u8]) {
    if let Some(reply) = storage::parse(data) {
        match reply.operation {
            operation::ENUMERATE_BUS => {
                info!(
                    "Disk: the host changed the disks of a Hyper-V SCSI controller; not seen yet"
                );
            }
            operation::REMOVE_DEVICE => info!("Disk: the host took a Hyper-V SCSI disk away"),
            _ => {}
        }
    }
}

/// How a command that ended with `reply` ends its request, and why for
/// the log.
fn judge(reply: &Answer, asked: InFlight) -> (Outcome, Option<&'static str>) {
    if reply.operation != operation::COMPLETE_IO || reply.status != 0 {
        return (Outcome::Failed, Some("the host failed the command"));
    }
    const GOOD: u8 = 0x00;
    const CHECK_CONDITION: u8 = 0x02;
    const BUSY: u8 = 0x08;
    const TASK_SET_FULL: u8 = 0x28;
    match reply.srb_status {
        srb_status::SUCCESS | srb_status::DATA_OVERRUN | srb_status::ERROR => {}
        srb_status::BUSY
        | srb_status::ABORTED
        | srb_status::BUS_RESET
        | srb_status::REQUEST_FLUSHED
        | srb_status::TIMEOUT
        | srb_status::COMMAND_TIMEOUT => {
            return (Outcome::Retry, Some(storage::status_name(reply.srb_status)));
        }
        srb_status::NO_DEVICE
        | srb_status::SELECTION_TIMEOUT
        | srb_status::INVALID_LUN
        | srb_status::INVALID_TARGET_ID
        | srb_status::INVALID_PATH_ID => return (Outcome::Failed, Some("the disk is gone")),
        status => return (Outcome::Failed, Some(storage::status_name(status))),
    }
    let sense = match reply.scsi_status {
        GOOD if reply.srb_status == srb_status::ERROR => {
            return (Outcome::Retry, Some("the host failed the command, and did not say why"));
        }
        GOOD if reply.transferred < asked.asked => {
            return (Outcome::Retry, Some("it moved less than asked"));
        }
        GOOD => return (Outcome::Done, None),
        BUSY | TASK_SET_FULL => return (Outcome::Retry, Some("the disk is busy")),
        CHECK_CONDITION if reply.sense_valid => Sense::parse(&reply.sense),
        CHECK_CONDITION => None,
        _ => return (Outcome::Failed, Some("the disk ended the command with another status")),
    };
    let Some(sense) = sense else {
        return (Outcome::Retry, Some("it failed the command, and did not say why"));
    };
    match sense.meaning() {
        Meaning::Fine => (Outcome::Done, None),
        Meaning::Unsupported if asked.flush => (Outcome::Done, None),
        Meaning::Attention | Meaning::Aborted => (Outcome::Retry, Some(sense.key_name())),
        Meaning::BecomingReady => (Outcome::Retry, Some("it is becoming ready")),
        Meaning::WriteProtected => (Outcome::Failed, Some("it is write-protected")),
        _ => (Outcome::Failed, Some(sense.key_name())),
    }
}

/// The frame numbers of `data` (physical address, length), which must be
/// one run of pages, the first entered at an offset, as a VMBus range
/// names them: its offset into the first page and the count of pages put
/// in `pages`; `None` if they are not.
fn page_list(data: &[(u64, u32)], pages: &mut [u64; MAX_PAGES]) -> Option<(u32, usize)> {
    let page = PAGE as u64;
    let mut count = 0;
    for (i, &(phys, len)) in data.iter().enumerate() {
        let last = i + 1 == data.len();
        let starts = i == 0 || phys.is_multiple_of(page);
        let ends = last || (phys + u64::from(len)).is_multiple_of(page);
        if len == 0 || !starts || !ends {
            return None;
        }
        let first = phys / page;
        let end = (phys + u64::from(len)).div_ceil(page);
        for frame in first..end {
            *pages.get_mut(count)? = frame;
            count += 1;
        }
    }
    let offset = data.first().map_or(0, |&(phys, _)| (phys % page) as u32);
    Some((offset, count))
}

/// Writes the command `packet`, moving the data in `data` (physical
/// address, length), as transaction `transaction`. Returns whether the
/// host is to be signalled.
fn send(
    rings: &Rings,
    packet: &[u8],
    data: &[(u64, u32)],
    transaction: u64,
) -> Result<bool, Error> {
    if data.is_empty() {
        return ring::send(rings, packet, transaction, true).map_err(|_| Error::Io);
    }
    let mut pages = [0u64; MAX_PAGES];
    let (offset, count) = page_list(data, &mut pages).ok_or(Error::OutOfRange)?;
    let len = data.iter().map(|&(_, len)| len).sum();
    let range = Range { len, offset, pages: &pages[..count] };
    ring::send_pages(rings, &[range], packet, transaction).map_err(|_| Error::Io)
}

/// A synthetic SCSI controller that its disks share.
pub struct Controller {
    shared: SpinLock<Shared>,
}

/// A disk at a logical unit of a controller, as the block queue drives it.
pub struct StorLun {
    controller: Arc<Controller, Kmalloc>,
    /// Which of the controller's disks it is.
    number: u8,
    target: u8,
    lun: u8,
    /// The 512-byte sectors in one of its blocks.
    unit: u64,
    /// Whether its reads and writes take 16-byte commands.
    sixteen: bool,
}

impl Lane for StorLun {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        self.controller.shared.lock_irq().rings.room() > COMMAND_ROOM
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
                    (Cdb::read(lba, blocks, self.sixteen), Direction::In)
                } else {
                    (Cdb::write(lba, blocks, self.sixteen), Direction::Out)
                };
                (cdb.ok_or(Error::OutOfRange)?, direction)
            }
            Op::Flush => (Cdb::synchronize_cache(), Direction::None),
            Op::Discard(_) | Op::Smart(..) => return Err(Error::Unsupported),
        };
        let asked = data.iter().map(|&(_, len)| len).sum::<u32>();
        let packet = storage::execute(self.target, self.lun, cdb.bytes(), direction, asked);
        let transaction = QUEUED | u64::from(self.number) << 8 | u64::from(slot);
        let at = usize::from(self.number) * SLOTS + usize::from(slot);
        let mut shared = self.controller.shared.lock_irq();
        let signal = send(&shared.rings, &packet, data, transaction)?;
        if let Some(place) = shared.in_flight.get_mut(at) {
            *place = InFlight { asked, flush: op == Op::Flush };
        }
        shared.signal |= signal;
        if !more {
            shared.ring();
        }
        Ok(())
    }

    fn kick(&mut self) {
        self.controller.shared.lock_irq().ring();
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        self.controller.shared.lock_irq().take(self.number)
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        let at = usize::from(self.number) * SLOTS + usize::from(slot);
        self.controller.shared.lock_irq().outcome.get(at).and_then(|&(_, why)| why)
    }
}

/// The controllers, for VMBus's interrupt handler: set once each, after
/// their disks.
static CONTROLLERS: [Once<Arc<Controller, Kmalloc>>; MAX_CONTROLLERS] =
    [const { Once::new() }; MAX_CONTROLLERS];
/// A disk of a controller, once it is set up.
type DiskSlot = Once<Arc<Disk<StorLun>, Kmalloc>>;
/// Each controller's disks, in order.
static DISKS: [[DiskSlot; MAX_DISKS]; MAX_CONTROLLERS] =
    [const { [const { Once::new() }; MAX_DISKS] }; MAX_CONTROLLERS];

/// Controller `index`'s channel has events: from VMBus's interrupt
/// handler.
pub fn on_event(index: usize) {
    let Some(controller) = CONTROLLERS.get(index).and_then(Once::get) else { return };
    let mail = controller.shared.lock_irq().drain();
    if mail == 0 {
        return;
    }
    for (number, disk) in DISKS[index].iter().enumerate() {
        let Some(disk) = disk.get() else { continue };
        if mail & (1 << number) != 0 {
            disk.collect(0);
        } else {
            // Room in the ring the others shared.
            disk.wake_room();
        }
    }
}

/// Carries the setup's and the scan's packets, one at a time, polled,
/// through a page of memory: before the disks exist.
struct Scan<'a> {
    rings: &'a Rings,
    connection: u32,
    page: &'a Dma,
    target: u8,
    /// The next transaction number.
    next: u64,
    /// A command went unanswered: the controller is given up.
    broken: bool,
}

impl Scan<'_> {
    /// Sends `packet`, with the data in `data`, and waits `timeout_ms` for
    /// its answer: the host's packet.
    fn exchange(
        &mut self,
        packet: &[u8],
        data: &[(u64, u32)],
        timeout_ms: u32,
    ) -> Option<[u8; storage::PACKET]> {
        if self.broken {
            return None;
        }
        let transaction = self.next;
        self.next += 1;
        match send(self.rings, packet, data, transaction) {
            Ok(true) => {
                hyperv::signal(self.connection);
            }
            Ok(false) => {}
            Err(_) => return None,
        }
        let started = sched::now_ns();
        let deadline = started.saturating_add(u64::from(timeout_ms) * 1_000_000);
        let mut answer = [0u8; storage::PACKET];
        loop {
            while let Some((header, len, wake)) = self.rings.read(&mut answer) {
                if wake {
                    hyperv::signal(self.connection);
                }
                if header.kind == COMPLETION && header.transaction == transaction {
                    answer[len..].fill(0);
                    return Some(answer);
                }
            }
            let now = sched::now_ns();
            if now >= deadline {
                self.broken = true;
                return None;
            }
            if now < started + SPIN_NS {
                core::hint::spin_loop();
            } else {
                queue::delay(100);
            }
        }
    }

    /// Sends `packet`, a step of the setup: the host's answer, if it took
    /// it.
    fn step(&mut self, packet: &[u8]) -> Option<[u8; storage::PACKET]> {
        let answer = self.exchange(packet, &[], SETUP_MS)?;
        let reply = storage::parse(&answer)?;
        (reply.operation == operation::COMPLETE_IO && reply.status == 0).then_some(answer)
    }
}

impl Commands for Scan<'_> {
    fn command(
        &mut self,
        lun: u8,
        cdb: &Cdb,
        data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<Reply, TransportError> {
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
        let direction = match &data {
            Data::None => Direction::None,
            _ if data.into_host() => Direction::In,
            _ => Direction::Out,
        };
        let packet = storage::execute(self.target, lun, cdb.bytes(), direction, length);
        let Some(answer) =
            self.exchange(&packet, frames, timeout_ms).and_then(|a| storage::parse(&a))
        else {
            return Err(TransportError::Failed("the controller did not answer"));
        };
        if let Data::In(bytes) = data {
            self.page.read(0, bytes);
        }
        if answer.operation != operation::COMPLETE_IO || answer.status != 0 {
            return Err(TransportError::Failed("the host failed the command"));
        }
        match answer.srb_status {
            srb_status::SUCCESS | srb_status::DATA_OVERRUN | srb_status::ERROR => {}
            // Nothing at that address: it is not there.
            srb_status::NO_DEVICE
            | srb_status::SELECTION_TIMEOUT
            | srb_status::INVALID_LUN
            | srb_status::INVALID_TARGET_ID
            | srb_status::INVALID_PATH_ID => return Err(TransportError::Gone),
            status => return Err(TransportError::Failed(storage::status_name(status))),
        }
        match answer.scsi_status {
            0x00 if answer.srb_status != srb_status::ERROR => {
                Ok(Reply::Good { residue: length.saturating_sub(answer.transferred) })
            }
            0x02 if answer.sense_valid => Ok(Reply::Failed(Sense::parse(&answer.sense))),
            0x08 | 0x28 => Err(TransportError::Failed("the disk is busy")),
            _ => Ok(Reply::Failed(None)),
        }
    }
}

/// A disk the scan found: where, and what it is.
#[derive(Clone, Copy)]
struct Found1 {
    target: u8,
    lun: u8,
    found: Found,
    /// Its serial number, padded with spaces; blank if it gives none.
    serial: [u8; SERIAL],
}

/// Sets the controller up: returns the most pages one command may span.
fn set_up(scan: &mut Scan<'_>) -> Result<usize, &'static str> {
    scan.step(&storage::begin()).ok_or("it did not begin its setup")?;
    let version = storage::VERSIONS
        .into_iter()
        .find(|&version| scan.step(&storage::query_version(version)).is_some())
        .ok_or("it speaks no protocol version the kernel does")?;
    let answer = scan.step(&storage::query_properties()).ok_or("it did not give its properties")?;
    scan.step(&storage::end()).ok_or("it did not end its setup")?;
    let properties = storage::properties(&answer);
    // A host that gives no limit takes 128 KiB, as FreeBSD's driver
    // sends at most.
    let most = match properties.max_transfer {
        0 => 128 << 10,
        bytes => (bytes as usize).min((MAX_PAGES - 1) * PAGE),
    };
    info!(
        "Disk: the Hyper-V SCSI controller speaks protocol {}.{}, up to {} a command",
        version >> 8,
        version & 0xff,
        Size(most as u64)
    );
    Ok((most / PAGE).max(1))
}

/// Looks for the disks behind the controller, up to [`MAX_DISKS`] into
/// `found`. Fails if the controller stopped answering. Kept out of
/// [`bind`]'s frame: its answer buffers are large.
#[inline(never)]
fn scan(
    scan: &mut Scan<'_>,
    instance: &Guid,
    found: &mut Vec<Found1, Kmalloc>,
) -> Result<(), &'static str> {
    let mut luns = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    luns.try_reserve_exact(64).map_err(|_| "out of memory")?;
    for target in 0..TARGETS {
        scan.target = target;
        luns.clear();
        units(scan, &mut luns);
        for &lun in &luns {
            let mut wait = |ms: u32| queue::delay(ms.saturating_mul(1000));
            match probe(scan, lun, &mut wait) {
                Ok(disk) if found.len() < MAX_DISKS => {
                    let serial = serial_number(scan, lun);
                    let _ = found.push_within_capacity(Found1 { target, lun, found: disk, serial });
                }
                Ok(_) => warn!(
                    "Disk: Hyper-V SCSI controller {instance}: more than {MAX_DISKS} disks; \
                     target {target} LUN {lun} is not used"
                ),
                Err(Absent::NotDisk(device_type::CD)) => info!(
                    "Disk: Hyper-V SCSI controller {instance}: target {target} LUN {lun} is a CD \
                     or DVD drive, which is not used yet"
                ),
                Err(Absent::NoUnit) => {}
                Err(why) => {
                    warn!(
                        "Disk: Hyper-V SCSI controller {instance}: target {target} LUN {lun}: {why}"
                    );
                }
            }
            if scan.broken {
                return Err("it stopped answering");
            }
        }
        if scan.broken {
            return Err("it stopped answering");
        }
    }
    Ok(())
}

/// Sets up the synthetic SCSI controller on `channel`, open, as
/// controller number `index`, and its disks, counted in `disks`: from
/// VMBus's thread, which routes the channel's events to [`on_event`].
pub fn bind(channel: Channel, index: usize, disks: &mut usize) -> Result<(), &'static str> {
    if index >= MAX_CONTROLLERS {
        return Err("too many controllers");
    }
    let Channel { connection, rings, instance, .. } = channel;
    let page = <Arch as Hyperv>::hyperv_alloc(1).ok_or("out of memory")?;
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut found = Vec::new_in(heap);
    found.try_reserve_exact(MAX_DISKS).map_err(|_| "out of memory")?;
    let mut commands =
        Scan { rings: &rings, connection, page: &page, target: 0, next: 1, broken: false };
    let segments = set_up(&mut commands)?;
    scan(&mut commands, &instance, &mut found)?;
    if found.is_empty() {
        info!("Disk: the Hyper-V SCSI controller {instance} has no disks");
        return Ok(());
    }
    let controller = new_controller(rings, connection)?;
    let mut made = Vec::new_in(heap);
    made.try_reserve_exact(found.len()).map_err(|_| "out of memory")?;
    for (number, disk) in found.iter().enumerate() {
        let (letter, name) = super::take_sd_name().ok_or("too many SATA and SCSI disks")?;
        match add((index, number), &controller, disk, segments, &instance, &name) {
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
    // Its events reach the disks from here, and their partition tables
    // are read through their queues.
    let _ = CONTROLLERS[index].set(controller);
    for (number, name, info) in &made {
        if let Some(disk) = DISKS[index][*number].get() {
            let one = &found[*number];
            publish(disk, ident(&one.found.inquiry, one.serial), name, *info);
        }
    }
    Ok(())
}

/// The controller that drives `rings`, on the heap, with room for every
/// disk's commands. Kept out of [`bind`]'s frame.
#[inline(never)]
fn new_controller(rings: Rings, connection: u32) -> Result<Arc<Controller, Kmalloc>, &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut in_flight = Vec::new_in(heap);
    in_flight.try_reserve_exact(MAX_DISKS * SLOTS).map_err(|_| "out of memory")?;
    let mut outcome = Vec::new_in(heap);
    outcome.try_reserve_exact(MAX_DISKS * SLOTS).map_err(|_| "out of memory")?;
    for _ in 0..MAX_DISKS * SLOTS {
        let _ = in_flight.push_within_capacity(InFlight::default());
        let _ = outcome.push_within_capacity((Outcome::Failed, None));
    }
    let shared =
        Shared { rings, connection, in_flight, outcome, ended: [0; MAX_DISKS], signal: false };
    Arc::try_new_in(Controller { shared: SpinLock::new(shared) }, heap).map_err(|_| "out of memory")
}

/// Makes the disk number `number` of controller `index`, which `disk`
/// says is there, taking `segments` pages a command, named `name`, and
/// sets it in [`DISKS`]; returns what it is. Kept out of [`bind`]'s
/// frame: the disk is built on the stack before it moves.
#[inline(never)]
fn add(
    (index, number): (usize, usize),
    controller: &Arc<Controller, Kmalloc>,
    disk: &Found1,
    segments: usize,
    instance: &Guid,
    name: &Name,
) -> Result<Info, &'static str> {
    let Found1 { target, lun, found, .. } = *disk;
    let capacity = found.capacity;
    let unit = u64::from(capacity.block_length) / SECTOR;
    let info = Info {
        sectors: capacity.blocks.saturating_mul(unit),
        logical_block: capacity.block_length,
        block_size: capacity.physical_block(),
        read_only: found.read_only,
        max_segments: segments,
        discard_sectors: 0,
        // A disk that does not say spins, as Linux takes it.
        rotational: true,
    };
    let lane = StorLun {
        controller: controller.clone(),
        number: number as u8,
        target,
        lun,
        unit,
        sixteen: found.sixteen,
    };
    let part = queue::part(lane).ok_or("out of memory")?;
    let once = core::iter::once(part);
    // The host reaches the guest's memory itself, past any IOMMU.
    let made = Disk::new(once, info, found.write_cache, false, true, name.clone(), None)
        .ok_or("out of memory")?;
    let made = Arc::try_new_in(made, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    let _ = DISKS[index][number].set(made);
    info!(
        "Disk: {name} is a SCSI disk of {} on the Hyper-V SCSI controller {instance}, target \
         {target} LUN {lun}: {} {}, {} bytes a block{}{}{}",
        Size(info.sectors * SECTOR),
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
/// `info`, with its partitions, once its controller's events reach it.
fn publish(disk: &Arc<Disk<StorLun>, Kmalloc>, ident: super::Ident, name: &Name, info: Info) {
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
