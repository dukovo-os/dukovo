// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The virtio sound card (virtio 1.2, section 5.14; ADR 0321).
//!
//! A sound card has four queues: control (0), events (1), and the samples
//! of output streams (2, "tx") and of input streams (3, "rx"). Its
//! configuration says how many jacks, PCM streams and channel maps it
//! has. The driver plays through the first output stream that takes
//! signed 16-bit samples, and uses no other: no jacks, no channel maps,
//! no input, so the event and rx queues are left unset (a driver only
//! SHOULD fill the event queue, 5.14.6.2).
//!
//! A control request is a buffer the device reads (a header with the
//! request's code, then its fields) and one it writes (a status, then
//! what it answers). The driver sends one at a time and waits for its
//! answer through the caller's `wait`, which the kernel makes a short
//! sleep: requests come only when a stream is set up, started or stopped.
//! A device that does not answer in the time `wait` allows is taken to be
//! broken and is reset, and the driver does nothing more with it.
//!
//! A stream is driven as the specification lays out (5.14.6.6.1):
//! SET_PARAMS (format, rate, channels and the sizes of its buffer and of
//! a period), PREPARE, then periods of samples on the tx queue, START,
//! and STOP and RELEASE at the end, which make the device hand back every
//! period it holds. Each period travels as two buffers: the 4-byte
//! transfer header (the stream's ID) followed by the samples, which the
//! device reads, and an 8-byte status (and latency) it writes when it has
//! played them. The header and the samples sit side by side in one slot
//! of DMA memory and go as one buffer, as a device must accept since
//! virtio 1.0 (any layout, 2.7.4), so a period takes two descriptors.
//!
//! The card has no volume of its own: the kernel scales the samples
//! before they reach the driver.

use radix_hw::{Dma, device_barrier};

use crate::queue::{Buffer, MAX_SIZE, Queue};
use crate::{Error, Transport, feature as common_feature, status};

/// Request codes (5.14.6.1).
pub mod code {
    /// Asks for the PCM streams' information.
    pub const PCM_INFO: u32 = 0x0100;
    /// Sets a stream's format, rate, channels and buffer.
    pub const PCM_SET_PARAMS: u32 = 0x0101;
    /// Readies a stream to start.
    pub const PCM_PREPARE: u32 = 0x0102;
    /// Lets go of a stream's buffers.
    pub const PCM_RELEASE: u32 = 0x0103;
    /// Starts playing.
    pub const PCM_START: u32 = 0x0104;
    /// Stops playing.
    pub const PCM_STOP: u32 = 0x0105;
    /// The request was carried out.
    pub const OK: u32 = 0x8000;
    /// The request was malformed.
    pub const BAD_MSG: u32 = 0x8001;
    /// The device does not do what was asked.
    pub const NOT_SUPP: u32 = 0x8002;
    /// The device failed.
    pub const IO_ERR: u32 = 0x8003;
}

/// The control queue.
const CONTROL: u16 = 0;
/// The output streams' samples.
const TX: u16 = 2;

/// Where the configuration keeps the number of PCM streams.
const CONFIG_STREAMS: usize = 4;

/// A stream that plays (VIRTIO_SND_D_OUTPUT).
const DIRECTION_OUTPUT: u8 = 0;
/// The format bit and value of signed 16-bit samples
/// (VIRTIO_SND_PCM_FMT_S16).
const FORMAT_S16: u8 = 5;

/// The bytes of a stream's information (`virtio_snd_pcm_info`).
const INFO_SIZE: usize = 32;
/// The most streams asked about in one request.
const INFO_BATCH: u32 = 32;
/// The bytes of the transfer header in front of a period's samples.
const XFER_HEADER: usize = 4;
/// The bytes of a period's status: status and latency.
const XFER_STATUS: usize = 8;

/// Where in the control memory a request goes, and its answer.
const REQUEST: usize = 0;
const ANSWER: usize = 64;

/// The most periods a stream has in flight: as many as the kernel's
/// buffers have fragments (`radix_audio::MAX_FRAGMENTS`).
pub const MAX_PERIODS: u32 = 32;

/// The output stream the driver plays through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    /// Its ID among the card's streams.
    pub stream: u32,
    /// The rates it plays: bit `i` for the `i`th of virtio's rates
    /// (5,512 to 384,000 frames a second, as `radix_audio::RATES`).
    pub rates: u32,
    /// The fewest channels it plays.
    pub channels_min: u8,
    /// The most channels it plays.
    pub channels_max: u8,
}

/// How the stream is set up: its slots of DMA memory, one for each
/// period.
struct Periods {
    slots: Dma,
    /// The bytes of a slot: the header, a period, the status, aligned.
    stride: usize,
    /// The bytes of a period.
    bytes: usize,
    /// How many slots there are.
    count: u32,
}

/// A virtio sound card, set up, with its output stream found.
pub struct Snd {
    transport: Transport,
    control: Queue,
    tx: Queue,
    /// One page: a control request and its answer.
    memory: Dma,
    output: Output,
    periods: Option<Periods>,
    /// The slots not in flight, a bit each.
    free: u32,
    /// For the head of each period's chain in flight, its slot.
    slot_of: [u8; MAX_SIZE as usize],
    /// Whether the stream is prepared (set up and not released).
    prepared: bool,
    /// Whether the card stopped answering and was reset.
    broken: bool,
}

/// A control request: its code and fields, at most [`ANSWER`] bytes.
struct Request {
    bytes: [u8; ANSWER],
    len: usize,
}

impl Request {
    fn new(code: u32) -> Request {
        let mut request = Request { bytes: [0; ANSWER], len: 0 };
        request.put32(code);
        request
    }

    fn put32(&mut self, value: u32) -> &mut Request {
        self.bytes[self.len..self.len + 4].copy_from_slice(&value.to_le_bytes());
        self.len += 4;
        self
    }

    fn put8(&mut self, value: u8) -> &mut Request {
        self.bytes[self.len] = value;
        self.len += 1;
        self
    }
}

impl Snd {
    /// Sets the card up: resets it, agrees on features, sets up its
    /// control and tx queues, with tx's interrupts on MSI-X entry
    /// `vector` (the control queue raises none: its answers are waited
    /// for), tells it the driver is ready, and finds its output stream.
    /// `alloc` gives zeroed DMA memory, in pages; `wait` waits a little
    /// for a control request's answer, and returns false once it has
    /// waited too long.
    pub fn new(
        transport: Transport,
        vector: u16,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<Snd, Error> {
        if !transport.reset() {
            return Err(Error::DeviceFailed);
        }
        transport.add_status(status::ACKNOWLEDGE);
        transport.add_status(status::DRIVER);
        let offered = transport.device_features();
        if offered & common_feature::VERSION_1 == 0 {
            transport.add_status(status::FAILED);
            return Err(Error::MissingFeature("virtio 1.0"));
        }
        let wanted = common_feature::EVENT_IDX | common_feature::ACCESS_PLATFORM;
        let features = common_feature::VERSION_1 | (offered & wanted);
        if let Err(err) = transport.set_driver_features(features) {
            transport.add_status(status::FAILED);
            return Err(err);
        }
        transport.no_config_interrupt();
        let streams = transport.read_config(|config| config.read32(CONFIG_STREAMS));
        let fail = |err: Error| {
            transport.add_status(status::FAILED);
            err
        };
        let no_vector = crate::NO_VECTOR;
        let mut control = Queue::new(&transport, CONTROL, 64, no_vector, features, (0, 0), alloc)
            .map_err(fail)?;
        control.set_quiet(true);
        let tx =
            Queue::new(&transport, TX, MAX_SIZE, vector, features, (0, 0), alloc).map_err(fail)?;
        let Some(memory) = alloc(1) else {
            transport.reset();
            return Err(Error::NoMemory);
        };
        transport.add_status(status::DRIVER_OK);
        let unset = Output { stream: 0, rates: 0, channels_min: 0, channels_max: 0 };
        let mut snd = Snd {
            transport,
            control,
            tx,
            memory,
            output: unset,
            periods: None,
            free: 0,
            slot_of: [0; MAX_SIZE as usize],
            prepared: false,
            broken: false,
        };
        snd.output = snd.find_output(streams, wait)?;
        Ok(snd)
    }

    /// Asks about the card's streams, in batches, and takes the first
    /// output that plays signed 16-bit samples at some rate.
    fn find_output(
        &mut self,
        streams: u32,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<Output, Error> {
        let mut start = 0;
        while start < streams {
            let count = (streams - start).min(INFO_BATCH);
            let mut request = Request::new(code::PCM_INFO);
            request.put32(start).put32(count).put32(INFO_SIZE as u32);
            self.command(&request, INFO_SIZE * count as usize, wait)?;
            for i in 0..count as usize {
                // The answers follow a 4-byte status, so their 8-byte
                // fields are not aligned: read as bytes.
                let mut info = [0u8; INFO_SIZE];
                self.memory.read(ANSWER + 4 + i * INFO_SIZE, &mut info);
                let le64 =
                    |at: usize| u64::from_le_bytes(core::array::from_fn(|byte| info[at + byte]));
                let formats = le64(8);
                let rates = le64(16) as u32 & 0x3fff;
                let [direction, channels_min, channels_max] = [info[24], info[25], info[26]];
                if direction == DIRECTION_OUTPUT
                    && formats & (1 << FORMAT_S16) != 0
                    && rates != 0
                    && channels_min >= 1
                {
                    return Ok(Output {
                        stream: start + i as u32,
                        rates,
                        channels_min,
                        channels_max: channels_max.max(channels_min),
                    });
                }
            }
            start += count;
        }
        Err(Error::MissingFeature("an output stream of 16-bit samples"))
    }

    /// Sends a control request and waits for its answer, which has
    /// `answer` bytes after its status. Fails with [`Error::Refused`] if
    /// the status is not OK, and with [`Error::NoAnswer`] (resetting the
    /// card) if none comes.
    fn command(
        &mut self,
        request: &Request,
        answer: usize,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.broken {
            return Err(Error::DeviceFailed);
        }
        let reply = 4 + answer;
        self.memory.write(REQUEST, &request.bytes[..request.len]);
        self.memory.write32(ANSWER, 0);
        let phys = self.memory.phys();
        let len = request.len as u32;
        self.control.add(2, |i| match i {
            0 => Buffer { phys: phys + REQUEST as u64, len, device_writes: false },
            _ => Buffer { phys: phys + ANSWER as u64, len: reply as u32, device_writes: true },
        })?;
        self.control.notify();
        loop {
            if self.control.used().is_some() {
                break;
            }
            if self.failed() || !wait() {
                // The request is still the card's: only a reset takes it
                // back.
                self.broken = true;
                self.transport.reset();
                return Err(Error::NoAnswer);
            }
        }
        device_barrier();
        match self.memory.read32(ANSWER) {
            code::OK => Ok(()),
            status => Err(Error::Refused(status)),
        }
    }

    /// The output stream played through.
    pub fn output(&self) -> Output {
        self.output
    }

    /// The most periods a stream can have: what its slots and the tx
    /// queue hold, two descriptors each.
    pub fn max_periods(&self) -> u32 {
        (u32::from(self.tx.size()) / 2).min(MAX_PERIODS)
    }

    /// Sets the stream up to play `channels` channels of signed 16-bit
    /// samples at the rate of bit `rate` (an index into virtio's rates),
    /// in `count` periods of `bytes` each, and prepares it. A stream that
    /// was set up before is stopped and released first, and every period
    /// it held is handed back ([`Snd::collect`] finds none of them).
    /// `alloc` gives the periods' DMA memory when what the stream has is
    /// too small.
    pub fn configure(
        &mut self,
        rate: u8,
        channels: u8,
        bytes: u32,
        count: u32,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if count == 0 || count > self.max_periods() || bytes == 0 || !bytes.is_multiple_of(2) {
            return Err(Error::BadRequest);
        }
        if self.output.rates & (1 << rate) == 0 {
            return Err(Error::BadRequest);
        }
        if self.prepared {
            self.release(wait)?;
        }
        let stride = (XFER_HEADER + bytes as usize).next_multiple_of(8) + XFER_STATUS;
        let need = stride * count as usize;
        let reuse = self.periods.take().filter(|periods| periods.slots.len() >= need);
        let slots = match reuse {
            Some(periods) => periods.slots,
            None => alloc(need.div_ceil(4096)).ok_or(Error::NoMemory)?,
        };
        self.periods = Some(Periods { slots, stride, bytes: bytes as usize, count });
        self.free = if count == 32 { u32::MAX } else { (1 << count) - 1 };
        let mut request = Request::new(code::PCM_SET_PARAMS);
        request
            .put32(self.output.stream)
            .put32(bytes * count)
            .put32(bytes)
            .put32(0)
            .put8(channels)
            .put8(FORMAT_S16)
            .put8(rate)
            .put8(0);
        self.command(&request, 0, wait)?;
        self.stream_command(code::PCM_PREPARE, wait)?;
        self.prepared = true;
        Ok(())
    }

    /// Sends a request about the stream with no fields of its own.
    fn stream_command(&mut self, code: u32, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let mut request = Request::new(code);
        request.put32(self.output.stream);
        self.command(&request, 0, wait)
    }

    /// Whether a period can be handed over now.
    pub fn can_submit(&self) -> bool {
        self.prepared && self.free != 0 && self.tx.room(2) > 0
    }

    /// Copies `samples` (a period's worth, at most) into a free slot and
    /// hands it to the card. Call [`Snd::notify`] after the periods of a
    /// batch.
    pub fn submit(&mut self, samples: &[u8]) -> Result<(), Error> {
        let Some(periods) = &self.periods else { return Err(Error::BadRequest) };
        if !self.prepared || samples.is_empty() || samples.len() > periods.bytes {
            return Err(Error::BadRequest);
        }
        if self.free == 0 || self.tx.room(2) == 0 {
            return Err(Error::Full);
        }
        let slot = self.free.trailing_zeros();
        let start = slot as usize * periods.stride;
        let status = start + (XFER_HEADER + periods.bytes).next_multiple_of(8);
        periods.slots.write32(start, self.output.stream);
        periods.slots.write(start + XFER_HEADER, samples);
        periods.slots.write64(status, 0);
        let phys = periods.slots.phys();
        let readable = (XFER_HEADER + samples.len()) as u32;
        let head = self.tx.add(2, |i| match i {
            0 => Buffer { phys: phys + start as u64, len: readable, device_writes: false },
            _ => {
                Buffer { phys: phys + status as u64, len: XFER_STATUS as u32, device_writes: true }
            }
        })?;
        self.free &= !(1 << slot);
        self.slot_of[usize::from(head)] = slot as u8;
        Ok(())
    }

    /// Rings the tx doorbell for the periods handed over since the last
    /// time.
    pub fn notify(&mut self) {
        self.tx.notify();
    }

    /// The next period the card has handed back, played (true) or not
    /// (false: the status was not OK); `None` when none waits.
    pub fn collect(&mut self) -> Option<bool> {
        let (head, _) = self.tx.used()?;
        device_barrier();
        let slot = u32::from(self.slot_of[usize::from(head)]);
        self.free |= 1 << slot;
        let periods = self.periods.as_ref()?;
        let status =
            slot as usize * periods.stride + (XFER_HEADER + periods.bytes).next_multiple_of(8);
        Some(periods.slots.read32(status) == code::OK)
    }

    /// Periods handed over and not yet handed back.
    pub fn in_flight(&self) -> u32 {
        let count = self.periods.as_ref().map_or(0, |periods| periods.count);
        count - self.free.count_ones()
    }

    /// Starts playing the periods handed over, and those to come.
    pub fn start(&mut self, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.stream_command(code::PCM_START, wait)
    }

    /// Stops playing. The periods the card holds stay its own until the
    /// stream is started again or released.
    pub fn stop(&mut self, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.stream_command(code::PCM_STOP, wait)
    }

    /// Lets go of the stream: the card hands back every period it held
    /// (5.14.6.6.5.1), which are then free, uncounted. The stream must be
    /// configured again before it plays.
    pub fn release(&mut self, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if !self.prepared {
            return Ok(());
        }
        self.prepared = false;
        let result = self.stream_command(code::PCM_RELEASE, wait);
        while self.tx.used().is_some() {}
        let count = self.periods.as_ref().map_or(0, |periods| periods.count);
        self.free = if count == 32 { u32::MAX } else { (1 << count) - 1 };
        result
    }

    /// Resets the card: it stops using every buffer it was given.
    pub fn reset(&self) {
        self.transport.reset();
    }

    /// Whether the card asks to be reset after an error, or stopped
    /// answering.
    pub fn failed(&self) -> bool {
        self.broken || self.transport.status() & status::NEEDS_RESET != 0
    }

    /// How many used entries the card gave that named no buffer in
    /// flight: a card that misbehaves.
    pub fn bad_entries(&self) -> u32 {
        self.control.bad_entries().saturating_add(self.tx.bad_entries())
    }
}

impl Drop for Snd {
    /// Resets the card before its queues and slots are given back, so
    /// that it reads and writes none of them after.
    fn drop(&mut self) {
        self.transport.reset();
    }
}
