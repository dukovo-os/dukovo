// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PC floppy drives (roadmap 11h, ADR 0435): the floppy disk controller
//! the firmware describes in its ACPI namespace (`PNP0700`), an Intel
//! 82077AA or a Super I/O chip's copy of one, which `radix_fdc` drives,
//! and its drives, `fd0` and `fd1`.
//!
//! [`find`] runs once uACPI has loaded the namespace. It takes the
//! controller's ports from its `_CRS`, the drives attached from its
//! `_FDE`, and what each drive is from the `_FDI` of the device under
//! the controller whose `_ADR` is the drive's number; a drive the
//! firmware says nothing of is taken for a 3.5-inch 1.44 MB one. Each
//! drive is named after its number on the controller, as Linux names
//! them, and is a drive whose medium comes and goes ([`super::removable`],
//! ADR 0430): empty, a device of no sectors, until a disk is found in it.
//! A disk found, 1.44 MB or 720 KB, is read and written whole, with no
//! partition table, as Linux reads a floppy disk.
//!
//! One thread runs the controller and its drives, since the controller
//! takes one command at a time: it carries out the disks' requests
//! ([`super::handed`] hands them over) a cylinder at a time, through a
//! buffer below 1 MiB that ISA DMA reaches (`Devices::isa_dma_buffer`),
//! and turns a drive's motor off three seconds after its last use. The
//! controller's interrupt is not routed: the thread polls it, asleep
//! between looks, as an IDE channel is polled (ADR 0244).
//!
//! A drive is never polled for a disk: its disk change line comes down
//! only when its heads step, which is heard. The thread looks at boot,
//! when radix-block(2)'s REREAD_PARTITIONS asks
//! ([`super::removable::Drive::look`]; `rdisk --reread /devices/fd0`),
//! and reads the change line before each request: a disk
//! taken out fails the request and leaves the drive empty until the next
//! look. EJECT lets the disk go, since a PC floppy drive has no motor to
//! push it out: the drive is empty, and stays so, until the change line
//! says the disk came out.
//!
//! A track is formatted when radix-block(2)'s FORMAT_TRACK asks (ADR
//! 0436; floppy(1)): the thread takes the disk out of the kernel, since
//! what it held is going, and formats the track with FORMAT TRACK, its
//! sector IDs in the DMA buffer. The next look finds the disk formatted.
//!
//! The `fd` names are shared with USB floppy drives
//! (`super::usb`, ADR 0436): a PC drive takes its number on the
//! controller, and a USB one the first number free once the PC's drives
//! are named ([`wait_for_names`]).
//!
//! When it breaks: the log says what the controller answered for a
//! request that failed (`Disk: fd0: ...`), with the controller's status
//! bytes, and `kdb`'s thread list shows the `floppy` thread's wait.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use radix_block::{DeviceRef, Empty, Info, SECTOR};
use radix_fdc::{Direction, DriveKind, Error, Fdc, Format, Found, dma};
use radix_hal::{Devices, UserMode};
use radix_hw::pci::Address;
use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::SpinLock;
use radix_uacpi::Node;
use radix_uacpi::power::Fields as _;

use super::handed;
use super::queue::{Op, Outcome};
use super::removable::{self, DriveRef, FormatError, Kind, SIZE_720K, SIZE_1440K, Size, Track};
use super::{Entry, Name, register};
use crate::Arch;
use crate::ioports::IoWindow;
use crate::sched::{self, Options, Policy};

/// The drives of a controller the kernel drives: the first two, as every
/// PC's cable has.
const MAX_UNITS: usize = 2;
/// How long a drive's motor takes to come up to speed: what the
/// 82077AA's datasheet gives a 3.5-inch drive, and Linux waits.
const SPIN_UP_NS: u64 = 500_000_000;
/// How long a drive's motor runs after its last use, as Linux's.
const MOTOR_IDLE_NS: u64 = 3_000_000_000;
/// How long the thread sleeps with nothing to do, at most.
const IDLE_NS: u64 = 1_000_000_000;
/// How soon a disk taken out is tried again while the mount table is
/// busy.
const BUSY_NS: u64 = 100_000_000;
/// How many times a transfer is tried before its request fails, the
/// heads recalibrated or the controller reset between tries.
const TRIES: u32 = 3;
/// The most buffers of one request.
const MAX_SEGMENTS: usize = 32;
/// The bytes of the most sector IDs a track's format takes: a 1.44 MB
/// disk's 18.
const MAX_IDS: usize = 18 * radix_fdc::format::ID_BYTES;
/// How long the end of boot waits for the drives' first looks: a motor
/// coming up to speed, and a try at each data rate.
pub const FIRST_LOOK_NS: u64 = 15_000_000_000;

/// Why a disk's requests fail once it is gone.
const TAKEN_OUT: &str = "its disk was taken out";
const LET_GO: &str = "its disk was let go, and may be taken out";
const FORMATTING: &str = "its disk is being formatted";

/// Drives whose first look is not done.
static LOOKING: AtomicUsize = AtomicUsize::new(0);
/// The `fd` numbers taken, a bit each, by PC and USB floppy drives.
static FD_NUMBERS: AtomicU32 = AtomicU32::new(0);
/// The PC's floppy drives are named: [`find`] is done.
static NAMED: AtomicBool = AtomicBool::new(false);

/// Takes the `fd` number `wanted`, or with `None` the first free; returns
/// it and the drive's name. `None` if it is taken, or all 32 are.
pub(super) fn take_fd_name(wanted: Option<u32>) -> Option<(u32, Name)> {
    let mut taken = FD_NUMBERS.load(Ordering::Acquire);
    loop {
        let number = wanted.unwrap_or((!taken).trailing_zeros());
        if number >= 32 || taken & 1 << number != 0 {
            return None;
        }
        let with = taken | 1 << number;
        match FD_NUMBERS.compare_exchange_weak(taken, with, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                let mut name = Name::new();
                let _ = write!(name, "fd{number}");
                return Some((number, name));
            }
            Err(now) => taken = now,
        }
    }
}

/// Gives back an `fd` number [`take_fd_name`] took.
pub(super) fn give_back_fd_number(number: u32) {
    FD_NUMBERS.fetch_and(!(1 << number), Ordering::AcqRel);
}

/// Waits until the PC's floppy drives are named, so that a USB floppy
/// drive's name does not take theirs: `fd0` is the PC's first drive when
/// it has one, each boot.
pub(super) fn wait_for_names() {
    while !NAMED.load(Ordering::Acquire) {
        sched::sleep_ns(10_000_000);
    }
}

/// The sizes of disk a drive of `kind` formats: those it reads.
fn sizes(kind: DriveKind) -> &'static [Size] {
    match kind.formats() {
        [_, _] => &[SIZE_1440K, SIZE_720K],
        [_] => &[SIZE_720K],
        _ => &[],
    }
}

/// At the end of boot: waits until every floppy drive found has been
/// looked at, for at most [`FIRST_LOOK_NS`].
pub fn wait_for_disks() {
    let deadline = sched::now_ns().saturating_add(FIRST_LOOK_NS);
    while LOOKING.load(Ordering::Acquire) > 0 {
        if sched::now_ns() >= deadline {
            info!("Disk: a floppy drive has not been looked at yet; going on");
            return;
        }
        sched::sleep_ns(10_000_000);
    }
}

/// Waits `us` microseconds, asleep: the controller's commands and the
/// times its datasheet gives are waited out in full.
fn wait_us(us: u32) {
    sched::sleep_ns(u64::from(us) * 1_000);
}

/// Finds the floppy disk controller the firmware describes, and its
/// drives, and starts the thread that runs them. Called once, after
/// uACPI has started.
pub fn find() {
    let mut found = None;
    if crate::acpi::running() {
        radix_uacpi::find_devices(c"PNP0700", &mut |node| {
            found = found.or(Some(node));
        });
    }
    if let Some(node) = found
        && let Err(why) = start(node)
    {
        warn!("Disk: could not set up the floppy disk controller {}: {why}", node.path());
    }
    NAMED.store(true, Ordering::Release);
}

/// A drive the controller has, handed to its thread.
struct Setup {
    number: u8,
    kind: DriveKind,
    name: Name,
    drive: DriveRef,
    empty: DeviceRef,
}

/// The controller and its drives, handed to its thread.
struct Handoff {
    fdc: Fdc<IoWindow>,
    buffer: u64,
    source: Address,
    units: [Option<Setup>; MAX_UNITS],
}

/// The controller [`find`] set up, until its thread takes it.
static HANDOFF: SpinLock<Option<Box<Handoff, Kmalloc>>> = SpinLock::new(None);

/// Sets up the controller at `node`: its ports, its DMA buffer, a reset,
/// and its drives, registered empty; then starts its thread.
fn start(node: Node) -> Result<(), &'static str> {
    let resources = node.bytes(c"_CRS", radix_fdc::format::resources).ok().flatten();
    let base = resources.map_or(radix_fdc::BASE, |found| found.base);
    if resources.and_then(|found| found.dma).is_some_and(|channel| channel != dma::CHANNEL) {
        return Err("it uses a DMA channel other than 2");
    }
    let (buffer, len) =
        <Arch as Devices>::isa_dma_buffer().ok_or("no memory below 1 MiB was kept for its DMA")?;
    if len < radix_fdc::MAX_TRANSFER {
        return Err("the memory kept for its DMA is too small");
    }
    // ISA DMA comes from the ISA bridge: behind an IOMMU, the buffer is
    // mapped for it, for good, as Linux maps the first 16 MiB for it.
    let source = isa_bridge().unwrap_or(Address { segment: 0, bus: 0, device: 0, function: 0 });
    if !<Arch as Devices>::map_dma(source, &[(buffer, len as u32)]) {
        return Err("its DMA buffer could not be mapped for the ISA bridge");
    }
    let window = |(first, count): (u16, u16)| IoWindow::new(first, count);
    let mut fdc = Fdc::new(
        IoWindow::new(base, radix_fdc::PORTS),
        window(dma::PORTS),
        window(dma::PAGE_PORTS),
    );
    if !fdc.present() {
        return Err("nothing answers at its ports");
    }
    fdc.reset(&mut wait_us).map_err(|err| err.as_str())?;
    let chip = if fdc.enhanced() { "an 82077AA" } else { "an older controller" };
    info!("Disk: a floppy disk controller at port {base:#x}: {chip}");

    let options = Options::new().held().policy(Policy::Fair { nice: -5 });
    let thread = sched::spawn("floppy", controller_main, 0, options)
        .map_err(|_| "its thread could not be started")?;
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut units = [const { None }; MAX_UNITS];
    for (number, kind) in attached(node).into_iter().enumerate() {
        let Some(kind) = kind else { continue };
        let number = number as u8;
        if kind.formats().is_empty() {
            info!(
                "Disk: drive {number} of the floppy disk controller is {kind}, which is not read"
            );
            continue;
        }
        let Some((_, name)) = take_fd_name(Some(u32::from(number))) else {
            warn!("Disk: drive {number} of the floppy disk controller: its name is taken");
            continue;
        };
        let empty: DeviceRef = Arc::try_new_in(Empty, heap).map_err(|_| "out of memory")?;
        let drive = removable::add(&name, thread.clone(), Kind::Floppy, sizes(kind))
            .ok_or("out of memory")?;
        info!("Disk: {name} is {kind}, drive {number} of the floppy disk controller");
        register(Entry { name: name.clone(), device: empty.clone(), place: None, ident: None });
        units[usize::from(number)] = Some(Setup { number, kind, name, drive, empty });
    }
    let count = units.iter().flatten().count();
    let handoff = Handoff { fdc, buffer, source, units };
    *HANDOFF.lock_irq() = Some(Box::try_new_in(handoff, heap).map_err(|_| "out of memory")?);
    LOOKING.fetch_add(count, Ordering::AcqRel);
    sched::release(&thread);
    Ok(())
}

/// What is attached to the controller at `node`, by drive number: the
/// drives its `_FDE` says are there (the first alone, if it has none),
/// each of the kind its `_FDI` says (a 3.5-inch 1.44 MB drive, if none).
fn attached(node: Node) -> [Option<DriveKind>; MAX_UNITS] {
    // _FDE (ACPI 6.5, 9.9.1): a 32-bit word for each of four drives, 1
    // for one that is there, then one for a tape drive.
    let present = node
        .bytes(c"_FDE", |bytes| {
            let mut present = [false; MAX_UNITS];
            for (there, word) in present.iter_mut().zip(bytes.as_chunks::<4>().0) {
                *there = u32::from_le_bytes(*word) == 1;
            }
            present
        })
        .unwrap_or([true, false]);
    // _FDI (ACPI 6.5, 9.9.2): its second element is the drive's type, as
    // CMOS RAM has it.
    let mut kinds = [None; MAX_UNITS];
    radix_uacpi::find_all_devices(&mut |child| {
        if child.parent() != Some(node) {
            return;
        }
        let Ok(number) = child.address() else { return };
        let Some(slot) = usize::try_from(number).ok().and_then(|at| kinds.get_mut(at)) else {
            return;
        };
        *slot = child.package(c"_FDI", |fdi| fdi.integer(1)).ok().flatten();
    });
    let mut found = [None; MAX_UNITS];
    for (at, slot) in found.iter_mut().enumerate() {
        let kind = kinds[at].and_then(DriveKind::from_number).unwrap_or(DriveKind::Inch35Hd);
        *slot = present[at].then_some(kind);
    }
    found
}

/// The PCI function that carries ISA DMA: the ISA bridge (a chipset's
/// LPC bridge).
fn isa_bridge() -> Option<Address> {
    let mut found = None;
    super::pci_functions(|function| {
        if found.is_none() && function.class >> 8 == 0x0601 {
            found = Some(function.address);
        }
    });
    found
}

/// The controller's thread.
fn controller_main(_: usize) {
    let Some(handoff) = HANDOFF.lock_irq().take() else { return };
    let Handoff { fdc, buffer, source, units } = *handoff;
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut frames = Vec::new_in(heap);
    let mut bytes = Vec::new_in(heap);
    let made = frames.try_reserve_exact(MAX_SEGMENTS).is_ok()
        && bytes.try_reserve_exact(radix_fdc::MAX_TRANSFER).is_ok();
    if !made {
        warn!(
            "Disk: the floppy disk controller's thread is out of memory; its drives are not used"
        );
        LOOKING.store(0, Ordering::Release);
        return;
    }
    while bytes.push_within_capacity(0).is_ok() {}
    let mut hardware = Hardware { fdc, buffer, source, frames, bytes };
    let mut units = units.map(|setup| setup.map(Unit::new));
    loop {
        let mut next = sched::now_ns().saturating_add(IDLE_NS);
        for slot in &mut units {
            let Some(unit) = slot else { continue };
            match unit.tend(&mut hardware) {
                Tended::Wanted(at) => next = next.min(at),
                Tended::Absent => {
                    unit.absent();
                    *slot = None;
                }
            }
        }
        let now = sched::now_ns();
        if next > now {
            sched::block_until(next, "floppy");
        }
    }
}

/// The controller and what its thread moves data with.
struct Hardware {
    fdc: Fdc<IoWindow>,
    /// The buffer ISA DMA reaches.
    buffer: u64,
    /// The ISA bridge, which ISA DMA comes from.
    source: Address,
    /// The buffers of the request carried out.
    frames: Vec<(u64, u32), Kmalloc>,
    /// A cylinder's bytes on their way between the request's buffers and
    /// [`Hardware::buffer`].
    bytes: Vec<u8, Kmalloc>,
}

/// A disk in a drive, as the thread keeps it: its format and its queue.
struct Loaded {
    format: &'static Format,
    read_only: bool,
    queue: handed::Medium,
}

/// What [`Unit::tend`] leaves to the thread.
enum Tended {
    /// The drive wants the thread at this time: at once, for a request
    /// waiting, or when its motor is to go off.
    Wanted(u64),
    /// The drive did not answer its first look: it is not there.
    Absent,
}

/// A drive, as the controller's thread runs it.
struct Unit {
    number: u8,
    kind: DriveKind,
    name: Name,
    drive: DriveRef,
    /// The device the drive is while empty.
    empty: DeviceRef,
    /// The disk in it, if one is that the kernel reads.
    loaded: Option<Loaded>,
    /// Its disk was taken out, `why`, and its requests fail, but it is
    /// still in `/devices` while the mount table is busy.
    out: Option<&'static str>,
    /// Its disk was let go: the drive is empty until the change line says
    /// the disk came out.
    left: bool,
    /// The change line was up before a request: the disk is taken out.
    changed: bool,
    /// The first look is not done.
    first: bool,
    /// When its motor came on, while it is on.
    motor: Option<u64>,
    /// When it was last used.
    used: u64,
}

impl Unit {
    fn new(setup: Setup) -> Unit {
        let Setup { number, kind, name, drive, empty } = setup;
        Unit {
            number,
            kind,
            name,
            drive,
            empty,
            loaded: None,
            out: None,
            left: false,
            changed: false,
            first: true,
            motor: None,
            used: 0,
        }
    }

    /// Does what the drive waits for: its first look, an eject or a look
    /// asked, a disk taken out, and the request waiting; turns its motor
    /// off once it has been idle long enough.
    fn tend(&mut self, hardware: &mut Hardware) -> Tended {
        if !self.finish_out() {
            return Tended::Wanted(sched::now_ns().saturating_add(BUSY_NS));
        }
        if self.drive.eject_asked() {
            self.take_out(LET_GO);
            self.left = true;
            self.drive.answer(true);
        }
        if self.first || self.drive.look_asked() {
            let looked = self.look(hardware);
            if core::mem::take(&mut self.first) {
                LOOKING.fetch_sub(1, Ordering::AcqRel);
                if looked == Err(Error::Seek) {
                    return Tended::Absent;
                }
            }
            if let Err(err) = looked {
                warn!("Disk: {}: it could not be looked at: {}", self.name, err.as_str());
            }
            self.drive.looked(looked.is_ok());
        }
        if let Some(track) = self.drive.format_asked() {
            let formatted = self.format(hardware, track);
            self.drive.formatted(formatted);
        }
        if core::mem::take(&mut self.changed) {
            self.take_out(TAKEN_OUT);
            return Tended::Wanted(0);
        }
        if self.serve_one(hardware) {
            return Tended::Wanted(0);
        }
        let now = sched::now_ns();
        match self.motor {
            Some(_) if now >= self.used.saturating_add(MOTOR_IDLE_NS) => {
                hardware.fdc.motor(self.number, false);
                self.motor = None;
                Tended::Wanted(u64::MAX)
            }
            Some(_) => Tended::Wanted(self.used.saturating_add(MOTOR_IDLE_NS)),
            None => Tended::Wanted(u64::MAX),
        }
    }

    /// Turns the drive's motor on, which selects it; with `spun`, waits
    /// until it is up to speed.
    fn motor_on(&mut self, hardware: &mut Hardware, spun: bool) {
        let now = sched::now_ns();
        // A reset turns the motors off: the controller's record says.
        let was = hardware.fdc.motor(self.number, true);
        if !was || self.motor.is_none() {
            self.motor = Some(now);
        }
        let since = self.motor.unwrap_or(now);
        let ready = since.saturating_add(SPIN_UP_NS);
        if spun && now < ready {
            sched::sleep_ns(ready - now);
        }
        self.used = sched::now_ns();
    }

    /// Looks whether a disk is in the drive: one taken out since the last
    /// look is taken out of the kernel, and one in it that is not read yet
    /// is found out and put in. Fails with [`Error::Seek`] when the heads
    /// do not step: on the first look, there is no drive.
    fn look(&mut self, hardware: &mut Hardware) -> Result<(), Error> {
        self.motor_on(hardware, false);
        if hardware.fdc.changed(self.number) {
            self.left = false;
            self.take_out(TAKEN_OUT);
        }
        if self.loaded.is_some() || self.left {
            return Ok(());
        }
        if !hardware.fdc.disk_in(self.number, &mut wait_us)? {
            return Ok(());
        }
        self.put_in(hardware)
    }

    /// Finds out what disk is in the drive, and puts in one the kernel
    /// reads: it gets a queue, and takes the empty drive's place.
    fn put_in(&mut self, hardware: &mut Hardware) -> Result<(), Error> {
        self.motor_on(hardware, true);
        let mut memory = |phys: u64, out: &mut [u8; radix_fdc::SECTOR]| {
            <Arch as UserMode>::read_frames(phys, out);
        };
        let found =
            hardware.fdc.detect(self.number, self.kind, hardware.buffer, &mut memory, &mut wait_us);
        let format = match found? {
            Found::Disk(format) => format,
            Found::Unformatted => {
                info!("Disk: {}: the disk in it is not formatted", self.name);
                return Ok(());
            }
            Found::Other { sectors, heads } => {
                info!(
                    "Disk: {}: the disk in it has {sectors} sectors a track on {heads} sides, \
                     a format that is not read",
                    self.name
                );
                return Ok(());
            }
        };
        let read_only = hardware.fdc.write_protected(self.number, &mut wait_us)?;
        let info = Info {
            sectors: u64::from(format.total()),
            logical_block: SECTOR as u32,
            block_size: SECTOR as u32,
            read_only,
            max_segments: MAX_SEGMENTS,
            discard_sectors: 0,
            rotational: true,
        };
        let (queue, device) = match handed::Medium::new(&self.name, hardware.source, info, 0) {
            Ok(made) => made,
            Err(why) => {
                warn!("Disk: {}: a disk went in, but {why}", self.name);
                return Ok(());
            }
        };
        let protected = if read_only { ", write-protected" } else { "" };
        info!("Disk: {}: a {} disk went in{protected}", self.name, format.name);
        // The drive was empty: nothing was mounted from it to lose.
        match super::replace(&self.name, device, &[], (&self.drive, false)) {
            super::Replaced::Done => self.loaded = Some(Loaded { format, read_only, queue }),
            _ => queue.gone(TAKEN_OUT),
        }
        Ok(())
    }

    /// Formats `track` of the disk in the drive, as radix-block(2)'s
    /// FORMAT_TRACK asks (ADR 0436): the disk leaves the kernel first, as
    /// what it held goes, and the drive is empty until a look finds it
    /// formatted. The track's sector IDs go to the controller from the
    /// DMA buffer. Tried [`TRIES`] times, as a transfer is.
    fn format(&mut self, hardware: &mut Hardware, track: Track) -> Result<(), FormatError> {
        let formats = self.kind.formats();
        let format = formats.iter().find(|f| f.total() == track.sectors);
        let format = format.ok_or(FormatError::Unsupported)?;
        let (Ok(cylinder), Ok(head)) = (u8::try_from(track.cylinder), u8::try_from(track.head))
        else {
            return Err(FormatError::OutOfRange);
        };
        if cylinder >= format.cylinders || head >= format.heads {
            return Err(FormatError::OutOfRange);
        }
        self.take_out(FORMATTING);
        // A disk let go and formatted is wanted after all.
        self.left = false;
        self.motor_on(hardware, true);
        let failed = |name: &Name, err: Error| {
            warn!(
                "Disk: {name}: cylinder {cylinder}, head {head} not formatted: {}{}",
                err.as_str(),
                Status(err)
            );
            match err {
                Error::WriteProtected => FormatError::WriteProtected,
                _ => FormatError::Failed,
            }
        };
        match hardware.fdc.disk_in(self.number, &mut wait_us) {
            Ok(true) => {}
            Ok(false) => return Err(FormatError::NoDisk),
            Err(err) => return Err(failed(&self.name, err)),
        }
        match hardware.fdc.write_protected(self.number, &mut wait_us) {
            Ok(false) => {}
            Ok(true) => return Err(FormatError::WriteProtected),
            Err(err) => return Err(failed(&self.name, err)),
        }
        if (cylinder, head) == (0, 0) {
            info!("Disk: {}: formatting its disk as a {} one", self.name, format.name);
        }
        let mut ids = [0u8; MAX_IDS];
        let len = format.track_ids(cylinder, head, &mut ids);
        <Arch as UserMode>::write_frames(hardware.buffer, &ids[..len]);
        let mut tries = 1;
        loop {
            let buffer = hardware.buffer;
            let place = (cylinder, head);
            let err =
                match hardware.fdc.format_track(self.number, format, place, buffer, &mut wait_us) {
                    Ok(()) => break,
                    Err(err) => err,
                };
            let again = err.may_pass() || matches!(err, Error::Seek | Error::Timeout);
            if !again || tries >= TRIES {
                return Err(failed(&self.name, err));
            }
            tries += 1;
            if err == Error::Timeout {
                hardware.fdc.reset(&mut wait_us).map_err(|err| failed(&self.name, err))?;
                self.motor_on(hardware, true);
            } else {
                let _ = hardware.fdc.recalibrate(self.number, &mut wait_us);
            }
        }
        self.used = sched::now_ns();
        Ok(())
    }

    /// Takes the drive's disk out of the kernel, if one is in: its
    /// requests fail `why` from now on, and the drive is empty, once the
    /// mount table lets what was mounted from it be lost.
    fn take_out(&mut self, why: &'static str) {
        let Some(loaded) = self.loaded.take() else { return };
        // First, so that an unmount that syncs it, with the mount table
        // held, fails at once and lets the table go.
        loaded.queue.gone(why);
        self.out = Some(why);
        self.finish_out();
    }

    /// Finishes taking out the disk [`Unit::take_out`] took out: what was
    /// mounted from it is lost, and the drive is empty in `/devices`.
    /// False while the mount table is busy.
    fn finish_out(&mut self) -> bool {
        let Some(why) = self.out else { return true };
        let lost = super::replace(&self.name, self.empty.clone(), &[], (&self.drive, true));
        if lost == super::Replaced::Busy {
            return false;
        }
        self.out = None;
        info!("Disk: {}: {why}", self.name);
        true
    }

    /// Carries out the request waiting, if one is; says whether one was.
    fn serve_one(&mut self, hardware: &mut Hardware) -> bool {
        let Some(loaded) = &self.loaded else { return false };
        let Some((op, sector)) = loaded.queue.take(&mut hardware.frames) else { return false };
        let (outcome, why) = self.carry_out(hardware, op, sector);
        if let Some(loaded) = &self.loaded {
            loaded.queue.finish(outcome, why);
        }
        true
    }

    /// Carries out `op` from `sector` with the buffers taken, a cylinder
    /// at a time, after reading the change line.
    fn carry_out(
        &mut self,
        hardware: &mut Hardware,
        op: Op,
        sector: u64,
    ) -> (Outcome, Option<&'static str>) {
        let Some(loaded) = &self.loaded else { return (Outcome::Failed, Some(TAKEN_OUT)) };
        let (format, read_only) = (loaded.format, loaded.read_only);
        let direction = match op {
            Op::Read => Direction::Read,
            Op::Write | Op::WriteThrough => Direction::Write,
            // The drive has no cache: what it wrote is on the disk.
            Op::Flush => return (Outcome::Done, None),
            _ => return (Outcome::Failed, Some("a floppy disk does not do that")),
        };
        if direction == Direction::Write && read_only {
            return (Outcome::Failed, Some(Error::WriteProtected.as_str()));
        }
        let total: u64 = hardware.frames.iter().map(|&(_, len)| u64::from(len)).sum();
        let count = total / SECTOR;
        if sector.saturating_add(count) > u64::from(format.total()) {
            return (Outcome::Failed, Some(Error::BadRequest.as_str()));
        }
        self.motor_on(hardware, true);
        if hardware.fdc.changed(self.number) {
            self.changed = true;
            return (Outcome::Failed, Some(TAKEN_OUT));
        }
        let mut done = 0;
        while done < count {
            // Both fit in a u32: the disk's sectors do.
            let first = (sector + done) as u32;
            let sectors = u64::from(format.left_in_cylinder(first)).min(count - done);
            let len = sectors as usize * radix_fdc::SECTOR;
            let at = done * SECTOR;
            let bytes = &mut hardware.bytes[..len];
            if direction == Direction::Write {
                gather(&hardware.frames, at, bytes);
                <Arch as UserMode>::write_frames(hardware.buffer, bytes);
            }
            if let Err(err) = self.transfer(hardware, direction, format, first, sectors as u32) {
                warn!(
                    "Disk: {}: sectors {first} to {}: {}{}",
                    self.name,
                    u64::from(first) + sectors - 1,
                    err.as_str(),
                    Status(err)
                );
                return (Outcome::Failed, Some(err.as_str()));
            }
            if direction == Direction::Read {
                let bytes = &mut hardware.bytes[..len];
                <Arch as UserMode>::read_frames(hardware.buffer, bytes);
                scatter(&hardware.frames, at, bytes);
            }
            done += sectors;
        }
        self.used = sched::now_ns();
        (Outcome::Done, None)
    }

    /// Moves `count` sectors from `first`, within a cylinder, between the
    /// disk and the DMA buffer: tried [`TRIES`] times, the heads
    /// recalibrated between tries, or the controller reset when it did
    /// not answer.
    fn transfer(
        &mut self,
        hardware: &mut Hardware,
        direction: Direction,
        format: &Format,
        first: u32,
        count: u32,
    ) -> Result<(), Error> {
        let mut tries = 1;
        loop {
            let buffer = hardware.buffer;
            let unit = self.number;
            let err = match hardware.fdc.transfer(
                unit,
                direction,
                format,
                first,
                count,
                buffer,
                &mut wait_us,
            ) {
                Ok(()) => return Ok(()),
                Err(err) => err,
            };
            let again = err.may_pass() || matches!(err, Error::Seek | Error::Timeout);
            if !again || tries >= TRIES {
                return Err(err);
            }
            tries += 1;
            if err == Error::Timeout {
                hardware.fdc.reset(&mut wait_us)?;
                self.motor_on(hardware, true);
            } else {
                // Where the heads are is found again.
                let _ = hardware.fdc.recalibrate(unit, &mut wait_us);
            }
        }
    }

    /// The drive did not answer its first look: there is no drive.
    fn absent(&mut self) {
        info!("Disk: {}: its heads did not step; there is no drive", self.name);
        removable::remove(self.name.as_str());
        super::unregister(self.name.as_str());
        give_back_fd_number(u32::from(self.number));
    }
}

/// A failed transfer's status bytes, for the log: ` (ST0 0x40, ST1 0x04,
/// ST2 0x00)`, or nothing for an error that has none.
struct Status(Error);

impl core::fmt::Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Error::Failed { st0, st1, st2 } => {
                write!(f, " (ST0 {st0:#04x}, ST1 {st1:#04x}, ST2 {st2:#04x})")
            }
            _ => Ok(()),
        }
    }
}

/// Copies the bytes of `frames` (physical address, length) from byte
/// `at` of them into `out`.
fn gather(frames: &[(u64, u32)], at: u64, out: &mut [u8]) {
    each_piece(frames, at, out.len(), |phys, range| {
        <Arch as UserMode>::read_frames(phys, &mut out[range]);
    });
}

/// Copies `bytes` into `frames` (physical address, length) from byte
/// `at` of them.
fn scatter(frames: &[(u64, u32)], at: u64, bytes: &[u8]) {
    each_piece(frames, at, bytes.len(), |phys, range| {
        <Arch as UserMode>::write_frames(phys, &bytes[range]);
    });
}

/// Calls `f` with each piece of `frames` that the `len` bytes from byte
/// `at` of them fall in: its physical address, and the range of the
/// bytes it holds.
fn each_piece(
    frames: &[(u64, u32)],
    at: u64,
    len: usize,
    mut f: impl FnMut(u64, core::ops::Range<usize>),
) {
    let (mut start, mut copied) = (0u64, 0usize);
    for &(phys, frame_len) in frames {
        let end = start + u64::from(frame_len);
        let wanted = at + copied as u64;
        if copied < len && wanted < end {
            let skip = wanted - start;
            let piece = ((end - wanted) as usize).min(len - copied);
            f(phys + skip, copied..copied + piece);
            copied += piece;
        }
        start = end;
    }
}
