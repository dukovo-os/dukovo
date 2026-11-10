// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! CD, DVD and Blu-ray drives (ADR 0431): what every optical drive
//! shares, whatever carries its commands, and the drives on AHCI ports
//! and IDE channels (ADR 0432), which this module runs. A USB drive's
//! thread is its stick's ([`super::usb`]).
//!
//! An optical drive is named `cd0`, `cd1` and on, the first number free,
//! in the order the drives are found: the SATA and IDE drives in the PCI
//! scan's order, then the USB drives. It is a drive whose medium comes and goes
//! ([`super::removable`], ADR 0430), empty, a device of no sectors, until
//! a disc of data is in it. Its driver asks the disc what it is
//! (`radix_scsi::mmc::disc`): a disc of data, CD, DVD or Blu-ray, BDXL
//! among them, is read in blocks of 2,048 bytes, read-only, whole (a disc
//! has no partition table, as Linux's `sr` reads none), from the start of
//! its last session on a disc written in several (the block device's
//! `last_session`, from which ISO 9660 reads the volume). A blank disc
//! and a music CD hold nothing to read: the drive stays empty as a device,
//! and `/kernel/block/<drive>/disc` says what is in it, for Spark and
//! volumed(8) to say.
//!
//! A drive on an AHCI port or an IDE channel has a thread of its own, as a
//! USB stick does, since the drive takes one command at a time and some
//! take seconds (a disc spinning up). The thread looks at the drive every
//! two seconds for a disc put in or taken out, with TEST UNIT READY, as
//! Linux polls an optical drive. While anything uses the disc, PREVENT
//! ALLOW MEDIUM REMOVAL keeps the tray shut; eject(1) has the thread let
//! it go and open the tray. Each command is waited for asleep: an AHCI
//! port's interrupt wakes the thread (`radix_ahci::Hba::quiet`), or, on a
//! controller without one and on an IDE channel, which is polled, the
//! thread looks again after a wait that doubles to 4 ms. A drive on an IDE
//! channel borrows the channel for each command from the disk on its
//! other end ([`super::ide::IdeLink`]).
//!
//! A disc's reads are READ(10) of the queue's buffers by DMA. On an AHCI
//! port they go out from the thread that reads and end in the port's
//! interrupt, which starts the next one, as Linux's libata has them; the
//! thread takes the port for its own commands between them
//! ([`super::optical_port`], ADR 0434). On an IDE channel, the thread
//! serves them between its looks ([`super::handed`] hands them over).
//!
//! Before each look, a drive that has media events is asked for them
//! (GET EVENT STATUS NOTIFICATION, [`ask_events`]), as Linux's `sr` asks:
//! a drive whose disc is locked in keeps it when its eject button is
//! pressed, and says the button was pressed. The press is counted in
//! `/kernel/block/<drive>/eject_requests`, and volumed(8) unmounts the
//! disc and ejects it, as eject(1) does (ADR 0432).
//!
//! At the end of boot, before the first program, the kernel waits for
//! each drive's first look, for at most [`FIRST_LOOK_NS`], so that a disc
//! in a drive at boot is there for Spark ([`wait_for_discs`]).
//!
//! Between its looks, the thread also runs the commands radix-block(2)'s
//! READ_TOC and READ_AUDIO ask of the drive, a music CD's tracks and
//! samples (ADR 0447): [`run_asked`] reads each one's answer into a few
//! pages of DMA memory the thread keeps once it needs them
//! ([`COMMAND_PAGES`]), and hands it to the one who asked. A USB drive's
//! thread runs them the same way.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use radix_abi::block as request;
use radix_ahci::ata::{PacketIdentity, trimmed};
use radix_ahci::{Packets, Port};
use radix_block::{DeviceRef, Empty, Info, SECTOR};
use radix_hal::Devices;
use radix_hw::pci::Address;
use radix_hw::{Dma, PAGE_SIZE};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::mmc::{self, Content, Disc as Found, Family, Profile};
use radix_scsi::{
    Absent, Cdb, Data, Found as Medium, Inquiry, Meaning, Reply, Transport, TransportError, key,
};
use radix_sync::SpinLock;

use super::handed;
use super::ide::IdeLink;
use super::optical_port::{DiscReads, Gone, OpticalPort};
use super::queue::{self, Controller, Disk, Op, Outcome, Volume};
use super::removable::{self, Asked, CommandError, Disc, Drive, DriveRef, Kind};
use super::{Entry, Ident, Name, Size, register};
use crate::sched::{self, Options, Policy, ThreadRef};

/// How long a read may take: Linux's `sr`'s.
const TIMEOUT_MS: u32 = 30_000;
/// How long the thread sleeps with nothing to do, at most.
const IDLE_NS: u64 = 1_000_000_000;
/// How often the drive is looked at for a disc put in or taken out: two
/// seconds, as Linux polls an optical drive.
const LOOK_NS: u64 = 2_000_000_000;
/// How soon a disc taken out is tried again while the mount table is
/// busy.
const BUSY_NS: u64 = 100_000_000;
/// How long a drive that says it is becoming ready is given before a read
/// goes again.
pub(super) const READY_NS: u64 = 1_000_000_000;
/// How long the end of boot waits for the drives' first looks: a disc
/// spins up in a few seconds.
pub const FIRST_LOOK_NS: u64 = 15_000_000_000;
/// The most optical drives on AHCI ports and IDE channels.
const MAX_DRIVES: usize = 8;

/// The pages of DMA memory a drive's thread reads the answer to a command
/// asked of it into (ADR 0447): room for READ TOC's longest, 65,535 bytes,
/// and for radix-block(2)'s READ_AUDIO_MAX sectors of audio.
pub(super) const COMMAND_PAGES: usize = 16;
const _: () = assert!(
    request::READ_AUDIO_MAX as usize * mmc::AUDIO_SECTOR as usize <= COMMAND_PAGES * PAGE_SIZE
);

/// Why a disc's reads fail once it is gone.
const TAKEN_OUT: &str = "its disc was taken out";
const EJECTED: &str = "its disc was ejected";
pub(super) const STOPPED: &str = "its drive stopped answering";

/// The `cd` numbers taken, a bit each.
static CD_NUMBERS: AtomicU32 = AtomicU32::new(0);
/// Drives whose first look is not done.
static LOOKING: AtomicUsize = AtomicUsize::new(0);

/// Takes the first `cd` number free; returns it and the drive's name, or
/// `None` once all 32 are taken.
pub(super) fn take_cd_name() -> Option<(u32, Name)> {
    let mut taken = CD_NUMBERS.load(Ordering::Acquire);
    loop {
        let number = (!taken).trailing_zeros();
        if number >= 32 {
            return None;
        }
        let with = taken | 1 << number;
        match CD_NUMBERS.compare_exchange_weak(taken, with, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                let mut name = Name::new();
                let _ = write!(name, "cd{number}");
                return Some((number, name));
            }
            Err(now) => taken = now,
        }
    }
}

/// Gives back a `cd` number [`take_cd_name`] took.
pub(super) fn give_back_cd_number(number: u32) {
    CD_NUMBERS.fetch_and(!(1 << number), Ordering::AcqRel);
}

/// At the end of boot: waits until every optical drive found has looked
/// at what is in it, for at most [`FIRST_LOOK_NS`].
pub fn wait_for_discs() {
    let deadline = sched::now_ns().saturating_add(FIRST_LOOK_NS);
    while LOOKING.load(Ordering::Acquire) > 0 {
        if sched::now_ns() >= deadline {
            info!("Disk: an optical drive has not said what is in it yet; going on");
            return;
        }
        sched::sleep_ns(10_000_000);
    }
}

/// Finds out what the disc in the optical drive at logical unit `lun`
/// behind `transport` is, waiting as `wait` waits while it spins up: what
/// the drive is to say is in it, and a disc of data as a medium the
/// kernel reads, read-only, with what the drive said of it. One with no
/// data fails with [`Absent::NotData`].
pub(super) fn probe(
    transport: &mut dyn Transport,
    lun: u8,
    inquiry: Inquiry,
    wait: &mut dyn FnMut(u32),
) -> (Disc, Result<(Medium, Found), Absent>) {
    let found = match mmc::disc(transport, lun, wait) {
        Ok(found) => found,
        Err(why @ (Absent::NoMedium | Absent::Gone)) => return (Disc::None, Err(why)),
        Err(why) => return (Disc::Unreadable, Err(why)),
    };
    let disc = Disc::In(found.profile, found.content);
    let Some(capacity) = found.capacity.filter(|_| found.content.has_data()) else {
        return (disc, Err(Absent::NotData(found.content)));
    };
    let medium = Medium { inquiry, capacity, read_only: true, write_cache: false, sixteen: false };
    (disc, Ok((medium, found)))
}

/// The inquiry data of an optical drive that says nothing of its vendor:
/// an ATAPI drive, whose model its IDENTIFY PACKET DEVICE gave.
fn atapi_inquiry() -> Inquiry {
    Inquiry {
        qualifier: 0,
        device_type: radix_scsi::device_type::CD,
        removable: true,
        version: 0,
        vendor: [b' '; 8],
        product: [b' '; 16],
        revision: [b' '; 4],
    }
}

/// What the log says of a disc of data that went in, `found`, which is
/// `medium`: `a disc of data (dvd-rom) of 4.3 GiB`, with its sessions if
/// it has more than one.
pub(super) fn describe(found: &Found, medium: &Medium) -> TextBuf<64> {
    described(found.profile, found.sessions, info(medium, 1).sectors)
}

/// [`describe`], of a disc of the kind `profile`, of `sessions`, and of
/// `sectors` of 512 bytes.
pub(super) fn described(profile: Profile, sessions: u16, sectors: u64) -> TextBuf<64> {
    let mut text = TextBuf::new();
    let _ = write!(text, "a disc of data ({profile}) of {}", Size(sectors * SECTOR));
    if sessions > 1 {
        let _ = write!(text, " in {sessions} sessions, read from the last");
    }
    text
}

/// Runs `asked`, a command a program's request asked of the optical drive
/// at logical unit `lun` behind `transport` (ADR 0447): its answer is read
/// into `memory`, whose pages the transport reaches by DMA, and copied to
/// the buffer asked with, cut to the bytes that came. A command that meets
/// a change of disc goes again once, as the drive says the change only
/// once.
pub(super) fn run_asked(
    transport: &mut dyn Transport,
    lun: u8,
    asked: Asked,
    memory: &Dma,
) -> Result<Vec<u8, Kmalloc>, CommandError> {
    let Asked { cdb, mut data } = asked;
    let length = data.len().min(memory.len());
    let mut frames = [(0u64, 0u32); COMMAND_PAGES];
    let mut count = 0;
    for (at, frame) in (0..length).step_by(PAGE_SIZE).zip(frames.iter_mut()) {
        *frame = (memory.phys() + at as u64, (length - at).min(PAGE_SIZE) as u32);
        count += 1;
    }
    for _ in 0..2 {
        let moved = Data::Frames { into_host: true, frames: &frames[..count] };
        match transport.command(lun, &cdb, moved, TIMEOUT_MS) {
            Ok(Reply::Good { residue }) => {
                let came = length.saturating_sub(residue as usize);
                memory.read(0, &mut data[..came]);
                data.truncate(came);
                return Ok(data);
            }
            Ok(Reply::Failed(Some(sense))) => match sense.meaning() {
                // The disc changed since the drive last said: once more.
                Meaning::Attention => {}
                Meaning::NoMedium => return Err(CommandError::NoDisc),
                Meaning::Unsupported => return Err(CommandError::Refused),
                _ => return Err(CommandError::Failed),
            },
            Ok(Reply::Failed(None)) | Err(_) => return Err(CommandError::Failed),
        }
    }
    Err(CommandError::Failed)
}

/// The DMA memory a drive's thread runs commands asked of it with
/// ([`run_asked`]), kept in `kept` once made for the PCI function
/// `source`; `None` when memory runs out.
pub(super) fn command_memory(kept: &mut Option<Dma>, source: Address) -> Option<&Dma> {
    if kept.is_none() {
        *kept = <crate::Arch as Devices>::alloc_dma(source, COMMAND_PAGES, false);
    }
    kept.as_ref()
}

/// Has the drive's thread run the command asked of `drive`, if one waits
/// (ADR 0447), with `transport` at logical unit `lun` and the memory kept
/// in `kept` for the PCI function `source`.
pub(super) fn serve_asked(
    drive: &Drive,
    transport: &mut dyn Transport,
    lun: u8,
    (kept, source): (&mut Option<Dma>, Address),
) {
    let Some((number, asked)) = drive.command_asked() else { return };
    let result = match command_memory(kept, source) {
        Some(memory) => run_asked(transport, lun, asked, memory),
        None => Err(CommandError::Failed),
    };
    drive.commanded(number, result);
}

/// What the log says of a disc with no data that went in, as `why`
/// ([`Absent::NotData`]).
pub(super) fn not_data(content: Content) -> &'static str {
    match content {
        Content::Audio => "a music CD went in; its tracks are not files, and it is not mounted",
        _ => "a blank disc went in; it holds nothing to read",
    }
}

/// Asks the optical drive at logical unit `lun` behind `transport`, the
/// drive `name` is, for its media events before a look, if it has them
/// (`*events`): a press of its eject button while its disc was locked in
/// is said, and counted in `drive`, for volumed(8) to unmount the disc
/// and eject it (ADR 0432). A drive without them is not asked again.
/// Returns whether the drive said its disc may have changed, which the
/// look takes as its TEST UNIT READY's.
pub(super) fn ask_events(
    transport: &mut dyn Transport,
    lun: u8,
    name: &Name,
    drive: &Drive,
    events: &mut bool,
) -> bool {
    if !*events {
        return false;
    }
    match mmc::events(transport, lun) {
        Ok(Some(said)) => {
            if said.eject_request {
                info!("Disk: {name}: its eject button was pressed");
                drive.eject_requested();
            }
            said.changed
        }
        Ok(None) => {
            *events = false;
            false
        }
        // The look says.
        Err(_) => false,
    }
}

/// The block device of a disc of data, `medium`, as `found` says it is:
/// its sectors, read-only, in requests of up to `max_segments` buffers.
pub(super) fn info(medium: &Medium, max_segments: usize) -> Info {
    let capacity = medium.capacity;
    let unit = u64::from(capacity.block_length) / SECTOR;
    Info {
        sectors: capacity.blocks.saturating_mul(unit),
        logical_block: capacity.block_length,
        block_size: capacity.block_length,
        read_only: true,
        max_segments,
        discard_sectors: 0,
        rotational: true,
    }
}

/// Where the last session of the disc `found` starts, in sectors, for its
/// block device ([`Volume::with_last_session`]).
pub(super) fn last_session(found: &Found, medium: &Medium) -> u64 {
    let unit = u64::from(medium.capacity.block_length) / SECTOR;
    u64::from(found.last_session).saturating_mul(unit)
}

// ---- Drives on AHCI ports and IDE channels ----

/// The disc of data in a drive, as its thread keeps it: what the drive
/// said of it, and its queue.
struct Loaded {
    medium: Medium,
    reads: Reads,
}

/// How a disc's reads reach its drive.
enum Reads {
    /// Through the drive's thread, which takes each and carries it out:
    /// on an IDE channel, which the drive borrows for each command.
    Handed(handed::Medium),
    /// Straight to the drive's AHCI port, from the threads that read, and
    /// back from the port's interrupt ([`super::optical_port`]).
    Sent {
        port: Arc<OpticalPort, Kmalloc>,
        gone: Arc<Gone, Kmalloc>,
        disk: Arc<Disk<DiscReads>, Kmalloc>,
    },
}

impl Loaded {
    /// Takes the read handed to the thread, if one waits: what it does,
    /// its first sector, and its buffers into `frames`.
    fn take(&self, frames: &mut Vec<(u64, u32), Kmalloc>) -> Option<(Op, u64)> {
        let Reads::Handed(queue) = &self.reads else { return None };
        queue.take(frames)
    }

    /// Hands back the read the thread ran, which ended with `outcome`,
    /// and tells its queue.
    fn finish(&self, outcome: Outcome, why: Option<&'static str>) {
        if let Reads::Handed(queue) = &self.reads {
            queue.finish(outcome, why);
        }
    }

    /// The disc is gone, `why`: its reads fail from now on, those waiting
    /// at once. One already on an AHCI port ends as the drive has it, and
    /// the port lets go of the disc once it has.
    fn gone(&self, why: &'static str) {
        match &self.reads {
            Reads::Handed(queue) => queue.gone(why),
            Reads::Sent { port, gone, disk } => {
                gone.set(why);
                disk.collect(0);
                port.forget_disc();
            }
        }
    }
}

/// The queue of a disc that `info` says what it is of, in the drive
/// `name` of the controller at `source`, driven by `lane`; its
/// completions raise an interrupt if `interrupt`.
fn queue_of<C: Controller>(
    lane: C,
    info: Info,
    interrupt: bool,
    name: &Name,
    source: Address,
) -> Result<Arc<Disk<C>, Kmalloc>, &'static str> {
    let part = queue::part(lane).ok_or("out of memory")?;
    let once = core::iter::once(part);
    let disk = Disk::new(once, info, false, false, interrupt, name.clone(), source)
        .ok_or("out of memory")?;
    Arc::try_new_in(disk, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")
}

/// The block device of the disc whose queue is `disk`, read from
/// `session`, its last session's first sector.
fn device_of<C: Controller + 'static>(
    disk: &Arc<Disk<C>, Kmalloc>,
    info: Info,
    session: u64,
) -> Result<DeviceRef, &'static str> {
    let volume = Volume::new(disk.clone(), 0, info).ok_or("out of memory")?;
    let volume = volume.with_last_session(session);
    let device: DeviceRef =
        Arc::try_new_in(volume, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    Ok(device)
}

/// What a read waits for after its command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Then {
    Nothing,
    /// The drive is becoming ready: a second.
    Ready,
    /// The drive says its disc was taken out or changed: the read fails,
    /// and the drive is looked at.
    Changed,
}

/// How a read that ended with `reply` ends, as Linux's SCSI layer has it
/// for an optical drive, and what its next try waits for.
pub(super) fn judge(reply: Result<Reply, TransportError>) -> (Outcome, Option<&'static str>, Then) {
    let sense = match reply {
        Ok(Reply::Good { residue: 0 }) => return (Outcome::Done, None, Then::Nothing),
        Ok(Reply::Good { .. }) => {
            return (Outcome::Retry, Some("it moved less than asked"), Then::Nothing);
        }
        Ok(Reply::Failed(Some(sense))) => sense,
        Ok(Reply::Failed(None)) => {
            let why = "it failed the read, and did not say why";
            return (Outcome::Retry, Some(why), Then::Nothing);
        }
        Err(TransportError::Gone) => return (Outcome::Failed, Some(STOPPED), Then::Nothing),
        Err(TransportError::Failed(why)) => return (Outcome::Retry, Some(why), Then::Nothing),
    };
    match sense.meaning() {
        Meaning::Fine => (Outcome::Done, None, Then::Nothing),
        Meaning::Attention if sense.medium_changed() => {
            (Outcome::Failed, Some("its disc may have changed"), Then::Changed)
        }
        Meaning::Attention | Meaning::Aborted => {
            (Outcome::Retry, Some(sense.key_name()), Then::Nothing)
        }
        Meaning::BecomingReady | Meaning::NeedsStart => {
            (Outcome::Retry, Some("it is becoming ready"), Then::Ready)
        }
        Meaning::NoMedium => (Outcome::Failed, Some(TAKEN_OUT), Then::Changed),
        // A scratched disc's block, or a music track's.
        _ if sense.key == key::MEDIUM_ERROR => {
            (Outcome::Failed, Some("its disc could not be read there"), Then::Nothing)
        }
        _ => (Outcome::Failed, Some(sense.key_name()), Then::Nothing),
    }
}

/// What carries an optical drive's commands.
enum Link {
    /// An AHCI port, which its disc's reads share.
    Ahci(Arc<OpticalPort, Kmalloc>),
    /// An end of an IDE channel.
    Ide(IdeLink),
}

impl Link {
    /// What the drive said of itself in IDENTIFY PACKET DEVICE.
    fn identity(&self) -> Option<PacketIdentity> {
        match self {
            Link::Ahci(port) => Some(*port.identity()),
            Link::Ide(link) => Some(*link.identity()),
        }
    }

    /// Whether the drive stopped answering for good.
    fn dead(&self) -> bool {
        match self {
            Link::Ahci(port) => port.dead(),
            Link::Ide(link) => link.dead(),
        }
    }

    /// The most buffers of one read.
    fn max_segments(&self) -> usize {
        match self {
            Link::Ahci(_) => radix_ahci::MAX_SEGMENTS,
            Link::Ide(_) => IdeLink::MAX_SEGMENTS,
        }
    }

    /// Looks into a disc's read the port's interrupt found in trouble,
    /// on an AHCI port ([`OpticalPort::settle`]).
    fn settle(&self) {
        if let Link::Ahci(port) = self {
            port.settle();
        }
    }

    /// Whether a disc's read on an AHCI port found the disc taken out or
    /// changed.
    fn disc_changed(&self) -> bool {
        match self {
            Link::Ahci(port) => port.disc_changed(),
            Link::Ide(_) => false,
        }
    }

    /// Runs `f` with the drive's commands, which wait asleep.
    fn with<R>(&mut self, f: impl FnOnce(&mut dyn Transport) -> R) -> R {
        let mut wait = wait_us;
        match self {
            Link::Ahci(shared) => {
                let mut port = shared.take();
                let reply = f(&mut Packets::new(&mut port, &mut wait));
                shared.put_back(port);
                reply
            }
            Link::Ide(link) => f(&mut link.packets(&mut wait)),
        }
    }
}

/// An optical drive on an AHCI port or an IDE channel, handed to its
/// thread.
struct Handoff {
    link: Link,
    name: Name,
    number: u32,
    source: Address,
    /// Where it is, for the log: `AHCI port 2 at 0000:00:1f.2`.
    place: TextBuf<64>,
    drive: DriveRef,
    empty: DeviceRef,
}

/// The drives handed to threads that have not taken them yet.
static HANDOFFS: SpinLock<[Option<Box<Handoff, Kmalloc>>; MAX_DRIVES]> =
    SpinLock::new([const { None }; MAX_DRIVES]);

/// Sets up the optical drive on `port`, the AHCI port `number` of the
/// controller at `source`, which raises interrupts if `interrupt`: names
/// it, registers it empty, and starts its thread. Returns the thread and
/// the port as its users share it, for the port's interrupt
/// ([`OpticalPort::interrupt`]). The thread is held until the caller has
/// noted the port, and released by [`release`]. Kept out of the frame of
/// the controller's scan.
#[inline(never)]
pub(super) fn add_ahci(
    port: Port,
    number: usize,
    source: Address,
    interrupt: bool,
) -> Result<(ThreadRef, Arc<OpticalPort, Kmalloc>), &'static str> {
    let identity = *port.packet_identity().ok_or("it is not an optical drive")?;
    let shared = OpticalPort::new(port, interrupt).ok_or("out of memory")?;
    let mut place = TextBuf::new();
    let _ = write!(place, "AHCI port {number} at {source}");
    let thread = add(Link::Ahci(shared.clone()), identity, place, source)?;
    shared.set_thread(thread.clone());
    Ok((thread, shared))
}

/// Sets up the optical drive `link` reaches on an IDE channel, `place`
/// (`the IDE secondary channel at 0000:00:01.1, master`), of the
/// controller at `source`, as [`add_ahci`] does one on an AHCI port.
#[inline(never)]
pub(super) fn add_ide(
    link: IdeLink,
    place: TextBuf<64>,
    source: Address,
) -> Result<ThreadRef, &'static str> {
    let identity = *link.identity();
    add(Link::Ide(link), identity, place, source)
}

/// Names the drive `link` reaches, registers it empty, and starts its
/// thread, held.
fn add(
    link: Link,
    identity: PacketIdentity,
    place: TextBuf<64>,
    source: Address,
) -> Result<ThreadRef, &'static str> {
    let (cd, name) = take_cd_name().ok_or("too many optical drives")?;
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let made = (|| {
        let empty: DeviceRef = Arc::try_new_in(Empty, heap).map_err(|_| "out of memory")?;
        let index = {
            let handoffs = HANDOFFS.lock_irq();
            handoffs.iter().position(Option::is_none).ok_or("too many optical drives")?
        };
        let options = Options::new().held().policy(Policy::Fair { nice: -5 });
        let thread = sched::spawn("optical", drive_main, index, options)
            .map_err(|_| "its thread could not be started")?;
        let drive =
            removable::add(&name, thread.clone(), Kind::Optical, &[]).ok_or("out of memory")?;
        let handoff = Handoff { link, name: name.clone(), number: cd, source, place, drive, empty };
        let handoff = Box::try_new_in(handoff, heap).map_err(|_| "out of memory")?;
        let ident = Ident {
            model: identity.model,
            serial: super::padded(&identity.serial),
            firmware: identity.firmware,
        };
        register(Entry {
            name: name.clone(),
            device: handoff.empty.clone(),
            place: None,
            ident: Some(ident),
        });
        HANDOFFS.lock_irq()[index] = Some(handoff);
        Ok(thread)
    })();
    if made.is_err() {
        give_back_cd_number(cd);
    }
    made
}

/// Lets the thread of a drive [`add_ahci`] or [`add_ide`] set up run.
pub(super) fn release(thread: &ThreadRef) {
    LOOKING.fetch_add(1, Ordering::AcqRel);
    sched::release(thread);
}

/// The thread of the drive handed over at `index`.
fn drive_main(index: usize) {
    let Some(handoff) = HANDOFFS.lock_irq()[index].take() else { return };
    let Some(mut drive) = OpticalDrive::new(handoff) else {
        LOOKING.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    drive.introduce();
    drive.serve();
}

/// An optical drive on an AHCI port or an IDE channel, as its thread
/// runs it.
struct OpticalDrive {
    link: Link,
    name: Name,
    /// Its `cd` number, given back when it goes.
    number: u32,
    place: TextBuf<64>,
    source: Address,
    drive: DriveRef,
    /// The device the drive is while empty.
    empty: DeviceRef,
    /// The disc of data in it, if one is.
    loaded: Option<Loaded>,
    /// What the drive reads.
    family: Family,
    /// When it is looked at next.
    next: u64,
    /// A read found that its disc was taken out or changed: the next look
    /// takes it out first.
    changed: bool,
    /// Its disc is locked in, and the drive takes the lock.
    locked: bool,
    lockable: bool,
    /// It has media events, of which its eject button's ([`ask_events`]).
    events: bool,
    /// The disc in it is left alone until it is taken out: ejected, of no
    /// data, or unreadable.
    left: bool,
    /// Its disc was taken out, `why`, and its reads fail, but it is still
    /// in `/devices` while the mount table is busy.
    out: Option<&'static str>,
    /// The first look is not done.
    first: bool,
    /// The buffers of the read it runs.
    frames: Vec<(u64, u32), Kmalloc>,
    /// The DMA memory the commands programs ask of it are run with, once
    /// one is (ADR 0447).
    memory: Option<Dma>,
}

/// Waits `us` microseconds, or until the port's interrupt wakes the
/// thread.
pub(super) fn wait_us(us: u32) {
    sched::block_until(sched::now_ns().saturating_add(u64::from(us) * 1_000), "optical");
}

/// Waits `ms` milliseconds, asleep: a disc spinning up.
fn wait_ms(ms: u32) {
    sched::sleep_ns(u64::from(ms) * 1_000_000);
}

impl OpticalDrive {
    /// The drive `handoff` names, as its thread runs it; `None` when
    /// memory runs out.
    fn new(handoff: Box<Handoff, Kmalloc>) -> Option<OpticalDrive> {
        let Handoff { link, name, number, source, place, drive, empty } = *handoff;
        let mut frames = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
        if frames.try_reserve_exact(link.max_segments()).is_err() {
            warn!("Disk: {name}: out of memory for its reads");
            return None;
        }
        Some(OpticalDrive {
            link,
            name,
            number,
            place,
            source,
            drive,
            empty,
            loaded: None,
            family: Family::Other,
            next: 0,
            changed: false,
            locked: false,
            lockable: true,
            events: true,
            left: false,
            out: None,
            first: true,
            frames,
            memory: None,
        })
    }

    /// Runs `f` with the drive's commands.
    fn packets<R>(&mut self, f: impl FnOnce(&mut dyn Transport) -> R) -> R {
        self.link.with(f)
    }

    /// Says what the drive is, in the log.
    fn introduce(&mut self) {
        self.family = self.packets(|t| mmc::drive(t, 0)).unwrap_or(Family::Other);
        let identity = self.link.identity();
        let model = identity.as_ref().map_or("", |id| trimmed(&id.model));
        let firmware = identity.as_ref().map_or("", |id| trimmed(&id.firmware));
        info!(
            "Disk: {} is {} on {}: {model} {firmware}",
            self.name,
            self.family.drive(),
            self.place
        );
    }

    /// Serves the disc's reads, and looks at the drive between them,
    /// until the drive stops answering.
    fn serve(&mut self) {
        loop {
            if self.link.dead() {
                self.stopped();
                return;
            }
            self.link.settle();
            let served = self.serve_one();
            let next = self.tend(sched::now_ns());
            if !served {
                let idle = sched::now_ns().saturating_add(IDLE_NS);
                sched::block_until(idle.min(next), "optical");
            }
        }
    }

    /// Serves the read waiting, if one is; says whether one was.
    fn serve_one(&mut self) -> bool {
        let Some(loaded) = &self.loaded else { return false };
        let Some((op, sector)) = loaded.take(&mut self.frames) else { return false };
        let (outcome, why) = self.read(op, sector);
        if let Some(loaded) = &self.loaded {
            loaded.finish(outcome, why);
        }
        true
    }

    /// Carries out `op` from `sector` with the buffers taken: a READ(10)
    /// of them all.
    fn read(&mut self, op: Op, sector: u64) -> (Outcome, Option<&'static str>) {
        let Some(loaded) = &self.loaded else { return (Outcome::Failed, Some(TAKEN_OUT)) };
        if op != Op::Read {
            return (Outcome::Failed, Some("a disc is only read"));
        }
        let block = u64::from(loaded.medium.capacity.block_length);
        let total: u64 = self.frames.iter().map(|&(_, len)| u64::from(len)).sum();
        let first = sector / (block / SECTOR);
        let Some(cdb) = u32::try_from(total / block).ok().and_then(|n| Cdb::read(first, n, false))
        else {
            return (Outcome::Failed, Some("a read past what a command reaches"));
        };
        let frames = &self.frames;
        let data = Data::Frames { into_host: true, frames };
        let reply = self.link.with(|t| t.command(0, &cdb, data, TIMEOUT_MS));
        let (outcome, why, then) = judge(reply);
        match then {
            Then::Nothing => {}
            Then::Ready => sched::sleep_ns(READY_NS),
            Then::Changed => self.changed = true,
        }
        (outcome, why)
    }

    /// Looks at the drive if it is due, ejects its disc if eject(1)
    /// asked, and locks the disc in while it is used. Returns when the
    /// drive is due next.
    fn tend(&mut self, now: u64) -> u64 {
        if !self.finish_out() {
            return now.saturating_add(BUSY_NS);
        }
        if self.link.disc_changed() {
            self.changed = true;
        }
        // A command a program asked of the drive (ADR 0447).
        let (drive, memory, source) = (&self.drive, &mut self.memory, self.source);
        self.link.with(|t| serve_asked(drive, t, 0, (memory, source)));
        let eject = self.drive.eject_asked();
        if !eject && !self.changed && now < self.next {
            return self.next;
        }
        if eject {
            self.eject();
        }
        self.look();
        self.lock();
        if core::mem::take(&mut self.first) {
            LOOKING.fetch_sub(1, Ordering::AcqRel);
        }
        self.next = sched::now_ns().saturating_add(LOOK_NS);
        self.next
    }

    /// Looks whether a disc is in the drive, as Linux's `sr` does: its
    /// media events first, of which its eject button's, then whether a
    /// disc is in it. One taken out, or changed, is taken out of the
    /// kernel, and one put in is put in.
    fn look(&mut self) {
        let (name, drive, mut events) = (&self.name, &self.drive, self.events);
        let said = self.link.with(|t| ask_events(t, 0, name, drive, &mut events));
        self.events = events;
        let Ok(found) = self.packets(|t| radix_scsi::look(t, 0)) else { return };
        let changed = core::mem::take(&mut self.changed) || found.changed || said;
        match found.medium {
            // The transport failed: the next look says.
            None => {}
            Some(false) => {
                self.left = false;
                self.emptied(TAKEN_OUT);
            }
            Some(true) => {
                if changed {
                    self.take_out(TAKEN_OUT);
                    self.left = false;
                }
                if self.loaded.is_none() && !self.left {
                    self.put_in();
                }
            }
        }
    }

    /// The disc in the drive, found by [`OpticalDrive::look`], is put in:
    /// one of data gets a queue, and takes the empty drive's place; one of
    /// none, or one that cannot be read, is left alone until it is taken
    /// out, and counted as a change, so that volumed(8) says what it is.
    fn put_in(&mut self) {
        let mut wait = wait_ms;
        let (disc, probed) = self.packets(|t| probe(t, 0, atapi_inquiry(), &mut wait));
        let _ = self.drive.set_disc(disc);
        let (medium, found) = match probed {
            Ok(probed) => probed,
            // Taken out again, or the drive gone: the next look says.
            Err(Absent::NoMedium | Absent::Gone) => return,
            Err(Absent::NotData(content)) => {
                info!("Disk: {}: {}", self.name, not_data(content));
                self.leave();
                self.drive.changed();
                return;
            }
            Err(why) => {
                info!("Disk: {}: a disc went in, but {why}; not used", self.name);
                self.leave();
                self.drive.changed();
                return;
            }
        };
        let (loaded, device) = match self.load(medium, &found) {
            Ok(made) => made,
            Err(why) => {
                warn!("Disk: {}: a disc went in, but {why}", self.name);
                let _ = self.drive.set_disc(Disc::Unreadable);
                self.leave();
                self.drive.changed();
                return;
            }
        };
        info!("Disk: {}: {} went in", self.name, describe(&found, &loaded.medium));
        // The drive was empty: nothing was mounted from it to lose.
        match super::replace(&self.name, device, &[], (&self.drive, false)) {
            super::Replaced::Done => self.loaded = Some(loaded),
            _ => loaded.gone(TAKEN_OUT),
        }
    }

    /// The queue of the disc of data `medium`, as `found` says it is, and
    /// its block device: on an AHCI port, its reads go straight to the
    /// port; on an IDE channel, through the thread.
    fn load(&self, medium: Medium, found: &Found) -> Result<(Loaded, DeviceRef), &'static str> {
        let info = info(&medium, self.link.max_segments());
        let session = last_session(found, &medium);
        let (reads, device) = match &self.link {
            Link::Ahci(port) => self.sent(port, medium.capacity.block_length, info, session)?,
            Link::Ide(_) => self.handed(info, session)?,
        };
        Ok((Loaded { medium, reads }, device))
    }

    /// The queue of a disc read in blocks of `block` bytes on the AHCI
    /// `port`, and its device: its reads go to the port. Kept out of the
    /// thread's frame, as the queue is built on the stack before it moves.
    #[inline(never)]
    fn sent(
        &self,
        port: &Arc<OpticalPort, Kmalloc>,
        block: u32,
        info: Info,
        session: u64,
    ) -> Result<(Reads, DeviceRef), &'static str> {
        let (lane, gone) = DiscReads::new(port, block, info.max_segments).ok_or("out of memory")?;
        let disk = queue_of(lane, info, port.has_interrupt(), &self.name, self.source)?;
        let device = device_of(&disk, info, session)?;
        port.set_disc(disk.clone());
        Ok((Reads::Sent { port: port.clone(), gone, disk }, device))
    }

    /// The queue of a disc on an IDE channel, and its device: its reads
    /// go to the drive's thread ([`super::handed`]).
    fn handed(&self, info: Info, session: u64) -> Result<(Reads, DeviceRef), &'static str> {
        let (queue, device) = handed::Medium::new(&self.name, self.source, info, session)?;
        Ok((Reads::Handed(queue), device))
    }

    /// Leaves the disc in the drive alone until it is taken out. One of
    /// no data, or that cannot be read, counts as a change, so that
    /// volumed(8) reads what the drive says is in it and says so.
    fn leave(&mut self) {
        self.left = true;
    }

    /// The drive has no disc in it now, or none to read, `why`: its disc
    /// is taken out of the kernel, and a disc of no data that was left
    /// alone counts a change as it goes, as one of data does.
    fn emptied(&mut self, why: &'static str) {
        let counted = self.loaded.is_some();
        let differs = self.drive.set_disc(Disc::None);
        self.take_out(why);
        if differs && !counted {
            self.drive.changed();
        }
    }

    /// Takes the drive's disc out of the kernel, if one is in: its reads
    /// fail `why` from now on, and the drive is empty, once the mount
    /// table lets what was mounted from it be lost.
    fn take_out(&mut self, why: &'static str) {
        let Some(loaded) = self.loaded.take() else { return };
        // First, so that an unmount that syncs it, with the mount table
        // held, fails at once and lets the table go.
        loaded.gone(why);
        self.locked = false;
        self.out = Some(why);
        self.finish_out();
    }

    /// Finishes taking out the disc [`OpticalDrive::take_out`] took out: what
    /// was mounted from it is lost, and the drive is empty in `/devices`.
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

    /// Keeps the tray shut while anything uses the disc, and lets it go
    /// when nothing does, as Linux's `sr` does while its disc is open. A
    /// drive that refuses the lock is not asked again.
    fn lock(&mut self) {
        let used = match self.loaded {
            None => false,
            // The mount table is busy: the next look says.
            Some(_) => match super::in_use_now(self.name.as_str()) {
                Some(used) => used,
                None => return,
            },
        };
        if !self.lockable || used == self.locked {
            return;
        }
        match self.packets(|t| radix_scsi::prevent_removal(t, 0, used)) {
            Ok(true) => self.locked = used,
            Ok(false) => {
                (self.lockable, self.locked) = (false, false);
                info!("Disk: {}: its drive does not keep its tray shut", self.name);
            }
            // The next look tries again.
            Err(_) => {}
        }
    }

    /// Ejects the drive's disc, as eject(1) asked through EJECT: lets the
    /// lock go, opens the tray, takes the disc out of the kernel, and
    /// answers the one who asked.
    fn eject(&mut self) {
        let ejected = match self.packets(|t| radix_scsi::eject(t, 0)) {
            Ok(()) => true,
            // An empty drive's tray opens all the same; one that refuses
            // lets the disc go.
            Err(Absent::Failed(_, sense)) => {
                let empty = sense.is_some_and(|s| matches!(s.meaning(), Meaning::NoMedium));
                if self.loaded.is_some() && !empty {
                    info!("Disk: {}: its drive does not eject; its disc is let go", self.name);
                }
                true
            }
            Err(_) => false,
        };
        if ejected {
            // A drive with no tray, or one that refused, still has it: it
            // is left alone until it is taken out, or the drive says it
            // changed.
            self.emptied(EJECTED);
            self.leave();
        }
        self.drive.answer(ejected);
    }

    /// The drive stopped answering: its disc goes, and the drive with it.
    fn stopped(&mut self) {
        warn!("Disk: {}: its drive stopped answering, and is gone", self.name);
        if let Some(loaded) = self.loaded.take() {
            loaded.gone(STOPPED);
        }
        if core::mem::take(&mut self.first) {
            LOOKING.fetch_sub(1, Ordering::AcqRel);
        }
        removable::remove(self.name.as_str());
        super::unregister(self.name.as_str());
        give_back_cd_number(self.number);
    }
}
