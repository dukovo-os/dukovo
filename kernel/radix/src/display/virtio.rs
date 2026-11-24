// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Virtio graphics cards (`radix-virtio`'s `gpu`, ADR 0704): `virtio0`
//! and on, the screens of a virtual machine.
//!
//! The device has screens and no pipes: a screen shows a rectangle of a
//! *resource*, a picture the host keeps, which it copies from the
//! guest's memory when it is told to. The card gives the layer a pipe
//! for each screen and lets any pipe feed any of them, so that one
//! frame can show on several (a mirror costs one copy), and a mode is
//! no more than the size of the rectangle shown.
//!
//! - A picture whose pixels a program draws is a resource with the
//!   program's own pages as its backing (`device`). A frame of it is
//!   one trip to the host: the rectangles that changed are copied and
//!   shown (`shown`). Nothing is waited for but a change of mode, which
//!   the device must confirm.
//! - A picture the card keeps (a text console's) has no memory in the
//!   guest: its rows travel through the card's text band, a quarter of
//!   a mebibyte whatever the screen's size (`radix_virtio::gpu::band`).
//!   The `virtio-gpu=store` flag gives each such picture a store of its
//!   own instead, for a host found to read a backing differently from
//!   the specification.
//! - The pointer is the device's own, 64 pixels a side, on a queue of
//!   its own: a move is one command, sent at once.
//! - The kernel's log, the startup screen and the kernel's report when
//!   it stops are drawn from the console's cells a row at a time
//!   (`log`), so the kernel keeps no picture of a screen either.
//!
//! The device's interrupts are two: one says the screens changed (a
//! monitor came or went, a window was resized) and wakes the card's
//! thread to look ([`Virtio::find_screens`]); the other says a command
//! was answered, and is on only while a thread waits for one. Without
//! MSI-X the card is polled: its thread asks each second whether the
//! screens changed, and an answer is looked for each millisecond.
//!
//! With `virtio-gpu=shared`, on a host that offers it, a program's
//! picture is not copied at all: the host shows the guest's pages.
//!
//! One lock keeps the device and all the card knows of it ([`State`]).
//! The layer's lock is taken first, always: the card's thread asks the
//! device what its screens are with the card's lock, and lets that go
//! before it tells the layer.

mod device;
mod log;
mod setup;
mod shown;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use radix_abi::Status;
use radix_abi::display::{FORMAT_ARGB8888, MODE_PREFERRED, Mode};
use radix_abi::errors::Kernel;
use radix_display::layer::{CARDS_MAX, Found};
use radix_display::state::PictureShape;
use radix_display::{Rect, monitor, same_mode};
use radix_hal::Timer;
use radix_klog::warn;
use radix_mm::Kmalloc;
use radix_mm::bytes::zeroed;
use radix_sync::{Mutex, Once};
use radix_virtio::Error;
use radix_virtio::gpu::{
    self, EDID_MAX, EVENT_DISPLAY, Gpu, Packed, SCANOUTS_MAX, Scanout, Target, code, put_whole,
};

use self::device::{Device, unreach};
use self::log::Log;
pub use self::setup::bind;
use super::{
    CardRef, Change, Driver, Failed, Frame, Layer, Pages, Picture, PlaneFault, invalid, kmalloc,
    no_memory,
};
use crate::sched::{self, wait};
use crate::{Arch, console};

/// The rate a screen's beat is kept at where its monitor names none, in
/// frames in 1000 seconds: the device has no rate of its own.
const REFRESH_MHZ: u32 = 60_000;
/// The shortest and the longest side of a mode.
const SIDE_LEAST: u32 = 32;
const SIDE_MOST: u32 = 8192;
/// The size a screen is given before a monitor says its own.
const UNPLUGGED: (u32, u32) = (1024, 768);
/// The modes kept of a screen at most.
const MODES: usize = 64;
/// The bit of a picture's handle that says its pages are shared with
/// the host; the rest is its resource.
const SHARED: u64 = 1 << 32;
/// How long the device has to answer.
const ANSWER_NS: u64 = 1_000_000_000;
/// How long a wait for an answer sleeps before it looks again, with no
/// interrupt to wake it.
const STEP_NS: u64 = 1_000_000;
/// How often a card with no interrupt asks whether its screens changed.
const POLL_NS: u64 = 1_000_000_000;
/// How often the card's thread looks for new lines of the kernel's log
/// while the log shows: nothing wakes it for one, as the log is written
/// with any lock held.
const LOG_NS: u64 = 20_000_000;

/// The `virtio-gpu` flag's `store`: no text band.
static STORES: AtomicBool = AtomicBool::new(false);
/// The flag's `shared`: the host shows the guest's own pages, if it can.
static SHARE: AtomicBool = AtomicBool::new(false);

/// What a card's interrupts share with its threads.
struct Slot {
    /// The card, once the layer has it.
    card: Once<CardRef>,
    /// Counts the interrupts of its control queue: an answer came.
    answers: AtomicU64,
    /// Set when its screens are to be looked at: by the interrupt that
    /// says they changed, and once as it starts.
    changed: AtomicBool,
}

impl Slot {
    const fn new() -> Slot {
        Slot { card: Once::new(), answers: AtomicU64::new(0), changed: AtomicBool::new(false) }
    }

    /// What a thread that waits for an answer sleeps on.
    fn key(&self) -> usize {
        core::ptr::from_ref(&self.answers).addr()
    }
}

/// A slot for each card the layer keeps.
static SLOTS: [Slot; CARDS_MAX] = [const { Slot::new() }; CARDS_MAX];
/// How many of them are taken.
static TAKEN: AtomicUsize = AtomicUsize::new(0);

/// Takes in the `virtio-gpu` flag: `store`, `shared`, or both with a
/// comma between.
pub(super) fn set_options(flag: Option<&str>) {
    for option in flag.unwrap_or_default().split(',').filter(|option| !option.is_empty()) {
        match option {
            "store" => STORES.store(true, Ordering::Relaxed),
            "shared" => SHARE.store(true, Ordering::Relaxed),
            other => warn!("Display: the virtio-gpu flag has no setting `{other}`"),
        }
    }
}

/// The interrupt that says card `slot`'s screens changed.
fn changed(slot: usize) {
    let Some(slot) = SLOTS.get(slot) else { return };
    slot.changed.store(true, Ordering::Release);
    if let Some(card) = slot.card.get() {
        card.wake();
    }
}

/// The interrupt that says card `slot` answered a command.
fn answered(slot: usize) {
    let Some(slot) = SLOTS.get(slot) else { return };
    slot.answers.fetch_add(1, Ordering::AcqRel);
    wait::wake_all(slot.key());
}

/// A `wait` for the card's answers: until its interrupt says one came,
/// or a millisecond has passed with none (a card with no interrupt, and
/// the pointer's queue, which has none), for a second at most in all.
fn waiter(slot: &'static Slot) -> impl FnMut() -> bool {
    let deadline = sched::now_ns().saturating_add(ANSWER_NS);
    // Read before the caller looks at its queue, each time: an answer
    // that comes after that look changes the count.
    let mut seen = slot.answers.load(Ordering::Acquire);
    move || {
        let now = sched::now_ns();
        if now >= deadline {
            return false;
        }
        let until = deadline.min(now + STEP_NS);
        wait::wait_until_or_timeout(slot.key(), "display", until, || {
            slot.answers.load(Ordering::Acquire) != seen
        });
        seen = slot.answers.load(Ordering::Acquire);
        true
    }
}

/// A `wait` for the kernel's report: nothing sleeps and no interrupt
/// comes, so it counts a second off in short delays.
fn stopped_waiter() -> impl FnMut() -> bool {
    const STEP_US: u64 = 20;
    let mut left = ANSWER_NS / 1000 / STEP_US;
    move || {
        if left == 0 {
            return false;
        }
        left -= 1;
        <Arch as Timer>::delay_us(STEP_US);
        true
    }
}

/// What went wrong, as the status a request ends with.
fn status(err: Error) -> Status {
    match err {
        Error::NoMemory | Error::Refused(code::ERR_OUT_OF_MEMORY) => no_memory(),
        Error::NoAnswer => Kernel::TimedOut.status(0),
        Error::DeviceFailed => Kernel::Gone.status(0),
        _ => invalid(),
    }
}

/// What went wrong, in words, for the log.
fn why(err: Error) -> &'static str {
    match err {
        Error::NoMemory => "out of memory",
        Error::NoAnswer => "the card did not answer",
        Error::Refused(_) => "the card refused a command",
        Error::NoVector => "the card did not take its interrupts",
        Error::NoQueue => "the card has no screen, or lacks a queue",
        _ => "the card failed",
    }
}

/// The mode a screen is found in: the size the host asks for, at the
/// rate the kernel's clock keeps.
fn mode_of(found: &Scanout) -> Mode {
    let sides = SIDE_LEAST..=SIDE_MOST;
    let fits = found.enabled && sides.contains(&found.width) && sides.contains(&found.height);
    let (width, height) = if fits { (found.width, found.height) } else { UNPLUGGED };
    Mode { width, height, refresh_mhz: REFRESH_MHZ, flags: MODE_PREFERRED, ..Mode::default() }
}

/// Waits for the host to take what is queued if no more can be.
fn room(gpu: &mut Gpu, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
    if gpu.has_room() { Ok(()) } else { gpu.finish(wait) }
}

/// What the card's lock keeps: the device, and the kernel's log on it.
struct State {
    device: Device,
    log: Box<Log, Kmalloc>,
}

/// A virtio graphics card.
struct Virtio {
    /// The card's number in the layer.
    id: u32,
    slot: &'static Slot,
    /// Whether a holder has had the screens since the log last drew.
    handed: AtomicBool,
    /// Whether the device raises no interrupt.
    polled: bool,
    /// When a card with no interrupt next asks whether its screens
    /// changed.
    ask_at: AtomicU64,
    state: Mutex<State>,
}

impl Virtio {
    /// Asks the device what each screen is now and tells `layer`:
    /// whether a monitor is plugged in, the size the host asks for and
    /// the modes the monitor lists after it, and the monitor's data.
    fn find_screens(&self, layer: &mut Layer) {
        let Some((found, count)) = self.ask_screens() else { return };
        let mut modes = Vec::new_in(kmalloc());
        let Ok(mut data) = zeroed(EDID_MAX, kmalloc()) else { return };
        if modes.try_reserve_exact(MODES).is_err() {
            return;
        }
        for (index, screen) in found.iter().enumerate().take(count) {
            let len = if screen.enabled { self.monitor_data(index as u32, &mut data) } else { 0 };
            modes.clear();
            // Room for it and the rest was reserved.
            let _ = modes.push_within_capacity(mode_of(screen));
            monitor::modes(&data[..len], &mut |mode| {
                let sides = SIDE_LEAST..=SIDE_MOST;
                let fits = sides.contains(&mode.width) && sides.contains(&mode.height);
                if fits && !modes.iter().any(|known| same_mode(known, &mode)) {
                    let _ = modes.push_within_capacity(Mode { flags: 0, ..mode });
                }
            });
            // A screen with no monitor keeps the modes it had.
            let modes = if screen.enabled { &modes[..] } else { &[] };
            let found = Found { plugged: screen.enabled, modes, data: &data[..len] };
            if let Err(why) = layer.screen_found(self.id, index, &found) {
                warn!("Display: card {} could not take in its screen {index}: {why}", self.id);
            }
        }
    }

    /// Asks the device what its screens show, and returns that with how
    /// many it has: `None`, said in the log, if it does not answer.
    fn ask_screens(&self) -> Option<([Scanout; SCANOUTS_MAX], usize)> {
        let mut state = self.state.lock();
        let device = &mut state.device;
        // First, so that a change while the device is asked is not
        // missed: it raises the interrupt again.
        let events = device.gpu.events();
        device.gpu.clear_events(events);
        match device.gpu.display_info(&mut waiter(self.slot)) {
            Ok(found) => {
                device.found = found;
                Some((found, device.shown.count))
            }
            Err(err) => {
                warn!("Display: card {} did not say what its screens show: {}", self.id, why(err));
                None
            }
        }
    }

    /// Reads the data of screen `scanout`'s monitor into `data`, and
    /// returns its length: 0 from a device that gives none.
    fn monitor_data(&self, scanout: u32, data: &mut [u8]) -> usize {
        let mut state = self.state.lock();
        let gpu = &mut state.device.gpu;
        let Ok(data) = <&mut [u8; EDID_MAX]>::try_from(data) else { return 0 };
        if !gpu.has_edid() {
            return 0;
        }
        gpu.edid(scanout, data, &mut waiter(self.slot)).unwrap_or(0)
    }
}

impl Driver for Virtio {
    fn shows_log(&self) -> bool {
        true
    }

    fn log_takes(&self, frame: &Frame<'_>) {
        let mut state = self.state.lock();
        let State { device, log } = &mut *state;
        let size = (frame.mode.width, frame.mode.height);
        let resized = log.size() != size;
        // A console for the screen's size first: the log draws on it.
        let laid = console::lay(size.0 as usize, size.1 as usize);
        // It draws again whatever a holder left: all of it.
        if self.handed.swap(false, Ordering::AcqRel) {
            console::give_back();
        }
        let took = if laid {
            log.take(device, frame, &mut waiter(self.slot)).map_err(why)
        } else {
            Err("no text fits a screen of that size")
        };
        match took {
            Ok(()) if resized => console::report(),
            Ok(()) => {}
            Err(why) => warn!("Display: card {} does not show the kernel's log: {why}", self.id),
        }
        // Its thread looks for the log's lines from now on.
        if let Some(card) = self.slot.card.get() {
            card.wake();
        }
    }

    fn log_gives(&self) {
        self.handed.store(true, Ordering::Release);
        console::hand_over();
        self.state.lock().log.give();
    }

    fn check(&self, _frame: &Frame<'_>) -> Result<(), PlaneFault> {
        // The layer has checked the sizes and the formats, and the
        // device takes any rectangle of a picture on any screen.
        Ok(())
    }

    fn make_picture(
        &self,
        shape: &PictureShape,
        row_bytes: u32,
        pages: Option<&Pages>,
    ) -> Result<u64, Status> {
        // A pointer's picture is read where it is, and drawn in the
        // pointer's own resource.
        if shape.format == FORMAT_ARGB8888 {
            return Ok(0);
        }
        let mut state = self.state.lock();
        let device = &mut state.device;
        let mut wait = waiter(self.slot);
        let resource = device.number().map_err(status)?;
        match pages {
            Some(pages) => {
                device
                    .make_backed(resource, (shape, row_bytes), pages, &mut wait)
                    .map_err(status)?;
                let shared = if device.gpu.shares_pages() { SHARED } else { 0 };
                Ok(u64::from(resource) | shared)
            }
            None => {
                device.make_kept(resource, shape, &mut wait).map_err(status)?;
                Ok(u64::from(resource))
            }
        }
    }

    fn drop_picture(&self, picture: &Picture) {
        if picture.handle == 0 {
            return;
        }
        let mut state = self.state.lock();
        let device = &mut state.device;
        let resource = picture.handle as u32;
        // The answer is waited for: from then on the host reads none of
        // the picture's pages. A device that gives none was reset, and
        // reads nothing.
        device.forget(resource, &mut waiter(self.slot));
        device.drop_store(resource);
        if let Some(pages) = &picture.pages {
            let shape = picture.shape;
            let bytes = radix_display::picture_bytes(
                shape.format,
                shape.width,
                shape.height,
                picture.row_bytes,
            );
            unreach(device.source, pages, bytes.unwrap_or(0) as usize, u32::MAX);
        }
    }

    fn show(&self, frame: &Frame<'_>, change: &Change<'_>) -> Result<(), Failed> {
        let mut state = self.state.lock();
        let State { device, log } = &mut *state;
        let mut wait = waiter(self.slot);
        let Device { gpu, shown, .. } = device;
        let sent = if change.mode {
            // A mode is set or it is not: the device's answer says.
            shown.frame(gpu, frame, change, &mut wait).and_then(|()| gpu.finish(&mut wait))
        } else {
            gpu.loosely(|gpu| shown.frame(gpu, frame, change, &mut wait))
        };
        log.release(device, &mut wait);
        sent.map_err(|_| Failed)
    }

    fn put(&self, picture: &Picture, rect: Rect, rows: &[u8], on: Option<&Frame<'_>>) {
        let mut state = self.state.lock();
        let (Ok(x), Ok(y)) = (u32::try_from(rect.x), u32::try_from(rect.y)) else { return };
        let rect = gpu::Rect { x, y, width: rect.width, height: rect.height };
        let Some(mut rows) = Packed::new(rect, rows) else { return };
        let resource = picture.handle as u32;
        let shown = on.is_some_and(|frame| frame.active);
        let to = Target { resource, width: picture.shape.width, shown };
        let mut wait = waiter(self.slot);
        let Device { gpu, band, stores, .. } = &mut state.device;
        // Rows the device does not take are counted, and said in the log.
        let _ = match band {
            Some(band) => band.put(gpu, to, rect, &mut rows, &mut wait),
            None => match stores.iter().find(|(of, _)| *of == resource) {
                Some((_, store)) => put_whole(gpu, store, to, rect, &mut rows, &mut wait),
                None => Ok(()),
            },
        };
    }

    fn look(&self, layer: &mut Layer) -> Option<u64> {
        let now = sched::now_ns();
        let mut again = None;
        if self.polled {
            if now >= self.ask_at.load(Ordering::Relaxed) {
                self.ask_at.store(now + POLL_NS, Ordering::Relaxed);
                if self.state.lock().device.gpu.events() & EVENT_DISPLAY != 0 {
                    self.slot.changed.store(true, Ordering::Release);
                }
            }
            again = Some(self.ask_at.load(Ordering::Relaxed));
        }
        if self.slot.changed.swap(false, Ordering::AcqRel) {
            self.find_screens(layer);
        }
        let mut state = self.state.lock();
        let State { device, log } = &mut *state;
        device.say_lost(self.id);
        if log.draws() {
            if let Err(err) = log.draw(device, &mut waiter(self.slot)) {
                log.give();
                warn!("Display: card {} no longer shows the kernel's log: {}", self.id, why(err));
            }
            again = again.into_iter().chain(Some(now + LOG_NS)).min();
        }
        again
    }

    fn show_report(&self) {
        let mut state = self.state.lock();
        let State { device, log } = &mut *state;
        log.report(device, &mut stopped_waiter());
    }
}
