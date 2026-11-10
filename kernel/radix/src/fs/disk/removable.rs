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
//! place before the request returns.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::mmc::{Content, Profile};
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
    /// One eject or look at a time: a second waits for the first's answer.
    one: Mutex<()>,
    /// The driver's thread, which an eject or a look wakes.
    thread: ThreadRef,
    kind: Kind,
    /// For an optical drive, the disc in it; `None` for another drive.
    disc: SpinLock<Option<Disc>>,
}

impl Drive {
    /// A drive of `kind` whose driver's thread is `thread`; an optical
    /// drive is empty until it says otherwise.
    pub fn new(thread: ThreadRef, kind: Kind) -> Drive {
        Drive {
            changes: AtomicU64::new(0),
            eject_requests: AtomicU64::new(0),
            eject: AtomicU8::new(IDLE),
            look: AtomicU8::new(IDLE),
            one: Mutex::new(()),
            thread,
            kind,
            disc: SpinLock::new((kind == Kind::Optical).then_some(Disc::None)),
        }
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

    /// Asks the driver's thread for what `state` stands for, and waits
    /// for its answer as `what`, for up to `limit_ns`.
    fn ask(
        &self,
        state: &AtomicU8,
        limit_ns: u64,
        what: &'static str,
    ) -> Result<(), radix_block::Error> {
        let _one = self.one.lock();
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

    /// What the one who asked for an eject or a look waits on.
    fn key(&self) -> usize {
        core::ptr::from_ref(self) as usize
    }
}

/// A drive the kernel has.
pub type DriveRef = Arc<Drive, Kmalloc>;

/// Every drive whose medium comes and goes, by name.
static DRIVES: Mutex<Option<Vec<(Name, DriveRef), Kmalloc>>> = Mutex::new(None);

/// Adds the drive `name`, of `kind`, of the driver whose thread is
/// `thread`; `None` when memory runs out.
pub fn add(name: &Name, thread: ThreadRef, kind: Kind) -> Option<DriveRef> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let drive = Arc::try_new_in(Drive::new(thread, kind), heap).ok()?;
    let mut drives = DRIVES.lock();
    let drives = drives.get_or_insert_with(|| Vec::new_in(heap));
    drives.try_reserve(1).ok()?;
    let _ = drives.push_within_capacity((name.clone(), drive.clone()));
    Some(drive)
}

/// Takes the drive `name` away: it was unplugged. Answers an eject or a
/// look that waits for it, so that whoever asked need not wait it out.
pub fn remove(name: &str) {
    let gone = {
        let mut drives = DRIVES.lock();
        let Some(drives) = drives.as_mut() else { return };
        let Some(at) = drives.iter().position(|(n, _)| n.as_str() == name) else { return };
        drives.remove(at)
    };
    gone.1.answer(false);
    gone.1.looked(false);
}

/// The drive `name`, if it is one whose medium comes and goes.
pub fn find(name: &str) -> Option<DriveRef> {
    let drives = DRIVES.lock();
    drives.as_ref()?.iter().find(|(n, _)| n.as_str() == name).map(|(_, drive)| drive.clone())
}
