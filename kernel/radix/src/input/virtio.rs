// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Virtio keyboards, mice and tablets (`radix_virtio::input`, ADR 0706):
//! the input devices a virtual machine's host gives it without a USB
//! controller or a PS/2 port between.
//!
//! [`bind`] sets a device up with one MSI-X interrupt for both its
//! queues, asks it what it is, and adds it to the input devices with
//! what it says: its name and numbers, the keys it has, how it moves,
//! where it points and in what range, and its lights. The first device
//! starts the thread `vinput`, which serves them all.
//!
//! A device's events are in Linux's codes already, so they are handed on
//! as they come, a frame for each run that ends with the device's
//! SYN_REPORT, with the time of the interrupt that brought them. Two
//! things are made the input files' own: a wheel that counts whole
//! notches is read in 120ths of one, as every wheel is there, and a key
//! the host sends again while it is held is the hub's to repeat. A kind
//! or a code the files do not carry is dropped.
//!
//! When consoled changes the lock lights, the thread sends each keyboard
//! that has them its new ones.

use alloc::boxed::Box;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use radix_abi::input::{
    ABS_X, ABS_Y, BUS_VIRTUAL, EV_ABS, EV_KEY, EV_LED, EV_REL, EV_SYN, KEY_CODES, LED_CAPS_LOCK,
    LED_CAPSL, LED_NUM_LOCK, LED_NUML, LED_SCROLL_LOCK, LED_SCROLLL, REL_HWHEEL_HI_RES,
    REL_WHEEL_HI_RES, REL_X, REL_Y, SYN_REPORT,
};
use radix_hal::Devices;
use radix_hw::pci::{Bar, Function, MsiX};
use radix_input::device::{NOTCH, padded};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Once, SpinLock};
use radix_virtio::Transport;
use radix_virtio::input::{ANSWER_MAX, Event, Input};

use super::{About, Axis, FRAME_MAX, Said};
use crate::Arch;
use crate::sched::{self, Options, Policy, ThreadRef};

/// The most devices: a bit each in [`PENDING`].
const MAX_DEVICES: usize = 16;

/// Linux's codes for a wheel that counts whole notches, which the input
/// files do not carry: the horizontal one, and the one a finger turns.
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;

/// The lights a keyboard may have: the bit consoled asks for each with
/// (SET_LEDS) and the code the device knows it by.
const LIGHTS: [(u32, u16); 3] =
    [(LED_NUM_LOCK, LED_NUML), (LED_CAPS_LOCK, LED_CAPSL), (LED_SCROLL_LOCK, LED_SCROLLL)];

/// How many devices were found.
static FOUND: AtomicUsize = AtomicUsize::new(0);
/// The thread, which the devices' interrupts wake.
static THREAD: Once<ThreadRef> = Once::new();
/// The devices whose interrupts came, waiting for the thread.
static PENDING: AtomicU32 = AtomicU32::new(0);
/// When each device's interrupt last came, on the kernel's clock: the
/// time its events carry.
static CAME_NS: [AtomicU64; MAX_DEVICES] = [const { AtomicU64::new(0) }; MAX_DEVICES];
/// Each device, from when it is set up until the thread takes it.
static BOUND: [SpinLock<Option<Box<Bound, Kmalloc>>>; MAX_DEVICES] =
    [const { SpinLock::new(None) }; MAX_DEVICES];

/// A device's interrupt vector, given back when the device goes. It is
/// dropped after the device itself, which a reset has made quiet by
/// then.
struct Vector(u32);

impl Drop for Vector {
    fn drop(&mut self) {
        <Arch as Devices>::free_interrupt(self.0);
    }
}

/// A device as the thread serves it.
struct Bound {
    index: usize,
    input: Input,
    /// After `input`, so that it is dropped after it.
    _vector: Vector,
    /// Its number among the input devices.
    number: u32,
    /// Whether it counts its wheel, and its horizontal wheel, in 120ths
    /// of a notch itself.
    fine: [bool; 2],
    /// The lights it has, as SET_LEDS bits, and those it was last sent
    /// (`u32::MAX` before any were).
    has_lights: u32,
    lights: u32,
    /// What it said since its last SYN_REPORT.
    said: [Said; FRAME_MAX],
    count: usize,
}

impl Drop for Bound {
    fn drop(&mut self) {
        super::remove(self.number);
    }
}

/// A device's interrupt: the time is noted, and the thread reads it.
fn on_interrupt(index: usize) {
    if let Some(came) = CAME_NS.get(index) {
        came.store(sched::now_ns(), Ordering::Release);
    }
    PENDING.fetch_or(1 << index, Ordering::AcqRel);
    wake();
}

fn wake() {
    if let Some(thread) = THREAD.get() {
        sched::wake(thread);
    }
}

/// The lock lights changed: the thread sends them to the keyboards.
pub(super) fn lights_changed() {
    wake();
}

/// Sets up the virtio input device `function`.
pub fn bind(function: &Function) {
    if let Err(why) = try_bind(function) {
        warn!("Input: could not set up the virtio input device at {}: {why}", function.address());
    }
}

/// [`bind`], saying why it failed.
#[inline(never)]
fn try_bind(function: &Function) -> Result<(), &'static str> {
    let index = FOUND.fetch_add(1, Ordering::AcqRel);
    if index >= MAX_DEVICES {
        return Err("too many virtio input devices");
    }
    function.enable_memory_and_dma();
    let mut map_bar = |_: usize, bar: Bar| match bar {
        Bar::Memory { address, size, .. } => {
            <Arch as Devices>::map_device(address, usize::try_from(size).ok()?)
        }
        Bar::Io { .. } => None,
    };
    let transport = Transport::find(function, &mut map_bar).map_err(|_| "no modern transport")?;
    // One MSI-X entry for both queues: they wake the same thread.
    let msix = MsiX::find(function).ok_or("it has no MSI-X")?;
    let (bar, offset) = msix.table_location();
    let Some(Bar::Memory { address, .. }) = function.bar(bar) else {
        return Err("its MSI-X table is not in memory");
    };
    let table = <Arch as Devices>::map_device(address + offset, msix.table_len())
        .ok_or("its MSI-X table could not be mapped")?;
    let (vector, message) =
        <Arch as Devices>::allocate_interrupt(function.address(), on_interrupt, index)
            .ok_or("no interrupt vector is free")?;
    let vector = Vector(vector);
    msix.enable_masked(function);
    msix.set_entry(&table, 0, message);
    msix.unmask_all(function);
    let bound = new_bound(function, transport, (index, vector))?;
    *BOUND[index].lock() = Some(bound);
    start_thread();
    wake();
    Ok(())
}

/// Starts the thread, at the first device. It runs on the boot CPU,
/// where the devices' interrupts arrive (`allocate_interrupt`): the
/// interrupt that wakes it has that CPU awake already, and no other is
/// woken for an event on its way to its reader.
fn start_thread() {
    if THREAD.get().is_some() {
        return;
    }
    let options = Options::new().pinned(0).policy(Policy::Fair { nice: -5 });
    match sched::spawn("vinput", run, 0, options) {
        Ok(thread) => {
            let _ = THREAD.set(thread);
        }
        Err(err) => warn!("Input: could not start the thread of the virtio input devices: {err}"),
    }
}

/// The device behind `transport`, set up, added to the input devices and
/// started, on the heap; `slot` is its place among these devices and its
/// interrupt vector.
#[inline(never)]
fn new_bound(
    function: &Function,
    transport: Transport,
    slot: (usize, Vector),
) -> Result<Box<Bound, Kmalloc>, &'static str> {
    let (index, vector) = slot;
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(function.address(), pages, false);
    let input =
        Input::new(transport, 0, &mut alloc).map_err(|_| "the device refused to be set up")?;
    let (about, fine) = about(&input, function);
    let has_lights = LIGHTS
        .iter()
        .filter(|(_, code)| about.lights.has(*code))
        .fold(0, |all, (bit, _)| all | bit);
    let number = super::add(&about).ok_or("there is no room for another input device")?;
    let bound = Bound {
        index,
        input,
        _vector: vector,
        number,
        fine,
        has_lights,
        lights: u32::MAX,
        said: [Said::key(0, false); FRAME_MAX],
        count: 0,
    };
    // Dropped on a failure from here, it takes the device away again.
    let mut bound = Box::try_new_in(bound, heap).map_err(|_| "out of memory")?;
    bound.input.start().map_err(|_| "the device took no buffers")?;
    info!(
        "Input: {} at {} is /devices/input/{number}",
        radix_input::device::text(&about.name),
        function.address()
    );
    Ok(bound)
}

/// The printable text of `bytes`, which a host wrote: what is not plain
/// ASCII is left out, so that a name can carry nothing to a terminal
/// that shows it.
fn plain(bytes: &[u8]) -> TextBuf<64> {
    let mut text = TextBuf::new();
    for &byte in bytes.iter().filter(|byte| (0x20..0x7f).contains(*byte)) {
        let _ = text.write_char(char::from(byte));
    }
    text
}

/// What `input` says of itself, for the input hub, and whether it counts
/// its two wheels in 120ths of a notch.
fn about(input: &Input, function: &Function) -> (About, [bool; 2]) {
    let mut answer = [0u8; ANSWER_MAX];
    let len = input.name(&mut answer);
    let name = plain(&answer[..len]);
    let name = if name.as_str().is_empty() { "Virtio input device" } else { name.as_str() };
    let mut place = TextBuf::<32>::new();
    let _ = write!(place, "pci-{}", function.address());
    let mut about = About::new(BUS_VIRTUAL, name, place.as_str());
    let len = input.serial(&mut answer);
    about.serial = padded(plain(&answer[..len]).as_str());
    let ids = input.ids();
    (about.vendor, about.product, about.version) = (ids.vendor, ids.product, ids.version);
    let has =
        |bits: &[u8; ANSWER_MAX], code: u16| bits[usize::from(code / 8)] & 1 << (code % 8) != 0;
    let keys = input.codes(EV_KEY);
    for code in (0..KEY_CODES as u16).filter(|&code| has(&keys, code)) {
        about.keys.set(code, true);
    }
    let moves = input.codes(EV_REL);
    let fine = [has(&moves, REL_WHEEL_HI_RES), has(&moves, REL_HWHEEL_HI_RES)];
    for (code, there) in [
        (REL_X, has(&moves, REL_X)),
        (REL_Y, has(&moves, REL_Y)),
        (REL_WHEEL_HI_RES, fine[0] || has(&moves, REL_WHEEL)),
        (REL_HWHEEL_HI_RES, fine[1] || has(&moves, REL_HWHEEL)),
    ] {
        about.moves.set(code, there);
    }
    let points = input.codes(EV_ABS);
    for code in [ABS_X, ABS_Y].into_iter().filter(|&code| has(&points, code)) {
        if let Some(range) = input.range(code) {
            about.add_axis(Axis {
                code,
                min: range.min,
                max: range.max,
                fuzz: range.fuzz,
                flat: range.flat,
                resolution: range.resolution,
            });
        }
    }
    let lights = input.codes(EV_LED);
    for (_, code) in LIGHTS {
        about.lights.set(code, has(&lights, code));
    }
    (about, fine)
}

impl Bound {
    /// What `event` says in the input files' terms; `None` for a kind or
    /// a code they do not carry.
    fn said(&self, event: Event) -> Option<Said> {
        let Event { kind, code, value } = event;
        match (kind, code) {
            // A value of 2 is the host's repeat of a held key.
            (EV_KEY, code) if u32::from(code) < KEY_CODES && value != 2 => {
                Some(Said::key(code, value != 0))
            }
            (EV_REL, REL_X | REL_Y | REL_WHEEL_HI_RES | REL_HWHEEL_HI_RES) => {
                Some(Said::moved(code, value))
            }
            (EV_REL, REL_WHEEL) if !self.fine[0] => {
                Some(Said::moved(REL_WHEEL_HI_RES, value.saturating_mul(NOTCH)))
            }
            (EV_REL, REL_HWHEEL) if !self.fine[1] => {
                Some(Said::moved(REL_HWHEEL_HI_RES, value.saturating_mul(NOTCH)))
            }
            (EV_ABS, ABS_X | ABS_Y) => Some(Said::at(code, value)),
            _ => None,
        }
    }

    /// Hands the hub what the device said since its last frame.
    fn hand_over(&mut self, came_ns: u64) {
        super::frame(self.number, came_ns, &self.said[..self.count]);
        self.count = 0;
    }

    /// Takes the events the device wrote and hands each frame of them
    /// on, with the time of the interrupt that brought them. What came
    /// after the last SYN_REPORT waits for the next.
    fn serve(&mut self) {
        let came_ns = match CAME_NS.get(self.index).map(|came| came.swap(0, Ordering::AcqRel)) {
            Some(came) if came != 0 => came,
            _ => sched::now_ns(),
        };
        while let Some(event) = self.input.event() {
            if (event.kind, event.code) == (EV_SYN, SYN_REPORT) {
                self.hand_over(came_ns);
                continue;
            }
            let Some(said) = self.said(event) else { continue };
            if self.count == FRAME_MAX {
                // More than a frame holds: the hub reads it as several.
                self.hand_over(came_ns);
            }
            self.said[self.count] = said;
            self.count += 1;
        }
        self.input.listen();
    }

    /// Sends the device the lock lights consoled last asked for, if it
    /// has lights and they changed since.
    fn light(&mut self) {
        let lights = super::lights() & self.has_lights;
        if self.has_lights == 0 || lights == self.lights {
            return;
        }
        let mut update = [(0u16, false); LIGHTS.len()];
        let mut count = 0;
        for (bit, code) in LIGHTS {
            if self.has_lights & bit != 0 {
                update[count] = (code, lights & bit != 0);
                count += 1;
            }
        }
        // A device that has not read the lights before these is sent
        // them when they next change.
        if self.input.light(&update[..count]) {
            self.lights = lights;
        }
    }
}

/// The thread `vinput`: takes each device as it is set up, reads those
/// whose interrupt came, and sends the keyboards their lights.
fn run(_: usize) {
    let mut devices: [Option<Box<Bound, Kmalloc>>; MAX_DEVICES] = [const { None }; MAX_DEVICES];
    loop {
        let found = FOUND.load(Ordering::Acquire).min(MAX_DEVICES);
        let pending = PENDING.swap(0, Ordering::AcqRel);
        for (index, device) in devices.iter_mut().enumerate().take(found) {
            if device.is_none() {
                *device = BOUND[index].lock().take();
            }
            let Some(bound) = device else { continue };
            if pending & (1 << index) != 0 {
                bound.serve();
            }
            bound.light();
            if bound.input.failed() {
                warn!("Input: /devices/input/{} stopped: its device needs a reset", bound.number);
                // Dropped, it leaves the input devices.
                *device = None;
            }
        }
        if PENDING.load(Ordering::Acquire) == 0 {
            sched::block("vinput");
        }
    }
}
