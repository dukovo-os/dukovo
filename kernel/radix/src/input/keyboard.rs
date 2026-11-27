// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The built-in keyboard, and the keyboards' thread.
//!
//! The PS/2 keyboard's interrupt hands each byte it sends to [`byte`],
//! which decodes it (`radix_input::Set1`) and queues the key with the
//! time of the interrupt. An interrupt hands the hub nothing itself
//! (waking a reader takes the wait table's lock, which interrupt handlers
//! must not take), so it wakes the `kbd` thread, which hands the keys
//! over as the reports of the keyboard's device. The keyboard sends a
//! held key again itself, and the hub reads that as the key's repeats.
//!
//! The thread does three more things. It lights the PS/2 keyboard's lock
//! lights (SET_LEDS): each byte of that command waits for the keyboard's
//! answer, which comes back through the interrupt. It has the hub repeat
//! the held key of a keyboard that does not repeat its keys itself, as a
//! USB one, when the key is due (`radix_input::hub::Hub::tick`). And it
//! shows the log behind the startup screen when Esc is pressed
//! (`console::reveal`, ADR 0426), or once the screen's time is up.
//!
//! Hyper-V's synthetic keyboard (ADR 0423) sends the bytes a PS/2
//! keyboard would, from VMBus's thread ([`set1_byte`]): it is a device of
//! its own, with a decoder of its own.
//!
//! The other keyboards' drivers light their lights from [`lights`]: a
//! USB controller's thread is woken when they change.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use radix_abi::input::{
    BUS_I8042, BUS_VIRTUAL, EV_KEY, LED_CAPSL, LED_NUML, LED_SCROLLL, RELEASED,
};
use radix_hal::Devices;
use radix_input::{Key, Set1, set1_keys};
use radix_klog::{error, info};
use radix_sync::{Mutex, Once, SpinLock};

use super::{About, Few, Said};
use crate::Arch;
use crate::sched::{self, Options, Policy, ThreadRef};

/// The keyboard's command that sets its lights, and its answers.
const SET_LIGHTS: u8 = 0xed;
const ACK: u8 = 0xfa;
const RESEND: u8 = 0xfe;

/// How long the thread waits for the keyboard to answer a byte, in
/// milliseconds.
const ANSWER_MS: u32 = 50;

/// The keys the interrupt keeps for the thread: more than a person
/// presses before the thread runs.
const QUEUED: usize = 32;

/// The keys the interrupt decoded that the thread has not handed over.
struct Queue {
    keys: [(u64, Key); QUEUED],
    /// Where the oldest is, and how many there are.
    first: usize,
    len: usize,
    decoder: Set1,
}

impl Queue {
    const fn new() -> Queue {
        let none = (0, Key { code: 0, value: RELEASED });
        Queue { keys: [none; QUEUED], first: 0, len: 0, decoder: Set1::new() }
    }

    /// Adds a key; with no room, the oldest goes, so that the newest,
    /// which may be a key going up, is never the one lost.
    fn push(&mut self, time_ns: u64, key: Key) {
        if self.len == QUEUED {
            self.first = (self.first + 1) % QUEUED;
            self.len -= 1;
        }
        self.keys[(self.first + self.len) % QUEUED] = (time_ns, key);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<(u64, Key)> {
        let key = (self.len > 0).then(|| self.keys[self.first])?;
        self.first = (self.first + 1) % QUEUED;
        self.len -= 1;
        Some(key)
    }
}

/// Touched by the interrupt, which may come on any CPU, and the thread.
static PS2: SpinLock<Queue> = SpinLock::new(Queue::new());
/// The PS/2 keyboard's device.
static PS2_DEVICE: Once<u32> = Once::new();
/// Hyper-V's keyboard: its device, made at its first byte, and its
/// decoder. Only VMBus's thread takes it.
static HYPERV: Mutex<(Option<u32>, Set1)> = Mutex::new((None, Set1::new()));
/// The thread.
static THREAD: Once<ThreadRef> = Once::new();
/// The lights wanted, and the lights the PS/2 keyboard was last sent
/// (`u32::MAX` before any are).
static LIGHTS: AtomicU32 = AtomicU32::new(0);
static LIGHTS_SET: AtomicU32 = AtomicU32::new(u32::MAX);
/// Set while the thread waits for the keyboard's answer, when 0xFA and
/// 0xFE are answers, not keys; and what the answer was.
static WAITING: AtomicBool = AtomicBool::new(false);
static ANSWER: AtomicU32 = AtomicU32::new(0);
/// The PS/2 keyboard is ready to be sent commands.
static STARTED: AtomicBool = AtomicBool::new(false);
/// When the thread next has the hub repeat a key: [`u64::MAX`] while
/// none repeats.
static REPEAT_AT: AtomicU64 = AtomicU64::new(u64::MAX);
/// Esc was pressed: the thread shows the log behind the startup screen,
/// if it shows.
static ESCAPE: AtomicBool = AtomicBool::new(false);

/// Wakes the thread.
fn wake_thread() {
    if let Some(thread) = THREAD.get() {
        sched::wake(thread);
    }
}

/// A byte from the PS/2 keyboard, in its interrupt.
fn byte(byte: u8) {
    if WAITING.load(Ordering::Acquire) && matches!(byte, ACK | RESEND) {
        ANSWER.store(u32::from(byte), Ordering::Release);
    } else {
        let now = sched::now_ns();
        let mut queue = PS2.lock_irq();
        let Some(key) = queue.decoder.feed(byte) else { return };
        queue.push(now, key);
    }
    wake_thread();
}

/// What a keyboard that sends scan code set 1 says of itself.
fn about(bus: u16, name: &str, place: &str) -> About {
    let mut about = About::new(bus, name, place);
    about.keys = set1_keys();
    about.repeats = true;
    about
}

/// What `key`, decoded from set 1, said.
fn said(key: Key) -> Said {
    Said { kind: EV_KEY, code: key.code, value: i32::from(key.value) }
}

/// A byte of scan code set 1 from Hyper-V's synthetic keyboard, which
/// came with the interrupt at `time_ns`: from VMBus's thread.
pub fn set1_byte(byte: u8, time_ns: u64) {
    let mut hyperv = HYPERV.lock();
    let Some(key) = hyperv.1.feed(byte) else { return };
    if hyperv.0.is_none() {
        hyperv.0 = super::add(&about(BUS_VIRTUAL, "Hyper-V keyboard", "vmbus"));
    }
    let Some(number) = hyperv.0 else { return };
    drop(hyperv);
    super::frame(number, time_ns, &[said(key)]);
}

/// Wakes the thread; `escape` says a report had Esc go down, for which
/// the thread shows the log behind the startup screen.
pub(super) fn wake(escape: bool) {
    if escape {
        ESCAPE.store(true, Ordering::Release);
    }
    wake_thread();
}

/// The key that repeats is next due at `at`, or none repeats: called
/// with the hub's lock held, so that the last word is the hub's. Returns
/// whether that is sooner than the thread sleeps for, which the caller
/// then wakes ([`wake`]) once the lock has gone.
pub(super) fn repeat_is(at: Option<u64>) -> bool {
    let at = at.unwrap_or(u64::MAX);
    REPEAT_AT.swap(at, Ordering::AcqRel) > at
}

/// The lock lights consoled(8) last asked for: LED_* bits.
pub fn lights() -> u32 {
    LIGHTS.load(Ordering::Acquire)
}

/// SET_LEDS: lights the lock lights `lights` names, on every keyboard.
pub(super) fn set_lights(lights: u32) {
    LIGHTS.store(lights & 7, Ordering::Release);
    wake_thread();
    crate::usb::lights_changed();
    super::virtio::lights_changed();
}

/// Starts the thread, then the PS/2 keyboard.
pub fn start() {
    let made = sched::spawn("kbd", run, 0, Options::new().policy(Policy::Fair { nice: -5 }));
    match made {
        Ok(thread) => {
            let _ = THREAD.set(thread);
        }
        Err(err) => {
            error!(
                "Keyboard: could not start the thread that hands the keyboard's keys over: {err}"
            );
            return;
        }
    }
    let Some(name) = Arch::start_keyboard(byte) else {
        info!("Keyboard: no built-in keyboard");
        return;
    };
    let mut about = about(BUS_I8042, "PS/2 keyboard", "ps2");
    about.lights = Few::of(&[LED_NUML, LED_CAPSL, LED_SCROLLL]);
    let Some(number) = super::add(&about) else { return };
    let _ = PS2_DEVICE.set(number);
    info!("Keyboard: {name} reads into /devices/kbd and /devices/input/{number}");
    STARTED.store(true, Ordering::Release);
    wake_thread();
}

/// Sends `byte` to the keyboard and waits for its answer: whether it took
/// it.
fn send(byte: u8) -> bool {
    for _ in 0..3 {
        ANSWER.store(0, Ordering::Release);
        WAITING.store(true, Ordering::Release);
        if !Arch::keyboard_send(byte) {
            break;
        }
        let mut answer = 0;
        for _ in 0..ANSWER_MS {
            answer = ANSWER.load(Ordering::Acquire);
            if answer != 0 {
                break;
            }
            sched::sleep_ns(1_000_000);
        }
        WAITING.store(false, Ordering::Release);
        match answer as u8 {
            ACK => return true,
            // Asked for it again.
            RESEND => {}
            _ => break,
        }
    }
    WAITING.store(false, Ordering::Release);
    false
}

/// Hands the hub the keys the PS/2 keyboard's interrupt queued, each as
/// a report of its own, with the time of its interrupt.
fn hand_over() {
    let Some(&number) = PS2_DEVICE.get() else { return };
    loop {
        let next = PS2.lock_irq().pop();
        let Some((time_ns, key)) = next else { return };
        super::frame(number, time_ns, &[said(key)]);
    }
}

/// Sends the PS/2 keyboard the lock lights if they are to change.
fn light() {
    let lights = LIGHTS.load(Ordering::Acquire);
    if !STARTED.load(Ordering::Acquire) || LIGHTS_SET.load(Ordering::Acquire) == lights {
        return;
    }
    // Set even if the keyboard does not answer, so that a keyboard
    // without lights is not asked again and again.
    LIGHTS_SET.store(lights, Ordering::Release);
    if !(send(SET_LIGHTS) && send(lights as u8)) {
        info!("Keyboard: the keyboard did not take the lock lights");
    }
}

/// The thread: hands the PS/2 keyboard's keys over, sets its lights when
/// they are to change, has the hub repeat a held key, and shows the log
/// behind the startup screen.
fn run(_: usize) {
    loop {
        // The log behind the startup screen, for Esc or once its time is up.
        let quiet_until = crate::console::quiet_until();
        if ESCAPE.swap(false, Ordering::AcqRel)
            || quiet_until.is_some_and(|at| sched::now_ns() >= at)
        {
            crate::console::reveal();
        }
        hand_over();
        if sched::now_ns() >= REPEAT_AT.load(Ordering::Acquire) {
            super::tick(sched::now_ns());
        }
        light();
        let repeat_at = Some(REPEAT_AT.load(Ordering::Acquire)).filter(|&at| at != u64::MAX);
        match repeat_at.into_iter().chain(crate::console::quiet_until()).min() {
            Some(at) => sched::block_until(at, "kbd"),
            None => sched::block("kbd"),
        }
    }
}
