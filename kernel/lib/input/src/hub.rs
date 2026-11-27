// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who reads what: the devices, the readers, and the rules between them
//! (ADR 0706, ADR 0708).
//!
//! Every driver hands the hub what a report of its device said
//! ([`Hub::frame`]). Every description of an input file is a reader
//! ([`Hub::open`]) with a ring of its own. The hub gives each reader that
//! is heard its file's records of the frame, and wakes it:
//!
//! - a reader of the device's own file reads the events and a SYN_REPORT;
//! - a reader of _/devices/kbd_ reads the keys, with their repeats;
//! - a reader of _/devices/mouse_ reads one `Motion`;
//! - a reader of _/devices/input/control_ reads the devices that come
//!   and go.
//!
//! **Input follows the screen.** A reader tied to a holder of the display
//! layer ([`Hub::tie`]) is heard only while that holder is in front
//! ([`Hub::front_is`]); one tied to none always is. A reader that stops
//! being heard is told: a device's file reads SYN_PAUSED,
//! _/devices/kbd_ reads the release of every key that is down, and
//! _/devices/mouse_ its buttons let go. Nothing reaches it until it is
//! heard again, when a device's file reads SYN_RESUMED. What it is asked
//! for the state meanwhile, it is told that nothing is down.
//!
//! **The secure key.** Ctrl+Alt+Delete, on any keyboards, begins a hold
//! if a holder has the display layer's attention file
//! ([`Hub::attention_is`]). From the press of Delete, which no reader
//! hears, only readers tied to that holder are heard, until the display
//! layer ends the hold ([`Hub::release`]). The hub begins the hold itself,
//! in the call that brought the key, so no event after the key reaches
//! anyone else; [`Hub::frame`] says so, for the kernel to bring the
//! holder to the front.
//!
//! The hub allocates when a device or a reader is added and when a ring
//! grows, with the allocator it is given, and answers a failed allocation
//! with a refusal or a reader's loss, never a panic.

use alloc::vec::Vec;
use core::alloc::Allocator;

use radix_abi::input::{
    DEVICE_ADDED, DEVICE_REMOVED, DEVICES_LOST, EV_KEY, EVENTS_KEPT, Event, INPUT_CHANGES_KEPT,
    INPUT_EVENTS_FIRST, INPUT_EVENTS_KEPT, InputChange, InputEvent, KEY_DELETE, KEY_KPDOT,
    KEY_LEFTALT, KEY_LEFTCTRL, KEY_RIGHTALT, KEY_RIGHTCTRL, MOTIONS_KEPT, Motion, PRESSED,
    RELEASED, REPEATED, SYN_DROPPED, SYN_PAUSED, SYN_REPORT, SYN_RESUMED,
};

use crate::device::{About, Device, Heard, Keys, Moved};
use crate::event::{FRAME_MAX, Record, Said, mark};
use crate::ring::{Full, Limits, Ring};

/// A device's file: 4 KiB at first, 64 KiB at most.
const EVENTS: Limits = Limits {
    first: INPUT_EVENTS_FIRST as usize,
    most: INPUT_EVENTS_KEPT as usize,
    full: Full::Forget,
};
/// _/devices/kbd_: 1 KiB at first, as many keys as it ever kept at most.
const KEYS: Limits = Limits { first: 64, most: EVENTS_KEPT as usize, full: Full::DropOldest };
/// _/devices/mouse_.
const MOTIONS: Limits = Limits { first: 32, most: MOTIONS_KEPT as usize, full: Full::DropOldest };
/// _/devices/input/control_.
const CHANGES: Limits =
    Limits { first: 16, most: INPUT_CHANGES_KEPT as usize, full: Full::DropOldest };

/// The most devices at once, and the most readers.
pub const DEVICES_MAX: usize = 256;
/// The most descriptions of input files open at once.
pub const READERS_MAX: usize = 1024;

/// The codes of buttons, which are keys of no keyboard: BTN_MISC up to
/// KEY_OK.
const BUTTON_CODES: core::ops::Range<u16> = 0x100..0x160;

/// How long a key of a keyboard that does not repeat its keys itself is
/// held before the hub repeats it, and how often it repeats then: Linux's
/// defaults.
pub const REPEAT_DELAY_NS: u64 = 250_000_000;
/// The time from one repeat to the next.
pub const REPEAT_PERIOD_NS: u64 = 33_000_000;

/// The key the hub repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Repeat {
    /// The device that holds it.
    device: u32,
    code: u16,
    /// When it repeats next.
    at: u64,
}

/// What the kernel gives the hub for each reader, to wake it.
pub trait Wake {
    /// The reader has something to read, or its file has changed: wakes
    /// the threads that wait in a read of it or poll it.
    fn wake(&self);
}

/// Which file a reader reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// _/devices/input/N_, for this N.
    Device(u32),
    /// _/devices/kbd_.
    Keyboard,
    /// _/devices/mouse_.
    Mouse,
    /// _/devices/input/control_.
    Control,
}

/// Why the hub did not do what it was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// There is no such device, or it has gone.
    NoDevice,
    /// The description is not a reader's.
    NoReader,
    /// No memory, or as many devices or readers as there may be.
    NoRoom,
    /// The reader is tied already.
    Tied,
    /// The file takes no such request.
    NotThisFile,
    /// The reader lost records: told once, then it reads on.
    Lost,
    /// The buffer cannot hold one record.
    TooSmall,
}

/// What `poll` finds on a reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    /// Nothing to read.
    Nothing,
    /// A record waits.
    Readable,
    /// Its device has gone and it has read everything.
    Gone,
}

/// What a reader has not read, in its file's records.
#[derive(Debug)]
enum Queue<A: Allocator> {
    Events(Ring<InputEvent, A>),
    Keys(Ring<Event, A>),
    Motions(Ring<Motion, A>),
    Changes(Ring<InputChange, A>),
}

impl<A: Allocator + Copy> Queue<A> {
    fn is_empty(&self) -> bool {
        match self {
            Queue::Events(ring) => ring.is_empty(),
            Queue::Keys(ring) => ring.is_empty(),
            Queue::Motions(ring) => ring.is_empty(),
            Queue::Changes(ring) => ring.is_empty(),
        }
    }

    fn clear(&mut self) {
        match self {
            Queue::Events(ring) => ring.clear(),
            Queue::Keys(ring) => ring.clear(),
            Queue::Motions(ring) => ring.clear(),
            Queue::Changes(ring) => ring.clear(),
        }
    }

    /// How many bytes of memory it holds.
    fn bytes(&self) -> usize {
        match self {
            Queue::Events(ring) => ring.room() * InputEvent::SIZE,
            Queue::Keys(ring) => ring.room() * Event::SIZE,
            Queue::Motions(ring) => ring.room() * Motion::SIZE,
            Queue::Changes(ring) => ring.room() * InputChange::SIZE,
        }
    }
}

/// One description of an input file.
#[derive(Debug)]
struct Reader<W, A: Allocator> {
    /// What the kernel knows the description by.
    key: usize,
    source: Source,
    /// The display holder it is tied to.
    tie: Option<u32>,
    /// Whether events reach it now.
    heard: bool,
    /// Its device has gone.
    gone: bool,
    queue: Queue<A>,
    waker: W,
}

/// Whether a reader tied to `tie` is heard while `front` is in front and,
/// if `held`, the secure key's hold is on for the holder `attention`.
fn hears(tie: Option<u32>, front: u32, attention: u32, held: bool) -> bool {
    match tie {
        _ if held => attention != 0 && tie == Some(attention),
        None => true,
        Some(holder) => holder != 0 && holder == front,
    }
}

/// What of one frame has not been given to the readers yet.
struct Frame {
    time_ns: u64,
    /// The events, with room for the SYN_REPORT after them.
    events: [InputEvent; FRAME_MAX + 1],
    count: usize,
    /// The keys _/devices/kbd_ reads.
    keys: [Event; FRAME_MAX],
    key_count: usize,
    moved: Moved,
}

impl Frame {
    fn new(time_ns: u64) -> Frame {
        Frame {
            time_ns,
            events: [mark(time_ns, SYN_REPORT); FRAME_MAX + 1],
            count: 0,
            keys: [Event { time_ns, code: 0, value: 0, reserved: 0 }; FRAME_MAX],
            key_count: 0,
            moved: Moved::default(),
        }
    }

    fn is_full(&self) -> bool {
        self.count == FRAME_MAX || self.key_count == FRAME_MAX
    }

    /// Adds `said`, which `device` heard as `heard`.
    fn add(&mut self, device: &mut Device, said: Said, heard: Heard) {
        if said.kind == EV_KEY && !BUTTON_CODES.contains(&said.code) {
            let value = match heard {
                Heard::Repeat => REPEATED,
                _ if said.value == 0 => RELEASED,
                _ => PRESSED,
            };
            self.keys[self.key_count] =
                Event { time_ns: self.time_ns, code: said.code, value, reserved: 0 };
            self.key_count += 1;
        }
        if heard == Heard::Changed {
            self.events[self.count] = said.event(self.time_ns);
            self.count += 1;
            device.add(&mut self.moved, said);
        }
    }
}

/// The devices, the readers, who is in front, and the secure key's hold:
/// with a `W` to wake each reader, in lists from `A`.
#[derive(Debug)]
pub struct Hub<W, A: Allocator + Copy> {
    alloc: A,
    /// The number the next device gets. Numbers are never used twice.
    next: u32,
    devices: Vec<Device, A>,
    readers: Vec<Reader<W, A>, A>,
    /// The display holder in front; 0 while nobody is.
    front: u32,
    /// The display holder that has the attention file; 0 while none has.
    attention: u32,
    /// The secure key's hold is on.
    held: bool,
    /// The Delete that began the hold, which no reader hears go up
    /// either: its device and its code.
    swallowed: Option<(u32, u16)>,
    /// The key that repeats, if one is held on a keyboard that does not
    /// repeat its keys itself.
    repeat: Option<Repeat>,
}

impl<W: Wake, A: Allocator + Copy> Hub<W, A> {
    /// A hub of no device and no reader, with nobody in front.
    pub const fn new_in(alloc: A) -> Hub<W, A> {
        Hub {
            devices: Vec::new_in(alloc),
            readers: Vec::new_in(alloc),
            alloc,
            next: 0,
            front: 0,
            attention: 0,
            held: false,
            swallowed: None,
            repeat: None,
        }
    }

    /// Adds a device that says `about` of itself, at `now`, and returns
    /// its number. The readers of the control file are told.
    ///
    /// # Errors
    ///
    /// [`Refused::NoRoom`] with no memory for it, with [`DEVICES_MAX`]
    /// devices already, or when every number has been used.
    pub fn add(&mut self, about: &About, now: u64) -> Result<u32, Refused> {
        let number = self.next;
        let next = number.checked_add(1).ok_or(Refused::NoRoom)?;
        if self.devices.len() >= DEVICES_MAX || self.devices.try_reserve(1).is_err() {
            return Err(Refused::NoRoom);
        }
        self.next = next;
        // Within the room just reserved.
        let _ = self.devices.push_within_capacity(Device::new(number, about));
        self.tell(InputChange { time_ns: now, device: number, what: DEVICE_ADDED });
        Ok(number)
    }

    /// Takes the device `number` away, at `now`: what it held down goes
    /// up, its readers read what waits and then find it gone, and the
    /// readers of the control file are told.
    pub fn remove(&mut self, number: u32, now: u64) {
        let Some(index) = self.index_of(number) else { return };
        let down = *self.devices[index].down();
        let mut ups = [Said::key(0, false); FRAME_MAX];
        let mut codes = down.codes().peekable();
        while codes.peek().is_some() {
            let mut count = 0;
            for (up, code) in ups.iter_mut().zip(codes.by_ref()) {
                *up = Said::key(code, false);
                count += 1;
            }
            self.frame(number, now, &ups[..count]);
        }
        self.devices.remove(index);
        for reader in &mut self.readers {
            if reader.source == Source::Device(number) {
                reader.gone = true;
                reader.waker.wake();
            }
        }
        self.tell(InputChange { time_ns: now, device: number, what: DEVICE_REMOVED });
    }

    /// Gives the readers of the control file `change`.
    fn tell(&mut self, change: InputChange) {
        for reader in &mut self.readers {
            if let Queue::Changes(ring) = &mut reader.queue {
                ring.push(&[change], change.time_ns, &CHANGES);
                reader.waker.wake();
            }
        }
    }

    fn index_of(&self, number: u32) -> Option<usize> {
        self.devices.iter().position(|device| device.number == number)
    }

    /// The devices, lowest number first.
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    /// INPUT_LIST: writes the devices' numbers into `out`, as many as
    /// fit, lowest first, and returns how many devices there are.
    pub fn list(&self, out: &mut [u32]) -> usize {
        for (slot, device) in out.iter_mut().zip(&self.devices) {
            *slot = device.number;
        }
        self.devices.len()
    }

    /// Makes the description `key` a reader of `source`, woken with
    /// `waker`. It is tied to no holder, and starts with what comes after.
    ///
    /// # Errors
    ///
    /// [`Refused::NoDevice`] for a device there is not, and
    /// [`Refused::NoRoom`] with no memory or [`READERS_MAX`] readers.
    pub fn open(&mut self, key: usize, source: Source, waker: W) -> Result<(), Refused> {
        if let Source::Device(number) = source {
            self.index_of(number).ok_or(Refused::NoDevice)?;
        }
        if self.readers.len() >= READERS_MAX || self.readers.try_reserve(1).is_err() {
            return Err(Refused::NoRoom);
        }
        let queue = match source {
            Source::Device(_) => Queue::Events(Ring::new_in(self.alloc)),
            Source::Keyboard => Queue::Keys(Ring::new_in(self.alloc)),
            Source::Mouse => Queue::Motions(Ring::new_in(self.alloc)),
            Source::Control => Queue::Changes(Ring::new_in(self.alloc)),
        };
        let heard = source == Source::Control || hears(None, self.front, self.attention, self.held);
        let reader = Reader { key, source, tie: None, heard, gone: false, queue, waker };
        // Within the room just reserved.
        let _ = self.readers.push_within_capacity(reader);
        Ok(())
    }

    /// The description `key` has closed.
    pub fn close(&mut self, key: usize) {
        if let Some(index) = self.readers.iter().position(|reader| reader.key == key) {
            self.readers.swap_remove(index);
        }
    }

    fn reader(&self, key: usize) -> Result<&Reader<W, A>, Refused> {
        self.readers.iter().find(|reader| reader.key == key).ok_or(Refused::NoReader)
    }

    fn reader_mut(&mut self, key: usize) -> Result<&mut Reader<W, A>, Refused> {
        self.readers.iter_mut().find(|reader| reader.key == key).ok_or(Refused::NoReader)
    }

    /// INPUT_TIE: ties the reader `key` to the display holder `holder`,
    /// at `now`. What it had not read is forgotten: it came before the
    /// tie. If the holder is not in front, a device's file reads
    /// SYN_PAUSED first.
    ///
    /// # Errors
    ///
    /// [`Refused::Tied`] if it is tied already, [`Refused::NotThisFile`]
    /// for the control file, and [`Refused::NoReader`].
    pub fn tie(&mut self, key: usize, holder: u32, now: u64) -> Result<(), Refused> {
        let (front, attention, held) = (self.front, self.attention, self.held);
        let reader = self.reader_mut(key)?;
        if reader.source == Source::Control {
            return Err(Refused::NotThisFile);
        }
        if reader.tie.is_some() {
            return Err(Refused::Tied);
        }
        reader.tie = Some(holder);
        reader.queue.clear();
        reader.heard = hears(reader.tie, front, attention, held);
        if let (Queue::Events(ring), false) = (&mut reader.queue, reader.heard) {
            ring.push(&[mark(now, SYN_PAUSED)], now, &EVENTS);
            reader.waker.wake();
        }
        Ok(())
    }

    /// The display holder `holder` has ended: the readers tied to it
    /// stay tied, to nobody, and hear nothing more. The number may be
    /// given to a new holder after this.
    pub fn forget(&mut self, holder: u32) {
        for reader in &mut self.readers {
            if reader.tie == Some(holder) {
                reader.tie = Some(0);
            }
        }
    }

    /// The display holder `holder` is in front from `now`; 0 for nobody.
    pub fn front_is(&mut self, holder: u32, now: u64) {
        if self.front != holder {
            self.front = holder;
            self.retune(now);
        }
    }

    /// The display holder `holder` has the attention file from `now`; 0
    /// for none, which ends a hold.
    pub fn attention_is(&mut self, holder: u32, now: u64) {
        if self.attention != holder {
            self.attention = holder;
            self.held &= holder != 0;
            self.retune(now);
        }
    }

    /// The secure key's hold ends at `now`.
    pub fn release(&mut self, now: u64) {
        if self.held {
            self.held = false;
            self.retune(now);
        }
    }

    /// Whether the secure key's hold is on.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Works out again who is heard, and tells each reader for which
    /// that changed.
    fn retune(&mut self, now: u64) {
        let (front, attention, held) = (self.front, self.attention, self.held);
        let mut down = Keys::new();
        let mut buttons = false;
        for device in &self.devices {
            down.merge(device.down());
            buttons |= device.buttons() != 0;
        }
        // No reader heard the secure key's Delete go down, so none reads
        // it let go.
        if let Some((_, code)) = self.swallowed {
            down.set(code, false);
        }
        for reader in &mut self.readers {
            let heard = hears(reader.tie, front, attention, held);
            if reader.source == Source::Control || heard == reader.heard {
                continue;
            }
            reader.heard = heard;
            match (&mut reader.queue, heard) {
                (Queue::Events(ring), false) => ring.push(&[mark(now, SYN_PAUSED)], now, &EVENTS),
                (Queue::Events(ring), true) => ring.push(&[mark(now, SYN_RESUMED)], now, &EVENTS),
                (Queue::Keys(ring), false) => let_go(ring, &down, now),
                (Queue::Motions(ring), false) if buttons => {
                    let still = Motion {
                        time_ns: now,
                        dx: 0,
                        dy: 0,
                        wheel: 0,
                        pan: 0,
                        x: 0,
                        y: 0,
                        buttons: 0,
                        flags: 0,
                    };
                    ring.push(&[still], now, &MOTIONS);
                }
                _ => continue,
            }
            reader.waker.wake();
        }
    }

    /// Whether a Ctrl and an Alt are down, on any keyboards.
    fn chord(&self) -> bool {
        let down = |codes: [u16; 2]| {
            self.devices.iter().any(|device| codes.iter().any(|&code| device.down().has(code)))
        };
        down([KEY_LEFTCTRL, KEY_RIGHTCTRL]) && down([KEY_LEFTALT, KEY_RIGHTALT])
    }

    /// What a report of the device `number` said, which came at
    /// `time_ns`: each reader that is heard reads its file's records of
    /// it. Returns true if it pressed the secure key and a hold began:
    /// the display layer is then to bring the attention holder to the
    /// front, or to [`Hub::release`] the hold if there is none after all.
    pub fn frame(&mut self, number: u32, time_ns: u64, said: &[Said]) -> bool {
        let Some(index) = self.index_of(number) else { return false };
        let mut secure = false;
        let mut frame = Frame::new(time_ns);
        for &said in said {
            let heard = self.devices[index].hear(said);
            if heard == Heard::Nothing {
                continue;
            }
            if said.kind == EV_KEY && self.swallows(number, said, heard) {
                continue;
            }
            if said.kind == EV_KEY && heard == Heard::Changed && self.begins_hold(said) {
                // What the report said before the key is for those who
                // were heard until now.
                self.send(index, &mut frame);
                self.held = true;
                self.swallowed = Some((number, said.code));
                // A key held since before the hold repeats for nobody.
                self.repeat = None;
                self.retune(time_ns);
                secure = true;
                continue;
            }
            if frame.is_full() {
                self.send(index, &mut frame);
            }
            self.note_repeat(index, time_ns, said, heard);
            frame.add(&mut self.devices[index], said, heard);
        }
        self.send(index, &mut frame);
        secure
    }

    /// Whether `said` is the Delete that began the hold repeating or
    /// going up, which no reader hears.
    fn swallows(&mut self, number: u32, said: Said, heard: Heard) -> bool {
        if self.swallowed != Some((number, said.code)) {
            return false;
        }
        if heard == Heard::Changed {
            self.swallowed = None;
        }
        true
    }

    /// Whether `said`, a key that changed, is the secure key going down
    /// while a holder has the attention file and no hold is on.
    fn begins_hold(&self, said: Said) -> bool {
        let delete = matches!(said.code, KEY_DELETE | KEY_KPDOT) && said.value != 0;
        delete && self.attention != 0 && !self.held && self.chord()
    }

    /// Gives the readers that are heard their records of `frame`, which
    /// the device at `index` said, and empties it.
    fn send(&mut self, index: usize, frame: &mut Frame) {
        let device = &mut self.devices[index];
        let motion = device.motion(frame.time_ns, &frame.moved);
        if frame.count == 0 && frame.key_count == 0 && motion.is_none() {
            return;
        }
        let now = frame.time_ns;
        frame.events[frame.count] = mark(now, SYN_REPORT);
        for reader in self.readers.iter_mut().filter(|reader| reader.heard) {
            match (&mut reader.queue, reader.source) {
                (Queue::Events(ring), Source::Device(of)) if of == device.number => {
                    if frame.count == 0 {
                        continue;
                    }
                    ring.push(&frame.events[..=frame.count], now, &EVENTS);
                }
                (Queue::Keys(ring), _) if frame.key_count > 0 => {
                    ring.push(&frame.keys[..frame.key_count], now, &KEYS);
                }
                (Queue::Motions(ring), _) => {
                    let Some(motion) = motion else { continue };
                    ring.push(&[motion], now, &MOTIONS);
                }
                _ => continue,
            }
            reader.waker.wake();
        }
        frame.count = 0;
        frame.key_count = 0;
        frame.moved = Moved::default();
    }

    /// Notes what `said`, which the device at `index` heard as `heard`,
    /// means for the key that repeats: a key that goes down on a keyboard
    /// that does not repeat its keys itself repeats from then, and any
    /// key that goes up on such a keyboard stops it, as on Linux.
    fn note_repeat(&mut self, index: usize, time_ns: u64, said: Said, heard: Heard) {
        let device = &self.devices[index];
        let key = said.kind == EV_KEY && !BUTTON_CODES.contains(&said.code);
        if !key || heard != Heard::Changed || device.about.repeats {
            return;
        }
        self.repeat = (said.value != 0).then_some(Repeat {
            device: device.number,
            code: said.code,
            at: time_ns.saturating_add(REPEAT_DELAY_NS),
        });
    }

    /// When the hub is next to be asked to repeat a key ([`Hub::tick`]),
    /// if a key that repeats is held.
    pub fn repeat_at(&self) -> Option<u64> {
        self.repeat.map(|repeat| repeat.at)
    }

    /// It is `now`: if the key that repeats is due, the readers of
    /// _/devices/kbd_ that are heard read a repeat of it. Returns when it
    /// is due next, if a key repeats. A repeat that comes late goes on
    /// from `now`, with no burst to catch up.
    pub fn tick(&mut self, now: u64) -> Option<u64> {
        let repeat = self.repeat?;
        if now < repeat.at {
            return Some(repeat.at);
        }
        let held =
            |device: &Device| device.number == repeat.device && device.down().has(repeat.code);
        if !self.devices.iter().any(held) {
            self.repeat = None;
            return None;
        }
        let key = Event { time_ns: now, code: repeat.code, value: REPEATED, reserved: 0 };
        for reader in self.readers.iter_mut().filter(|reader| reader.heard) {
            if let Queue::Keys(ring) = &mut reader.queue {
                ring.push(&[key], now, &KEYS);
                reader.waker.wake();
            }
        }
        let at = repeat.at.saturating_add(REPEAT_PERIOD_NS).max(now.saturating_add(1));
        self.repeat = Some(Repeat { at, ..repeat });
        Some(at)
    }

    /// The waker of the reader `key`, for the kernel to find what it
    /// keeps for the reader's pollers.
    pub fn waker(&self, key: usize) -> Option<&W> {
        self.reader(key).ok().map(|reader| &reader.waker)
    }

    /// Reads the reader `key`'s oldest records into `out`, as many as it
    /// holds whole, and returns their bytes: 0 if nothing waits. A
    /// device's file that lost events reads a SYN_DROPPED first, and the
    /// control file a DEVICES_LOST.
    ///
    /// # Errors
    ///
    /// [`Refused::TooSmall`] if `out` cannot hold one record;
    /// [`Refused::Lost`], once, if _/devices/kbd_ or _/devices/mouse_
    /// lost records; [`Refused::NoDevice`] once the device has gone and
    /// nothing waits; [`Refused::NoReader`].
    pub fn read(&mut self, key: usize, out: &mut [u8]) -> Result<usize, Refused> {
        self.read_as(key, out, Told::Now)
    }

    /// Reads on for a caller whose read has records already, as
    /// [`Hub::read`] does but for one thing: a loss that only an error
    /// can tell of is left for the caller's next read to find, and 0 is
    /// returned, so that the records it has are not lost with it.
    ///
    /// # Errors
    ///
    /// [`Hub::read`]'s, without [`Refused::Lost`].
    pub fn read_on(&mut self, key: usize, out: &mut [u8]) -> Result<usize, Refused> {
        self.read_as(key, out, Told::NextRead)
    }

    fn read_as(&mut self, key: usize, out: &mut [u8], told: Told) -> Result<usize, Refused> {
        let reader = self.reader_mut(key)?;
        let done = match &mut reader.queue {
            Queue::Events(ring) => {
                let lost = |time_ns| mark(time_ns, SYN_DROPPED);
                drain(ring, &EVENTS, out, &Loss::Record(&lost))?
            }
            Queue::Keys(ring) => drain(ring, &KEYS, out, &Loss::Error(told))?,
            Queue::Motions(ring) => drain(ring, &MOTIONS, out, &Loss::Error(told))?,
            Queue::Changes(ring) => {
                let lost = |time_ns| InputChange { time_ns, device: 0, what: DEVICES_LOST };
                drain(ring, &CHANGES, out, &Loss::Record(&lost))?
            }
        };
        if done == 0 && reader.gone {
            return Err(Refused::NoDevice);
        }
        Ok(done)
    }

    /// What `poll` finds on the reader `key`.
    pub fn ready(&self, key: usize) -> Ready {
        match self.reader(key) {
            Ok(reader) if !reader.queue.is_empty() => Ready::Readable,
            Ok(reader) if reader.gone => Ready::Gone,
            _ => Ready::Nothing,
        }
    }

    /// The device the reader `key` reads, and whether the reader is
    /// heard, for the requests that ask about the device.
    ///
    /// # Errors
    ///
    /// [`Refused::NotThisFile`] for a reader of another file,
    /// [`Refused::NoDevice`] if the device has gone, and
    /// [`Refused::NoReader`].
    pub fn asked(&self, key: usize) -> Result<(&Device, bool), Refused> {
        let reader = self.reader(key)?;
        let Source::Device(number) = reader.source else { return Err(Refused::NotThisFile) };
        let index = self.index_of(number).ok_or(Refused::NoDevice)?;
        Ok((&self.devices[index], reader.heard))
    }

    /// How many bytes the reader `key`'s ring holds.
    pub fn kept(&self, key: usize) -> usize {
        self.reader(key).map_or(0, |reader| reader.queue.bytes())
    }

    /// How many readers there are.
    pub fn readers(&self) -> usize {
        self.readers.len()
    }

    /// How many readers the own file of the device `number` has.
    pub fn readers_of(&self, number: u32) -> usize {
        self.readers.iter().filter(|reader| reader.source == Source::Device(number)).count()
    }
}

/// Gives a reader of _/devices/kbd_ that is no longer heard the release
/// of every key of `down`, at `now`.
fn let_go<A: Allocator + Copy>(ring: &mut Ring<Event, A>, down: &Keys, now: u64) {
    for code in down.codes().filter(|code| !BUTTON_CODES.contains(code)) {
        ring.push(&[Event { time_ns: now, code, value: RELEASED, reserved: 0 }], now, &KEYS);
    }
}

/// When a reader is told of a loss that only an error can tell of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Told {
    /// By this read's error.
    Now,
    /// By its next read's: this one has records already.
    NextRead,
}

/// How a file tells its reader that records were lost.
enum Loss<'a, T> {
    /// With the record this makes of the time the newest was lost.
    Record(&'a dyn Fn(u64) -> T),
    /// By [`Refused::Lost`]: the file has no such record.
    Error(Told),
}

/// Takes `ring`'s oldest records into `out`, as many as it holds whole,
/// and returns their bytes. If records were lost, the reader is told
/// first, as `loss` says.
fn drain<T: Record, A: Allocator + Copy>(
    ring: &mut Ring<T, A>,
    limits: &Limits,
    out: &mut [u8],
    loss: &Loss<'_, T>,
) -> Result<usize, Refused> {
    if out.len() < T::SIZE {
        return Err(Refused::TooSmall);
    }
    let mut done = 0;
    match loss {
        Loss::Record(lost) => {
            if let Some(time_ns) = ring.take_lost() {
                lost(time_ns).encode(&mut out[..T::SIZE]);
                done = T::SIZE;
            }
        }
        Loss::Error(Told::NextRead) if ring.lost() => return Ok(0),
        Loss::Error(_) => {
            if ring.take_lost().is_some() {
                return Err(Refused::Lost);
            }
        }
    }
    while out.len() - done >= T::SIZE {
        let Some(record) = ring.pop(limits) else { break };
        record.encode(&mut out[done..done + T::SIZE]);
        done += T::SIZE;
    }
    Ok(done)
}

#[cfg(test)]
mod tests;
