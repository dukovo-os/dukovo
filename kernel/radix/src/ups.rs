// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The UPSes plugged in by USB (ADR 0741): what each last said, which
//! `/kernel/class/power_supply` shows (`fs::sysfs`), and the clean
//! shutdown when one's battery is low.
//!
//! A USB controller's thread finds a UPS among a device's HID interfaces
//! (`usb::hid`), [`add`]s it, and reads its reports every
//! [`POLL_NS`], giving [`update`] what they said. A UPS is named `ups0`,
//! `ups1`... in the order they come, the lowest number free.
//!
//! When a UPS runs the machine on its battery and says it is low
//! (`radix_hid::power::State::low`) [`LOW_READS`] reads in a row, Spark
//! is asked to power the machine off, as for the power button
//! (`user::power::battery_low`), unless the kernel was started with
//! `ups=show`. A UPS also says, once each time it changes, that the mains
//! went or came back and that its battery should be replaced.

use core::sync::atomic::{AtomicBool, Ordering};

use radix_hid::power::State;
use radix_klog::{TextBuf, info, warn};
use radix_sync::SpinLock;

/// How often a UPS's reports are read.
pub const POLL_NS: u64 = 2_000_000_000;
/// Reads in a row that find the battery low before the machine shuts
/// down: one report read as the UPS changes over is not enough.
pub const LOW_READS: u8 = 2;
/// The most UPSes kept.
const MAX_UPSES: usize = 4;

/// Whether a low battery shuts the machine down: not with `ups=show`.
static SHUTS_DOWN: AtomicBool = AtomicBool::new(true);

/// A UPS, as the sysfs shows it.
#[derive(Clone, Debug)]
pub struct Ups {
    /// `ups0`.
    pub name: TextBuf<8>,
    /// Its USB strings: its model and maker, empty where it has none.
    pub model: TextBuf<64>,
    pub maker: TextBuf<64>,
    /// What it said last; nothing until its reports are first read.
    pub state: State,
    /// Whether its reports were read.
    pub read: bool,
    /// Reads in a row that found the battery low.
    low: u8,
}

static UPSES: SpinLock<[Option<Ups>; MAX_UPSES]> = SpinLock::new([const { None }; MAX_UPSES]);

/// Sets whether a low battery shuts the machine down; `show_only` with
/// `ups=show`.
pub fn configure(show_only: bool) {
    SHUTS_DOWN.store(!show_only, Ordering::Release);
}

/// Whether a low battery shuts the machine down.
pub fn shuts_down() -> bool {
    SHUTS_DOWN.load(Ordering::Acquire)
}

/// Keeps a UPS just found, `model` by `maker`; gives its slot, or None
/// when [`MAX_UPSES`] are kept.
pub fn add(model: &str, maker: &str) -> Option<usize> {
    let mut upses = UPSES.lock();
    let slot = upses.iter().position(Option::is_none)?;
    let mut ups = Ups {
        name: TextBuf::new(),
        model: TextBuf::new(),
        maker: TextBuf::new(),
        state: State::default(),
        read: false,
        low: 0,
    };
    let _ = core::fmt::Write::write_fmt(&mut ups.name, format_args!("ups{slot}"));
    let _ = core::fmt::Write::write_str(&mut ups.model, model);
    let _ = core::fmt::Write::write_str(&mut ups.maker, maker);
    upses[slot] = Some(ups);
    drop(upses);
    let what = if SHUTS_DOWN.load(Ordering::Acquire) {
        "the machine shuts down cleanly when its battery is low"
    } else {
        "shown only (ups=show)"
    };
    let by = if maker.is_empty() { "" } else { " by " };
    info!("UPS: ups{slot} is {model}{by}{maker}; {what}");
    Some(slot)
}

/// The UPS in `slot` was unplugged.
pub fn remove(slot: usize) {
    if let Some(ups) = UPSES.lock().get_mut(slot).and_then(Option::take) {
        info!("UPS: {} is gone", ups.name);
    }
}

/// What the UPS in `slot` said now: says what changed, and asks for the
/// machine to be powered off if its battery is low.
pub fn update(slot: usize, state: State) {
    let mut upses = UPSES.lock();
    let Some(ups) = upses.get_mut(slot).and_then(Option::as_mut) else { return };
    let (before, first) = (ups.state, !ups.read);
    ups.state = state;
    ups.read = true;
    ups.low = if state.low() { ups.low.saturating_add(1) } else { 0 };
    let (name, low) = (ups.name.clone(), ups.low);
    drop(upses);
    let percent = Charge(state.percent());
    if state.on_battery() && (first || !before.on_battery()) {
        warn!("UPS: {name}: the mains is gone; the machine runs on its battery, at {percent}");
    } else if !state.on_battery() && !first && before.on_battery() {
        info!("UPS: {name}: the mains is back; its battery is at {percent}");
    }
    if state.need_replacement == Some(true) && before.need_replacement != Some(true) {
        warn!("UPS: {name}: its battery should be replaced");
    }
    if low == LOW_READS {
        if SHUTS_DOWN.load(Ordering::Acquire) {
            crate::user::power::battery_low(name.as_str());
        } else {
            warn!("UPS: {name}: its battery is low, at {percent}; ups=show keeps the machine on");
        }
    }
}

/// Calls `each` with the name of each UPS kept, by slot, until it gives
/// false. Copies only the names out of the lock, to keep the caller's
/// frame small.
pub fn names(each: &mut dyn FnMut(&str) -> bool) {
    let names: [Option<TextBuf<8>>; MAX_UPSES] =
        UPSES.lock().each_ref().map(|ups| ups.as_ref().map(|ups| ups.name.clone()));
    for name in names.iter().flatten() {
        if !each(name.as_str()) {
            return;
        }
    }
}

/// The UPS named `name`, as it is now. Clones only that one.
pub fn find(name: &str) -> Option<Ups> {
    UPSES.lock().iter().flatten().find(|ups| ups.name.as_str() == name).cloned()
}

/// A charge for the log: `80%`, or `a charge it does not say`.
struct Charge(Option<u32>);

impl core::fmt::Display for Charge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(percent) => write!(f, "{percent}%"),
            None => f.write_str("a charge it does not say"),
        }
    }
}
