// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's settings (ADR 0476): the few values that change how it
//! runs and that a machine's work may want otherwise, which tune(8) sets
//! from a profile through radix-kernel-settings(2) and everyone reads in
//! `/kernel/settings/`.
//!
//! Each takes a list of words or a range of whole numbers, and changes
//! what runs from then on:
//!
//! - `cpu.energy_preference`: what HWP or CPPC weighs the CPUs' speed
//!   against their power with ([`pstate::set_preference`]);
//! - `sched.slice_us`: the fair class's base slice, each thread's from its
//!   next turn ([`sched::set_slice_ns`]);
//! - `disk.read_ahead_kib`: the page cache's read-ahead
//!   ([`radix_vfs::cache::set_read_ahead`]);
//! - `net.congestion`: the congestion control of new TCP sockets
//!   ([`radix_net::set_congestion`]).
//!
//! The value each had when the kernel started is kept, for tune(8) to put
//! back. Nothing is kept across a restart.

use core::fmt::{self, Write};

use radix_klog::{TextBuf, info};
use radix_net::Congestion;
use radix_pstate::Preference;
use radix_sync::Once;

use crate::{pstate, sched};

/// What a setting takes.
enum Takes {
    /// One of these words, separated by single spaces.
    Words(&'static str),
    /// A whole number from `min` to `max`, a multiple of `step`.
    Range { min: u64, max: u64, step: u64 },
}

impl Takes {
    /// Whether it takes `value`, written as `list` writes values: a word
    /// of the list, or a number in decimal with no sign and no leading 0.
    fn takes(&self, value: &str) -> bool {
        match *self {
            Takes::Words(words) => words.split(' ').any(|word| word == value),
            Takes::Range { min, max, step } => {
                let canonical = !value.is_empty()
                    && value.bytes().all(|b| b.is_ascii_digit())
                    && (value == "0" || !value.starts_with('0'));
                canonical
                    && value.parse::<u64>().is_ok_and(|n| (min..=max).contains(&n) && n % step == 0)
            }
        }
    }

    fn write(&self, out: &mut dyn Write) -> fmt::Result {
        match *self {
            Takes::Words(words) => out.write_str(words),
            Takes::Range { min, max, step: 1 } => write!(out, "{min}-{max}"),
            Takes::Range { min, max, step } => write!(out, "{min}-{max}/{step}"),
        }
    }
}

/// A setting.
struct Setting {
    name: &'static str,
    takes: Takes,
    /// Writes its value.
    get: fn(&mut dyn Write) -> fmt::Result,
    /// Sets it to a value it takes; false when this machine cannot have
    /// that value.
    set: fn(&str) -> bool,
}

/// Every setting, in the order `list` numbers them.
static SETTINGS: [Setting; 4] = [
    Setting {
        name: "cpu.energy_preference",
        takes: Takes::Words("firmware performance balance_performance balance_power power"),
        get: |out| out.write_str(pstate::preference().map_or("firmware", Preference::name)),
        set: |value| pstate::set_preference(Preference::parse(value)),
    },
    Setting {
        name: "sched.slice_us",
        takes: Takes::Range { min: 500, max: 20_000, step: 1 },
        get: |out| write!(out, "{}", sched::slice_ns() / 1000),
        set: |value| {
            let us: u64 = value.parse().unwrap_or(0);
            sched::set_slice_ns(us * 1000);
            true
        },
    },
    Setting {
        name: "disk.read_ahead_kib",
        takes: Takes::Range { min: 0, max: 1024, step: 4 },
        get: |out| write!(out, "{}", radix_vfs::cache::read_ahead() * 4),
        set: |value| {
            let kib: usize = value.parse().unwrap_or(0);
            radix_vfs::cache::set_read_ahead(kib / 4);
            true
        },
    },
    Setting {
        name: "net.congestion",
        takes: Takes::Words("cubic reno"),
        get: |out| out.write_str(radix_net::congestion().name()),
        set: |value| {
            if let Some(congestion) = Congestion::parse(value) {
                radix_net::set_congestion(congestion);
            }
            true
        },
    },
];

/// The values the settings had when the kernel started.
static STARTS: [Once<TextBuf<32>>; 4] = [const { Once::new() }; 4];

/// Keeps each setting's value at start. Called once the CPUs' speed
/// control has started, which the energy preference depends on.
pub fn init() {
    for (setting, start) in SETTINGS.iter().zip(&STARTS) {
        let mut value = TextBuf::new();
        let _ = (setting.get)(&mut value);
        let _ = start.set(value);
    }
}

/// How many settings there are.
pub fn count() -> usize {
    SETTINGS.len()
}

/// Setting `index`'s name.
pub fn name(index: usize) -> Option<&'static str> {
    SETTINGS.get(index).map(|setting| setting.name)
}

/// Writes setting `index`'s value.
pub fn value(index: usize, out: &mut dyn Write) -> fmt::Result {
    (SETTINGS.get(index).ok_or(fmt::Error)?.get)(out)
}

/// Writes setting `index`'s line, as radix-kernel-settings(2)'s `list`
/// gives it: its name, value, value at start and the values it takes.
pub fn line(index: usize, out: &mut dyn Write) -> fmt::Result {
    let setting = SETTINGS.get(index).ok_or(fmt::Error)?;
    write!(out, "{} ", setting.name)?;
    (setting.get)(out)?;
    let start = STARTS[index].get().map_or("", |start| start.as_str());
    write!(out, " {start} ")?;
    setting.takes.write(out)
}

/// Why a setting was not set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No setting has the name.
    NoSuchSetting,
    /// The setting does not take the value.
    Value,
    /// This machine cannot have the value.
    NotHere,
}

/// Sets setting `name` to `value` for process `by`, and logs it.
pub fn set(name: &str, value: &str, by: &str) -> Result<(), Refused> {
    let setting = SETTINGS.iter().find(|s| s.name == name).ok_or(Refused::NoSuchSetting)?;
    if !setting.takes.takes(value) {
        return Err(Refused::Value);
    }
    if !(setting.set)(value) {
        return Err(Refused::NotHere);
    }
    info!("Kernel settings: `{by}` set {name} to {value}");
    Ok(())
}
