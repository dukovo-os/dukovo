// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB disks: the logical units behind a USB stick's, a card reader's or
//! a USB disk's mass storage interface (Phase 11b, ADRs 0161 and 0162).
//!
//! [`run`] is a stick's thread (`crate::usb::storage`), which carries the
//! interface's commands over the Bulk-Only Transport
//! (`radix_usb::storage::Bot`) or USB Attached SCSI
//! (`radix_usb::storage::uas`). It finds out how many logical units the
//! interface has (Get Max LUN over Bulk-Only, REPORT LUNS over UAS, as
//! Linux's SCSI scan asks) and what each is (`radix_scsi::probe`). Each
//! that is a disk with a medium in it takes the first `sd` letter free
//! once the PCI scan has named the SATA disks (and, at boot, the sticks
//! found before it theirs, so that the names are the same each boot), and
//! appears in `/devices` and `/kernel/block` with its partitions, whose
//! table the thread reads itself, since it is the one that serves the
//! disk.
//!
//! Then it serves the disks' requests, which come through the shared
//! queue ([`super::queue`]): one at a time over Bulk-Only, as the
//! transport takes them; over UAS as many at once as the device has tags
//! for, shared among its disks, each request's commands under a tag of
//! its own. The queue's part of a disk ([`UsbLun`]) hands its requests to
//! the thread, which sends each as READ(10) or WRITE(10) (READ(16) and
//! WRITE(16) on a disk with more blocks than 32 bits count), or as
//! SYNCHRONIZE CACHE for a flush if the disk says it has a write cache,
//! and hands back how it ended. As Linux's `sd` does for USB disks, the
//! last eight blocks go one a command (card readers fail to read the last
//! block with others); a command that a UNIT ATTENTION, an abort, a
//! hardware error or a broken transport ends goes again (the queue's
//! tries); one that finds the disk stopped starts it first, and one that
//! finds it becoming ready waits a second. A request the queue waits 30 s
//! for has its transfer stopped, which resets the Bulk-Only interface, or
//! has the UAS device reset; either way it goes again.
//!
//! A logical unit whose medium comes and goes (ADR 0430), a card
//! reader's slot or a stick that says its medium is removable, is a drive
//! ([`super::removable`]): it takes its letter whether or not a medium is
//! in it, and an empty one is a device of no sectors. The thread looks at
//! each drive every two seconds when it has nothing else to send, with
//! TEST UNIT READY, as Linux's `sd` does: a medium put in gets a queue of
//! its own and takes the empty device's place, with its partitions, and
//! one taken out goes as an unplugged stick's disk does, and leaves the
//! drive empty. While anything uses the medium, PREVENT ALLOW MEDIUM
//! REMOVAL locks it in, where the drive can; eject(1) has the thread let
//! it go and eject it.
//!
//! A USB floppy drive (UFI, ADR 0436), over the Control/Bulk/Interrupt
//! transport (`radix_usb::storage::Cbi`) or Bulk-Only, is a drive named
//! `fd` as a PC's floppy drive is, the first number free once the PC's
//! are named. It is looked at only when asked, as a PC floppy drive is
//! (ADR 0435): radix-block(2)'s REREAD_PARTITIONS has the thread look,
//! and a request that finds the disk changed has it look too. Its disk
//! is read whole, with no partition table, and the thread formats it a
//! track at a time, as FORMAT_TRACK asks (`radix_scsi::ufi`).
//!
//! A logical unit that is a CD, DVD or Blu-ray drive (ADR 0431) is a
//! drive too, named `cd0` and on ([`super::optical`]): the thread asks
//! the disc in it what it is with the commands of MMC, reads a disc of
//! data whole, read-only, from the start of its last session, and reads
//! no partition table on it, as Linux's `sr` reads none. A blank disc or
//! a music CD leaves the drive empty, and the drive says what is in it.
//!
//! When the stick is unplugged, every request in flight and after fails,
//! the disks and their partitions leave `/devices` and `/kernel/block`,
//! their letters are free again, and the thread ends.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use radix_block::mem::KVec;
use radix_block::{DeviceRef, Empty, Error, Info, SECTOR, partition};
use radix_hw::Dma;
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::{
    Absent, Cdb, Data, Found, Inquiry, Meaning, Reply, Transport, TransportError, identify, key,
    look, medium, prevent_removal, reported_luns, ufi,
};
use radix_sync::SpinLock;
use radix_usb::storage::cbi::Cbi;
use radix_usb::storage::uas::{MAX_TAGS, OneAtATime, STATUS_LENGTH, Uas};
use radix_usb::storage::{Bot, MAX_LUNS};

use super::queue::{self, Controller, Disk, Op, Outcome, Part, Request, SLOTS, Volume};
use super::removable::{self, Disc, DriveRef, FormatError, Kind, SIZE_720K, SIZE_1440K, Track};
use super::{Entry, Ident, Name, Size, floppy, optical, register};
use crate::sched::{self, ThreadRef};
use crate::usb::storage::{Link, LinkRef, Protocol, StickPipes, UasPipes};

/// The most buffers a request to a USB 2 disk has over Bulk-Only: 120
/// KiB in pages, the most Linux's usb-storage sends, since some sticks
/// fail more.
const USB2_SEGMENTS: usize = 30;
/// The most a request to a SuperSpeed disk over Bulk-Only has: 512 KiB in
/// pages, which one transfer of the controller holds.
const SUPER_SEGMENTS: usize = radix_xhci::MAX_TD_BUFFERS;
/// The most a request over UAS has: what one transfer on a stream holds,
/// at any speed.
const UAS_SEGMENTS: usize = radix_xhci::MAX_STREAM_TD_BUFFERS;
/// How long a read, write or flush may take: Linux's `sd`'s.
const TIMEOUT_MS: u32 = 30_000;
/// The blocks at a disk's end that go one a command (Linux's
/// SD_LAST_BUGGY_SECTORS).
const LAST_BLOCKS: u64 = 8;
/// How long the thread sleeps with nothing to do, at most.
const IDLE_NS: u64 = 1_000_000_000;
/// How long a disk that says it is becoming ready is given before its
/// request goes again.
const READY_NS: u64 = 1_000_000_000;
/// How many times the partition table's reads go.
const TRIES: u32 = 3;
/// How often a drive is looked at for a medium put in or taken out:
/// udisks' two seconds, as Linux polls a removable disk for it.
const LOOK_NS: u64 = 2_000_000_000;
/// How soon a medium taken out is tried again while the mount table is
/// busy ([`super::replace`]).
const BUSY_NS: u64 = 100_000_000;
/// How much of a REPORT LUNS answer is read: its header and 63 units.
const REPORT_LENGTH: usize = 512;
/// The sizes of disk a USB floppy drive formats: a 3.5-inch drive's.
const FLOPPY_SIZES: &[removable::Size] = &[SIZE_1440K, SIZE_720K];
/// A 3.5-inch floppy disk's cylinders and heads, of either size.
const FLOPPY_TRACKS: (u32, u32) = (80, 2);

/// Where a request is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// None: room for one.
    Idle,
    /// Handed to the thread, as the queue's slot.
    Waiting(u8),
    /// The thread runs it.
    Running(u8),
    /// It ended so.
    Done(u8, Outcome),
}

/// A request, between the queue and the stick's thread.
struct Job {
    stage: Stage,
    op: Op,
    /// Its first sector, of 512 bytes.
    sector: u64,
    /// Its buffers (physical address, length).
    frames: Vec<(u64, u32), Kmalloc>,
}

/// A logical unit's requests, as many as it may have in flight.
struct Jobs {
    jobs: Vec<Job, Kmalloc>,
    /// Why the last request in each of the queue's slots failed or went
    /// again, for the log.
    why: Vec<Option<&'static str>, Kmalloc>,
    /// The stick is gone: requests fail at once.
    gone: bool,
}

/// What a logical unit's disk shares with its stick's thread.
struct Shared {
    jobs: SpinLock<Jobs>,
    /// The stick's thread, which a request wakes.
    thread: ThreadRef,
    /// The stick's link, which an abort goes through.
    link: LinkRef,
}

/// A logical unit as the queue drives it.
pub struct UsbLun {
    shared: Arc<Shared, Kmalloc>,
}

impl Controller for UsbLun {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        // A gone disk takes every request, to fail it.
        let jobs = self.shared.jobs.lock_irq();
        jobs.gone || jobs.jobs.iter().any(|job| job.stage == Stage::Idle)
    }

    fn submit(&mut self, slot: u8, request: &Request<'_>, _more: bool) -> Result<(), Error> {
        {
            let mut jobs = self.shared.jobs.lock_irq();
            if jobs.gone {
                return Err(Error::Io);
            }
            let Some(job) = jobs.jobs.iter_mut().find(|job| job.stage == Stage::Idle) else {
                return Err(Error::OutOfRange);
            };
            if request.data.len() > job.frames.capacity() {
                return Err(Error::OutOfRange);
            }
            job.frames.clear();
            for &frame in request.data {
                let _ = job.frames.push_within_capacity(frame);
            }
            job.op = request.op;
            job.sector = request.sector;
            job.stage = Stage::Waiting(slot);
        }
        sched::wake(&self.shared.thread);
        Ok(())
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        let mut jobs = self.shared.jobs.lock_irq();
        let job = jobs.jobs.iter_mut().find(|job| matches!(job.stage, Stage::Done(..)))?;
        let Stage::Done(slot, outcome) = job.stage else { return None };
        job.stage = Stage::Idle;
        Some((slot, outcome))
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        self.shared.jobs.lock_irq().why.get(usize::from(slot)).copied().flatten()
    }

    fn reset(lanes: &mut [Part<UsbLun>]) -> bool {
        // The thread stops the transfer it waits for, which resets a
        // Bulk-Only interface, or resets a UAS device; the requests go
        // again.
        for lane in lanes.iter() {
            lane.shared.link.abort();
        }
        true
    }
}

/// A logical unit's medium, as its stick's thread keeps it: a disk's, or
/// the one put in a drive, and its queue.
struct Medium {
    found: Found,
    shared: Arc<Shared, Kmalloc>,
    disk: Arc<Disk<UsbLun>, Kmalloc>,
    /// Over UAS: it said it had stopped, and a START STOP UNIT is to go
    /// to it before its requests do.
    start: bool,
    /// The START STOP UNIT is in flight.
    starting: bool,
    /// It said it is becoming ready: its requests wait until then.
    not_before: u64,
    /// A request of its ended since its queue last heard.
    ended: bool,
}

/// A logical unit with a disk, or a drive whose medium comes and goes, as
/// its stick's thread keeps it.
struct Lun {
    number: u8,
    name: Name,
    /// Its `sd` letter, or its `cd` number.
    named: Named,
    /// What it says it is.
    inquiry: Inquiry,
    /// How many requests it has in flight at most.
    jobs: usize,
    /// Its medium: a disk's, always; a drive's, while one is in it.
    medium: Option<Medium>,
    /// For a drive, what the thread keeps of it.
    watch: Option<Watch>,
}

/// What a logical unit's name was made of, given back when it goes.
#[derive(Clone, Copy, Debug)]
enum Named {
    /// A disk's or a drive's `sd` letter.
    Sd(u32),
    /// An optical drive's `cd` number.
    Cd(u32),
    /// A floppy drive's `fd` number.
    Fd(u32),
}

impl Named {
    fn give_back(self) {
        match self {
            Named::Sd(letter) => super::give_back_sd_letter(letter),
            Named::Cd(number) => optical::give_back_cd_number(number),
            Named::Fd(number) => floppy::give_back_fd_number(number),
        }
    }
}

/// What the probe found of an optical drive: what is in it, and of a
/// disc of data, where its last session starts, in sectors, and how many
/// sessions it has.
#[derive(Clone, Copy, Debug)]
struct Optical {
    disc: Disc,
    last_session: u64,
    sessions: u16,
}

/// What the thread keeps of a drive whose medium comes and goes.
struct Watch {
    drive: DriveRef,
    /// The device the drive is while empty.
    empty: DeviceRef,
    /// When it is looked at next.
    next: u64,
    /// A request found that its medium was taken out or changed: the next
    /// look takes it out first.
    changed: bool,
    /// Its medium is locked in.
    locked: bool,
    /// It takes the lock: one that refused it is not asked again, as
    /// Linux's `sd` asks no more.
    lockable: bool,
    /// An optical drive that has media events, of which its eject
    /// button's ([`optical::ask_events`]).
    events: bool,
    /// The medium in it is left alone, until the drive says one was taken
    /// out or put in: it was ejected, or could not be used.
    left: bool,
    /// Its medium was taken out, `why`, and its requests fail, but it is
    /// still in `/devices` while the mount table is busy.
    out: Option<&'static str>,
    /// For an optical drive, the DMA memory the commands programs ask of it
    /// are run with, once one is (ADR 0447).
    memory: Option<Dma>,
}

impl Medium {
    /// Takes the first request waiting, if one is: where it is, its slot,
    /// what it does, its first sector and its length in bytes, its
    /// buffers into `frames` if given.
    fn take(
        &self,
        frames: Option<&mut Vec<(u64, u32), Kmalloc>>,
    ) -> Option<(usize, u8, Op, u64, u64)> {
        let mut jobs = self.shared.jobs.lock_irq();
        let at = jobs.jobs.iter().position(|job| matches!(job.stage, Stage::Waiting(_)))?;
        let job = &mut jobs.jobs[at];
        let Stage::Waiting(slot) = job.stage else { return None };
        job.stage = Stage::Running(slot);
        if let Some(frames) = frames {
            frames.clear();
            for &frame in &job.frames {
                // As much room as the request's.
                let _ = frames.push_within_capacity(frame);
            }
        }
        let bytes = job.frames.iter().map(|&(_, len)| u64::from(len)).sum();
        Some((at, slot, job.op, job.sector, bytes))
    }

    /// The buffers of request `at` that hold its bytes `from` to `from +
    /// len`, into `out`.
    fn part(&self, at: usize, from: u64, len: u64, out: &mut Vec<(u64, u32), Kmalloc>) {
        let jobs = self.shared.jobs.lock_irq();
        match jobs.jobs.get(at) {
            Some(job) => slice(&job.frames, from, len, out),
            None => out.clear(),
        }
    }

    /// Hands back request `at`, which ended with `outcome`; its queue
    /// hears of it at [`Medium::collect`].
    fn finish(&mut self, at: usize, outcome: Outcome, why: Option<&'static str>) {
        {
            let mut jobs = self.shared.jobs.lock_irq();
            let Jobs { jobs, why: whys, .. } = &mut *jobs;
            let Some(job) = jobs.get_mut(at) else { return };
            let Stage::Running(slot) = job.stage else { return };
            job.stage = Stage::Done(slot, outcome);
            if let Some(entry) = whys.get_mut(usize::from(slot)) {
                *entry = why;
            }
        }
        self.ended = true;
    }

    /// Tells its queue of the requests that ended.
    fn collect(&mut self) {
        if core::mem::take(&mut self.ended) {
            self.disk.collect(0);
        }
    }

    /// The medium is gone, `why`: its stick was unplugged, or it was
    /// taken out of its drive. Fails the requests it has, and every one
    /// after.
    fn gone(&self, why: &'static str) {
        {
            let mut jobs = self.shared.jobs.lock_irq();
            let Jobs { jobs, why: whys, gone } = &mut *jobs;
            *gone = true;
            for job in jobs.iter_mut() {
                if let Stage::Waiting(slot) | Stage::Running(slot) = job.stage {
                    job.stage = Stage::Done(slot, Outcome::Failed);
                    if let Some(entry) = whys.get_mut(usize::from(slot)) {
                        *entry = Some(why);
                    }
                }
            }
        }
        self.disk.collect(0);
    }
}

/// Why a medium's requests fail once it is gone.
const UNPLUGGED: &str = "its USB device was unplugged";
const TAKEN_OUT: &str = "its medium was taken out";
const EJECTED: &str = "its medium was ejected";
const FORMATTING: &str = "its disk is being formatted";

impl Lun {
    /// Looks at the drive if it is one and is due, or a floppy drive
    /// asked to; ejects its medium if eject(1) asked, formats a floppy
    /// disk's track if FORMAT_TRACK asked; and locks the medium in while
    /// it is used, with commands that `transport` carries one at a time.
    /// Returns when the drive is due next: never, for a disk or a floppy
    /// drive.
    fn tend(&mut self, link: &LinkRef, transport: &mut dyn Transport, now: u64) -> u64 {
        if !self.finish_out() {
            return now.saturating_add(BUSY_NS);
        }
        let Some(watch) = self.watch.as_mut() else { return u64::MAX };
        let drive = watch.drive.clone();
        // A command a program asked of an optical drive (ADR 0447).
        optical::serve_asked(&drive, transport, self.number, (&mut watch.memory, link.source));
        // A disc being burned is not looked at, and stays in; once burned,
        // it is looked at afresh (ADR 0449).
        let (held, ended) = drive.held(now);
        if held {
            self.lock(transport);
            return now.saturating_add(LOOK_NS);
        }
        if ended {
            watch.changed = true;
        }
        let (eject, asked) = (drive.eject_asked(), drive.look_asked());
        let format = drive.format_asked();
        // A track formatted is not looked at: the disk is found once
        // floppy(1) asks, after the last.
        let look = eject || asked || watch.changed || now >= watch.next;
        if !look && format.is_none() {
            return watch.next;
        }
        if eject {
            self.eject(transport);
        }
        if let Some(track) = format {
            drive.formatted(self.format(transport, track));
        }
        if look {
            let looked = self.look(link, transport);
            if asked {
                drive.looked(looked);
            }
        }
        self.lock(transport);
        let Some(watch) = self.watch.as_mut() else { return u64::MAX };
        watch.next = match drive.kind() {
            Kind::Floppy => u64::MAX,
            _ => sched::now_ns().saturating_add(LOOK_NS),
        };
        watch.next
    }

    /// Formats `track` of the disk in the floppy drive, as FORMAT_TRACK
    /// asked (ADR 0436): the disk leaves the kernel first, as what it held
    /// goes, and the drive is empty until a look finds it formatted.
    fn format(&mut self, transport: &mut dyn Transport, track: Track) -> Result<(), FormatError> {
        let (cylinders, heads) = FLOPPY_TRACKS;
        if track.cylinder >= cylinders || track.head >= heads {
            return Err(FormatError::OutOfRange);
        }
        self.take_out(FORMATTING);
        // A disk left alone, unformatted, is wanted after all.
        if let Some(watch) = self.watch.as_mut() {
            watch.left = false;
        }
        let place = (track.cylinder as u8, track.head as u8);
        if place == (0, 0) {
            let size = FLOPPY_SIZES.iter().find(|size| size.sectors == track.sectors);
            let size = size.map_or("", |size| size.name);
            info!("Disk: {}: formatting its disk as a {size} one", self.name);
        }
        let why = match ufi::format_track(transport, self.number, place, track.sectors) {
            Ok(()) => return Ok(()),
            Err(why) => why,
        };
        let sense = match why {
            Absent::NoMedium => return Err(FormatError::NoDisk),
            Absent::Failed(_, sense) => sense,
            _ => None,
        };
        match sense.map(|sense| (sense.meaning(), sense.asc)) {
            Some((Meaning::NoMedium, _)) => Err(FormatError::NoDisk),
            Some((Meaning::WriteProtected, _)) => Err(FormatError::WriteProtected),
            // The disk does not take that size: a double-density disk
            // formatted as a 1.44 MB one (incompatible medium).
            Some((Meaning::Unsupported, _) | (_, 0x30)) => Err(FormatError::Unsupported),
            _ => {
                let (cylinder, head) = place;
                warn!("Disk: {}: cylinder {cylinder}, head {head} not formatted: {why}", self.name);
                Err(FormatError::Failed)
            }
        }
    }

    /// Looks whether a medium is in the drive, as Linux's `sd_check_events`
    /// does, after an optical drive's media events, as `sr`'s: one taken
    /// out, or changed, is taken out of the kernel, and one put in is put
    /// in. Says whether it could look.
    fn look(&mut self, link: &LinkRef, transport: &mut dyn Transport) -> bool {
        let Some(watch) = self.watch.as_mut() else { return false };
        let (name, number) = (&self.name, self.number);
        let said = optical::ask_events(transport, number, name, &watch.drive, &mut watch.events);
        let Ok(found) = look(transport, self.number) else { return false };
        let Some(watch) = self.watch.as_mut() else { return false };
        // What a request found counts once.
        let changed = core::mem::take(&mut watch.changed) || found.changed || said;
        match found.medium {
            // The transport failed: the next look says.
            None => {}
            Some(false) => {
                watch.left = false;
                self.emptied(TAKEN_OUT);
            }
            Some(true) => {
                if changed {
                    watch.left = false;
                    self.take_out(TAKEN_OUT);
                }
                let left = self.watch.as_ref().is_some_and(|watch| watch.left);
                if self.medium.is_none() && !left {
                    self.put_in(link, transport);
                }
            }
        }
        true
    }

    /// The medium in the drive, found by [`look`], is put in: it gets a
    /// queue, and takes the empty drive's place with its partitions. One
    /// that cannot be used is left alone until it is taken out.
    fn put_in(&mut self, link: &LinkRef, transport: &mut dyn Transport) {
        let mut wait = |ms: u32| sched::sleep_ns(u64::from(ms) * 1_000_000);
        let (number, inquiry) = (self.number, self.inquiry);
        let mut disc = None;
        let probed = if inquiry.is_optical() {
            let (what, probed) = optical::probe(transport, number, inquiry, &mut wait);
            if let Some(watch) = &self.watch {
                let _ = watch.drive.set_disc(what);
            }
            probed.map(|(medium, found)| {
                disc = Some(optical::describe(&found, &medium));
                (medium, optical::last_session(&found, &medium))
            })
        } else {
            medium(transport, number, inquiry, &mut wait).map(|found| (found, 0))
        };
        let (found, last_session) = match probed {
            Ok(probed) => probed,
            // Taken out again, or the stick unplugged: the next look says.
            Err(Absent::NoMedium | Absent::Gone) => return,
            Err(Absent::NotData(content)) => {
                info!("Disk: {}: {}", self.name, optical::not_data(content));
                self.unusable();
                return;
            }
            Err(why) => {
                info!("Disk: {}: a medium went in, but {why}; not used", self.name);
                self.unusable();
                return;
            }
        };
        let made = new_medium(link, self.jobs, found, last_session, &self.name);
        let (medium, device) = match made {
            Ok(made) => made,
            Err(why) => {
                warn!("Disk: {}: a medium went in, but {why}", self.name);
                self.unusable();
                return;
            }
        };
        let info = device.info();
        if let Some(disc) = disc {
            info!("Disk: {}: {disc} went in", self.name);
        } else {
            info!(
                "Disk: {}: a medium of {} went in, {} bytes a block{}{}{}",
                self.name,
                Size(info.sectors * SECTOR),
                info.block_size,
                super::sector_note(&info),
                if found.read_only { ", write-protected" } else { "" },
                if found.write_cache { ", write cache" } else { "" },
            );
        }
        // A disc has no partition table, as Linux's `sr` reads none, nor
        // a floppy disk, as Linux reads none on one.
        let parts = if inquiry.is_optical() || link.ufi {
            None
        } else {
            read_table(transport, self.number, &found, &self.name, &device)
        };
        let Some(watch) = &self.watch else { return };
        let parts = parts.as_deref().unwrap_or(&[]);
        // The drive was empty: nothing was mounted from it to lose.
        match super::replace(&self.name, device, parts, (&watch.drive, false)) {
            super::Replaced::Done => self.medium = Some(medium),
            _ => medium.gone(TAKEN_OUT),
        }
    }

    /// Leaves the medium in the drive alone until it is taken out.
    fn leave(&mut self) {
        if let Some(watch) = self.watch.as_mut() {
            watch.left = true;
        }
    }

    /// Leaves the medium that went in, which cannot be used, alone until
    /// it is taken out. In an optical drive, it counts as a change, so
    /// that volumed(8) reads what the drive says is in it and says so.
    fn unusable(&mut self) {
        self.leave();
        if let Some(watch) = &self.watch
            && self.inquiry.is_optical()
        {
            watch.drive.changed();
        }
    }

    /// The drive has no medium in it now, or none to use, `why`: its
    /// medium is taken out of the kernel, and an optical drive's disc of
    /// no data, left alone, counts a change as it goes, as one of data
    /// does.
    fn emptied(&mut self, why: &'static str) {
        let Some(watch) = &self.watch else { return };
        let drive = watch.drive.clone();
        let counted = self.medium.is_some();
        let differs = drive.set_disc(Disc::None);
        self.take_out(why);
        if differs && !counted {
            drive.changed();
        }
    }

    /// Takes the drive's medium out of the kernel, if one is in: its
    /// requests fail `why` from now on, and the drive is empty, once the
    /// mount table lets what was mounted from it be lost.
    fn take_out(&mut self, why: &'static str) {
        let Some(watch) = &mut self.watch else { return };
        let Some(medium) = self.medium.take() else { return };
        // First, so that an unmount that syncs it, with the mount table
        // held, fails at once and lets the table go.
        medium.gone(why);
        // A drive locked takes the lock off with the medium.
        watch.locked = false;
        watch.out = Some(why);
        self.finish_out();
    }

    /// Finishes taking out the medium [`Lun::take_out`] took out: what was
    /// mounted from it is lost, and the drive is empty in `/devices`.
    /// False while the mount table is busy.
    fn finish_out(&mut self) -> bool {
        let Some(watch) = &mut self.watch else { return true };
        let Some(why) = watch.out else { return true };
        let lost = super::replace(&self.name, watch.empty.clone(), &[], (&watch.drive, true));
        if lost == super::Replaced::Busy {
            return false;
        }
        watch.out = None;
        info!("Disk: {}: {why}", self.name);
        true
    }

    /// Locks the drive's medium in while anything uses it or its
    /// partitions, and lets it go when nothing does, as Linux's `sd` does
    /// while its disk is open. A drive that refuses the lock is not asked
    /// again.
    fn lock(&mut self, transport: &mut dyn Transport) {
        let used = match self.medium {
            None => false,
            // The mount table is busy: the next look says.
            Some(_) => match super::in_use_now(self.name.as_str()) {
                Some(used) => used,
                None => return,
            },
        };
        let Some(watch) = self.watch.as_mut() else { return };
        // A disc being burned stays in too (ADR 0449).
        let used = used || watch.drive.is_held(sched::now_ns());
        if !watch.lockable || used == watch.locked {
            return;
        }
        match prevent_removal(transport, self.number, used) {
            Ok(true) => watch.locked = used,
            Ok(false) => {
                (watch.lockable, watch.locked) = (false, false);
                info!("Disk: {}: its drive does not lock its medium in", self.name);
            }
            // The next look tries again.
            Err(_) => {}
        }
    }

    /// Ejects the drive's medium, as eject(1) asked through EJECT: lets
    /// the lock go, ejects it, takes it out of the kernel, and answers
    /// the one who asked. A drive that refuses to eject lets the medium
    /// go all the same, as a card reader's slot does: it can be taken
    /// out.
    fn eject(&mut self, transport: &mut dyn Transport) {
        let ejected = match radix_scsi::eject(transport, self.number) {
            Ok(()) => true,
            Err(Absent::Failed(_, sense)) => {
                let empty = sense.is_some_and(|s| matches!(s.meaning(), Meaning::NoMedium));
                if self.medium.is_some() && !empty {
                    info!("Disk: {}: its drive does not eject; its medium is let go", self.name);
                }
                true
            }
            Err(_) => false,
        };
        if ejected {
            self.emptied(EJECTED);
            self.leave();
        }
        if let Some(watch) = &self.watch {
            watch.drive.answer(ejected);
        }
    }
}

/// What a request waits for after a command of it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    Nothing,
    /// The disk is becoming ready: a second.
    Ready,
    /// It had stopped: a START STOP UNIT.
    Start,
    /// It says its medium was taken out, or changed: a drive's request
    /// fails, and the drive is looked at.
    Changed,
}

/// How a command that ended with `reply` ends its request, as Linux's SCSI
/// layer has it for a USB disk, and what its next try waits for: a
/// `flush` the disk does not take did not need doing. A drive's request
/// that finds its medium changed fails instead (`Then::Changed`), as
/// Linux's does for a removable disk.
fn judge(
    reply: Result<Reply, TransportError>,
    flush: bool,
) -> (Outcome, Option<&'static str>, Then) {
    let sense = match reply {
        Ok(Reply::Good { residue: 0 }) => return (Outcome::Done, None, Then::Nothing),
        Ok(Reply::Good { .. }) => {
            return (Outcome::Retry, Some("it moved less than asked"), Then::Nothing);
        }
        Ok(Reply::Failed(Some(sense))) => sense,
        Ok(Reply::Failed(None)) => {
            let why = "it failed the command, and did not say why";
            return (Outcome::Retry, Some(why), Then::Nothing);
        }
        Err(TransportError::Gone) => {
            return (Outcome::Failed, Some("its USB device was unplugged"), Then::Nothing);
        }
        Err(TransportError::Failed(why)) => return (Outcome::Retry, Some(why), Then::Nothing),
    };
    match sense.meaning() {
        Meaning::Fine => (Outcome::Done, None, Then::Nothing),
        Meaning::Unsupported if flush => (Outcome::Done, None, Then::Nothing),
        Meaning::Attention if sense.medium_changed() => {
            (Outcome::Retry, Some("its medium may have changed"), Then::Changed)
        }
        Meaning::Attention | Meaning::Aborted => {
            (Outcome::Retry, Some(sense.key_name()), Then::Nothing)
        }
        Meaning::BecomingReady => (Outcome::Retry, Some("it is becoming ready"), Then::Ready),
        Meaning::NeedsStart => (Outcome::Retry, Some("it had stopped"), Then::Start),
        // USB bridges say so of errors they recover from (Linux's
        // retry_hwerror).
        Meaning::Error if sense.key == key::HARDWARE_ERROR => {
            (Outcome::Retry, Some(sense.key_name()), Then::Nothing)
        }
        Meaning::NoMedium => (Outcome::Failed, Some(TAKEN_OUT), Then::Changed),
        Meaning::WriteProtected => {
            (Outcome::Failed, Some("its medium is write-protected"), Then::Nothing)
        }
        _ => (Outcome::Failed, Some(sense.key_name()), Then::Nothing),
    }
}

/// What the thread works with: a request's buffers, and a part of them.
struct Work {
    frames: Vec<(u64, u32), Kmalloc>,
    part: Vec<(u64, u32), Kmalloc>,
}

/// What a UAS command carries.
#[derive(Clone, Copy, Debug)]
enum Carried {
    /// A part of request `job` of the disk at `lun`, which reads (or
    /// writes) `blocks` blocks from block `first`: its blocks `done` to
    /// `done + count`.
    Part { lun: usize, job: usize, into_host: bool, first: u64, blocks: u64, done: u64, count: u64 },
    /// Request `job` of the disk at `lun`, a flush.
    Flush { lun: usize, job: usize },
    /// A START STOP UNIT to the disk at `lun`, which had stopped.
    Start { lun: usize },
}

/// A UAS stick's commands, as its thread runs them.
struct Queued<'a> {
    uas: Box<Uas, Kmalloc>,
    pipes: UasPipes<'a>,
    /// What each tag carries, by tag less one.
    carried: Vec<Option<Carried>, Kmalloc>,
    /// Where a status lands.
    status: [u8; STATUS_LENGTH],
    /// The buffers of the part of a request that a command carries.
    part: Vec<(u64, u32), Kmalloc>,
}

impl<'a> Queued<'a> {
    /// The commands of the UAS stick `link`, whose status and data pipes
    /// have `streams` streams; `None` when memory runs out. Kept out of
    /// its callers' frames, which the engine would not fit in with them.
    #[inline(never)]
    fn new(link: &'a Link, streams: Option<usize>) -> Option<Box<Queued<'a>, Kmalloc>> {
        let heap = HEAP.with(Flags::MAY_SLEEP);
        let uas = Box::try_new_in(Uas::new(streams), heap).ok()?;
        let pipes = UasPipes::new(link)?;
        let (mut carried, mut part) = (Vec::new_in(heap), Vec::new_in(heap));
        carried.try_reserve_exact(MAX_TAGS).ok()?;
        part.try_reserve_exact(UAS_SEGMENTS).ok()?;
        while carried.push_within_capacity(None).is_ok() {}
        let queued = Queued { uas, pipes, carried, status: [0; STATUS_LENGTH], part };
        Box::try_new_in(queued, heap).ok()
    }
}

/// How a stick's commands go.
enum Wire<'a> {
    Bot(Bot<StickPipes<'a>>),
    Uas(Box<Queued<'a>, Kmalloc>),
    /// A floppy drive's Control/Bulk/Interrupt transport.
    Cbi(Cbi<StickPipes<'a>>),
}

impl Wire<'_> {
    /// Runs `f` with the stick's commands one at a time: over UAS, while
    /// no other is in flight.
    fn one<R>(&mut self, f: impl FnOnce(&mut dyn Transport) -> R) -> R {
        match self {
            Wire::Bot(bot) => f(bot),
            Wire::Cbi(cbi) => f(cbi),
            Wire::Uas(queued) => {
                let Queued { uas, pipes, .. } = &mut **queued;
                f(&mut OneAtATime { uas, pipes })
            }
        }
    }
}

/// The thread of the stick `link`: finds its disks, serves them until it
/// is unplugged, and takes them away then.
pub fn run(link: &LinkRef) {
    let stick = Stick::new(link);
    crate::usb::storage::probed(link);
    let Some(mut stick) = stick else { return };
    stick.serve();
    for lun in &stick.luns {
        if let Some(medium) = &lun.medium {
            medium.gone(UNPLUGGED);
        }
        if lun.watch.is_some() {
            removable::remove(lun.name.as_str());
        }
        super::unregister(lun.name.as_str());
        lun.named.give_back();
        info!("Disk: {} is gone with its USB device", lun.name);
    }
}

/// A stick's transport and disks, as its thread runs them.
struct Stick<'a> {
    link: &'a LinkRef,
    wire: Wire<'a>,
    luns: Vec<Lun, Kmalloc>,
    work: Work,
}

impl<'a> Stick<'a> {
    /// Finds the disks behind the stick `link`; `None` if it has none, or
    /// memory runs out. Kept out of [`run`]'s frame, which the probe's
    /// buffers would not fit in.
    #[inline(never)]
    fn new(link: &'a LinkRef) -> Option<Stick<'a>> {
        let heap = HEAP.with(Flags::MAY_SLEEP);
        let mut frames = Vec::new_in(heap);
        let mut part = Vec::new_in(heap);
        let mut luns = Vec::new_in(heap);
        let reserved = frames.try_reserve_exact(SUPER_SEGMENTS).is_ok()
            && part.try_reserve_exact(SUPER_SEGMENTS).is_ok();
        let wire = match link.protocol {
            Protocol::Bot { .. } => StickPipes::new(link).map(|p| {
                let bot = Bot::new(p, link.interface);
                Wire::Bot(if link.ufi { bot.ufi() } else { bot })
            }),
            Protocol::Uas { streams, .. } => Queued::new(link, streams).map(Wire::Uas),
            Protocol::Cbi { interrupt, .. } => StickPipes::new(link)
                .map(|p| Wire::Cbi(Cbi::new(p, link.interface, interrupt.is_some(), link.ufi))),
        };
        let (Some(mut wire), true) = (wire, reserved) else {
            warn!("Disk: USB {}: out of memory for its disks", link.place);
            return None;
        };
        // A stick unplugged already finds none.
        let mut numbers = [0u8; MAX_LUNS];
        let count = logical_units(&mut wire, &mut numbers)?;
        // Over UAS the tags are shared among the units, each its part.
        let jobs = match &wire {
            Wire::Bot(_) | Wire::Cbi(_) => 1,
            Wire::Uas(queued) => (queued.uas.depth() / count).max(1),
        };
        let mut wait = |ms: u32| sched::sleep_ns(u64::from(ms) * 1_000_000);
        for &number in &numbers[..count] {
            let mut lun = TextBuf::<16>::new();
            if count > 1 || number > 0 {
                let _ = core::fmt::Write::write_fmt(&mut lun, format_args!(" LUN {number}"));
            }
            let inquiry = match wire.one(|transport| identify(transport, number)) {
                Ok(inquiry) if inquiry.is_disk() || inquiry.is_optical() => inquiry,
                Ok(inquiry) => {
                    let why = Absent::NotDisk(inquiry.device_type);
                    info!("Disk: USB {}{lun}: {why}; not used", link.place);
                    continue;
                }
                Err(Absent::Gone) => break,
                Err(why) => {
                    info!("Disk: USB {}{lun}: {why}; not used", link.place);
                    continue;
                }
            };
            // A drive with no medium in it, or one that cannot be used,
            // is a drive all the same: one put in later fills it.
            let (found, optical) = if inquiry.is_optical() {
                let probe = |t: &mut dyn Transport| optical::probe(t, number, inquiry, &mut wait);
                let (disc, probed) = wire.one(probe);
                let last_session = probed.as_ref().map_or(0, |(m, f)| optical::last_session(f, m));
                let sessions = probed.as_ref().map_or(0, |(_, found)| found.sessions);
                let optical = Optical { disc, last_session, sessions };
                (probed.map(|(medium, _)| medium), Some(optical))
            } else {
                (wire.one(|transport| medium(transport, number, inquiry, &mut wait)), None)
            };
            let found = match found {
                Ok(found) => Ok(found),
                Err(Absent::Gone) => break,
                Err(Absent::NoMedium) => Err(None),
                Err(why) if inquiry.removable || optical.is_some() => Err(Some(why)),
                Err(why) => {
                    info!("Disk: USB {}{lun}: {why}; not used", link.place);
                    continue;
                }
            };
            if luns.try_reserve(1).is_err() {
                warn!("Disk: USB {}{lun}: out of memory", link.place);
                break;
            }
            match add(link, &mut wire, (number, jobs), (inquiry, optical), found, &lun) {
                Ok(added) => {
                    let _ = luns.push_within_capacity(added);
                }
                Err(why) => warn!("Disk: USB {}{lun}: {why}", link.place),
            }
        }
        if luns.is_empty() {
            return None;
        }
        Some(Stick { link, wire, luns, work: Work { frames, part } })
    }

    /// Serves the disks' requests until the stick is unplugged.
    fn serve(&mut self) {
        match self.wire {
            Wire::Bot(_) | Wire::Cbi(_) => self.serve_one_at_a_time(),
            Wire::Uas(_) => self.serve_queued(),
        }
    }

    /// Serves the requests one at a time, over Bulk-Only or a floppy
    /// drive's CBI, and looks at the drives between them.
    fn serve_one_at_a_time(&mut self) {
        while !self.link.is_gone() {
            let mut served = false;
            for at in 0..self.luns.len() {
                let Some(medium) = &self.luns[at].medium else { continue };
                let Some((job, _, op, sector, _)) = medium.take(Some(&mut self.work.frames)) else {
                    continue;
                };
                served = true;
                self.link.forget_abort();
                let (outcome, why) = self.execute(at, op, sector);
                if let Some(medium) = self.luns[at].medium.as_mut() {
                    medium.finish(job, outcome, why);
                    medium.collect();
                }
            }
            let Stick { link, wire, luns, .. } = self;
            let next = wire.one(|transport| tend(link, transport, luns));
            if !served {
                let idle = sched::now_ns().saturating_add(IDLE_NS);
                sched::block_until(idle.min(next), "usb-storage");
            }
        }
    }

    /// Carries out `op` from `sector` with the buffers in the work's, on
    /// the disk at `at`, one command at a time; returns how it ended, and
    /// why if it did not work.
    fn execute(&mut self, at: usize, op: Op, sector: u64) -> (Outcome, Option<&'static str>) {
        let number = self.luns[at].number;
        let Some(found) = self.luns[at].medium.as_ref().map(|medium| medium.found) else {
            return (Outcome::Failed, Some(TAKEN_OUT));
        };
        let into_host = match op {
            Op::Read => true,
            Op::Write | Op::WriteThrough => false,
            Op::Flush => {
                let cdb = Cdb::synchronize_cache();
                let reply = self.wire.one(|t| t.command(number, &cdb, Data::None, TIMEOUT_MS));
                return self.after(at, judge(reply, true));
            }
            Op::Discard(_) => return (Outcome::Failed, Some("it does not discard")),
            Op::Smart(..) => return (Outcome::Failed, Some("it keeps no SMART record")),
        };
        let block = u64::from(found.capacity.block_length);
        let unit = block / SECTOR;
        let total: u64 = self.work.frames.iter().map(|&(_, len)| u64::from(len)).sum();
        let (first, blocks) = (sector / unit, total / block);
        let mut done = 0;
        while done < blocks {
            let (start, count) = part_of(&found, first + done, blocks - done);
            slice(&self.work.frames, done * block, count * block, &mut self.work.part);
            let cdb = if into_host {
                Cdb::read(start, count as u32, found.sixteen)
            } else {
                Cdb::write(start, count as u32, found.sixteen)
            };
            let Some(cdb) = cdb else {
                return (Outcome::Failed, Some("its blocks are past what a command reaches"));
            };
            let data = Data::Frames { into_host, frames: &self.work.part };
            let reply = self.wire.one(|t| t.command(number, &cdb, data, TIMEOUT_MS));
            let (outcome, why) = self.after(at, judge(reply, false));
            if outcome != Outcome::Done {
                return (outcome, why);
            }
            done += count;
        }
        (Outcome::Done, None)
    }

    /// Does what a request of the logical unit at `at` waits for before
    /// it goes again, one command at a time: a second for a disk becoming
    /// ready, a START STOP UNIT for one that stopped. A drive's request
    /// that found its medium changed fails, and the drive is looked at.
    fn after(
        &mut self,
        at: usize,
        (outcome, why, then): (Outcome, Option<&'static str>, Then),
    ) -> (Outcome, Option<&'static str>) {
        let number = self.luns[at].number;
        match then {
            Then::Nothing => {}
            Then::Ready => sched::sleep_ns(READY_NS),
            Then::Start => {
                let start = Cdb::start_stop_unit(true, false);
                let _ = self.wire.one(|t| t.command(number, &start, Data::None, TIMEOUT_MS));
            }
            Then::Changed => {
                if let Some(watch) = self.luns[at].watch.as_mut() {
                    watch.changed = true;
                    return (Outcome::Failed, why);
                }
            }
        }
        (outcome, why)
    }

    /// Serves the requests many at once, over UAS.
    fn serve_queued(&mut self) {
        let Stick { link, wire, luns, .. } = self;
        let Wire::Uas(queued) = wire else { return };
        let queued = &mut **queued;
        loop {
            if link.is_gone() {
                queued.uas.gone();
                queued.finish_all(luns);
                return;
            }
            let mut busy = false;
            if link.take_abort() {
                // A request went unanswered 30 s: the device is reset.
                queued.uas.timed_out();
            }
            loop {
                if queued.uas.is_gone() {
                    break;
                }
                let Some((pipe, n, result)) = queued.pipes.take_ended(&mut queued.status) else {
                    break;
                };
                queued.uas.ended(pipe, n, result, &queued.status);
                busy = true;
            }
            if let Some(why) = queued.uas.broken() {
                warn!("Disk: USB {}: {why}; it is reset", link.place);
                let result = queued.pipes.reset();
                queued.uas.reset(result);
                busy = true;
            }
            busy |= queued.finish_all(luns);
            busy |= queued.begin(luns);
            busy |= queued.pipes.act(&mut queued.uas);
            for medium in luns.iter_mut().filter_map(|lun| lun.medium.as_mut()) {
                medium.collect();
            }
            // The drives are looked at while no command is in flight, as
            // a command one at a time takes the device to itself.
            let mut next = u64::MAX;
            if queued.uas.in_flight() == 0 && !queued.uas.is_gone() && queued.uas.broken().is_none()
            {
                let Queued { uas, pipes, .. } = &mut *queued;
                next = tend(link, &mut OneAtATime { uas, pipes }, luns);
            }
            if !busy {
                let idle = sched::now_ns().saturating_add(IDLE_NS);
                sched::block_until(idle.min(next), "usb-storage");
            }
        }
    }
}

/// Tends the drives among `luns`, the logical units of the stick `link`,
/// with commands `transport` carries one at a time ([`Lun::tend`]).
/// Returns when the first is due next.
fn tend(link: &LinkRef, transport: &mut dyn Transport, luns: &mut [Lun]) -> u64 {
    let now = sched::now_ns();
    luns.iter_mut().map(|lun| lun.tend(link, transport, now)).min().unwrap_or(u64::MAX)
}

impl Queued<'_> {
    /// Hands back the commands that finished; says whether any did.
    fn finish_all(&mut self, luns: &mut [Lun]) -> bool {
        let mut any = false;
        while let Some((tag, reply)) = self.uas.next_done() {
            any = true;
            let Some(carried) = self.carried[usize::from(tag) - 1].take() else { continue };
            self.finished(luns, carried, reply);
        }
        any
    }

    /// A command that carried `carried` ended with `reply`.
    fn finished(
        &mut self,
        luns: &mut [Lun],
        carried: Carried,
        reply: Result<Reply, TransportError>,
    ) {
        let (at, job, flush) = match carried {
            Carried::Start { lun } => {
                if let Some(medium) = luns[lun].medium.as_mut() {
                    medium.starting = false;
                }
                return;
            }
            Carried::Flush { lun, job } => (lun, job, true),
            Carried::Part { lun, job, .. } => (lun, job, false),
        };
        // A medium is taken out only while no command is in flight: the
        // command was its.
        let Lun { number, medium, watch, .. } = &mut luns[at];
        let Some(medium) = medium.as_mut() else { return };
        let (mut outcome, why, then) = judge(reply, flush);
        if let (Outcome::Done, Carried::Part { into_host, first, blocks, done, count, .. }) =
            (outcome, carried)
            && done + count < blocks
        {
            // The next part goes under the tag this one freed.
            self.part(medium, (at, *number, job), (into_host, first, blocks), done + count);
            return;
        }
        match then {
            Then::Nothing => {}
            Then::Ready => medium.not_before = sched::now_ns().saturating_add(READY_NS),
            Then::Start => medium.start = true,
            Then::Changed => {
                if let Some(watch) = watch.as_mut() {
                    watch.changed = true;
                    outcome = Outcome::Failed;
                }
            }
        }
        medium.finish(job, outcome, why);
    }

    /// Starts the requests waiting, as the free tags let them, and the
    /// START STOP UNITs disks wait for; says whether any started.
    fn begin(&mut self, luns: &mut [Lun]) -> bool {
        let now = sched::now_ns();
        let mut any = false;
        for (at, lun) in luns.iter_mut().enumerate() {
            let number = lun.number;
            let Some(medium) = lun.medium.as_mut() else { continue };
            if medium.start
                && !medium.starting
                && let Some(tag) = self.uas.free_tag()
            {
                let start = Cdb::start_stop_unit(true, false);
                if self.command(tag, number, &start, None, Carried::Start { lun: at }) {
                    (medium.start, medium.starting) = (false, true);
                    any = true;
                }
            }
            if medium.start || medium.starting || now < medium.not_before {
                continue;
            }
            while let Some(tag) = self.uas.free_tag() {
                let Some((job, _, op, sector, bytes)) = medium.take(None) else { break };
                any = true;
                let found = medium.found;
                let block = u64::from(found.capacity.block_length);
                let first = sector / (block / SECTOR);
                let into_host = match op {
                    Op::Read => true,
                    Op::Write | Op::WriteThrough => false,
                    Op::Flush => {
                        let cdb = Cdb::synchronize_cache();
                        let carried = Carried::Flush { lun: at, job };
                        if !self.command(tag, number, &cdb, None, carried) {
                            medium.finish(job, Outcome::Retry, Some("its device is being reset"));
                        }
                        continue;
                    }
                    Op::Discard(_) => {
                        medium.finish(job, Outcome::Failed, Some("it does not discard"));
                        continue;
                    }
                    Op::Smart(..) => {
                        medium.finish(job, Outcome::Failed, Some("it keeps no SMART record"));
                        continue;
                    }
                };
                let blocks = bytes / block;
                if blocks == 0 {
                    medium.finish(job, Outcome::Done, None);
                    continue;
                }
                self.part(medium, (at, number, job), (into_host, first, blocks), 0);
            }
        }
        any
    }

    /// Starts the part of request `job` of `medium`, the medium of the
    /// logical unit `number` at `at`, from its block `done`, of a request
    /// that reads (or writes) `blocks` blocks from block `first`.
    fn part(
        &mut self,
        medium: &mut Medium,
        (at, number, job): (usize, u8, usize),
        (into_host, first, blocks): (bool, u64, u64),
        done: u64,
    ) {
        let found = medium.found;
        let block = u64::from(found.capacity.block_length);
        let (start, count) = part_of(&found, first + done, blocks - done);
        let cdb = if into_host {
            Cdb::read(start, count as u32, found.sixteen)
        } else {
            Cdb::write(start, count as u32, found.sixteen)
        };
        let Some(cdb) = cdb else {
            let why = "its blocks are past what a command reaches";
            medium.finish(job, Outcome::Failed, Some(why));
            return;
        };
        let Some(tag) = self.uas.free_tag() else {
            medium.finish(job, Outcome::Retry, Some("its device is being reset"));
            return;
        };
        medium.part(job, done * block, count * block, &mut self.part);
        self.pipes.set_data(tag, &self.part);
        let data = Some((into_host, (count * block) as u32));
        let carried = Carried::Part { lun: at, job, into_host, first, blocks, done, count };
        if !self.command(tag, number, &cdb, data, carried) {
            medium.finish(job, Outcome::Retry, Some("its device is being reset"));
        }
    }

    /// Starts `cdb` to logical unit `number` under `tag`, with data of
    /// `data`'s direction and length in the buffers given the tag, for
    /// `carried`. False if it could not start: the device needs a reset.
    fn command(
        &mut self,
        tag: u16,
        number: u8,
        cdb: &Cdb,
        data: Option<(bool, u32)>,
        carried: Carried,
    ) -> bool {
        let Some(iu) = self.uas.start(tag, number, cdb, data) else { return false };
        self.pipes.place_iu(tag, &iu);
        self.carried[usize::from(tag) - 1] = Some(carried);
        true
    }
}

/// The blocks of a read or write of `count` blocks from block `start` of
/// the disk `found` that go in its next command: all of them, but for the
/// last eight blocks, which go one a command.
fn part_of(found: &Found, start: u64, count: u64) -> (u64, u64) {
    let last = found.capacity.blocks.saturating_sub(LAST_BLOCKS);
    if start + count <= last {
        (start, count)
    } else if start < last {
        (start, last - start)
    } else {
        (start, 1)
    }
}

/// Puts the buffers of `frames` that hold bytes `from` to `from + len`
/// (counting from the first buffer's) in `out`, cutting the first and
/// last.
fn slice(frames: &[(u64, u32)], from: u64, len: u64, out: &mut Vec<(u64, u32), Kmalloc>) {
    out.clear();
    let (end, mut at) = (from + len, 0u64);
    for &(phys, size) in frames {
        let (start, stop) = (at, at + u64::from(size));
        at = stop;
        if stop <= from || start >= end {
            continue;
        }
        let (lo, hi) = (start.max(from), stop.min(end));
        // No more parts than the buffers.
        let _ = out.push_within_capacity((phys + (lo - start), (hi - lo) as u32));
    }
}

/// The logical units behind the stick, into `numbers`: Get Max LUN's over
/// Bulk-Only, REPORT LUNS's over UAS (logical unit 0 alone if it fails,
/// as Linux's SCSI scan then takes it). Returns how many; `None` if the
/// stick is gone.
#[inline(never)]
fn logical_units(wire: &mut Wire<'_>, numbers: &mut [u8; MAX_LUNS]) -> Option<usize> {
    let max_lun = match wire {
        Wire::Bot(bot) => bot.max_lun().ok()?,
        // A UFI drive has one.
        Wire::Cbi(_) => 0,
        Wire::Uas(_) => {
            let mut bytes = [0u8; REPORT_LENGTH];
            let cdb = Cdb::report_luns(REPORT_LENGTH as u32);
            let reply = wire.one(|t| t.command(0, &cdb, Data::In(&mut bytes), TIMEOUT_MS));
            let mut count = 0;
            match reply {
                Err(TransportError::Gone) => return None,
                Ok(Reply::Good { residue }) => {
                    let came = REPORT_LENGTH.saturating_sub(residue as usize);
                    for lun in reported_luns(&bytes[..came]) {
                        if count < MAX_LUNS && !numbers[..count].contains(&lun) {
                            numbers[count] = lun;
                            count += 1;
                        }
                    }
                }
                _ => {}
            }
            if count == 0 {
                numbers[0] = 0;
                count = 1;
            }
            return Some(count);
        }
    };
    for (number, slot) in numbers.iter_mut().enumerate().take(usize::from(max_lun) + 1) {
        *slot = number as u8;
    }
    Some(usize::from(max_lun) + 1)
}

/// Gives the logical unit `number`, a disk or a drive as `inquiry` says
/// it is (an optical drive with what `optical` says is in it), a name
/// and, with the medium `found` in it, a queue of `jobs` requests in
/// flight, and registers it and the medium's partitions. A unit with no
/// medium in it, or with one that cannot be used (`why`), is a drive,
/// registered empty.
#[inline(never)]
fn add(
    link: &LinkRef,
    wire: &mut Wire<'_>,
    (number, jobs): (u8, usize),
    (inquiry, optical): (Inquiry, Option<Optical>),
    found: Result<Found, Option<Absent>>,
    lun: &TextBuf<16>,
) -> Result<Lun, &'static str> {
    // The SATA disks and drives found at boot are named first, then the
    // sticks' in the order they were found.
    super::wait_for_scan();
    crate::usb::storage::wait_for_turn(link);
    let (named, name) = if optical.is_some() {
        let (number, name) = optical::take_cd_name().ok_or("too many optical drives")?;
        (Named::Cd(number), name)
    } else if link.ufi {
        // After the PC's floppy drives, which take their numbers.
        floppy::wait_for_names();
        let (number, name) = floppy::take_fd_name(None).ok_or("too many floppy drives")?;
        (Named::Fd(number), name)
    } else {
        let (letter, name) = super::take_sd_name().ok_or("too many SATA and USB disks")?;
        (Named::Sd(letter), name)
    };
    add_named(link, wire, (number, jobs), (inquiry, optical), found, (name, named), lun)
        .inspect_err(|_| named.give_back())
}

/// [`add`]'s part with the unit named `name`, after `named`.
fn add_named(
    link: &LinkRef,
    wire: &mut Wire<'_>,
    (number, jobs): (u8, usize),
    (inquiry, optical): (Inquiry, Option<Optical>),
    found: Result<Found, Option<Absent>>,
    (name, named): (Name, Named),
    lun: &TextBuf<16>,
) -> Result<Lun, &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let last_session = optical.map_or(0, |optical| optical.last_session);
    let made = match found {
        Ok(found) => Some(new_medium(link, jobs, found, last_session, &name)?),
        Err(_) => None,
    };
    // A drive whose medium comes and goes: one that says so, an optical
    // or a floppy drive, or one that has none in it now, as a card
    // reader's empty slot.
    let watch = if inquiry.removable || optical.is_some() || link.ufi || made.is_none() {
        let empty: DeviceRef = Arc::try_new_in(Empty, heap).map_err(|_| "out of memory")?;
        let thread = sched::current().ok_or("it has no thread")?;
        let (kind, formats) = match optical {
            Some(_) => (Kind::Optical, &[][..]),
            None if link.ufi => (Kind::Floppy, FLOPPY_SIZES),
            None => (Kind::Polled, &[][..]),
        };
        let drive = removable::add(&name, thread, kind, formats).ok_or("out of memory")?;
        if let Some(optical) = optical {
            let _ = drive.set_disc(optical.disc);
        }
        // A floppy drive is looked at only when asked (ADR 0435).
        let next = match kind {
            Kind::Floppy => u64::MAX,
            _ => sched::now_ns().saturating_add(LOOK_NS),
        };
        let left = matches!(found, Err(Some(_)));
        let (changed, locked, lockable, out) = (false, false, true, None);
        // An optical drive is asked for its media events (ADR 0432).
        let events = optical.is_some();
        let memory = None;
        Some(Watch { drive, empty, next, changed, locked, lockable, events, left, out, memory })
    } else {
        None
    };
    let how = match link.protocol {
        Protocol::Uas { .. } => ", USB Attached SCSI",
        Protocol::Bot { .. } => "",
        Protocol::Cbi { interrupt: Some(_), .. } => ", Control/Bulk/Interrupt",
        Protocol::Cbi { interrupt: None, .. } => ", Control/Bulk",
    };
    let ident = Some(ident(link, &inquiry));
    let mut added = Lun { number, name, named, inquiry, jobs, medium: None, watch };
    let (vendor, product, revision) = (inquiry.vendor(), inquiry.product(), inquiry.revision());
    if let Some(optical) = optical {
        let (name, place) = (&added.name, &link.place);
        info!(
            "Disk: {name} is a USB optical drive on {place}{lun}: {vendor} {product} \
             {revision}{how}"
        );
        match (&made, found, optical.disc) {
            (Some((_, device)), _, Disc::In(profile, _)) => {
                let sectors = device.info().sectors;
                let disc = optical::described(profile, optical.sessions, sectors);
                info!("Disk: {name}: {disc} is in it");
            }
            (None, Err(Some(Absent::NotData(content))), _) => {
                info!("Disk: {name}: {}", optical::not_data(content));
            }
            (None, Err(Some(why)), _) => {
                info!("Disk: {name}: the disc in it {why}; not used until it is taken out");
            }
            _ => {}
        }
    }
    let Some((medium, device)) = made else {
        let (name, place) = (&added.name, &link.place);
        if optical.is_none() {
            let drive = if link.ufi { "floppy drive" } else { "drive" };
            info!(
                "Disk: {name} is a USB {drive} with no medium in it on {place}{lun}: {vendor} \
                 {product} {revision}{how}"
            );
            if let Err(Some(why)) = found {
                info!("Disk: {name}: the medium in it {why}; not used until it is taken out");
            }
        }
        let empty = added.watch.as_ref().map(|watch| watch.empty.clone());
        let device = empty.ok_or("it has no device")?;
        register(Entry { name: added.name.clone(), device, place: None, ident });
        return Ok(added);
    };
    let found = medium.found;
    let info = device.info();
    if optical.is_none() {
        info!(
            "Disk: {} is a USB {} of {} on {}{lun}: {vendor} {product} {revision}, {} bytes a \
             block{}{}{}{}{how}",
            added.name,
            if link.ufi { "floppy drive with a disk" } else { "disk" },
            Size(info.sectors * SECTOR),
            link.place,
            info.block_size,
            super::sector_note(&info),
            if inquiry.removable { ", removable" } else { "" },
            if found.read_only { ", write-protected" } else { "" },
            if found.write_cache { ", write cache" } else { "" },
        );
    }
    register(Entry { name: added.name.clone(), device: device.clone(), place: None, ident });
    added.medium = Some(medium);
    // A disc has no partition table, as Linux's `sr` reads none, nor a
    // floppy disk.
    if optical.is_none() && !link.ufi {
        let parts = wire.one(|t| read_table(t, number, &found, &added.name, &device));
        for part in parts.iter().flatten() {
            super::add_partition(&added.name, &device, part);
        }
    }
    Ok(added)
}

/// The requests of a logical unit, `count` of them in flight of up to
/// `segments` buffers each.
fn jobs(count: usize, segments: usize) -> Option<Jobs> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut jobs = Vec::new_in(heap);
    jobs.try_reserve_exact(count).ok()?;
    for _ in 0..count {
        let mut frames = Vec::new_in(heap);
        frames.try_reserve_exact(segments).ok()?;
        let job = Job { stage: Stage::Idle, op: Op::Read, sector: 0, frames };
        let _ = jobs.push_within_capacity(job);
    }
    let mut why = Vec::new_in(heap);
    why.try_reserve_exact(SLOTS).ok()?;
    while why.push_within_capacity(None).is_ok() {}
    Some(Jobs { jobs, why, gone: false })
}

/// The queue of the medium `found` in a logical unit of the stick `link`,
/// the disk `name`, with `count` requests in flight, its volume read from
/// `last_session` (where a disc's last session starts, in sectors; 0 on
/// anything else): the medium as the thread keeps it, and its block
/// device.
fn new_medium(
    link: &LinkRef,
    count: usize,
    found: Found,
    last_session: u64,
    name: &Name,
) -> Result<(Medium, DeviceRef), &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let capacity = found.capacity;
    let unit = u64::from(capacity.block_length) / SECTOR;
    let max_segments = match link.protocol {
        Protocol::Uas { .. } => UAS_SEGMENTS,
        Protocol::Bot { .. } if link.speed.is_super() => SUPER_SEGMENTS,
        Protocol::Bot { .. } | Protocol::Cbi { .. } => USB2_SEGMENTS,
    };
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
    let jobs = jobs(count, max_segments).ok_or("out of memory")?;
    let thread = sched::current().ok_or("it has no thread")?;
    let shared = Shared { jobs: SpinLock::new(jobs), thread, link: link.clone() };
    let shared = Arc::try_new_in(shared, heap).map_err(|_| "out of memory")?;
    let part = queue::part(UsbLun { shared: shared.clone() }).ok_or("out of memory")?;
    let once = core::iter::once(part);
    let disk = Disk::new(once, info, found.write_cache, false, true, name.clone(), link.source)
        .ok_or("out of memory")?;
    let disk = Arc::try_new_in(disk, heap).map_err(|_| "out of memory")?;
    let volume = Volume::new(disk.clone(), 0, info).ok_or("out of memory")?;
    let volume = volume.with_last_session(last_session);
    let device: DeviceRef = Arc::try_new_in(volume, heap).map_err(|_| "out of memory")?;
    let medium =
        Medium { found, shared, disk, start: false, starting: false, not_before: 0, ended: false };
    Ok((medium, device))
}

/// What a USB disk says it is (`inquiry`), as `/kernel/block` shows it:
/// its vendor and product as the model, its revision as the firmware's,
/// and its device's serial number, as Linux's udev gives it
/// (`ID_SERIAL_SHORT`).
fn ident(link: &Link, inquiry: &Inquiry) -> Ident {
    let mut model = [b' '; 40];
    let (vendor, product) = (inquiry.vendor(), inquiry.product());
    let words = [vendor.as_bytes(), b" ", product.as_bytes()];
    let mut at = 0;
    for byte in words.iter().flat_map(|word| word.iter()) {
        if at < model.len() && !(at == 0 && *byte == b' ') {
            model[at] = *byte;
            at += 1;
        }
    }
    let mut firmware = [b' '; 8];
    let revision = inquiry.revision().as_bytes();
    let n = revision.len().min(firmware.len());
    firmware[..n].copy_from_slice(&revision[..n]);
    Ident { model, serial: super::padded(link.serial.as_str().as_bytes()), firmware }
}

/// The partitions of the medium `found` in the logical unit `number`, the
/// disk `name` whose block device is `device`, as its table has them,
/// logged; `None` if it has none. The table is read through `transport`
/// itself, since the thread that would serve the disk's queue is this
/// one.
fn read_table(
    transport: &mut dyn Transport,
    number: u8,
    found: &Found,
    name: &Name,
    device: &DeviceRef,
) -> Option<KVec<partition::Partition>> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let block = found.capacity.block_length as usize;
    let mut buffer = Vec::new_in(heap);
    if buffer.try_reserve_exact(block).is_err() {
        warn!("Disk: {name}: out of memory for its partition table");
        return None;
    }
    while buffer.len() < block && buffer.push_within_capacity(0u8).is_ok() {}
    let unit = (block / SECTOR as usize) as u64;
    let mut read = |sector: u64, out: &mut [u8; SECTOR as usize]| {
        let cdb = Cdb::read(sector / unit, 1, found.sixteen).ok_or(partition::Error::Io)?;
        for _ in 0..TRIES {
            let data = Data::In(&mut buffer);
            match transport.command(number, &cdb, data, TIMEOUT_MS) {
                Ok(Reply::Good { residue: 0 }) => {
                    let at = ((sector % unit) * SECTOR) as usize;
                    out.copy_from_slice(&buffer[at..at + SECTOR as usize]);
                    return Ok(());
                }
                Err(TransportError::Gone) => break,
                _ => {}
            }
        }
        Err(partition::Error::Io)
    };
    super::table(name, device, &mut read)
}
