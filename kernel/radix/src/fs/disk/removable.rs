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

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::Mutex;

use super::Name;
use crate::sched::{self, ThreadRef, wait};

/// How long an eject may take before the one who asked gives up: the 30
/// seconds a START STOP UNIT is given, and the lock let go before it.
const EJECT_NS: u64 = 40_000_000_000;

/// The states of a drive's eject, in [`Drive::eject`]'s byte. No eject is
/// asked.
const IDLE: u8 = 0;
/// One is asked, and the driver's thread has not answered yet.
const ASKED: u8 = 1;
/// The thread ejected the medium, or let it go.
const EJECTED: u8 = 2;
/// The thread could not: the transport failed.
const FAILED: u8 = 3;

/// A drive whose medium comes and goes, as the rest of the kernel sees
/// it. Its driver's thread keeps the rest.
pub struct Drive {
    /// How many times a medium went in or came out since the drive was
    /// found: `/kernel/block/<drive>/media_changes`.
    changes: AtomicU64,
    /// Where an eject asked of the thread is: [`IDLE`], [`ASKED`], or the
    /// thread's answer.
    eject: AtomicU8,
    /// One eject at a time: a second waits for the first's answer.
    one: Mutex<()>,
    /// The driver's thread, which an eject wakes.
    thread: ThreadRef,
}

impl Drive {
    /// A drive whose driver's thread is `thread`.
    pub fn new(thread: ThreadRef) -> Drive {
        Drive {
            changes: AtomicU64::new(0),
            eject: AtomicU8::new(IDLE),
            one: Mutex::new(()),
            thread,
        }
    }

    /// How many times a medium went in or came out.
    pub fn changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    /// A medium went in, or came out: counted.
    pub fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
    }

    /// Ejects the drive's medium, as eject(1) asks: the driver's thread
    /// lets the lock go and ejects it, and the drive is empty after.
    /// Waits for the thread's answer, for up to [`EJECT_NS`]. Fails with
    /// [`radix_block::Error::Io`] if the transport failed, the drive was
    /// unplugged, or no answer came.
    pub fn eject(&self) -> Result<(), radix_block::Error> {
        let _one = self.one.lock();
        self.eject.store(ASKED, Ordering::Release);
        sched::wake(&self.thread);
        let deadline = sched::now_ns().saturating_add(EJECT_NS);
        wait::wait_until_or_timeout(self.key(), "eject", deadline, || {
            self.eject.load(Ordering::Acquire) != ASKED
        });
        // An answer that comes after this finds the eject no longer
        // asked, and is dropped.
        match self.eject.swap(IDLE, Ordering::AcqRel) {
            EJECTED => Ok(()),
            _ => Err(radix_block::Error::Io),
        }
    }

    /// Whether an eject waits for the driver's thread.
    pub fn eject_asked(&self) -> bool {
        self.eject.load(Ordering::Acquire) == ASKED
    }

    /// The driver's thread answers the eject asked: whether the medium
    /// was ejected. Wakes the one who asked.
    pub fn answer(&self, ejected: bool) {
        let to = if ejected { EJECTED } else { FAILED };
        if self.eject.compare_exchange(ASKED, to, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            wait::wake_all(self.key());
        }
    }

    /// What the one who asked for an eject waits on.
    fn key(&self) -> usize {
        core::ptr::from_ref(self) as usize
    }
}

/// A drive the kernel has.
pub type DriveRef = Arc<Drive, Kmalloc>;

/// Every drive whose medium comes and goes, by name.
static DRIVES: Mutex<Option<Vec<(Name, DriveRef), Kmalloc>>> = Mutex::new(None);

/// Adds the drive `name`, of the driver whose thread is `thread`;
/// `None` when memory runs out.
pub fn add(name: &Name, thread: ThreadRef) -> Option<DriveRef> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let drive = Arc::try_new_in(Drive::new(thread), heap).ok()?;
    let mut drives = DRIVES.lock();
    let drives = drives.get_or_insert_with(|| Vec::new_in(heap));
    drives.try_reserve(1).ok()?;
    let _ = drives.push_within_capacity((name.clone(), drive.clone()));
    Some(drive)
}

/// Takes the drive `name` away: it was unplugged. Answers an eject that
/// waits for it, so that whoever asked need not wait it out.
pub fn remove(name: &str) {
    let gone = {
        let mut drives = DRIVES.lock();
        let Some(drives) = drives.as_mut() else { return };
        let Some(at) = drives.iter().position(|(n, _)| n.as_str() == name) else { return };
        drives.remove(at)
    };
    gone.1.answer(false);
}

/// The drive `name`, if it is one whose medium comes and goes.
pub fn find(name: &str) -> Option<DriveRef> {
    let drives = DRIVES.lock();
    drives.as_ref()?.iter().find(|(n, _)| n.as_str() == name).map(|(_, drive)| drive.clone())
}
