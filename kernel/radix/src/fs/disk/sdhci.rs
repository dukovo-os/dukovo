// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SD card readers on PCI (`radix-sdhci`, Phase 11g, ADR 0247): the SD,
//! SDHC and SDXC memory cards in an SD host controller's slots, each slot
//! named `mmcblk0` and on, as Linux names the cards, and a card's
//! partitions `mmcblk0p1` and on.
//!
//! [`bind`] turns on the controller's registers and bus master and reads
//! where its slots are (the Slot Information register in configuration
//! space: the first slot's BAR, and how many follow it). A card in a slot
//! at boot is set up there and then ([`radix_sdhci::Slot::identify`]), and
//! is one lane of a queue of its own ([`Disk`]).
//!
//! A slot for a card that comes and goes is a drive whose medium comes and
//! goes (ADR 0430, ADR 0437): it keeps its name while it is empty, a
//! device of no sectors, and a thread for each controller watches its
//! slots' card detects ([`radix_sdhci::Detect`]) every two seconds. A card
//! put in is given 200 ms to settle, set up, and takes the empty slot's
//! place with its partitions; one taken out is taken out of the kernel,
//! and what was mounted from it is lost. eject(1) lets a card go: it
//! leaves the kernel, its power goes off, and it is left alone until it
//! is taken out. A card is set up, and a slot reset, by the thread alone,
//! never while the queue of a card before it may use the slot's
//! registers ([`Held`]).
//!
//! A controller with MSI interrupts for its requests' ends, an error, and
//! a card put in or taken out: the handler hands each card's queue what
//! finished and wakes the thread for the card detect. One without MSI is
//! polled: the threads waiting for a request look for its end, as an IDE
//! channel's do, and the thread looks at the card detects every two
//! seconds. QEMU's `sdhci-pci` has no MSI.
//!
//! A controller whose descriptors are 32-bit (one before version 3.00, as
//! QEMU's `sdhci-pci` is by default) reaches only the first 4 GiB, which
//! page caches are not kept to: a request whose buffers all lie below it
//! goes to them, and any other through a bounce buffer of [`SEGMENTS`]
//! pages below 4 GiB, copied before a write and after a read.
//!
//! An SD card keeps no write cache the host can see: a flush is done at
//! once, and every write is through. Discarding (the cards' ERASE) is
//! left for later. A slot for a device soldered to the board (an eMMC's)
//! is set up at boot only.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering, fence};

use radix_block::{DeviceRef, Empty, Error, Info};
use radix_hal::{Devices, UserMode};
use radix_hw::pci::{Address, Bar, Function, Msi};
use radix_hw::{Dma, Mmio};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sdhci::{Card, Detect, Events, Op as SdOp, Presence, Slot};
use radix_sync::{Once, SpinLock};

use super::queue::{self, Controller, Disk, Op, Outcome, Part, Request, Volume};
use super::removable::{self, DriveRef, Kind};
use super::{Entry, Ident, Name, Replaced, Size, partitions, register};
use crate::Arch;
use crate::sched::{self, Options, Policy, ThreadRef};

/// The most segments of a request, each at most a page: the bounce
/// buffer's pages.
const SEGMENTS: usize = 32;
const PAGE: usize = 4096;
/// The most slots a controller has.
const MAX_SLOTS: u8 = 6;
/// The most SD host controllers driven.
const MAX_READERS: usize = 8;
/// How often the thread looks at a slot's card detect: as often as a USB
/// card reader's slot is looked at (ADR 0430). A controller that
/// interrupts says at once.
const LOOK_NS: u64 = 2_000_000_000;
/// How soon a card detect still settling is looked at again.
const SETTLING_NS: u64 = 20_000_000;
/// How long a card put in is given to settle in its slot before it is
/// powered, as Linux gives it.
const SETTLE_NS: u64 = 200_000_000;
/// How soon the thread tries again to take a card out while the mount
/// table is busy.
const BUSY_NS: u64 = 100_000_000;
/// Why a card's requests fail once it is gone.
const TAKEN_OUT: &str = "its card was taken out";
const LET_GO: &str = "its card was let go, and may be taken out";

/// The next slot's number, in its name `mmcblk{n}`.
static NEXT: AtomicU32 = AtomicU32::new(0);
/// The next controller's place in [`READERS`].
static READER_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The controllers, for their interrupt handlers: set once each.
static READERS: [Once<Arc<Reader, Kmalloc>>; MAX_READERS] = [const { Once::new() }; MAX_READERS];
/// The slots each controller's thread watches, handed over to it when it
/// starts.
static HANDOFFS: SpinLock<[Option<Box<Watchers, Kmalloc>>; MAX_READERS]> =
    SpinLock::new([const { None }; MAX_READERS]);

/// A controller, as its interrupt handler and its thread share it.
struct Reader {
    /// Its slots with a card in them, or whose cards come and go, each set
    /// once it is set up.
    slots: [Once<Arc<Watched, Kmalloc>>; MAX_SLOTS as usize],
    /// The thread that watches them, which its interrupt wakes.
    thread: Once<ThreadRef>,
}

/// A slot for cards that come and go, as the interrupt handler and the
/// thread share it.
struct Watched {
    detect: Detect,
    /// The queue of the card in the slot, while one is: the interrupt
    /// handler hands it what finished.
    disk: SpinLock<Option<Arc<Disk<SdSlot>, Kmalloc>>>,
    /// What the card detect said that the interrupt handler read, for the
    /// thread: [`INSERTED`] and [`REMOVED`].
    events: AtomicU8,
}

const INSERTED: u8 = 1 << 0;
const REMOVED: u8 = 1 << 1;

/// The controller `index`'s interrupt: the card detects' events for the
/// thread, and each card's queue's ends.
fn on_interrupt(index: usize) {
    let Some(reader) = READERS.get(index).and_then(Once::get) else { return };
    let mut wake = false;
    for watched in reader.slots.iter().filter_map(Once::get) {
        if !watched.detect.interrupting() {
            continue;
        }
        // Cleared here, so that the controller's interrupt can come again.
        let events = watched.detect.events();
        if events.any() {
            let bits = if events.inserted { INSERTED } else { 0 }
                | if events.removed { REMOVED } else { 0 };
            watched.events.fetch_or(bits, Ordering::AcqRel);
            wake = true;
        }
        if let Some(disk) = watched.disk.lock_irq().as_ref() {
            disk.collect(0);
        }
    }
    if wake && let Some(thread) = reader.thread.get() {
        sched::wake(thread);
    }
}

/// What a card's queue shares with the thread watching its slot.
struct Held {
    /// The card is gone, `why`: its requests fail from now on, and its
    /// slot's registers may be another card's. The queue's part uses the
    /// registers only with this held, and finds it `None`.
    gone: SpinLock<Option<&'static str>>,
    /// The queue resets the slot after a request went unanswered: the
    /// thread waits for that to end before another card has the slot.
    resetting: AtomicBool,
}

/// The request in flight.
#[derive(Clone, Copy)]
struct Flight {
    slot: u8,
    /// A read through the bounce buffer, whose bytes go to the request's
    /// buffers, kept in `buffers`, once it is done.
    bounced_read: bool,
    /// The transfer, to start it again once if it comes back garbled in
    /// a tuned mode, with `descriptors` as they were.
    op: SdOp,
    block: u64,
    count: u32,
    /// It was started again.
    again: bool,
}

/// A card's slot as the queue drives it.
pub struct SdSlot {
    slot: Slot,
    held: Arc<Held, Kmalloc>,
    flight: Option<Flight>,
    /// A request done without the card: a flush, or one for a card gone.
    done: Option<(u8, Outcome)>,
    /// The bounce buffer, below 4 GiB, if the slot needs it.
    bounce: Option<Dma>,
    /// The buffers of a bounced read.
    buffers: Vec<(u64, u32), Kmalloc>,
    /// The buffers the descriptors point at.
    descriptors: Vec<(u64, u32), Kmalloc>,
    /// A page between a request's buffer and the bounce buffer.
    copy: Vec<u8, Kmalloc>,
    /// Why the last request failed, for the log.
    why: Option<(u8, &'static str)>,
    /// The slot's place, for the log: the controller's address and the
    /// slot's number.
    place: TextBuf<40>,
}

impl SdSlot {
    /// Copies a bounced read's bytes to its buffers.
    fn copy_back(&mut self) {
        let Some(bounce) = &self.bounce else { return };
        let mut at = bounce.phys();
        for &(phys, len) in &self.buffers {
            let part = &mut self.copy[..len as usize];
            <Arch as UserMode>::read_frames(at, part);
            <Arch as UserMode>::write_frames(phys, part);
            at += u64::from(len);
        }
    }

    /// The descriptors' buffers for `data`: themselves when the slot
    /// reaches them, else the bounce buffer, filled for a write.
    fn place_buffers(&mut self, data: &[(u64, u32)], write: bool) -> Result<bool, Error> {
        self.descriptors.clear();
        self.buffers.clear();
        if data.len() > SEGMENTS || data.iter().any(|&(_, len)| len as usize > PAGE) {
            return Err(Error::OutOfRange);
        }
        let reached = |&(phys, len): &(u64, u32)| phys + u64::from(len) <= 1 << 32;
        let bounce = match &self.bounce {
            Some(bounce) if !data.iter().all(reached) => bounce.phys(),
            _ => {
                for &buffer in data {
                    describe(&mut self.descriptors, buffer);
                }
                return Ok(false);
            }
        };
        let mut at = bounce;
        for &(phys, len) in data {
            let _ = self.buffers.push_within_capacity((phys, len));
            describe(&mut self.descriptors, (at, len));
            if write {
                let part = &mut self.copy[..len as usize];
                <Arch as UserMode>::read_frames(phys, part);
                <Arch as UserMode>::write_frames(at, part);
            }
            at += u64::from(len);
        }
        Ok(true)
    }
}

/// Adds `buffer` to `descriptors`, in the last one if it follows it in
/// memory and the two fit one descriptor: a bounced request's pages, and
/// a buffer's pages that lie together, take a descriptor for each 60 KiB
/// rather than each page, which the controller moves through faster.
fn describe(descriptors: &mut Vec<(u64, u32), Kmalloc>, (phys, len): (u64, u32)) {
    if let Some((last, last_len)) = descriptors.last_mut()
        && *last + u64::from(*last_len) == phys
        && *last_len + len <= radix_sdhci::MAX_BUFFER
    {
        *last_len += len;
        return;
    }
    let _ = descriptors.push_within_capacity((phys, len));
}

impl Controller for SdSlot {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        self.flight.is_none() && self.done.is_none()
    }

    fn submit(&mut self, slot: u8, request: &Request<'_>, _more: bool) -> Result<(), Error> {
        if request.target != 0 {
            return Err(Error::OutOfRange);
        }
        let held = self.held.clone();
        let gone = held.gone.lock_irq();
        if let Some(why) = *gone {
            self.why = Some((slot, why));
            self.done = Some((slot, Outcome::Failed));
            return Ok(());
        }
        let op = match request.op {
            Op::Read => SdOp::Read,
            Op::Write | Op::WriteThrough => SdOp::Write,
            Op::Flush => {
                self.done = Some((slot, Outcome::Done));
                return Ok(());
            }
            Op::Discard(_) | Op::Smart(_) => return Err(Error::Unsupported),
        };
        let bytes: u64 = request.data.iter().map(|&(_, len)| u64::from(len)).sum();
        let count =
            u32::try_from(bytes / u64::from(radix_sdhci::BLOCK)).map_err(|_| Error::OutOfRange)?;
        let bounced_read = self.place_buffers(request.data, op == SdOp::Write)? && op == SdOp::Read;
        let block = request.sector;
        self.slot.start(op, block, count, &self.descriptors, &mut queue::delay).map_err(|err| {
            match err {
                radix_sdhci::Error::BadRequest => Error::OutOfRange,
                _ => Error::Io,
            }
        })?;
        self.flight = Some(Flight { slot, bounced_read, op, block, count, again: false });
        Ok(())
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        if let Some(done) = self.done.take() {
            return Some(done);
        }
        let flight = self.flight?;
        let held = self.held.clone();
        let gone = held.gone.lock_irq();
        if let Some(why) = *gone {
            self.flight = None;
            self.why = Some((flight.slot, why));
            return Some((flight.slot, Outcome::Failed));
        }
        let result = self.slot.poll()?;
        // A sampling point that drifted: the slot is tuned again as the
        // transfer starts again, once (ADR 0438), with the card held.
        if result.is_err() && self.slot.retuning() && !flight.again {
            let Flight { op, block, count, .. } = flight;
            if self.slot.start(op, block, count, &self.descriptors, &mut queue::delay).is_ok() {
                self.flight = Some(Flight { again: true, ..flight });
                return None;
            }
        }
        drop(gone);
        self.flight = None;
        match result {
            Ok(()) => {
                if flight.bounced_read {
                    self.copy_back();
                }
                Some((flight.slot, Outcome::Done))
            }
            Err(err) => {
                self.why = Some((flight.slot, err.as_str()));
                Some((flight.slot, Outcome::Failed))
            }
        }
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        self.why.filter(|&(failed, _)| failed == slot).map(|(_, why)| why)
    }

    fn reset(lanes: &mut [Part<SdSlot>]) -> bool {
        for lane in lanes {
            // Said before the card is looked at, as the thread says the
            // card is gone before it looks at this: one of the two sees
            // the other's.
            lane.held.resetting.store(true, Ordering::SeqCst);
            fence(Ordering::SeqCst);
            let gone = *lane.held.gone.lock_irq();
            if gone.is_none() {
                lane.slot.abandon(&mut queue::delay);
                warn!(
                    "Disk: a request to the SD card in the reader at {} went unanswered for 30 \
                     s; the slot was reset, and what was in flight goes again",
                    lane.place
                );
            }
            lane.held.resetting.store(false, Ordering::SeqCst);
            if let Some(flight) = lane.flight.take() {
                lane.done = Some(match gone {
                    Some(why) => {
                        lane.why = Some((flight.slot, why));
                        (flight.slot, Outcome::Failed)
                    }
                    None => (flight.slot, Outcome::Retry),
                });
            }
        }
        true
    }
}

/// A card set up in a slot, as a disk: its device, its queue, and what
/// the queue shares with the thread.
struct Loaded {
    device: DeviceRef,
    disk: Arc<Disk<SdSlot>, Kmalloc>,
    held: Arc<Held, Kmalloc>,
}

/// Makes `card`, set up in `slot`, a disk named `name`, whose requests
/// raise `interrupt`s.
fn load(
    slot: Slot,
    card: &Card,
    name: &Name,
    place: &TextBuf<40>,
    (source, interrupt): (Address, bool),
) -> Result<Loaded, &'static str> {
    let bounce = if slot.reaches_64() {
        None
    } else {
        let bounce = <Arch as Devices>::alloc_dma(source, SEGMENTS, true);
        Some(bounce.ok_or("out of memory below 4 GiB")?)
    };
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let held = Held { gone: SpinLock::new(None), resetting: AtomicBool::new(false) };
    let held = Arc::try_new_in(held, heap).map_err(|_| "out of memory")?;
    let controller = new_slot(slot, held.clone(), bounce, place.clone())?;
    let info = Info {
        sectors: card.blocks,
        logical_block: radix_sdhci::BLOCK,
        block_size: radix_sdhci::BLOCK,
        read_only: card.read_only,
        max_segments: SEGMENTS,
        discard_sectors: 0,
        rotational: false,
    };
    let disk = new_disk(controller, info, name.clone(), source, interrupt)?;
    let volume = Volume::new(disk.clone(), 0, info).ok_or("out of memory")?;
    let volume = Arc::try_new_in(volume, heap).map_err(|_| "out of memory")?;
    let device: DeviceRef = volume;
    Ok(Loaded { device, disk, held })
}

/// Sets up the SD host controller `function` and the cards in its slots,
/// counting them in `next`. Kept out of the frame of `init`'s scan.
#[inline(never)]
pub fn bind(function: &Function, next: &mut usize) -> Result<(), &'static str> {
    let information = function.read8(usize::from(radix_sdhci::SLOT_INFORMATION));
    let first = usize::from(information & 0x7);
    let slots = (information >> 4 & 0x7) + 1;
    if first > 5 || slots > MAX_SLOTS {
        return Err("its Slot Information register names no BAR");
    }
    let index = READER_COUNT.fetch_add(1, Ordering::Relaxed);
    if index >= MAX_READERS {
        return Err("too many SD card readers");
    }
    function.enable_memory_and_dma();
    let source = function.address();
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let reader = Reader { slots: [const { Once::new() }; MAX_SLOTS as usize], thread: Once::new() };
    let reader = Arc::try_new_in(reader, heap).map_err(|_| "out of memory")?;
    let _ = READERS[index].set(reader.clone());
    // Its interrupt, before its slots are set up, as a card's partition
    // table is read through its queue: the handler finds each slot as it
    // is set up.
    let interrupt = Msi::find(function).and_then(|msi| {
        let (vector, message) = <Arch as Devices>::allocate_interrupt(source, on_interrupt, index)?;
        msi.enable(function, message);
        Some(vector)
    });
    if let Some(vector) = interrupt {
        info!("Disk: the SD card reader at {source} interrupts at MSI vector {vector:#x}");
    }
    let mut watchers = Vec::new_in(heap);
    watchers.try_reserve_exact(usize::from(slots)).map_err(|_| "out of memory")?;
    for number in 0..usize::from(slots) {
        let mut place = TextBuf::<40>::new();
        let _ = write!(place, "{}, slot {}", source, number + 1);
        let at = (&*reader, number);
        match add_slot(function, first + number, &place, at, interrupt.is_some()) {
            Ok((watcher, card)) => {
                *next += usize::from(card);
                if let Some(watcher) = watcher {
                    let _ = watchers.push_within_capacity(watcher);
                }
            }
            Err(why) => warn!("Disk: the SD card reader at {place}: {why}"),
        }
    }
    if watchers.is_empty() {
        return Ok(());
    }
    let options = Options::new().held().policy(Policy::Fair { nice: -5 });
    let thread = sched::spawn("sdcard", reader_main, index, options)
        .map_err(|_| "its thread could not be started")?;
    for watcher in &mut watchers {
        let drive = removable::add(&watcher.name, thread.clone(), Kind::Polled, &[]);
        watcher.drive = Some(drive.ok_or("out of memory")?);
    }
    let _ = reader.thread.set(thread.clone());
    let watchers = Box::try_new_in(Watchers(watchers), heap).map_err(|_| "out of memory")?;
    HANDOFFS.lock_irq()[index] = Some(watchers);
    sched::release(&thread);
    Ok(())
}

/// Sets up the slot whose registers are in BAR `bar`, the `number`th of
/// `reader`, and the card in it, its requests raising an `interrupt` or
/// not. Returns the slot's watcher, for a slot whose cards come and go,
/// and whether a card was in it.
#[inline(never)]
fn add_slot(
    function: &Function,
    bar: usize,
    place: &TextBuf<40>,
    (reader, number): (&Reader, usize),
    interrupt: bool,
) -> Result<(Option<Watcher>, bool), &'static str> {
    let Some(Bar::Memory { address, size, .. }) = function.bar(bar) else {
        return Err("its registers are not in memory");
    };
    if size < radix_sdhci::REGISTERS as u64 {
        return Err("its registers are too few");
    }
    let regs = <Arch as Devices>::map_device(address, radix_sdhci::REGISTERS)
        .ok_or("its registers could not be mapped")?;
    let source = function.address();
    let mut alloc = |pages: usize, below: bool| <Arch as Devices>::alloc_dma(source, pages, below);
    let mut slot =
        Slot::new(regs.clone(), &mut alloc, &mut queue::delay).map_err(|err| err.as_str())?;
    let removable = slot.removable();
    if !removable && !slot.present() {
        info!("Disk: the SD card reader at {place} is empty");
        return Ok((None, false));
    }
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let detect = Detect::new(regs.clone()).ok_or("its registers are too few")?;
    let watched = Watched { detect, disk: SpinLock::new(None), events: AtomicU8::new(0) };
    let watched = Arc::try_new_in(watched, heap).map_err(|_| "out of memory")?;
    let name = slot_name();
    let (major, minor) = slot.version();
    let card = if slot.present() {
        Some(slot.identify(&mut queue::delay).map_err(|err| err.as_str())?)
    } else {
        None
    };
    let loaded = match &card {
        Some(card) => Some(load(slot, card, &name, place, (source, interrupt))?),
        None => None,
    };
    // Handed to the interrupt handler, and the slot's interrupt on, before
    // the card's partition table is read through its queue.
    *watched.disk.lock_irq() = loaded.as_ref().map(|loaded| loaded.disk.clone());
    let _ = reader.slots[number].set(watched.clone());
    watched.detect.interrupts(interrupt);
    let empty: DeviceRef = Arc::try_new_in(Empty, heap).map_err(|_| "out of memory")?;
    match (&card, &loaded) {
        (Some(card), Some(loaded)) => {
            info!(
                "Disk: {name} is an {} in the SD card reader at {place}: {}, a version \
                 {major}.{minor:02} reader",
                Described(card),
                details(card),
            );
            let device = loaded.device.clone();
            register(Entry { name: name.clone(), device, place: None, ident: ident(card) });
            partitions(&name, &loaded.device);
        }
        _ => {
            info!(
                "Disk: {name} is an empty slot of the SD card reader at {place}, a version \
                 {major}.{minor:02} reader"
            );
            let device = empty.clone();
            register(Entry { name: name.clone(), device, place: None, ident: None });
        }
    }
    if !removable {
        return Ok((None, card.is_some()));
    }
    let watcher = Watcher {
        watched,
        regs,
        source,
        interrupt,
        name,
        place: place.clone(),
        drive: None,
        empty,
        held: loaded.map(|loaded| loaded.held),
        left: false,
        out: None,
        next: 0,
    };
    Ok((Some(watcher), card.is_some()))
}

/// The next slot's name: `mmcblk0` and on.
fn slot_name() -> Name {
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = Name::new();
    let _ = write!(name, "mmcblk{number}");
    name
}

/// The slots a controller's thread watches.
struct Watchers(Vec<Watcher, Kmalloc>);

/// The thread of the controller at `index` in [`READERS`]: looks at each
/// of its slots when it is due, or woken.
fn reader_main(index: usize) {
    let Some(mut watchers) = HANDOFFS.lock_irq()[index].take() else { return };
    loop {
        let now = sched::now_ns();
        let next = watchers.0.iter_mut().map(|watcher| watcher.tend(now)).min();
        sched::block_until(next.unwrap_or(u64::MAX), "sdcard");
    }
}

/// A slot for cards that come and go, as its thread watches it.
struct Watcher {
    watched: Arc<Watched, Kmalloc>,
    /// Its registers, for each card's [`Slot`].
    regs: Mmio,
    source: Address,
    /// Its controller interrupts.
    interrupt: bool,
    name: Name,
    place: TextBuf<40>,
    /// Set before the thread starts.
    drive: Option<DriveRef>,
    /// The device the slot is while empty.
    empty: DeviceRef,
    /// The card in it, while one is: what its queue shares.
    held: Option<Arc<Held, Kmalloc>>,
    /// The card in it is left alone until it is taken out: let go, or
    /// one that could not be set up.
    left: bool,
    /// Its card was taken out, `why`, and its requests fail, but it is
    /// still in `/devices` while the mount table is busy.
    out: Option<&'static str>,
    /// When it is looked at next.
    next: u64,
}

impl Watcher {
    /// Looks at the slot if it is due, or its card detect said something,
    /// or eject(1) or a look asked; lets its card go if eject(1) asked.
    /// Returns when it is due next.
    fn tend(&mut self, now: u64) -> u64 {
        if !self.finish_out() {
            return now.saturating_add(BUSY_NS);
        }
        let Some(drive) = self.drive.clone() else { return u64::MAX };
        let events = self.events();
        let (eject, asked) = (drive.eject_asked(), drive.look_asked());
        if !events.any() && !eject && !asked && now < self.next {
            return self.next;
        }
        if eject {
            self.take_out(LET_GO);
            self.left = true;
            drive.answer(true);
        }
        let settling = self.look(events);
        if asked {
            drive.looked(true);
        }
        let wait = if settling { SETTLING_NS } else { LOOK_NS };
        self.next = sched::now_ns().saturating_add(wait);
        self.next
    }

    /// What the card detect said since the last look: what the interrupt
    /// handler read, and what is left in the slot's status.
    fn events(&self) -> Events {
        let taken = self.watched.events.swap(0, Ordering::AcqRel);
        let read = self.watched.detect.events();
        Events {
            inserted: read.inserted || taken & INSERTED != 0,
            removed: read.removed || taken & REMOVED != 0,
        }
    }

    /// Looks at the slot's card detect: a card taken out, or swapped for
    /// another since the last look, is taken out of the kernel, and one
    /// put in is set up. Returns whether the card detect is still
    /// settling.
    fn look(&mut self, events: Events) -> bool {
        match self.watched.detect.presence() {
            Presence::Settling => return true,
            Presence::Empty => {
                self.left = false;
                self.take_out(TAKEN_OUT);
            }
            Presence::Card { .. } => {
                if events.removed {
                    self.left = false;
                    self.take_out(TAKEN_OUT);
                }
                if self.held.is_none() && self.out.is_none() && !self.left {
                    if events.inserted {
                        sched::sleep_ns(SETTLE_NS);
                    }
                    self.put_in();
                }
            }
        }
        false
    }

    /// Sets up the card in the slot, which takes the empty slot's place
    /// with its partitions. One that cannot be set up is left alone until
    /// it is taken out.
    fn put_in(&mut self) {
        let Some(drive) = self.drive.clone() else { return };
        let source = self.source;
        let mut alloc =
            |pages: usize, below: bool| <Arch as Devices>::alloc_dma(source, pages, below);
        let slot = Slot::new(self.regs.clone(), &mut alloc, &mut queue::delay);
        let mut slot = match slot {
            Ok(slot) => slot,
            Err(err) => {
                warn!("Disk: {}: a card went in, but the slot {}", self.name, err.as_str());
                self.left = true;
                return;
            }
        };
        let card = match slot.identify(&mut queue::delay) {
            Ok(card) => card,
            // Taken out again: the next look says.
            Err(radix_sdhci::Error::NoCard) => return,
            Err(err) => {
                info!("Disk: {}: a card went in, but {}; not used", self.name, err.as_str());
                self.left = true;
                self.watched.detect.power_off();
                self.watched.detect.interrupts(self.interrupt);
                return;
            }
        };
        let loaded = load(slot, &card, &self.name, &self.place, (source, self.interrupt));
        // The new slot's set-up turned its interrupt off.
        self.watched.detect.interrupts(self.interrupt);
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(why) => {
                warn!("Disk: {}: a card went in, but {why}", self.name);
                self.left = true;
                return;
            }
        };
        info!("Disk: {}: an {} went in: {}", self.name, Described(&card), details(&card));
        // Its queue's interrupts reach it from now on, as its partition
        // table is read through it.
        *self.watched.disk.lock_irq() = Some(loaded.disk.clone());
        let mut read = |sector: u64, out: &mut [u8; 512]| {
            radix_block::read_sector(&*loaded.device, sector, out)
                .map_err(|_| radix_block::partition::Error::Io)
        };
        let parts = super::table(&self.name, &loaded.device, &mut read);
        let parts = parts.as_deref().unwrap_or(&[]);
        super::set_ident(&self.name, ident(&card));
        // The slot was empty: nothing was mounted from it to lose.
        match super::replace(&self.name, loaded.device.clone(), parts, (&drive, false)) {
            Replaced::Done => self.held = Some(loaded.held),
            _ => {
                *loaded.held.gone.lock_irq() = Some(TAKEN_OUT);
                *self.watched.disk.lock_irq() = None;
            }
        }
    }

    /// Takes the slot's card out of the kernel, if one is in: its
    /// requests fail `why` from now on, its power goes off, and the slot
    /// is empty, once the mount table lets what was mounted from it be
    /// lost.
    fn take_out(&mut self, why: &'static str) {
        let Some(held) = self.held.take() else { return };
        // First, so that an unmount that syncs it, with the mount table
        // held, fails at once and lets the table go.
        *held.gone.lock_irq() = Some(why);
        fence(Ordering::SeqCst);
        // A reset the queue began before it saw the card gone ends first:
        // the slot is the next card's from now on.
        while held.resetting.load(Ordering::SeqCst) {
            sched::sleep_ns(1_000_000);
        }
        let disk = self.watched.disk.lock_irq().take();
        // Its waiters, polling or asleep, find their requests failed.
        if let Some(disk) = &disk {
            disk.collect(0);
        }
        self.watched.detect.power_off();
        self.out = Some(why);
        self.finish_out();
    }

    /// Finishes taking out the card [`Watcher::take_out`] took out: what
    /// was mounted from it is lost, and the slot is empty in `/devices`.
    /// False while the mount table is busy.
    fn finish_out(&mut self) -> bool {
        let Some(why) = self.out else { return true };
        let Some(drive) = &self.drive else { return true };
        let lost = super::replace(&self.name, self.empty.clone(), &[], (drive, true));
        if lost == Replaced::Busy {
            return false;
        }
        self.out = None;
        super::set_ident(&self.name, None);
        info!("Disk: {}: {why}", self.name);
        true
    }
}

/// A card's kind and size, as the log says it: "SDHC card of 32 GiB".
struct Described<'a>(&'a Card);

impl core::fmt::Display for Described<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let card = self.0;
        write!(f, "{} card of {}", card.kind(), Size(card.blocks * radix_block::SECTOR))
    }
}

/// What the log says of a card after its kind: its name, its bus speed
/// mode, clock and bus, and its write-protect switch.
fn details(card: &Card) -> TextBuf<80> {
    let product = card.name();
    let mut text = TextBuf::<80>::new();
    let _ = write!(
        text,
        "{}, {} at {} MHz on a {}-bit bus{}",
        if product.is_empty() { "unnamed" } else { product },
        card.speed.name(),
        card.khz / 1000,
        card.width,
        if card.read_only { ", write-protected" } else { "" },
    );
    text
}

/// What a card says it is, as Linux shows it: its product's name, its
/// serial number in hex and its revision.
fn ident(card: &Card) -> Option<Ident> {
    let mut serial = TextBuf::<16>::new();
    let _ = write!(serial, "0x{:08x}", card.cid.serial);
    let mut firmware = TextBuf::<8>::new();
    let _ = write!(firmware, "{}.{}", card.cid.revision >> 4, card.cid.revision & 0xf);
    Some(Ident {
        model: super::padded(card.name().as_bytes()),
        serial: super::padded(serial.as_str().as_bytes()),
        firmware: super::padded(firmware.as_str().as_bytes()),
    })
}

/// The slot's part of the queue, on the heap.
#[inline(never)]
fn new_slot(
    slot: Slot,
    held: Arc<Held, Kmalloc>,
    bounce: Option<Dma>,
    place: TextBuf<40>,
) -> Result<Part<SdSlot>, &'static str> {
    let heap = || HEAP.with(Flags::MAY_SLEEP);
    let mut buffers = Vec::new_in(heap());
    let mut descriptors = Vec::new_in(heap());
    let mut copy = Vec::new_in(heap());
    buffers.try_reserve_exact(SEGMENTS).map_err(|_| "out of memory")?;
    descriptors.try_reserve_exact(SEGMENTS).map_err(|_| "out of memory")?;
    if bounce.is_some() {
        copy.try_reserve_exact(PAGE).map_err(|_| "out of memory")?;
        while copy.len() < PAGE {
            let _ = copy.push_within_capacity(0);
        }
    }
    let controller = SdSlot {
        slot,
        held,
        flight: None,
        done: None,
        bounce,
        buffers,
        descriptors,
        copy,
        why: None,
        place,
    };
    queue::part(controller).ok_or("out of memory")
}

/// The queue of a card, on the heap: with no write cache, interrupting
/// when its controller does, else polled.
#[inline(never)]
fn new_disk(
    controller: Part<SdSlot>,
    info: Info,
    name: Name,
    source: Address,
    interrupt: bool,
) -> Result<Arc<Disk<SdSlot>, Kmalloc>, &'static str> {
    let once = core::iter::once(controller);
    let disk =
        Disk::new(once, info, false, false, interrupt, name, source).ok_or("out of memory")?;
    Arc::try_new_in(disk, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")
}
