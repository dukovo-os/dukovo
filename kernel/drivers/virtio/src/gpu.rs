// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The virtio graphics card in its 2D mode (virtio 1.2, section 5.7; ADR
//! 0704).
//!
//! The card has two queues: control (0) and the pointer's (1, "cursor").
//! Its configuration says how many screens it has (*scanouts*, at most
//! 16) and, in `events_read`, that what the host shows on them changed,
//! which the driver answers by asking again (GET_DISPLAY_INFO) and
//! writing the bit to `events_clear`.
//!
//! The host keeps every picture, as a *resource* the driver names with a
//! number. A resource has a *backing*: memory of the guest the host
//! copies pixels from when told to (TRANSFER_TO_HOST_2D), a rectangle at
//! a time. A screen shows a rectangle of a resource (SET_SCANOUT), and
//! the host looks again at the part it is told changed (RESOURCE_FLUSH).
//! With VIRTIO_GPU_F_RESOURCE_BLOB the backing can be shared instead:
//! the host shows the guest's own pages, and nothing is copied.
//!
//! A control command is a buffer the device reads (a header with the
//! command's code, then its fields) and one it writes (a header with the
//! answer's code, then what it answers). Commands are queued, as many as
//! a change needs, and the host is told once ([`Gpu::finish`]), which
//! also waits for every answer through the caller's `wait`: the driver
//! itself never waits. A card that does not answer in the time `wait`
//! allows is taken to be broken and is reset, and the driver does
//! nothing more with it. The card interrupts for its answers only while
//! a caller waits for them.
//!
//! Rows written into a picture the host keeps are not waited for at all
//! ([`Gpu::loosely`]): their commands are queued, the host is told, and
//! the answers are taken when the driver is next called. One the host
//! refuses is counted ([`Gpu::lost`]) and is nobody's error.
//!
//! The pointer has a queue of its own so that it never waits behind a
//! frame: a move is one command there, sent at once, whose buffer the
//! device hands back unanswered.
//!
//! Of a transfer the specification says only that `offset` is where in
//! the backing the rectangle starts. With a backing as large as the
//! resource that is the place of its first pixel, and each of its rows
//! is a row of the whole resource after the one before, so that is how
//! a host reads it: row `n` at `offset` plus `n` rows of the resource.
//! The driver relies on that and no more, and it is what lets a store of
//! a few rows ([`Sheet`]) carry text to a resource of any height. A host
//! that held `offset` to the rectangle's own place would refuse such a
//! transfer, and for one the kernel keeps a whole store for each picture
//! instead.

use radix_hw::{Dma, device_barrier};

pub use self::band::{Band, Packed, Rows, Same, Sheet, Target, put_whole};
use crate::queue::{Buffer, Queue};
use crate::{Error, NO_VECTOR, Transport, feature as common_feature, status};

mod band;

/// The card's own feature bits (5.7.3).
pub mod feature {
    /// GET_EDID gives a screen's monitor data.
    pub const EDID: u64 = 1 << 1;
    /// A resource's backing can be shared pages.
    pub const RESOURCE_BLOB: u64 = 1 << 3;
}

/// Command and answer codes (5.7.6.7).
pub mod code {
    /// Asks what the host shows on each screen.
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    /// Makes a resource of a format and a size.
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    /// Lets go of a resource.
    pub const RESOURCE_UNREF: u32 = 0x0102;
    /// Shows a rectangle of a resource on a screen.
    pub const SET_SCANOUT: u32 = 0x0103;
    /// Has the host look again at a rectangle of a resource.
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    /// Copies a rectangle from a resource's backing to the host.
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    /// Gives a resource its backing.
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    /// Asks for a screen's monitor data.
    pub const GET_EDID: u32 = 0x010a;
    /// Makes a resource whose backing is shared.
    pub const RESOURCE_CREATE_BLOB: u32 = 0x010c;
    /// Shows a rectangle of a shared resource on a screen.
    pub const SET_SCANOUT_BLOB: u32 = 0x010d;
    /// Sets the pointer's picture, hot spot and place.
    pub const UPDATE_CURSOR: u32 = 0x0300;
    /// Moves the pointer.
    pub const MOVE_CURSOR: u32 = 0x0301;
    /// The command was carried out.
    pub const OK_NODATA: u32 = 0x1100;
    /// GET_DISPLAY_INFO's answer.
    pub const OK_DISPLAY_INFO: u32 = 0x1101;
    /// GET_EDID's answer.
    pub const OK_EDID: u32 = 0x1104;
    /// The command failed, with no more said.
    pub const ERR_UNSPEC: u32 = 0x1200;
    /// The host is out of memory for it.
    pub const ERR_OUT_OF_MEMORY: u32 = 0x1201;
    /// No such screen.
    pub const ERR_INVALID_SCANOUT_ID: u32 = 0x1202;
    /// No such resource.
    pub const ERR_INVALID_RESOURCE_ID: u32 = 0x1203;
    /// A field is outside what the host takes.
    pub const ERR_INVALID_PARAMETER: u32 = 0x1205;
}

/// Pixel formats (5.7.6.8), named by their bytes in memory order.
pub mod format {
    /// Blue, green, red, alpha: FORMAT_ARGB8888 of radix-display(2).
    pub const BGRA: u32 = 1;
    /// Blue, green, red and a byte unused: FORMAT_XRGB8888.
    pub const BGRX: u32 = 2;
}

/// The most screens a card has.
pub const SCANOUTS_MAX: usize = 16;
/// The side of the pointer's picture, the one size the device takes.
pub const POINTER_SIDE: u32 = 64;
/// The most bytes of monitor data a screen gives.
pub const EDID_MAX: usize = 1024;
/// In `events_read`: what the host shows on the screens changed.
pub const EVENT_DISPLAY: u32 = 1;
/// The bytes of an entry of a backing's table: an address and a length.
pub const ENTRY_SIZE: usize = 16;

const CONTROL: u16 = 0;
const POINTER: u16 = 1;
/// The entries the control queue is given.
const CONTROL_SIZE: u16 = 64;
/// The entries the pointer's queue is given.
const POINTER_SIZE: u16 = 16;

const CONFIG_EVENTS_READ: usize = 0;
const CONFIG_EVENTS_CLEAR: usize = 4;
const CONFIG_SCANOUTS: usize = 8;

/// The bytes of a command's and of an answer's header.
const HEADER: usize = 24;
/// The commands in flight at most: a slot of the first page each.
const SLOTS: usize = 32;
/// The bytes of a slot: a command, then its answer's header.
const SLOT: usize = 128;
/// The most bytes of a command, and where in a slot its answer is.
const ANSWER_AT: usize = 96;
/// Where the one long answer is: the second page's first half.
const LONG_AT: usize = 4096;
const LONG_MAX: usize = 2048;
/// Where the pointer's commands are, and the bytes of each.
const POINTER_AT: usize = LONG_AT + LONG_MAX;
const POINTER_SLOT: usize = 64;

/// The guest's memory backs the resource (VIRTIO_GPU_BLOB_MEM_GUEST).
const BLOB_MEM_GUEST: u32 = 1;
/// The host may share the pages (VIRTIO_GPU_BLOB_FLAG_USE_SHAREABLE).
const BLOB_USE_SHAREABLE: u32 = 2;

const _: () = assert!(SLOTS * SLOT == LONG_AT && ANSWER_AT + HEADER <= SLOT);
const _: () = assert!(POINTER_AT + POINTER_SIZE as usize * POINTER_SLOT <= 2 * 4096);

/// A rectangle of a resource or of a screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Its left column.
    pub x: u32,
    /// Its top row.
    pub y: u32,
    /// Its width in pixels.
    pub width: u32,
    /// Its height.
    pub height: u32,
}

impl Rect {
    /// The rectangle of `width` by `height` at the top left.
    pub const fn sized(width: u32, height: u32) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }
}

/// What the host shows on one screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scanout {
    /// Whether it shows it at all: a monitor is plugged in.
    pub enabled: bool,
    /// The width it would like, in pixels.
    pub width: u32,
    /// The height.
    pub height: u32,
}

/// Where the pointer is: a screen and a pixel of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Place {
    /// The screen.
    pub scanout: u32,
    /// The column of its hot spot.
    pub x: u32,
    /// The row.
    pub y: u32,
}

/// A shared resource on a screen: what SET_SCANOUT_BLOB says of its
/// pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shared {
    /// The resource.
    pub resource: u32,
    /// A [`format`].
    pub format: u32,
    /// Its width in pixels.
    pub width: u32,
    /// Its height.
    pub height: u32,
    /// The bytes from one of its rows to the next.
    pub row_bytes: u32,
}

/// A control command: its header and fields, at most [`ANSWER_AT`]
/// bytes.
struct Command {
    bytes: [u8; ANSWER_AT],
    len: usize,
}

impl Command {
    /// A command of `code`, with no flags, fence or context.
    fn new(code: u32) -> Command {
        let mut command = Command { bytes: [0; ANSWER_AT], len: HEADER };
        command.bytes[..4].copy_from_slice(&code.to_le_bytes());
        command
    }

    fn put32(mut self, value: u32) -> Command {
        self.bytes[self.len..self.len + 4].copy_from_slice(&value.to_le_bytes());
        self.len += 4;
        self
    }

    fn put64(mut self, value: u64) -> Command {
        self.bytes[self.len..self.len + 8].copy_from_slice(&value.to_le_bytes());
        self.len += 8;
        self
    }

    fn rect(self, rect: Rect) -> Command {
        self.put32(rect.x).put32(rect.y).put32(rect.width).put32(rect.height)
    }
}

/// The interrupts a card is given: MSI-X entries, or [`NO_VECTOR`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vectors {
    /// For a change of its configuration: what the screens show.
    pub config: u16,
    /// For the answers of its control queue.
    pub control: u16,
}

/// A virtio graphics card, set up.
pub struct Gpu {
    transport: Transport,
    control: Queue,
    pointer: Queue,
    /// Two pages: the commands and their answers, the long answer, and
    /// the pointer's commands.
    memory: Dma,
    features: u64,
    scanouts: u32,
    /// The slots of commands in flight, a bit each.
    busy: u32,
    /// For the head of each chain in flight on the control queue, its
    /// slot.
    slot_of: [u8; CONTROL_SIZE as usize],
    /// The slots of the pointer's commands in flight.
    pointer_busy: u16,
    pointer_slot_of: [u8; POINTER_SIZE as usize],
    /// The slot whose answer is the long one, the code that answer is
    /// to have, and its bytes.
    long: Option<(u8, u32, u32)>,
    /// The slots of commands nobody waits for, and whether the next
    /// command queued is one.
    loose: u32,
    loose_now: bool,
    /// How many of those the card refused.
    lost: u32,
    /// The first answer that was not what its command expects, since
    /// the last [`Gpu::finish`].
    refused: Option<u32>,
    /// Whether the card stopped answering and was reset.
    broken: bool,
    /// Whether the card was started again to be polled, and is never to
    /// interrupt.
    polled: bool,
}

impl Gpu {
    /// Sets the card up: resets it, agrees on features (shared pages
    /// only if `shared`), sets up its two queues, and tells it the
    /// driver is ready. `alloc` gives zeroed DMA memory, in pages.
    ///
    /// # Errors
    ///
    /// Why the card cannot be driven.
    pub fn new(
        transport: Transport,
        vectors: Vectors,
        shared: bool,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    ) -> Result<Gpu, Error> {
        if !transport.reset() {
            return Err(Error::DeviceFailed);
        }
        let features = Gpu::agree(&transport, shared)?;
        let fail = |err: Error| {
            transport.add_status(status::FAILED);
            err
        };
        if !transport.set_config_vector(vectors.config) {
            return Err(fail(Error::NoVector));
        }
        let scanouts = transport.read_config(|config| config.read32(CONFIG_SCANOUTS));
        if scanouts == 0 || scanouts as usize > SCANOUTS_MAX {
            return Err(fail(Error::MissingFeature("one to sixteen screens")));
        }
        let none = (0, 0);
        let mut control =
            Queue::new(&transport, CONTROL, CONTROL_SIZE, vectors.control, features, none, alloc)
                .map_err(fail)?;
        // It interrupts only while a caller waits ([`Gpu::finish`]).
        control.set_quiet(true);
        // The pointer's commands are handed back unanswered, and looked
        // for when the next one is sent.
        let mut pointer =
            Queue::new(&transport, POINTER, POINTER_SIZE, NO_VECTOR, features, none, alloc)
                .map_err(fail)?;
        pointer.set_quiet(true);
        let Some(memory) = alloc(2) else {
            transport.reset();
            return Err(Error::NoMemory);
        };
        transport.add_status(status::DRIVER_OK);
        Ok(Gpu {
            transport,
            control,
            pointer,
            memory,
            features,
            scanouts,
            busy: 0,
            slot_of: [0; CONTROL_SIZE as usize],
            pointer_busy: 0,
            pointer_slot_of: [0; POINTER_SIZE as usize],
            long: None,
            loose: 0,
            loose_now: false,
            lost: 0,
            refused: None,
            broken: false,
            polled: false,
        })
    }

    /// Tells the card the driver knows it and agrees on the features.
    fn agree(transport: &Transport, shared: bool) -> Result<u64, Error> {
        transport.add_status(status::ACKNOWLEDGE);
        transport.add_status(status::DRIVER);
        let offered = transport.device_features();
        if offered & common_feature::VERSION_1 == 0 {
            transport.add_status(status::FAILED);
            return Err(Error::MissingFeature("virtio 1.0"));
        }
        let blob = if shared { feature::RESOURCE_BLOB } else { 0 };
        let wanted =
            common_feature::EVENT_IDX | common_feature::ACCESS_PLATFORM | feature::EDID | blob;
        let features = common_feature::VERSION_1 | (offered & wanted);
        if let Err(err) = transport.set_driver_features(features) {
            transport.add_status(status::FAILED);
            return Err(err);
        }
        Ok(features)
    }

    /// Starts the card again from nothing, with the memory it has, no
    /// interrupts, and every answer looked for: for the panic report,
    /// whatever the card was in the middle of. The host forgets every
    /// resource. False if the card does not come back.
    pub fn restart(&mut self) -> bool {
        self.broken = true;
        if !self.transport.reset() {
            return false;
        }
        let Ok(features) = Gpu::agree(&self.transport, false) else { return false };
        self.transport.set_config_vector(NO_VECTOR);
        let again = self.control.again(&self.transport, NO_VECTOR, features).is_ok()
            && self.pointer.again(&self.transport, NO_VECTOR, features).is_ok();
        if !again {
            return false;
        }
        self.transport.add_status(status::DRIVER_OK);
        (self.busy, self.pointer_busy, self.refused, self.long) = (0, 0, None, None);
        (self.loose, self.loose_now) = (0, false);
        self.features = features;
        (self.broken, self.polled) = (false, true);
        true
    }

    /// How many screens the card has.
    pub fn scanouts(&self) -> u32 {
        self.scanouts
    }

    /// Whether the card gives its screens' monitor data.
    pub fn has_edid(&self) -> bool {
        self.features & feature::EDID != 0
    }

    /// Whether a resource's backing can be shared pages.
    pub fn shares_pages(&self) -> bool {
        self.features & feature::RESOURCE_BLOB != 0
    }

    /// The events the card has for the driver: [`EVENT_DISPLAY`].
    pub fn events(&self) -> u32 {
        self.transport.read_config(|config| config.read32(CONFIG_EVENTS_READ))
    }

    /// Tells the card the driver has seen `events`.
    pub fn clear_events(&self, events: u32) {
        self.transport.device_config().write32(CONFIG_EVENTS_CLEAR, events);
    }

    /// Queues `command`, with `table` (an address and a length) as a
    /// second buffer the device reads, if it has one.
    fn queue(&mut self, command: &Command, table: Option<(u64, u32)>) -> Result<(), Error> {
        self.submit(command, table, None)
    }

    /// Queues `command` and its `table`. With `long`, a code and a
    /// length, its answer is to have that code and that many bytes, and
    /// goes in the long answer's place.
    fn submit(
        &mut self,
        command: &Command,
        table: Option<(u64, u32)>,
        long: Option<(u32, u32)>,
    ) -> Result<(), Error> {
        if self.broken {
            return Err(Error::DeviceFailed);
        }
        let buffers = if table.is_some() { 3 } else { 2 };
        let slot = (!self.busy).trailing_zeros() as usize;
        if slot >= SLOTS || self.control.room(buffers) == 0 {
            return Err(Error::Full);
        }
        let at = slot * SLOT;
        let answer_at = if long.is_some() { LONG_AT } else { at + ANSWER_AT };
        self.memory.write(at, &command.bytes[..command.len]);
        self.memory.write32(answer_at, 0);
        let phys = self.memory.phys();
        let answer = Buffer {
            phys: phys + answer_at as u64,
            len: long.map_or(HEADER as u32, |(_, bytes)| bytes),
            device_writes: true,
        };
        let request =
            Buffer { phys: phys + at as u64, len: command.len as u32, device_writes: false };
        let head = self.control.add(buffers, |i| match (i, table) {
            (0, _) => request,
            (1, Some((table, bytes))) => Buffer { phys: table, len: bytes, device_writes: false },
            _ => answer,
        })?;
        self.busy |= 1 << slot;
        self.loose = self.loose & !(1 << slot) | u32::from(self.loose_now) << slot;
        self.slot_of[usize::from(head)] = slot as u8;
        if let Some((expect, bytes)) = long {
            self.long = Some((slot as u8, expect, bytes));
        }
        Ok(())
    }

    /// Whether another command can be queued now. When not, the caller
    /// sends what is queued ([`Gpu::finish`]) first.
    pub fn has_room(&self) -> bool {
        self.busy != u32::MAX && self.control.room(3) > 0
    }

    /// How many commands wait for their answer, as of the last
    /// [`Gpu::collect`].
    pub fn in_flight(&self) -> u32 {
        self.busy.count_ones()
    }

    /// Runs `queue`, whose commands nobody waits for, and tells the card
    /// of them: one the card refuses is counted ([`Gpu::lost`]) and is
    /// not the error of the next [`Gpu::finish`].
    pub fn loosely<T>(&mut self, queue: impl FnOnce(&mut Gpu) -> T) -> T {
        self.loose_now = true;
        let out = queue(self);
        self.loose_now = false;
        self.control.notify();
        out
    }

    /// How many commands nobody waited for the card refused.
    pub fn lost(&self) -> u32 {
        self.lost
    }

    /// Tells the card of the commands queued since it was last told.
    pub fn kick(&mut self) {
        self.control.notify();
    }

    /// Takes the answers the card has given, and the pointer's commands
    /// it has handed back. Returns how many commands still wait for
    /// their answer.
    pub fn collect(&mut self) -> u32 {
        while let Some((head, len)) = self.control.used() {
            device_barrier();
            let slot = self.slot_of[usize::from(head)];
            self.busy &= !(1 << slot);
            let (at, expect, bytes) = match self.long {
                Some((long, expect, bytes)) if long == slot => {
                    self.long = None;
                    (LONG_AT, expect, bytes)
                }
                _ => (usize::from(slot) * SLOT + ANSWER_AT, code::OK_NODATA, HEADER as u32),
            };
            let said = if len as usize >= HEADER { self.memory.read32(at) } else { 0 };
            if said == expect && len >= bytes {
                continue;
            }
            // An answer of the right kind that is cut short says nothing.
            let kind = if said == expect { 0 } else { said };
            if self.loose & 1 << slot != 0 {
                self.lost = self.lost.saturating_add(1);
            } else if self.refused.is_none() {
                self.refused = Some(kind);
            }
        }
        while let Some((head, _)) = self.pointer.used() {
            self.pointer_busy &= !(1 << self.pointer_slot_of[usize::from(head)]);
        }
        self.busy.count_ones()
    }

    /// Tells the card of what is queued and waits, through `wait`, for
    /// every answer. `wait` waits a little, and returns false once it
    /// has waited too long.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] with the first answer that was not what its
    /// command expects (0 for one cut short), and [`Error::NoAnswer`],
    /// with the card reset, if one never came.
    pub fn finish(&mut self, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if self.broken {
            return Err(Error::DeviceFailed);
        }
        self.control.notify();
        if self.collect() > 0 {
            // The card interrupts for the answers still to come.
            self.control.set_quiet(self.polled);
            let answered = loop {
                if self.collect() == 0 {
                    break true;
                }
                if self.failed() || !wait() {
                    break false;
                }
            };
            self.control.set_quiet(true);
            if !answered {
                // The commands are still the card's: only a reset takes
                // them back.
                self.give_up();
                return Err(Error::NoAnswer);
            }
        }
        match self.refused.take() {
            None => Ok(()),
            Some(kind) => Err(Error::Refused(kind)),
        }
    }

    /// Resets a card that stopped answering, for good.
    fn give_up(&mut self) {
        self.broken = true;
        self.transport.reset();
        (self.busy, self.pointer_busy, self.refused, self.long) = (0, 0, None, None);
    }

    /// Sends `command` once nothing else is in flight, and waits for its
    /// answer of code `expect` and of `bytes` bytes with its header,
    /// which is then in the long answer's place.
    fn ask(
        &mut self,
        command: &Command,
        expect: u32,
        bytes: usize,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.finish(wait)?;
        self.submit(command, None, Some((expect, bytes as u32)))?;
        self.finish(wait)
    }

    /// GET_DISPLAY_INFO: what the host shows on each screen now.
    ///
    /// # Errors
    ///
    /// As [`Gpu::finish`], and [`Error::Refused`] for an answer cut
    /// short.
    pub fn display_info(
        &mut self,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<[Scanout; SCANOUTS_MAX], Error> {
        const ONE: usize = 24;
        let command = Command::new(code::GET_DISPLAY_INFO);
        self.ask(&command, code::OK_DISPLAY_INFO, HEADER + SCANOUTS_MAX * ONE, wait)?;
        let mut screens = [Scanout::default(); SCANOUTS_MAX];
        for (index, screen) in screens.iter_mut().enumerate().take(self.scanouts as usize) {
            let at = LONG_AT + HEADER + index * ONE;
            *screen = Scanout {
                width: self.memory.read32(at + 8),
                height: self.memory.read32(at + 12),
                enabled: self.memory.read32(at + 16) != 0,
            };
        }
        Ok(screens)
    }

    /// GET_EDID: the monitor data of screen `scanout` into `out`.
    /// Returns its bytes.
    ///
    /// # Errors
    ///
    /// As [`Gpu::finish`]; [`Error::BadRequest`] for a card that gives
    /// none.
    pub fn edid(
        &mut self,
        scanout: u32,
        out: &mut [u8; EDID_MAX],
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<usize, Error> {
        if !self.has_edid() || scanout >= self.scanouts {
            return Err(Error::BadRequest);
        }
        let command = Command::new(code::GET_EDID).put32(scanout).put32(0);
        self.ask(&command, code::OK_EDID, HEADER + 8 + EDID_MAX, wait)?;
        let len = (self.memory.read32(LONG_AT + HEADER) as usize).min(EDID_MAX);
        self.memory.read(LONG_AT + HEADER + 8, &mut out[..len]);
        Ok(len)
    }

    /// Queues RESOURCE_CREATE_2D: resource `resource` of `width` by
    /// `height` pixels of `format`.
    ///
    /// # Errors
    ///
    /// [`Error::Full`] when no more can be queued, and
    /// [`Error::DeviceFailed`] for a card that was reset.
    pub fn create(
        &mut self,
        resource: u32,
        format: u32,
        width: u32,
        height: u32,
    ) -> Result<(), Error> {
        let command = Command::new(code::RESOURCE_CREATE_2D)
            .put32(resource)
            .put32(format)
            .put32(width)
            .put32(height);
        self.queue(&command, None)
    }

    /// Queues RESOURCE_ATTACH_BACKING: `resource`'s backing is the `len`
    /// bytes at `phys`.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn attach(&mut self, resource: u32, phys: u64, len: u32) -> Result<(), Error> {
        let command = Command::new(code::RESOURCE_ATTACH_BACKING)
            .put32(resource)
            .put32(1)
            .put64(phys)
            .put32(len)
            .put32(0);
        self.queue(&command, None)
    }

    /// Queues RESOURCE_ATTACH_BACKING: `resource`'s backing is the
    /// pieces the `entries` entries of the table at `table` name, each an
    /// address and a length ([`entry`]). The table is the caller's until
    /// the command is answered.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn attach_table(&mut self, resource: u32, table: u64, entries: u32) -> Result<(), Error> {
        let command = Command::new(code::RESOURCE_ATTACH_BACKING).put32(resource).put32(entries);
        let len = entries.checked_mul(ENTRY_SIZE as u32).ok_or(Error::BadRequest)?;
        self.queue(&command, Some((table, len)))
    }

    /// Queues RESOURCE_CREATE_BLOB: resource `resource` is the `bytes`
    /// bytes of guest memory the table names, which the host may show
    /// as they are.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`], and [`Error::BadRequest`] for a card that
    /// shares no pages.
    pub fn create_shared(
        &mut self,
        resource: u32,
        bytes: u64,
        table: u64,
        entries: u32,
    ) -> Result<(), Error> {
        if !self.shares_pages() {
            return Err(Error::BadRequest);
        }
        let command = Command::new(code::RESOURCE_CREATE_BLOB)
            .put32(resource)
            .put32(BLOB_MEM_GUEST)
            .put32(BLOB_USE_SHAREABLE)
            .put32(entries)
            .put64(0)
            .put64(bytes);
        let len = entries.checked_mul(ENTRY_SIZE as u32).ok_or(Error::BadRequest)?;
        self.queue(&command, Some((table, len)))
    }

    /// Queues RESOURCE_UNREF: the host lets go of `resource`, and of its
    /// backing.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn unref(&mut self, resource: u32) -> Result<(), Error> {
        self.queue(&Command::new(code::RESOURCE_UNREF).put32(resource).put32(0), None)
    }

    /// Queues SET_SCANOUT: screen `scanout` shows `rect` of `resource`,
    /// or nothing for resource 0.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn set_scanout(&mut self, scanout: u32, resource: u32, rect: Rect) -> Result<(), Error> {
        let command = Command::new(code::SET_SCANOUT).rect(rect).put32(scanout).put32(resource);
        self.queue(&command, None)
    }

    /// Queues SET_SCANOUT_BLOB: screen `scanout` shows `rect` of the
    /// shared resource `shared`.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn set_scanout_shared(
        &mut self,
        scanout: u32,
        shared: &Shared,
        rect: Rect,
    ) -> Result<(), Error> {
        let mut command = Command::new(code::SET_SCANOUT_BLOB)
            .rect(rect)
            .put32(scanout)
            .put32(shared.resource)
            .put32(shared.width)
            .put32(shared.height)
            .put32(shared.format)
            .put32(0)
            .put32(shared.row_bytes);
        // The other three planes' row lengths, and the four offsets.
        command.len += 7 * 4;
        self.queue(&command, None)
    }

    /// Queues TRANSFER_TO_HOST_2D: the host copies `rect` of `resource`
    /// from its backing, whose byte `offset` is the rectangle's first.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn transfer(&mut self, resource: u32, rect: Rect, offset: u64) -> Result<(), Error> {
        let command = Command::new(code::TRANSFER_TO_HOST_2D)
            .rect(rect)
            .put64(offset)
            .put32(resource)
            .put32(0);
        self.queue(&command, None)
    }

    /// Queues RESOURCE_FLUSH: the screens that show `resource` show
    /// `rect` of it as it is now.
    ///
    /// # Errors
    ///
    /// As [`Gpu::create`].
    pub fn flush(&mut self, resource: u32, rect: Rect) -> Result<(), Error> {
        let command = Command::new(code::RESOURCE_FLUSH).rect(rect).put32(resource).put32(0);
        self.queue(&command, None)
    }

    /// Sends a command of the pointer's, at once. With every slot in
    /// flight it waits, through `wait`, for the card to hand one back.
    fn point(
        &mut self,
        code: u32,
        place: Place,
        picture: (u32, u32, u32),
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.broken {
            return Err(Error::DeviceFailed);
        }
        while self.pointer_busy == u16::MAX || self.pointer.room(1) == 0 {
            self.collect();
            if self.pointer_busy != u16::MAX && self.pointer.room(1) > 0 {
                break;
            }
            if self.failed() || !wait() {
                self.give_up();
                return Err(Error::NoAnswer);
            }
        }
        let command = Command::new(code)
            .put32(place.scanout)
            .put32(place.x)
            .put32(place.y)
            .put32(0)
            .put32(picture.0)
            .put32(picture.1)
            .put32(picture.2)
            .put32(0);
        let slot = (!self.pointer_busy).trailing_zeros() as usize;
        let at = POINTER_AT + slot * POINTER_SLOT;
        self.memory.write(at, &command.bytes[..command.len]);
        let phys = self.memory.phys() + at as u64;
        let buffer = Buffer { phys, len: command.len as u32, device_writes: false };
        let head = self.pointer.add(1, |_| buffer)?;
        self.pointer_busy |= 1 << slot;
        self.pointer_slot_of[usize::from(head)] = slot as u8;
        self.pointer.notify();
        Ok(())
    }

    /// UPDATE_CURSOR: the pointer is `resource`, a picture of
    /// [`POINTER_SIDE`] a side whose pixel `hot` is the one it points
    /// with, at `place`; resource 0 hides it.
    ///
    /// # Errors
    ///
    /// [`Error::NoAnswer`], with the card reset, if it hands nothing
    /// back while `wait` allows.
    pub fn set_pointer(
        &mut self,
        place: Place,
        resource: u32,
        hot: (u32, u32),
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.point(code::UPDATE_CURSOR, place, (resource, hot.0, hot.1), wait)
    }

    /// MOVE_CURSOR: the pointer is at `place`.
    ///
    /// # Errors
    ///
    /// As [`Gpu::set_pointer`].
    pub fn move_pointer(
        &mut self,
        place: Place,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.point(code::MOVE_CURSOR, place, (0, 0, 0), wait)
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
        self.control.bad_entries().saturating_add(self.pointer.bad_entries())
    }
}

impl Drop for Gpu {
    /// Resets the card before its queues and commands are given back, so
    /// that it reads and writes none of them after.
    fn drop(&mut self) {
        self.transport.reset();
    }
}

/// An entry of a backing's table: the `len` bytes at `phys`.
pub fn entry(phys: u64, len: u32) -> [u8; ENTRY_SIZE] {
    let mut out = [0; ENTRY_SIZE];
    out[..8].copy_from_slice(&phys.to_le_bytes());
    out[8..12].copy_from_slice(&len.to_le_bytes());
    out
}
