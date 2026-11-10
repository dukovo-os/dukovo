// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Disks whose requests their driver's thread carries out, one at a time:
//! the disc in an optical drive on an IDE channel (ADR 0432), which
//! borrows the channel for each command, and the disk in a floppy drive
//! (ADR 0435). Such a drive takes one command at a time, and some take
//! seconds, so the thread that talks to it does all of it.
//!
//! The disk's queue hands a request to the thread ([`Lane`]), which
//! takes it ([`Medium::take`]), carries it out and hands it back with
//! how it ended ([`Medium::finish`]). When the medium is gone, taken out
//! or its drive stopped, every request fails at once ([`Medium::gone`]).
//! A request stuck here shows as the thread's: `kdb`'s thread list shows
//! what it waits on.

use alloc::sync::Arc;
use alloc::vec::Vec;

use radix_block::{DeviceRef, Error, Info};
use radix_hw::pci::Address;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::SpinLock;

use super::Name;
use super::queue::{self, Controller, Disk, Op, Outcome, SLOTS, Volume};
use crate::sched::{self, ThreadRef};

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

/// A medium's request, between its queue and the drive's thread.
struct Job {
    stage: Stage,
    op: Op,
    /// Its first sector, of 512 bytes.
    sector: u64,
    /// Its buffers (physical address, length).
    frames: Vec<(u64, u32), Kmalloc>,
    /// Why the last request in each of the queue's slots failed or went
    /// again, for the log.
    why: [Option<&'static str>; SLOTS],
    /// The medium is gone, why: requests fail at once.
    gone: Option<&'static str>,
}

/// What a medium's queue shares with the drive's thread.
struct Shared {
    job: SpinLock<Job>,
    /// The drive's thread, which a request wakes.
    thread: ThreadRef,
}

/// A medium as its queue drives it: the queue's requests go to the
/// drive's thread.
pub struct Lane {
    shared: Arc<Shared, Kmalloc>,
}

impl Controller for Lane {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        // A gone medium takes every request, to fail it.
        let job = self.shared.job.lock_irq();
        job.gone.is_some() || job.stage == Stage::Idle
    }

    fn submit(&mut self, slot: u8, request: &queue::Request<'_>, _more: bool) -> Result<(), Error> {
        {
            let mut job = self.shared.job.lock_irq();
            if job.gone.is_some() {
                return Err(Error::Io);
            }
            if job.stage != Stage::Idle || request.data.len() > job.frames.capacity() {
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
        let mut job = self.shared.job.lock_irq();
        let Stage::Done(slot, outcome) = job.stage else { return None };
        job.stage = Stage::Idle;
        Some((slot, outcome))
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        self.shared.job.lock_irq().why.get(usize::from(slot)).copied().flatten()
    }

    fn reset(_lanes: &mut [queue::Part<Lane>]) -> bool {
        // The drive's thread gives each request its own time, and resets
        // the drive's link itself when one goes unanswered.
        true
    }
}

/// A medium in a drive, as the drive's thread keeps it: its queue.
pub struct Medium {
    shared: Arc<Shared, Kmalloc>,
    disk: Arc<Disk<Lane>, Kmalloc>,
}

impl Medium {
    /// The queue of a medium that `info` says what it is of, in the drive
    /// `name` of the controller at `source`, whose thread is the one
    /// calling, and its block device: read from `last_session`, a disc's
    /// last session's first sector (0 for any other medium). Requests of
    /// up to `info.max_segments` buffers. Kept out of the caller's frame,
    /// as the queue is built on the stack before it moves.
    #[inline(never)]
    pub fn new(
        name: &Name,
        source: Address,
        info: Info,
        last_session: u64,
    ) -> Result<(Medium, DeviceRef), &'static str> {
        let heap = HEAP.with(Flags::MAY_SLEEP);
        let mut frames = Vec::new_in(heap);
        frames.try_reserve_exact(info.max_segments).map_err(|_| "out of memory")?;
        let job = Job {
            stage: Stage::Idle,
            op: Op::Read,
            sector: 0,
            frames,
            why: [None; SLOTS],
            gone: None,
        };
        let thread = sched::current().ok_or("it has no thread")?;
        let shared = Shared { job: SpinLock::new(job), thread };
        let shared = Arc::try_new_in(shared, heap).map_err(|_| "out of memory")?;
        let part = queue::part(Lane { shared: shared.clone() }).ok_or("out of memory")?;
        let once = core::iter::once(part);
        let disk = Disk::new(once, info, false, false, true, name.clone(), source)
            .ok_or("out of memory")?;
        let disk = Arc::try_new_in(disk, heap).map_err(|_| "out of memory")?;
        let volume = Volume::new(disk.clone(), 0, info).ok_or("out of memory")?;
        let volume = volume.with_last_session(last_session);
        let device: DeviceRef = Arc::try_new_in(volume, heap).map_err(|_| "out of memory")?;
        Ok((Medium { shared, disk }, device))
    }

    /// Takes the request waiting, if one is: what it does and its first
    /// sector, with its buffers into `frames`, which has room for as many
    /// as the medium's requests have.
    pub fn take(&self, frames: &mut Vec<(u64, u32), Kmalloc>) -> Option<(Op, u64)> {
        let mut job = self.shared.job.lock_irq();
        let Stage::Waiting(slot) = job.stage else { return None };
        job.stage = Stage::Running(slot);
        frames.clear();
        for &frame in &job.frames {
            let _ = frames.push_within_capacity(frame);
        }
        Some((job.op, job.sector))
    }

    /// Hands back the request running, which ended with `outcome`, `why`,
    /// and tells its queue.
    pub fn finish(&self, outcome: Outcome, why: Option<&'static str>) {
        {
            let mut job = self.shared.job.lock_irq();
            let Stage::Running(slot) = job.stage else { return };
            job.stage = Stage::Done(slot, outcome);
            if let Some(entry) = job.why.get_mut(usize::from(slot)) {
                *entry = why;
            }
        }
        self.disk.collect(0);
    }

    /// The medium is gone, `why`: fails the request it has, and every one
    /// after.
    pub fn gone(&self, why: &'static str) {
        {
            let mut job = self.shared.job.lock_irq();
            job.gone = Some(why);
            if let Stage::Waiting(slot) | Stage::Running(slot) = job.stage {
                job.stage = Stage::Done(slot, Outcome::Failed);
                if let Some(entry) = job.why.get_mut(usize::from(slot)) {
                    *entry = Some(why);
                }
            }
        }
        self.disk.collect(0);
    }
}
