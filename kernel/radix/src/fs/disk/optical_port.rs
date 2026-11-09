// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An optical drive's AHCI port, as the drive's thread, its disc's queue
//! and the port's interrupt share it (ADR 0434).
//!
//! A disc's reads go out as Linux's libata sends them: from the thread
//! that reads, as READ(10) of the queue's buffers by DMA ([`DiscReads`]),
//! and they end in the port's interrupt ([`OpticalPort::interrupt`]),
//! which hands each read back to the queue and starts the next one
//! waiting. The drive takes one command at a time, so reads wait their
//! turn in the order they came, and a large read's commands follow one
//! another with no thread woken between them.
//!
//! The drive's thread ([`super::optical`]) still does all that takes long
//! or sleeps. For its own commands (its look every two seconds, the
//! disc's lock, an eject) it takes the port out once the read on it is
//! over ([`OpticalPort::take`]); reads that come meanwhile wait, and the
//! first goes when it puts the port back. A read the drive fails, or that
//! goes unanswered for the queue's 30 s, is the thread's to look into
//! ([`OpticalPort::settle`]): it restarts the port, asks the drive why
//! (REQUEST SENSE) or resets the link, and hands the read back judged as
//! one the thread carried out is.
//!
//! When it breaks: a read stuck shows as its reader's wait on `disk` in
//! kdb's thread list, and the drive's thread as its wait on `optical`;
//! the log names each read that failed and why (`Disk: cd0: ...`).

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use radix_ahci::ata::PacketIdentity;
use radix_ahci::{Hba, MAX_SEGMENTS, PacketEnd, Port};
use radix_block::{Error, SECTOR};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::{Cdb, Data, Reply, TransportError};
use radix_sync::{Once, SpinLock};

use super::optical::{READY_NS, STOPPED, Then, judge, wait_us};
use super::queue::{self, Controller, Disk, Op, Outcome, PARTS, SLOTS};
use crate::sched::{self, ThreadRef};

/// How long the thread sleeps at most while it waits for the port, before
/// it looks again.
const TAKE_NS: u64 = 1_000_000_000;

/// Where the read on the port is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The drive has it.
    Running,
    /// An error stopped the port, with this status: the thread finds out
    /// why.
    Stopped(u32),
    /// It went unanswered for the queue's 30 s: the thread resets the
    /// link.
    Unanswered,
    /// The thread found that it ended so: its queue takes it back.
    Judged(Outcome, Option<&'static str>),
}

/// The read on the port.
#[derive(Clone, Copy, Debug)]
struct Read {
    /// Its queue's slot.
    slot: u8,
    /// How many bytes it moves.
    bytes: u32,
    stage: Stage,
}

/// What the port's users share, under one lock.
struct State {
    /// The port, but while the thread has it out for its own commands.
    port: Option<Port>,
    /// The disc's read on the port, if one is.
    read: Option<Read>,
    /// The thread waits for the port: no read starts.
    wanted: bool,
    /// The disc's queue, for the interrupt to hand its reads back to.
    disc: Option<Arc<Disk<DiscReads>, Kmalloc>>,
}

impl State {
    /// Puts back the port the thread had out. The interrupt handler may
    /// have turned its interrupt off meanwhile, which only the
    /// controller saw ([`Hba::quiet`]): the next read turns it on again.
    fn put_back(&mut self, mut port: Port) {
        port.forget_interrupts();
        self.port = Some(port);
    }
}

/// An optical drive's AHCI port, shared by the drive's thread, its disc's
/// queue and the port's interrupt.
pub struct OpticalPort {
    state: SpinLock<State>,
    /// What the drive said of itself in IDENTIFY PACKET DEVICE.
    identity: PacketIdentity,
    /// Whether the controller raises interrupts; without them, those who
    /// wait for a read look at the port themselves.
    interrupt: bool,
    /// The drive's thread, set once it is started.
    thread: Once<ThreadRef>,
    /// Interrupt handlers handing reads back to the disc's queue now: the
    /// thread lets go of a disc's queue only once none does, so that the
    /// last of it is never let go in an interrupt.
    collecting: AtomicU32,
    /// A read found that the disc was taken out or changed: the thread
    /// looks at the drive.
    changed: AtomicBool,
}

impl OpticalPort {
    /// The port of the optical drive on `port`, of a controller that
    /// raises interrupts if `interrupt`; `None` when memory runs out or
    /// the port has no optical drive.
    pub fn new(port: Port, interrupt: bool) -> Option<Arc<OpticalPort, Kmalloc>> {
        let identity = *port.packet_identity()?;
        let state = State { port: Some(port), read: None, wanted: false, disc: None };
        let shared = OpticalPort {
            state: SpinLock::new(state),
            identity,
            interrupt,
            thread: Once::new(),
            collecting: AtomicU32::new(0),
            changed: AtomicBool::new(false),
        };
        Arc::try_new_in(shared, HEAP.with(Flags::MAY_SLEEP)).ok()
    }

    /// The drive's thread is `thread`.
    pub fn set_thread(&self, thread: ThreadRef) {
        let _ = self.thread.set(thread);
    }

    /// What the drive said of itself.
    pub fn identity(&self) -> &PacketIdentity {
        &self.identity
    }

    /// Whether the drive stopped answering for good.
    pub fn dead(&self) -> bool {
        self.state.lock_irq().port.as_ref().is_some_and(Port::dead)
    }

    /// Whether a read found the disc taken out or changed since this was
    /// last asked.
    pub fn disc_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    fn wake_thread(&self) {
        if let Some(thread) = self.thread.get() {
            sched::wake(thread);
        }
    }

    /// The port's interrupt, `number` on `hba`: hands the read that ended
    /// back to its disc's queue, which starts the next. Otherwise the
    /// thread has the port, or a read is the thread's to look into, or
    /// the port has news for the thread: the port's interrupt goes off
    /// until its next command, and the thread is woken. An interrupt for
    /// a read already handed back finds nothing to say and does nothing.
    pub fn interrupt(&self, hba: &Hba, number: usize) {
        let disc = {
            let mut guard = self.state.lock_irq();
            let state = &mut *guard;
            let running = state.read.is_some_and(|read| read.stage == Stage::Running);
            match (&mut state.port, &state.disc) {
                (Some(_), Some(disc)) if running => {
                    self.collecting.fetch_add(1, Ordering::AcqRel);
                    Some(disc.clone())
                }
                (Some(port), _) if state.read.is_none() && !port.status_set() => return,
                (Some(port), _) => {
                    port.quiet();
                    None
                }
                (None, _) => {
                    hba.quiet(number);
                    None
                }
            }
        };
        match disc {
            Some(disc) => {
                disc.collect(0);
                drop(disc);
                self.collecting.fetch_sub(1, Ordering::AcqRel);
            }
            None => self.wake_thread(),
        }
    }

    /// The disc of data `disc` is in the drive: the interrupt hands its
    /// reads back to it.
    pub fn set_disc(&self, disc: Arc<Disk<DiscReads>, Kmalloc>) {
        self.state.lock_irq().disc = Some(disc);
    }

    /// The disc in the drive is gone, and no read of it starts: once the
    /// read on the port is over and handed back, the interrupt no longer
    /// hands reads back to the disc's queue, which the port lets go of once
    /// no interrupt handler has it. Sleeps.
    pub fn forget_disc(&self) {
        let port = self.take();
        let disc = {
            let mut state = self.state.lock_irq();
            state.put_back(port);
            state.disc.take()
        };
        while self.collecting.load(Ordering::Acquire) != 0 {
            core::hint::spin_loop();
        }
        drop(disc);
    }

    /// Takes the port out for the thread's own commands, once the read on
    /// it is over: the reads that come meanwhile wait. Looks into a read
    /// in trouble while it waits ([`OpticalPort::settle`]). Sleeps.
    pub fn take(&self) -> Port {
        loop {
            {
                let mut state = self.state.lock_irq();
                if state.read.is_none()
                    && let Some(port) = state.port.take()
                {
                    state.wanted = false;
                    return port;
                }
                state.wanted = true;
            }
            if !self.settle() {
                // The read's end wakes this thread, as it sees it wanted.
                sched::block_until(sched::now_ns().saturating_add(TAKE_NS), "optical");
            }
        }
    }

    /// Puts back the port [`OpticalPort::take`] took out: the first read
    /// that came meanwhile goes.
    pub fn put_back(&self, port: Port) {
        let disc = {
            let mut state = self.state.lock_irq();
            state.put_back(port);
            state.disc.clone()
        };
        if let Some(disc) = disc {
            disc.collect(0);
        }
    }

    /// Looks into the read in trouble on the port, if one is: restarts the
    /// port and asks the drive why it failed it, or resets the link of one
    /// unanswered; a drive becoming ready is given a second. Hands the read
    /// back judged as [`judge`] has a read the thread carried out, and
    /// notes a disc taken out or changed ([`OpticalPort::disc_changed`]).
    /// Returns whether one was. Sleeps.
    pub fn settle(&self) -> bool {
        let (mut port, read) = {
            let mut state = self.state.lock_irq();
            let Some(read) = state.read else { return false };
            if !matches!(read.stage, Stage::Stopped(_) | Stage::Unanswered) {
                return false;
            }
            let Some(port) = state.port.take() else { return false };
            (port, read)
        };
        let mut wait = wait_us;
        let reply = match read.stage {
            Stage::Stopped(status) => {
                port.packet_reply(PacketEnd::Stopped(status), read.bytes, &mut wait)
            }
            _ => Err(port.packet_timed_out(&mut wait)),
        };
        let (outcome, why, then) = judge(reply);
        match then {
            Then::Nothing => {}
            Then::Ready => sched::sleep_ns(READY_NS),
            Then::Changed => self.changed.store(true, Ordering::Release),
        }
        let disc = {
            let mut state = self.state.lock_irq();
            state.put_back(port);
            state.read = Some(Read { stage: Stage::Judged(outcome, why), ..read });
            state.disc.clone()
        };
        if let Some(disc) = disc {
            disc.collect(0);
        }
        true
    }

    /// Whether the controller raises interrupts for the disc's queue.
    pub fn has_interrupt(&self) -> bool {
        self.interrupt
    }
}

/// Why a disc's reads fail at once, once it is gone.
pub struct Gone(SpinLock<Option<&'static str>>);

impl Gone {
    /// The disc is gone, `why`.
    pub fn set(&self, why: &'static str) {
        *self.0.lock_irq() = Some(why);
    }

    fn get(&self) -> Option<&'static str> {
        *self.0.lock_irq()
    }
}

/// A read waiting for the port.
#[derive(Clone, Copy, Debug)]
struct Waiting {
    slot: u8,
    /// Its first block, and how many.
    block: u32,
    blocks: u32,
    /// How many buffers it has, in its place in [`DiscReads::frames`].
    frames: usize,
}

/// The reads waiting for the port, oldest first: a ring of [`PARTS`]
/// places.
struct Ring {
    places: [Waiting; PARTS],
    first: usize,
    count: usize,
}

impl Ring {
    fn new() -> Ring {
        let empty = Waiting { slot: 0, block: 0, blocks: 0, frames: 0 };
        Ring { places: [empty; PARTS], first: 0, count: 0 }
    }

    /// Whether every place is taken.
    fn full(&self) -> bool {
        self.count == PARTS
    }

    /// The place the next read waiting goes in.
    fn next_place(&self) -> usize {
        (self.first + self.count) % PARTS
    }

    /// Puts `read` in the next place; the caller made sure one is free.
    fn push(&mut self, read: Waiting) {
        self.places[self.next_place()] = read;
        self.count += 1;
    }

    /// The oldest read waiting, taken out: its place, and it.
    fn pop(&mut self) -> Option<(usize, Waiting)> {
        if self.count == 0 {
            return None;
        }
        let place = self.first;
        self.first = (self.first + 1) % PARTS;
        self.count -= 1;
        Some((place, self.places[place]))
    }
}

/// A disc's reads, as its queue drives them: sent from the thread that
/// reads, or from the port's interrupt when the read before ends.
pub struct DiscReads {
    port: Arc<OpticalPort, Kmalloc>,
    gone: Arc<Gone, Kmalloc>,
    /// Its blocks, in sectors of 512 bytes.
    unit: u64,
    /// The most buffers of one read.
    segments: usize,
    /// The reads waiting for the port.
    waiting: Ring,
    /// The buffers of the read in each place of the ring, `segments` a
    /// place.
    frames: Vec<(u64, u32), Kmalloc>,
    /// Whether one of this disc's reads is on the port.
    on_port: bool,
    /// Slots of reads the port refused, a bit each: handed back failed.
    refused: u64,
    /// Why the last read in each of the queue's slots failed or went
    /// again, for the log: [`SLOTS`] of them, on the heap.
    why: Vec<Option<&'static str>, Kmalloc>,
}

impl DiscReads {
    /// The reads of a disc in the drive on `port`, by DMA in blocks of
    /// `block` bytes, in requests of up to `segments` buffers; and what
    /// the drive's thread sets when the disc is gone.
    pub fn new(
        port: &Arc<OpticalPort, Kmalloc>,
        block: u32,
        segments: usize,
    ) -> Option<(DiscReads, Arc<Gone, Kmalloc>)> {
        let heap = HEAP.with(Flags::MAY_SLEEP);
        let segments = segments.clamp(1, MAX_SEGMENTS);
        let mut frames = Vec::new_in(heap);
        frames.try_reserve_exact(PARTS * segments).ok()?;
        while frames.len() < PARTS * segments {
            let _ = frames.push_within_capacity((0, 0));
        }
        let mut why = Vec::new_in(heap);
        why.try_reserve_exact(SLOTS).ok()?;
        while why.len() < SLOTS {
            let _ = why.push_within_capacity(None);
        }
        let gone = Arc::try_new_in(Gone(SpinLock::new(None)), heap).ok()?;
        let reads = DiscReads {
            port: port.clone(),
            gone: gone.clone(),
            unit: u64::from(block) / SECTOR,
            segments,
            waiting: Ring::new(),
            frames,
            on_port: false,
            refused: 0,
            why,
        };
        Some((reads, gone))
    }

    /// Sends the oldest read waiting, if the port is free: from the thread
    /// that reads, or from the interrupt as the read before ends. One the
    /// port refuses fails at once.
    fn send_next(&mut self) {
        if self.on_port || self.waiting.count == 0 || self.gone.get().is_some() {
            return;
        }
        let mut state = self.port.state.lock_irq();
        if state.read.is_some() || state.wanted || state.port.is_none() {
            return;
        }
        while let Some((place, next)) = self.waiting.pop() {
            let Some(port) = state.port.as_mut() else { return };
            let at = place * self.segments;
            let frames = &self.frames[at..at + next.frames];
            let bytes: u32 = frames.iter().map(|&(_, len)| len).sum();
            let data = Data::Frames { into_host: true, frames };
            let sent = match Cdb::read(u64::from(next.block), next.blocks, false) {
                Some(cdb) => port.send_packet(&cdb, &data),
                None => Err(TransportError::Failed("a read past what a command reaches")),
            };
            match sent {
                Ok(()) => {
                    state.read = Some(Read { slot: next.slot, bytes, stage: Stage::Running });
                    self.on_port = true;
                    return;
                }
                Err(err) => {
                    let why = match err {
                        TransportError::Gone => STOPPED,
                        TransportError::Failed(why) => why,
                    };
                    self.why[usize::from(next.slot)] = Some(why);
                    self.refused |= 1 << next.slot;
                }
            }
        }
    }

    /// The read of this disc's on the port, if it ended: its slot, how,
    /// and why. Hands one in trouble to the thread.
    fn ended(&mut self) -> Option<(u8, Outcome, Option<&'static str>)> {
        if !self.on_port {
            return None;
        }
        let mut state = self.port.state.lock_irq();
        let read = state.read?;
        let (outcome, why) = match read.stage {
            Stage::Running => {
                let port = state.port.as_mut()?;
                match port.packet_ended()? {
                    PacketEnd::Done(moved) => {
                        let residue = read.bytes - moved.min(read.bytes);
                        let (outcome, why, _) = judge(Ok(Reply::Good { residue }));
                        (outcome, why)
                    }
                    PacketEnd::Stopped(status) => {
                        state.read = Some(Read { stage: Stage::Stopped(status), ..read });
                        drop(state);
                        self.port.wake_thread();
                        return None;
                    }
                }
            }
            Stage::Judged(outcome, why) => (outcome, why),
            Stage::Stopped(_) | Stage::Unanswered => return None,
        };
        state.read = None;
        let wanted = state.wanted;
        drop(state);
        self.on_port = false;
        if wanted {
            self.port.wake_thread();
        }
        Some((read.slot, outcome, why))
    }
}

impl Controller for DiscReads {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        // A gone disc takes every read, to fail it.
        self.gone.get().is_some() || !self.waiting.full()
    }

    fn submit(&mut self, slot: u8, request: &queue::Request<'_>, _more: bool) -> Result<(), Error> {
        if self.gone.get().is_some() {
            return Err(Error::Io);
        }
        if request.op != Op::Read {
            return Err(Error::ReadOnly);
        }
        let total: u64 = request.data.iter().map(|&(_, len)| u64::from(len)).sum();
        let block_bytes = self.unit * SECTOR;
        let whole = total.is_multiple_of(block_bytes) && request.sector.is_multiple_of(self.unit);
        let block = u32::try_from(request.sector / self.unit).ok();
        let blocks = u32::try_from(total / block_bytes).ok();
        let (Some(block), Some(blocks)) = (block, blocks) else { return Err(Error::OutOfRange) };
        if !whole || request.data.len() > self.segments || self.waiting.full() {
            return Err(Error::OutOfRange);
        }
        let at = self.waiting.next_place() * self.segments;
        self.frames[at..at + request.data.len()].copy_from_slice(request.data);
        self.waiting.push(Waiting { slot, block, blocks, frames: request.data.len() });
        self.send_next();
        Ok(())
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        if self.refused != 0 {
            let slot = self.refused.trailing_zeros() as u8;
            self.refused &= !(1 << slot);
            return Some((slot, Outcome::Failed));
        }
        if let Some((slot, outcome, why)) = self.ended() {
            self.why[usize::from(slot)] = why;
            self.send_next();
            return Some((slot, outcome));
        }
        if let Some(why) = self.gone.get() {
            // The read on the port ends as the drive has it; those waiting
            // fail now.
            let (_, read) = self.waiting.pop()?;
            let slot = read.slot;
            self.why[usize::from(slot)] = Some(why);
            return Some((slot, Outcome::Failed));
        }
        // The port came back from the thread: the first read waiting goes.
        self.send_next();
        None
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        self.why.get(usize::from(slot)).copied().flatten()
    }

    fn reset(lanes: &mut [queue::Part<DiscReads>]) -> bool {
        // The read on the port went unanswered: the drive's thread resets
        // the link, and hands it back.
        for lane in lanes.iter() {
            if !lane.on_port {
                continue;
            }
            let mut state = lane.port.state.lock_irq();
            if let Some(read) = state.read.as_mut()
                && read.stage == Stage::Running
            {
                read.stage = Stage::Unanswered;
                drop(state);
                lane.port.wake_thread();
            }
        }
        true
    }
}
