// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Drives whose medium comes and goes (ADR 0430): a card reader's slots,
//! and a stick or USB disk that says its medium is removable.
//!
//! Such a drive keeps its name while it has no medium in it. Its device
//! is then a [`radix_block::Empty`], which has no sectors, and the device
//! of each medium put in takes that one's place under the same name
//! ([`super::replace`]). The drive's driver looks at it every two
//! seconds, as Linux's `sd` looks at a removable disk, and counts each
//! medium that goes in or comes out ([`Drive::changed`]). volumed(8)
//! reads the count in `/kernel/block/<drive>/media_changes`, and mounts
//! or unmounts when it moves. eject(1) asks for the medium to be ejected
//! through radix-block(2)'s EJECT, which [`Drive::eject`] hands to the
//! driver's thread, since that thread is the one that talks to the drive.
//!
//! An optical drive keeps its disc in while it is locked, and asks for it
//! to be let go when its eject button is pressed: its driver counts each
//! request ([`Drive::eject_requested`]), and volumed unmounts the disc
//! and ejects it, as eject(1) does (ADR 0432).
//!
//! A floppy drive cannot say that a disk went in: its disk change line
//! comes down only when its heads step with a disk in, which is heard, so
//! it is not looked at every two seconds but when asked (ADR 0435):
//! radix-block(2)'s REREAD_PARTITIONS on the drive has its driver's
//! thread look ([`Drive::look`]), and a disk found takes the drive's
//! place before the request returns. A USB floppy drive finds a disk by
//! the same change line, and is looked at the same way.
//!
//! A floppy drive formats its disk a track at a time, as radix-block(2)'s
//! FORMAT_TRACK asks (ADR 0436): [`Drive::format`] hands each track to the
//! driver's thread, and waits for it. The drive's formats, the sizes of
//! disk it writes, are in `/kernel/block/<drive>/formats`.
//!
//! An optical drive reads a disc's table of contents and a CD's audio
//! when radix-block(2)'s READ_TOC and READ_AUDIO ask (ADR 0447):
//! [`Drive::command`] hands the command the kernel made for the request,
//! with the buffer for its answer, to the driver's thread, which runs it
//! between its looks and the disc's reads.
//!
//! burn(1) writes a disc with the MMC commands radix-block(2)'s
//! MMC_COMMAND passes it (ADR 0449), run the same way. While it writes,
//! the drive is held ([`Drive::hold`]): its thread does not look at it,
//! as a look in the middle of a burn reads a disc that is not whole, and
//! keeps its tray shut. The hold ends when burn(1) asks for the disc to
//! be looked at afresh ([`Drive::release`]), or [`HOLD_NS`] after the
//! last command that wrote, if it never does.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::mmc::{Content, Profile};
use radix_scsi::{Cdb, Sense};
use radix_sync::{Mutex, SpinLock};

use super::Name;
use crate::sched::{self, ThreadRef, wait};

/// How long an eject may take before the one who asked gives up: the 30
/// seconds a START STOP UNIT is given, and the lock let go before it.
const EJECT_NS: u64 = 40_000_000_000;
/// How long a look may take before the one who asked gives up: a floppy
/// drive's motor coming up to speed, its heads stepping, a try at each
/// data rate, and a controller reset on the way.
const LOOK_NS: u64 = 20_000_000_000;
/// How long a track's format may take before the one who asked gives up:
/// the motor, a seek, the format's two turns and a try again, or a USB
/// drive's 30 seconds for the command.
const FORMAT_NS: u64 = 40_000_000_000;
/// How long a command asked of an optical drive may take: a disc spinning
/// up, the 30 seconds a read is given, and its try again.
const COMMAND_NS: u64 = 70_000_000_000;
/// How long after a program's last command that wrote a disc its drive
/// stays held (ADR 0449): longer than burn(1) waits between two writes
/// while it reads the files it burns, or between two looks at the
/// progress of a blank.
pub const HOLD_NS: u64 = 30_000_000_000;

/// The states of what is asked of a drive's thread, an eject or a look,
/// in [`Drive::eject`]'s byte and [`Drive::look`]'s. Nothing is asked.
const IDLE: u8 = 0;
/// It is asked, and the driver's thread has not answered yet.
const ASKED: u8 = 1;
/// The thread did it: ejected the medium, or let it go; or looked.
const DONE: u8 = 2;
/// The thread could not: the transport failed.
const FAILED: u8 = 3;

/// How a drive finds out that its medium went in or came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A card reader's slot, or a stick or USB disk whose medium is
    /// removable: its driver looks every two seconds.
    Polled,
    /// A CD, DVD or Blu-ray drive: looked at every two seconds too, and
    /// it says what disc is in it.
    Optical,
    /// A floppy drive: looked at only when asked ([`Drive::look`]).
    Floppy,
}

/// A size of floppy disk a drive formats (ADR 0436): its name, as
/// `/kernel/block/<drive>/formats` and floppy(1) give it, and its sectors
/// of 512 bytes, which radix-block(2)'s FORMAT_TRACK names it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// `1.44M`, `720K`.
    pub name: &'static str,
    /// Its sectors.
    pub sectors: u32,
}

/// A 3.5-inch high-density disk.
pub const SIZE_1440K: Size = Size { name: "1.44M", sectors: 2880 };
/// A 3.5-inch double-density disk.
pub const SIZE_720K: Size = Size { name: "720K", sectors: 1440 };

/// A track of a floppy disk to format, as FORMAT_TRACK asks: the disk's
/// size, in sectors, and the track's cylinder and head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Track {
    /// The size of disk the track is formatted for ([`Size::sectors`]).
    pub sectors: u32,
    /// Its cylinder, from 0.
    pub cylinder: u32,
    /// Its head, 0 or 1.
    pub head: u32,
}

/// Why a track could not be formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The drive has no disk in it.
    NoDisk,
    /// The disk is write-protected.
    WriteProtected,
    /// The drive does not format disks of that size.
    Unsupported,
    /// The track is not on a disk of that size.
    OutOfRange,
    /// The drive failed, or was unplugged, or did not answer.
    Failed,
}

/// A command a program's request has an optical drive's thread run: READ
/// TOC or READ CD, which the kernel makes from the request (ADR 0447), or
/// one of the recording commands MMC_COMMAND passes (ADR 0449); and the
/// buffer for its data, of the length asked for, which comes from the
/// drive into it or, for a command `to_drive`, goes to the drive from it.
pub struct Asked {
    pub cdb: Cdb,
    pub data: Vec<u8, Kmalloc>,
    /// Whether `data` goes to the drive.
    pub to_drive: bool,
    /// How long the drive has for it.
    pub timeout_ms: u32,
    /// Whether the drive's sense goes back as it is, in
    /// [`CommandError::Checked`] (MMC_COMMAND's): a command that meets a
    /// change of disc is not run again then, as a disc changed in the
    /// middle of a burn is not the one being burned.
    pub raw: bool,
}

impl Asked {
    /// A command the kernel makes, whose answer comes into `data`, given
    /// the 30 seconds a read is.
    pub fn read(cdb: Cdb, data: Vec<u8, Kmalloc>) -> Asked {
        Asked { cdb, data, to_drive: false, timeout_ms: 30_000, raw: false }
    }
}

/// Why a command asked of an optical drive failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The drive has no disc in it.
    NoDisc,
    /// The drive refused it: a block past the disc's end, a sector of
    /// another kind than asked (data read as audio), or a field it does
    /// not take, such as a format of TOC it does not have.
    Refused,
    /// The drive failed, was unplugged, or did not answer.
    Failed,
    /// The drive ended a raw command with this sense ([`Asked::raw`]).
    Checked(Sense),
}

/// What is in an optical drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disc {
    /// Nothing.
    None,
    /// A disc of this kind, holding this: data, which is read, or music
    /// or nothing (a blank disc), which is not.
    In(Profile, Content),
    /// A disc the drive could not read.
    Unreadable,
}

impl fmt::Display for Disc {
    /// As `/kernel/block/<drive>/disc` has it: `none`, `dvd-rom data`,
    /// `cd-rom audio`, `dvd+r blank`, `unreadable`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Disc::None => f.write_str("none"),
            Disc::In(profile, content) => write!(f, "{profile} {}", content.name()),
            Disc::Unreadable => f.write_str("unreadable"),
        }
    }
}

/// A drive whose medium comes and goes, as the rest of the kernel sees
/// it. Its driver's thread keeps the rest.
pub struct Drive {
    /// How many times a medium went in or came out since the drive was
    /// found: `/kernel/block/<drive>/media_changes`.
    changes: AtomicU64,
    /// How many times an optical drive's eject button was pressed while
    /// its disc was locked in: `/kernel/block/<drive>/eject_requests`.
    eject_requests: AtomicU64,
    /// Where an eject asked of the thread is: [`IDLE`], [`ASKED`], or the
    /// thread's answer.
    eject: AtomicU8,
    /// Where a look asked of a floppy drive's thread is, as `eject`.
    look: AtomicU8,
    /// Where a track's format asked of a floppy drive's thread is, as
    /// `eject`; the track, and why it failed.
    format: AtomicU8,
    track: SpinLock<Option<Track>>,
    format_failed: SpinLock<Option<FormatError>>,
    /// Where a command asked of an optical drive's thread is, as `eject`;
    /// the command until the thread takes it, and its answer. Each command
    /// has a number of its own, so that the answer to one whose asker gave
    /// up is not taken for the next's.
    command: AtomicU8,
    commands: AtomicU64,
    asked: SpinLock<Option<(u64, Asked)>>,
    answer: SpinLock<Option<Result<Vec<u8, Kmalloc>, CommandError>>>,
    /// For a floppy drive, the sizes of disk it formats; empty for
    /// another drive.
    formats: &'static [Size],
    /// One eject, look, format or command at a time: a second waits for
    /// the first's answer.
    one: Mutex<()>,
    /// The driver's thread, which an eject, a look or a format wakes.
    thread: ThreadRef,
    kind: Kind,
    /// For an optical drive, the disc in it; `None` for another drive.
    disc: SpinLock<Option<Disc>>,
    /// While a program writes the drive's disc (ADR 0449), when the hold
    /// ends if nothing else ends it first; [`RELEASED`] once it is ended,
    /// until the thread looks; [`NOT_HELD`] otherwise.
    hold: AtomicU64,
}

/// [`Drive::hold`]'s value of a drive not held.
const NOT_HELD: u64 = 0;
/// Its value once a hold ended, until the thread takes it.
const RELEASED: u64 = 1;

impl Drive {
    /// A drive of `kind` whose driver's thread is `thread`, which formats
    /// disks of `formats` if it is a floppy drive; an optical drive is
    /// empty until it says otherwise.
    pub fn new(thread: ThreadRef, kind: Kind, formats: &'static [Size]) -> Drive {
        Drive {
            changes: AtomicU64::new(0),
            eject_requests: AtomicU64::new(0),
            eject: AtomicU8::new(IDLE),
            look: AtomicU8::new(IDLE),
            format: AtomicU8::new(IDLE),
            track: SpinLock::new(None),
            format_failed: SpinLock::new(None),
            command: AtomicU8::new(IDLE),
            commands: AtomicU64::new(0),
            asked: SpinLock::new(None),
            answer: SpinLock::new(None),
            formats,
            one: Mutex::new(()),
            thread,
            kind,
            disc: SpinLock::new((kind == Kind::Optical).then_some(Disc::None)),
            hold: AtomicU64::new(NOT_HELD),
        }
    }

    /// Holds an optical drive whose disc a program writes (ADR 0449),
    /// from `now` for [`HOLD_NS`]: its thread does not look at it, and
    /// keeps its tray shut.
    pub fn hold(&self, now: u64) {
        self.hold.store(now.saturating_add(HOLD_NS).max(RELEASED + 1), Ordering::Release);
    }

    /// Ends a hold, if the drive is held: its thread looks at the disc
    /// afresh, as one written since it last looked.
    pub fn release(&self) {
        let _ = self.hold.try_update(Ordering::AcqRel, Ordering::Acquire, |until| {
            (until != NOT_HELD).then_some(RELEASED)
        });
    }

    /// Whether the drive is held at `now`.
    pub fn is_held(&self, now: u64) -> bool {
        now < self.hold.load(Ordering::Acquire)
    }

    /// For the driver's thread at `now`: whether the drive is held, and
    /// whether a hold ended since it last asked, which it takes, to look
    /// at the disc afresh.
    pub fn held(&self, now: u64) -> (bool, bool) {
        let until = self.hold.load(Ordering::Acquire);
        if until == NOT_HELD {
            return (false, false);
        }
        if now < until {
            return (true, false);
        }
        // Ended, by release() or by time: once, unless held again since.
        let ended = self
            .hold
            .compare_exchange(until, NOT_HELD, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        (false, ended)
    }

    /// How it finds out that its medium went in or came out.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// For an optical drive, the disc in it.
    pub fn disc(&self) -> Option<Disc> {
        *self.disc.lock_irq()
    }

    /// An optical drive's driver says what disc is in it now; returns
    /// whether that is not what it said before.
    pub fn set_disc(&self, disc: Disc) -> bool {
        let mut now = self.disc.lock_irq();
        let differs = now.is_some_and(|was| was != disc);
        if now.is_some() {
            *now = Some(disc);
        }
        differs
    }

    /// How many times a medium went in or came out.
    pub fn changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    /// A medium went in, or came out: counted.
    pub fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
    }

    /// How many times its eject button was pressed while its disc was
    /// locked in.
    pub fn eject_requests(&self) -> u64 {
        self.eject_requests.load(Ordering::Acquire)
    }

    /// Its eject button was pressed while its disc was locked in: counted,
    /// for volumed(8) to unmount the disc and eject it (ADR 0432).
    pub fn eject_requested(&self) {
        self.eject_requests.fetch_add(1, Ordering::AcqRel);
    }

    /// Ejects the drive's medium, as eject(1) asks: the driver's thread
    /// lets the lock go and ejects it, and the drive is empty after.
    /// Waits for the thread's answer, for up to [`EJECT_NS`]. Fails with
    /// [`radix_block::Error::Io`] if the transport failed, the drive was
    /// unplugged, or no answer came.
    pub fn eject(&self) -> Result<(), radix_block::Error> {
        self.ask(&self.eject, EJECT_NS, "eject")
    }

    /// Whether an eject waits for the driver's thread.
    pub fn eject_asked(&self) -> bool {
        self.eject.load(Ordering::Acquire) == ASKED
    }

    /// The driver's thread answers the eject asked: whether the medium
    /// was ejected. Wakes the one who asked.
    pub fn answer(&self, ejected: bool) {
        self.reply(&self.eject, ejected);
    }

    /// Has a floppy drive's thread look whether a disk is in it (ADR
    /// 0435): one found takes the drive's place, and one taken out
    /// leaves it, before this returns. Waits for the thread's answer, for
    /// up to [`LOOK_NS`]. Fails with [`radix_block::Error::Io`] if the
    /// controller failed, the drive went, or no answer came.
    pub fn look(&self) -> Result<(), radix_block::Error> {
        self.ask(&self.look, LOOK_NS, "look")
    }

    /// Whether a look waits for the driver's thread.
    pub fn look_asked(&self) -> bool {
        self.look.load(Ordering::Acquire) == ASKED
    }

    /// The driver's thread answers the look asked: whether it could look.
    /// Wakes the one who asked.
    pub fn looked(&self, worked: bool) {
        self.reply(&self.look, worked);
    }

    /// The sizes of disk a floppy drive formats.
    pub fn formats(&self) -> &'static [Size] {
        self.formats
    }

    /// Has a floppy drive's thread format `track` of the disk in it (ADR
    /// 0436), and waits for its answer, for up to [`FORMAT_NS`]. What the
    /// disk held is gone, and the disk leaves the kernel: the drive is
    /// empty until a look finds the disk formatted.
    pub fn format(&self, track: Track) -> Result<(), FormatError> {
        if !self.formats.iter().any(|size| size.sectors == track.sectors) {
            return Err(FormatError::Unsupported);
        }
        let _one = self.one.lock();
        *self.track.lock_irq() = Some(track);
        *self.format_failed.lock_irq() = None;
        match self.ask_held(&self.format, FORMAT_NS, "format") {
            Ok(()) => Ok(()),
            Err(_) => Err(self.format_failed.lock_irq().take().unwrap_or(FormatError::Failed)),
        }
    }

    /// The track whose format waits for the driver's thread, if one does.
    pub fn format_asked(&self) -> Option<Track> {
        if self.format.load(Ordering::Acquire) != ASKED {
            return None;
        }
        *self.track.lock_irq()
    }

    /// The driver's thread answers the format asked: how it went. Wakes
    /// the one who asked.
    pub fn formatted(&self, result: Result<(), FormatError>) {
        if let Err(why) = result {
            *self.format_failed.lock_irq() = Some(why);
        }
        self.reply(&self.format, result.is_ok());
    }

    /// Has an optical drive's thread run `asked` (ADR 0447), and waits for
    /// its answer, for up to [`COMMAND_NS`], or the command's own time and
    /// 40 seconds more for one given longer: the buffer asked with, cut to
    /// the bytes that moved.
    pub fn command(&self, asked: Asked) -> Result<Vec<u8, Kmalloc>, CommandError> {
        if self.kind != Kind::Optical {
            return Err(CommandError::Refused);
        }
        let limit = COMMAND_NS.max(u64::from(asked.timeout_ms) * 1_000_000 + 40_000_000_000);
        let _one = self.one.lock();
        let number = self.commands.fetch_add(1, Ordering::AcqRel) + 1;
        *self.answer.lock_irq() = None;
        *self.asked.lock_irq() = Some((number, asked));
        let done = self.ask_held(&self.command, limit, "command");
        // One the thread never took is dropped here.
        drop(self.asked.lock_irq().take());
        let answer = self.answer.lock_irq().take();
        match (done, answer) {
            (Ok(()), Some(Ok(data))) => Ok(data),
            (_, Some(Err(why))) => Err(why),
            _ => Err(CommandError::Failed),
        }
    }

    /// The command that waits for the driver's thread, if one does, and
    /// its number: taken, for the thread to run and answer with
    /// [`Drive::commanded`].
    pub fn command_asked(&self) -> Option<(u64, Asked)> {
        if self.command.load(Ordering::Acquire) != ASKED {
            return None;
        }
        self.asked.lock_irq().take()
    }

    /// The driver's thread answers the command `number` it took: the
    /// buffer with the drive's answer, or why it failed. Wakes the one who
    /// asked, if that one still waits for it.
    pub fn commanded(&self, number: u64, result: Result<Vec<u8, Kmalloc>, CommandError>) {
        if self.commands.load(Ordering::Acquire) != number {
            return;
        }
        let done = result.is_ok();
        *self.answer.lock_irq() = Some(result);
        self.reply(&self.command, done);
    }

    /// Asks the driver's thread for what `state` stands for, and waits
    /// for its answer as `what`, for up to `limit_ns`.
    fn ask(
        &self,
        state: &AtomicU8,
        limit_ns: u64,
        what: &'static str,
    ) -> Result<(), radix_block::Error> {
        let _one = self.one.lock();
        self.ask_held(state, limit_ns, what)
    }

    /// [`Drive::ask`], with `one` held already.
    fn ask_held(
        &self,
        state: &AtomicU8,
        limit_ns: u64,
        what: &'static str,
    ) -> Result<(), radix_block::Error> {
        state.store(ASKED, Ordering::Release);
        sched::wake(&self.thread);
        let deadline = sched::now_ns().saturating_add(limit_ns);
        wait::wait_until_or_timeout(self.key(), what, deadline, || {
            state.load(Ordering::Acquire) != ASKED
        });
        // An answer that comes after this finds nothing asked, and is
        // dropped.
        match state.swap(IDLE, Ordering::AcqRel) {
            DONE => Ok(()),
            _ => Err(radix_block::Error::Io),
        }
    }

    /// Answers what `state` asked: whether it was done. Wakes the one who
    /// asked.
    fn reply(&self, state: &AtomicU8, done: bool) {
        let to = if done { DONE } else { FAILED };
        if state.compare_exchange(ASKED, to, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            wait::wake_all(self.key());
        }
    }

    /// What the one who asked for an eject, a look or a format waits on.
    fn key(&self) -> usize {
        core::ptr::from_ref(self) as usize
    }
}

/// A drive the kernel has.
pub type DriveRef = Arc<Drive, Kmalloc>;

/// Every drive whose medium comes and goes, by name.
static DRIVES: Mutex<Option<Vec<(Name, DriveRef), Kmalloc>>> = Mutex::new(None);

/// Adds the drive `name`, of `kind`, of the driver whose thread is
/// `thread`, which formats disks of `formats` if it is a floppy drive;
/// `None` when memory runs out.
pub fn add(
    name: &Name,
    thread: ThreadRef,
    kind: Kind,
    formats: &'static [Size],
) -> Option<DriveRef> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let drive = Arc::try_new_in(Drive::new(thread, kind, formats), heap).ok()?;
    let mut drives = DRIVES.lock();
    let drives = drives.get_or_insert_with(|| Vec::new_in(heap));
    drives.try_reserve(1).ok()?;
    let _ = drives.push_within_capacity((name.clone(), drive.clone()));
    Some(drive)
}

/// Takes the drive `name` away: it was unplugged. Answers an eject, a
/// look, a format or a command that waits for it, so that whoever asked need not
/// wait it out.
pub fn remove(name: &str) {
    let gone = {
        let mut drives = DRIVES.lock();
        let Some(drives) = drives.as_mut() else { return };
        let Some(at) = drives.iter().position(|(n, _)| n.as_str() == name) else { return };
        drives.remove(at)
    };
    gone.1.answer(false);
    gone.1.looked(false);
    gone.1.formatted(Err(FormatError::Failed));
    let number = gone.1.commands.load(Ordering::Acquire);
    gone.1.commanded(number, Err(CommandError::Failed));
}

/// The drive `name`, if it is one whose medium comes and goes.
pub fn find(name: &str) -> Option<DriveRef> {
    let drives = DRIVES.lock();
    drives.as_ref()?.iter().find(|(n, _)| n.as_str() == name).map(|(_, drive)| drive.clone())
}
