// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The output stream: a cyclic buffer the controller plays round and
//! round, cut into slots of a period each (section 3.6).
//!
//! The kernel hands the driver a period at a time and wants each back
//! once it has played. An HDA stream has no periods to hand back: its
//! DMA engine plays the whole buffer, round and round, as long as it
//! runs, and says only how far it is (SD_LPIB, the link position in the
//! buffer). So the driver keeps the periods' book itself. Each slot of the
//! buffer is one entry of the buffer descriptor list; a period is written
//! into the next slot ahead of the DMA engine, and once the link position
//! is past the slot's end the period has played, and the slot is filled
//! with silence again so that the next time round it plays nothing. What
//! the engine plays when no period waits is that silence.
//!
//! Positions are counted in bytes from where the stream was set up, never
//! wrapping ([`Stream::position`]); slot `k` of that count is slot
//! `k % slots` of the buffer. The engine reads ahead of the link position
//! by up to its FIFO (SD_FIFOS), so a period goes only into a slot that
//! starts at least that far, and two milliseconds more, ahead of it.
//!
//! Interrupts on completion are left off (no IOC bit in the list): the
//! kernel's worker looks at the position every 20 ms while the stream
//! plays. QEMU's controller holds a stream's transfers while its
//! completion bit is set, so an interrupt a period would stall the stream
//! whenever the worker was late to clear it. So that a look that comes
//! late cannot find the engine a whole lap on, the buffer holds at least
//! half a second, in more slots than the kernel's periods if they are
//! short; the slots beyond the periods only ever hold silence.
//!
//! Stopping the engine and starting it again does not resume where it
//! was on every controller (QEMU's starts the list from its first entry,
//! as its reset does), so each start resets the stream and writes the
//! list afresh beginning at the next slot to play: the slots, and the
//! periods in them, stay where they are in memory.

use radix_hw::{Dma, Mmio, device_barrier};

use crate::controller::StreamSlot;
use crate::regs::*;
use crate::{Error, Host};

/// The shortest the buffer is, in milliseconds of sound.
const MIN_BUFFER_MS: u32 = 500;

/// How long the link position may stand still while the stream runs
/// before the stream is taken to have stopped for good.
const STALL_NS: u64 = 3_000_000_000;

/// How many times, 10 microseconds apart, the driver looks for a stream
/// to stop or leave its reset: 10 ms.
const STREAM_LOOKS: u32 = 1000;

/// The tag the output's stream carries on the link, which its converters
/// are set to take (1 to 15; one stream plays).
pub const TAG: u8 = 1;

/// An output stream set up for a format and its periods.
pub struct Stream {
    slot: StreamSlot,
    /// The format word (SD_FMT and the converters').
    format: u16,
    /// The bytes of a period and of a slot.
    bytes: u32,
    /// The slots of the buffer.
    slots: u32,
    /// The most periods the stream holds at once.
    count: u32,
    /// The bytes the engine may read ahead of the link position, and a
    /// little more.
    guard: u32,
    /// The buffer, `slots` slots of `bytes`.
    buffer: Dma,
    /// The buffer descriptor list.
    list: Dma,
    /// Which slots of the buffer hold a period not yet played.
    filled: [u64; 4],
    /// How many do.
    pending: u32,
    /// The first slot that may hold a period not yet played.
    tail: u64,
    /// The slot after the last period written.
    head: u64,
    /// How far the link has played, in bytes from the setup.
    position: u64,
    /// SD_LPIB when last read.
    last_lpib: u32,
    /// When the link position last moved.
    moved_ns: u64,
    running: bool,
    stalled: bool,
}

impl Stream {
    /// Sets up `slot` of the controller at `mmio` to play `format` (a format word) in periods of
    /// `bytes`, at most `count` at once, `bytes_per_second` of them,
    /// with memory from `host`, below 4 GiB unless `wide`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mmio: &Mmio,
        slot: StreamSlot,
        format: u16,
        bytes: u32,
        count: u32,
        bytes_per_second: u32,
        wide: bool,
        host: &mut dyn Host,
    ) -> Result<Stream, Error> {
        // Each slot is an entry of the list, whose buffers start on 128
        // bytes.
        if bytes == 0 || !bytes.is_multiple_of(128) || count == 0 {
            return Err(Error::BadFormat);
        }
        let shortest = (u64::from(bytes_per_second) * u64::from(MIN_BUFFER_MS) / 1000) as u32;
        let slots = (count + 2).max(shortest.div_ceil(bytes)).min(BDL_MAX as u32);
        if slots < count + 2 {
            return Err(Error::BadFormat);
        }
        let total = slots as usize * bytes as usize;
        let buffer = host.alloc(total.div_ceil(4096), !wide).ok_or(Error::NoMemory)?;
        let list = host.alloc(1, !wide).ok_or(Error::NoMemory)?;
        let mut stream = Stream {
            slot,
            format,
            bytes,
            slots,
            count,
            guard: 0,
            buffer,
            list,
            filled: [0; 4],
            pending: 0,
            tail: 0,
            head: 0,
            position: 0,
            last_lpib: 0,
            moved_ns: 0,
            running: false,
            stalled: false,
        };
        stream.reset(mmio, host);
        // What the engine reads ahead, once it knows the format.
        mmio.write16(slot.offset + SD_FMT, format);
        let fifo = u32::from(mmio.read16(slot.offset + SD_FIFOS)) + 1;
        stream.guard = fifo.max(64) + bytes_per_second / 500;
        Ok(stream)
    }

    /// The slots of the buffer.
    pub fn slots(&self) -> u32 {
        self.slots
    }

    /// How far the link has played, in bytes since the setup.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Whether the stream plays.
    pub fn running(&self) -> bool {
        self.running
    }

    /// Whether the link position stood still for [`STALL_NS`] while the
    /// stream ran.
    pub fn stalled(&self) -> bool {
        self.stalled
    }

    /// Periods written and not yet played.
    pub fn in_flight(&self) -> u32 {
        self.pending
    }

    fn is_filled(&self, slot: u64) -> bool {
        let index = (slot % u64::from(self.slots)) as usize;
        self.filled[index / 64] & (1 << (index % 64)) != 0
    }

    fn set_filled(&mut self, slot: u64, filled: bool) {
        let index = (slot % u64::from(self.slots)) as usize;
        if filled {
            self.filled[index / 64] |= 1 << (index % 64);
        } else {
            self.filled[index / 64] &= !(1 << (index % 64));
        }
    }

    /// Where slot `slot` starts in the buffer.
    fn offset_of(&self, slot: u64) -> usize {
        (slot % u64::from(self.slots)) as usize * self.bytes as usize
    }

    /// The slot the next period goes into: the one after the last
    /// written, or, if the engine has reached it, the first one far
    /// enough ahead of the engine.
    fn next_slot(&self) -> u64 {
        let bytes = u64::from(self.bytes);
        let ahead = if self.running { u64::from(self.guard) } else { 0 };
        self.head.max((self.position + ahead).div_ceil(bytes))
    }

    /// Whether a period can be written now: fewer than the most are in
    /// flight, and the next slot is not one the engine is about to read
    /// on its next lap.
    pub fn can_submit(&self) -> bool {
        if self.pending >= self.count || self.stalled {
            return false;
        }
        let next = self.next_slot();
        let end = (next + 1) * u64::from(self.bytes);
        let lap = u64::from(self.slots) * u64::from(self.bytes);
        end <= self.position + lap && !self.is_filled(next)
    }

    /// Writes a period of `samples` (at most a slot; the rest of the slot
    /// is silence) into the next slot. False if it cannot be written now.
    pub fn submit(&mut self, samples: &[u8]) -> bool {
        if samples.len() > self.bytes as usize || !self.can_submit() {
            return false;
        }
        let next = self.next_slot();
        let at = self.offset_of(next);
        self.buffer.write(at, samples);
        if samples.len() < self.bytes as usize {
            for offset in samples.len()..self.bytes as usize {
                self.buffer.write8(at + offset, 0);
            }
        }
        device_barrier();
        if self.pending == 0 {
            self.tail = next;
        }
        self.set_filled(next, true);
        self.pending += 1;
        self.head = next + 1;
        true
    }

    /// Reads the link position, at `now_ns`.
    fn update(&mut self, mmio: &Mmio, now_ns: u64) {
        if !self.running {
            return;
        }
        let lap = self.slots * self.bytes;
        let lpib = mmio.read32(self.slot.offset + SD_LPIB);
        if lpib >= lap {
            // All ones, from a controller that is gone, or nonsense.
            self.note_still(now_ns);
            return;
        }
        let forward = (lpib + lap - self.last_lpib) % lap;
        let backward = (self.last_lpib + lap - lpib) % lap;
        // A position a little behind the last one is the link position's
        // jitter, not most of a lap: the worker looks many times a lap.
        if forward == 0 || backward < lap / 8 {
            self.note_still(now_ns);
            return;
        }
        self.position += u64::from(forward);
        self.last_lpib = lpib;
        self.moved_ns = now_ns;
    }

    fn note_still(&mut self, now_ns: u64) {
        if now_ns.saturating_sub(self.moved_ns) > STALL_NS {
            self.stalled = true;
        }
    }

    /// The next period that has played, if one has, at `now_ns`: its slot
    /// is silence again. Some(true) for each.
    pub fn collect(&mut self, mmio: &Mmio, now_ns: u64) -> Option<bool> {
        self.update(mmio, now_ns);
        let bytes = u64::from(self.bytes);
        while self.pending > 0 && self.tail < self.head {
            let slot = self.tail;
            if (slot + 1) * bytes > self.position {
                return None;
            }
            self.tail += 1;
            if self.is_filled(slot) {
                self.silence(slot);
                self.set_filled(slot, false);
                self.pending -= 1;
                return Some(true);
            }
        }
        None
    }

    /// Fills slot `slot` with silence.
    fn silence(&self, slot: u64) {
        let at = self.offset_of(slot);
        for offset in (0..self.bytes as usize).step_by(8) {
            self.buffer.write64(at + offset, 0);
        }
    }

    /// Starts the stream at `now_ns`, from the next slot to play: resets
    /// it, writes its list from that slot round, and sets it running.
    pub fn start(&mut self, mmio: &Mmio, host: &mut dyn Host, now_ns: u64) -> bool {
        if self.running {
            return true;
        }
        if self.stalled {
            return false;
        }
        let bytes = u64::from(self.bytes);
        let first = self.position.div_ceil(bytes);
        self.position = first * bytes;
        self.head = self.head.max(first);
        if self.pending == 0 {
            self.tail = self.head;
        }
        for entry in 0..u64::from(self.slots) {
            let at = entry as usize * BDL_ENTRY;
            let address = self.buffer.phys() + self.offset_of(first + entry) as u64;
            self.list.write64(at, address);
            self.list.write32(at + 8, self.bytes);
            self.list.write32(at + 12, 0);
        }
        device_barrier();
        self.reset(mmio, host);
        let offset = self.slot.offset;
        let mut control = u32::from(TAG) << SD_CTL_STREAM_SHIFT;
        if self.slot.bidirectional {
            control |= SD_CTL_DIR_OUT;
        }
        mmio.write32(offset + SD_CTL, control);
        mmio.write32(offset + SD_CBL, self.slots * self.bytes);
        mmio.write16(offset + SD_LVI, (self.slots - 1) as u16);
        mmio.write16(offset + SD_FMT, self.format);
        let list = self.list.phys();
        mmio.write32(offset + SD_BDPL, list as u32);
        mmio.write32(offset + SD_BDPU, (list >> 32) as u32);
        device_barrier();
        mmio.write32(offset + SD_CTL, control | SD_CTL_RUN);
        self.last_lpib = 0;
        self.moved_ns = now_ns;
        self.running = true;
        true
    }

    /// Stops the stream at `now_ns`. The periods not yet played stay, to
    /// play when it starts again; a period the link was part way through
    /// counts as played.
    pub fn stop(&mut self, mmio: &Mmio, host: &mut dyn Host, now_ns: u64) -> bool {
        if !self.running {
            return true;
        }
        self.update(mmio, now_ns);
        let offset = self.slot.offset;
        let control = mmio.read32(offset + SD_CTL) & 0x00ff_ffff;
        mmio.write32(offset + SD_CTL, control & !SD_CTL_RUN);
        let stopped = self.wait_control(mmio, SD_CTL_RUN, false, host);
        self.update(mmio, now_ns);
        self.running = false;
        let bytes = u64::from(self.bytes);
        self.position = self.position.div_ceil(bytes) * bytes;
        stopped
    }

    /// Holds the stream in reset and lets it go: it stops, and its
    /// position and list are forgotten.
    fn reset(&self, mmio: &Mmio, host: &mut dyn Host) {
        let offset = self.slot.offset;
        let control = mmio.read32(offset + SD_CTL) & 0x00ff_ffff;
        if control & SD_CTL_RUN != 0 {
            mmio.write32(offset + SD_CTL, control & !SD_CTL_RUN);
            self.wait_control(mmio, SD_CTL_RUN, false, host);
        }
        mmio.write32(offset + SD_CTL, SD_CTL_SRST);
        self.wait_control(mmio, SD_CTL_SRST, true, host);
        mmio.write32(offset + SD_CTL, 0);
        self.wait_control(mmio, SD_CTL_SRST, false, host);
        mmio.write8(offset + SD_STS, SD_STS_CLEAR);
    }

    /// Waits until the bits `mask` of SD_CTL read `set`; false if they do
    /// not within 10 ms.
    fn wait_control(&self, mmio: &Mmio, mask: u32, set: bool, host: &mut dyn Host) -> bool {
        for _ in 0..STREAM_LOOKS {
            if (mmio.read32(self.slot.offset + SD_CTL) & mask != 0) == set {
                return true;
            }
            host.delay_us(10);
        }
        false
    }

    /// Stops the stream and holds it in reset, so that its memory can go.
    pub fn release(&mut self, mmio: &Mmio, host: &mut dyn Host) {
        self.reset(mmio, host);
        self.running = false;
        self.pending = 0;
        self.filled = [0; 4];
    }
}
