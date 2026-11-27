// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Virtio input devices (virtio 1.2, section 5.8): the keyboards, mice
//! and tablets a virtual machine's host gives it (ADR 0706).
//!
//! A device says what it is through its configuration: the driver
//! writes what it asks (`select`, and `subsel` for the kind of event or
//! the axis) and reads how many bytes the answer has and the answer,
//! which is a name, a bitmap with a bit for each code, an axis's range,
//! or the device's numbers. [`Input::name`], [`Input::serial`],
//! [`Input::ids`], [`Input::codes`] and [`Input::range`] ask.
//!
//! What happens on the device comes on its event queue, queue 0: the
//! driver keeps it full of eight-byte buffers ([`QUEUE_SIZE`] of them,
//! in part of a page), and the device writes an event into each, a
//! kind, a code and a value in Linux's numbers, ending what belongs
//! together with a SYN_REPORT as Linux's drivers do. [`Input::event`]
//! takes the oldest and gives its buffer back. The lights of a keyboard
//! go the other way, as events on the status queue, queue 1
//! ([`Input::light`]).
//!
//! Nothing here waits: both queues' completions raise MSI-X entry
//! `vector`, and the kernel's thread for the devices does the rest. The
//! host is not trusted to count: an answer's length is held to the room
//! it has, an event shorter than eight bytes is dropped, and the queue
//! itself skips a completion that names no buffer the driver gave.

use radix_hw::{Dma, device_barrier};

use crate::queue::{Buffer, Queue};
use crate::{Error, Transport, feature as common_feature, status};

/// What the driver asks the device's configuration: its `select`
/// (5.8.4).
mod select {
    /// The device's name.
    pub const NAME: u8 = 0x01;
    /// Its serial number.
    pub const SERIAL: u8 = 0x02;
    /// Its bus, maker's, model's and version numbers.
    pub const IDS: u8 = 0x03;
    /// A bit for each code it has of the kind `subsel` names.
    pub const CODES: u8 = 0x11;
    /// The range of the axis `subsel` names.
    pub const RANGE: u8 = 0x12;
}

/// Where the configuration's fields are (5.8.4).
const CONFIG_SELECT: usize = 0;
const CONFIG_SUBSEL: usize = 1;
const CONFIG_SIZE: usize = 2;
const CONFIG_ANSWER: usize = 8;
/// The most bytes an answer has.
pub const ANSWER_MAX: usize = 128;

/// The buffers of the event queue: enough for 21 reports of a mouse that
/// moves on both axes between two runs of the thread.
pub const QUEUE_SIZE: u16 = 64;
/// The buffers of the status queue: a keyboard's three lights and the
/// mark after them, twice over.
const STATUS_SIZE: u16 = 8;
/// An event's bytes: the kind (2), the code (2) and the value (4).
pub const EVENT_SIZE: usize = 8;
/// Where the status queue's buffers start in the page of buffers; the
/// event queue's are at its start.
const STATUS_AT: usize = 2048;

const _: () = assert!(QUEUE_SIZE as usize * EVENT_SIZE <= STATUS_AT);
const _: () = assert!(STATUS_AT + STATUS_SIZE as usize * EVENT_SIZE <= 4096);

/// The kind and the code of the mark that ends what belongs together.
const EV_SYN: u16 = 0;
const SYN_REPORT: u16 = 0;
/// The kind of an event that sets a light.
const EV_LED: u16 = 0x11;

/// One event of a device, or one sent to it: Linux's kinds and codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// What kind of event: an EV_*.
    pub kind: u16,
    /// Which key, axis, switch or light: a code of that kind.
    pub code: u16,
    /// What it did: 1 down and 0 up, how far, where, lit or not.
    pub value: i32,
}

impl Event {
    fn decode(bytes: [u8; EVENT_SIZE]) -> Event {
        let [k0, k1, c0, c1, value @ ..] = bytes;
        Event {
            kind: u16::from_le_bytes([k0, k1]),
            code: u16::from_le_bytes([c0, c1]),
            value: i32::from_le_bytes(value),
        }
    }

    fn encode(self) -> [u8; EVENT_SIZE] {
        let mut bytes = [0u8; EVENT_SIZE];
        bytes[..2].copy_from_slice(&self.kind.to_le_bytes());
        bytes[2..4].copy_from_slice(&self.code.to_le_bytes());
        bytes[4..].copy_from_slice(&self.value.to_le_bytes());
        bytes
    }
}

/// A device's numbers: how its host says it is connected (a Linux
/// BUS_*), and its maker's, model's and version numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ids {
    /// How it is connected.
    pub bus: u16,
    /// Its maker's number.
    pub vendor: u16,
    /// Its model's number.
    pub product: u16,
    /// Its version.
    pub version: u16,
}

/// The range of an axis that says where the device points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    /// The least it says.
    pub min: i32,
    /// The most it says.
    pub max: i32,
    /// Changes smaller than this are noise.
    pub fuzz: i32,
    /// Values this near the middle are the middle.
    pub flat: i32,
    /// Its units to a millimetre, or 0 if the device does not say.
    pub resolution: i32,
}

/// A virtio input device, set up.
pub struct Input {
    transport: Transport,
    events: Queue,
    status: Queue,
    /// The page of event buffers and, from [`STATUS_AT`], of the events
    /// the driver sends.
    buffers: Dma,
    /// Events the device wrote less than a whole one of.
    short: u32,
}

impl Input {
    /// Sets the device up: resets it, agrees on features, and sets up
    /// its two queues with completions on MSI-X entry `vector`. `alloc`
    /// gives zeroed DMA memory, in pages. The device sends nothing until
    /// [`Input::start`].
    pub fn new(
        transport: Transport,
        vector: u16,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    ) -> Result<Input, Error> {
        if !transport.reset() {
            return Err(Error::DeviceFailed);
        }
        transport.add_status(status::ACKNOWLEDGE);
        transport.add_status(status::DRIVER);
        let made = Input::set_up(&transport, vector, alloc);
        match made {
            Ok((events, status, buffers)) => {
                Ok(Input { transport, events, status, buffers, short: 0 })
            }
            Err(err) => {
                transport.add_status(status::FAILED);
                Err(err)
            }
        }
    }

    /// The features, the queues and the page of buffers.
    fn set_up(
        transport: &Transport,
        vector: u16,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    ) -> Result<(Queue, Queue, Dma), Error> {
        let offered = transport.device_features();
        if offered & common_feature::VERSION_1 == 0 {
            return Err(Error::MissingFeature("virtio 1.0"));
        }
        // The device has no features of its own.
        let common = common_feature::EVENT_IDX | common_feature::ACCESS_PLATFORM;
        let features = common_feature::VERSION_1 | (offered & common);
        transport.set_driver_features(features)?;
        transport.no_config_interrupt();
        if transport.device_config().len() < CONFIG_ANSWER + ANSWER_MAX {
            return Err(Error::MissingStructure("device configuration"));
        }
        let events = Queue::new(transport, 0, QUEUE_SIZE, vector, features, (0, 0), alloc)?;
        let status = Queue::new(transport, 1, STATUS_SIZE, vector, features, (0, 0), alloc)?;
        let buffers = alloc(1).ok_or(Error::NoMemory)?;
        Ok((events, status, buffers))
    }

    /// Asks the configuration for `select` of `subsel`: the answer, and
    /// how many bytes of it the device says it has, [`ANSWER_MAX`] at
    /// most.
    fn ask(&self, select: u8, subsel: u8) -> ([u8; ANSWER_MAX], usize) {
        self.transport.read_config(|config| {
            config.write8(CONFIG_SELECT, select);
            config.write8(CONFIG_SUBSEL, subsel);
            device_barrier();
            let size = usize::from(config.read8(CONFIG_SIZE)).min(ANSWER_MAX);
            let mut answer = [0u8; ANSWER_MAX];
            // By words: each read leaves the machine for its host.
            for (word, at) in
                answer[..size.next_multiple_of(4)].as_chunks_mut::<4>().0.iter_mut().zip(0..)
            {
                *word = config.read32(CONFIG_ANSWER + 4 * at).to_le_bytes();
            }
            answer[size..].fill(0);
            (answer, size)
        })
    }

    /// A piece of text of the configuration: up to its first NUL.
    fn text(&self, select: u8, out: &mut [u8; ANSWER_MAX]) -> usize {
        let (answer, size) = self.ask(select, 0);
        let len = answer[..size].iter().position(|&byte| byte == 0).unwrap_or(size);
        *out = answer;
        len
    }

    /// Writes the device's name into `out` and returns its length.
    pub fn name(&self, out: &mut [u8; ANSWER_MAX]) -> usize {
        self.text(select::NAME, out)
    }

    /// Writes the device's serial number into `out` and returns its
    /// length: 0 if it has none.
    pub fn serial(&self, out: &mut [u8; ANSWER_MAX]) -> usize {
        self.text(select::SERIAL, out)
    }

    /// The device's numbers; all 0 if it gives none.
    pub fn ids(&self) -> Ids {
        let (answer, size) = self.ask(select::IDS, 0);
        if size < 8 {
            return Ids::default();
        }
        let word = |at: usize| u16::from_le_bytes([answer[at], answer[at + 1]]);
        Ids { bus: word(0), vendor: word(2), product: word(4), version: word(6) }
    }

    /// The codes the device has of the events of `kind` (an EV_*): a bit
    /// for each, the code's eighth in the byte and the rest the bit in
    /// it, as Linux's bitmaps are. All clear if it has no such events.
    pub fn codes(&self, kind: u16) -> [u8; ANSWER_MAX] {
        let Ok(kind) = u8::try_from(kind) else { return [0; ANSWER_MAX] };
        self.ask(select::CODES, kind).0
    }

    /// The range of the axis `code` (an ABS_*), if the device gives one.
    pub fn range(&self, code: u16) -> Option<Range> {
        let (answer, size) = self.ask(select::RANGE, u8::try_from(code).ok()?);
        if size < 20 {
            return None;
        }
        let word = |at: usize| {
            i32::from_le_bytes([answer[at], answer[at + 1], answer[at + 2], answer[at + 3]])
        };
        Some(Range {
            min: word(0),
            max: word(4),
            fuzz: word(8),
            flat: word(12),
            resolution: word(16),
        })
    }

    /// Tells the device the driver is ready and gives it every buffer of
    /// the event queue: events come from now.
    pub fn start(&mut self) -> Result<(), Error> {
        self.transport.add_status(status::DRIVER_OK);
        while self.events.room(1) > 0 {
            self.give(self.events.next_free())?;
        }
        self.events.notify();
        Ok(())
    }

    /// Gives the device the event buffer that goes with descriptor
    /// `head`, which is free.
    fn give(&mut self, head: u16) -> Result<(), Error> {
        let phys = self.buffers.phys() + (usize::from(head) * EVENT_SIZE) as u64;
        let buffer = Buffer { phys, len: EVENT_SIZE as u32, device_writes: true };
        self.events.add(1, |_| buffer).map(|_| ())
    }

    /// The oldest event the device wrote that the driver has not taken;
    /// its buffer goes back to the device. Call [`Input::listen`] once
    /// those there are have been taken.
    pub fn event(&mut self) -> Option<Event> {
        loop {
            let (head, len) = self.events.used()?;
            let mut bytes = [0u8; EVENT_SIZE];
            // The queue gave a head it had in flight, so within the
            // queue's size, each of which has its buffer.
            self.buffers.read(usize::from(head) * EVENT_SIZE, &mut bytes);
            // The buffer's descriptor is free again, and is the next one
            // the queue gives out.
            let _ = self.give(self.events.next_free());
            if len as usize >= EVENT_SIZE {
                return Some(Event::decode(bytes));
            }
            self.short = self.short.saturating_add(1);
        }
    }

    /// Tells the device of the buffers given back since the last call.
    pub fn listen(&mut self) {
        self.events.notify();
    }

    /// Events dropped because the device wrote less than a whole one.
    pub fn short_events(&self) -> u32 {
        self.short
    }

    /// Completions that named no buffer the driver gave, on either
    /// queue.
    pub fn bad_entries(&self) -> u32 {
        self.events.bad_entries().saturating_add(self.status.bad_entries())
    }

    /// Sets the lights `lights` names, each a code (a LED_*) and whether
    /// it is lit, as one update. A light that does not fit in the status
    /// queue because the device has not read the ones before is left for
    /// the next call, which the caller makes when the lights next
    /// change; false then.
    pub fn light(&mut self, lights: &[(u16, bool)]) -> bool {
        // What the device has read frees its buffers.
        while self.status.used().is_some() {}
        if self.status.room(1) <= lights.len() {
            return false;
        }
        let events = lights
            .iter()
            .map(|&(code, lit)| Event { kind: EV_LED, code, value: i32::from(lit) })
            .chain([Event { kind: EV_SYN, code: SYN_REPORT, value: 0 }]);
        for event in events {
            let head = self.status.next_free();
            let at = STATUS_AT + usize::from(head) * EVENT_SIZE;
            self.buffers.write(at, &event.encode());
            let phys = self.buffers.phys() + at as u64;
            let buffer = Buffer { phys, len: EVENT_SIZE as u32, device_writes: false };
            if self.status.add(1, |_| buffer).is_err() {
                return false;
            }
        }
        self.status.notify();
        true
    }

    /// Whether the device asks to be reset after an error.
    pub fn failed(&self) -> bool {
        self.transport.status() & status::NEEDS_RESET != 0
    }
}

impl Drop for Input {
    /// Resets the device before its queues' memory is given back, so that
    /// it writes to none of it after.
    fn drop(&mut self) {
        self.transport.reset();
    }
}
