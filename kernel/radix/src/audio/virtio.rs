// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Virtio sound cards (`radix-virtio`'s `snd`, ADR 0321), the outputs
//! `virtio0` and on.
//!
//! [`bind`] maps the card's registers, turns on bus mastering, gives it
//! one MSI-X interrupt for the periods it has played (without one, the
//! worker's looks every 20 ms while it plays find them), and sets it up.
//! A control request's answer is waited for in steps of a millisecond,
//! for at most a second, on the worker's thread or, while the card is set
//! up, the boot CPU's.

use alloc::boxed::Box;

use radix_abi::audio::KIND_VIRTIO;
use radix_audio::{Format, Fragments, RATES, Volume};
use radix_hal::Devices;
use radix_hw::pci::{Address, Bar, Function, MsiX};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_virtio::snd::Snd;
use radix_virtio::{Error, NO_VECTOR, Transport};

use super::{ANSWER_MS, Caps, Driver};
use crate::Arch;
use crate::sched;

/// A virtio sound card as the worker drives it.
struct Virtio {
    snd: Snd,
    /// The card's PCI function, which its DMA memory is for.
    source: Address,
    /// Its interrupt vector, if it has one.
    vector: Option<u32>,
}

/// A `wait` for the card's answers: a millisecond at a time, at most
/// [`ANSWER_MS`] of them.
fn waiter() -> impl FnMut() -> bool {
    let mut waited = 0;
    move || {
        if waited >= ANSWER_MS {
            return false;
        }
        waited += 1;
        sched::sleep_ns(1_000_000);
        true
    }
}

/// What went wrong, in words, for the log.
fn why(err: Error) -> &'static str {
    match err {
        Error::NoMemory => "out of memory",
        Error::Refused(_) => "the card refused the format",
        Error::NoAnswer => "the card did not answer",
        Error::BadRequest => "the format is not the card's",
        _ => "the card failed",
    }
}

impl Driver for Virtio {
    fn max_periods(&self) -> u32 {
        self.snd.max_periods()
    }

    fn configure(&mut self, format: Format, fragments: Fragments) -> Result<(), &'static str> {
        let rate = RATES.iter().position(|&rate| rate == format.rate).ok_or("no such rate")?;
        let source = self.source;
        let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(source, pages, false);
        self.snd
            .configure(
                rate as u8,
                format.channels,
                fragments.bytes,
                fragments.count,
                &mut alloc,
                &mut waiter(),
            )
            .map_err(why)
    }

    fn can_submit(&self) -> bool {
        self.snd.can_submit()
    }

    fn submit(&mut self, samples: &[u8]) -> bool {
        self.snd.submit(samples).is_ok()
    }

    fn notify(&mut self) {
        self.snd.notify();
    }

    fn collect(&mut self) -> Option<bool> {
        self.snd.collect()
    }

    fn in_flight(&self) -> u32 {
        self.snd.in_flight()
    }

    fn start(&mut self) -> bool {
        self.snd.start(&mut waiter()).is_ok()
    }

    fn stop(&mut self) -> bool {
        self.snd.stop(&mut waiter()).is_ok()
    }

    fn release(&mut self) {
        let _ = self.snd.release(&mut waiter());
    }

    fn failed(&self) -> bool {
        self.snd.failed()
    }

    // A virtio card has no volume: the kernel scales the samples.
    fn set_volume(&mut self, _volume: Volume) -> bool {
        false
    }
}

impl Drop for Virtio {
    fn drop(&mut self) {
        // The card first, so that it raises nothing after its vector goes.
        self.snd.reset();
        if let Some(vector) = self.vector {
            <Arch as Devices>::free_interrupt(vector);
        }
    }
}

/// Sets up the virtio sound card `function` as an output, saying why in
/// the log if it cannot be.
pub fn bind(function: &Function) {
    if let Err(why) = try_bind(function) {
        warn!("Audio: could not set up the virtio sound card at {}: {why}", function.address());
    }
}

/// [`bind`]. Its frame holds the driver before it goes on the heap: kept
/// out of `init`'s (tools/xtask/src/frames.rs allows it).
#[inline(never)]
fn try_bind(function: &Function) -> Result<(), &'static str> {
    let index = super::reserve().ok_or("too many sound outputs")?;
    function.enable_memory_and_dma();
    let mut map_bar = |_: usize, bar: Bar| match bar {
        Bar::Memory { address, size, .. } => {
            <Arch as Devices>::map_device(address, usize::try_from(size).ok()?)
        }
        Bar::Io { .. } => None,
    };
    let transport = Transport::find(function, &mut map_bar).map_err(|_| "no modern transport")?;
    // One MSI-X entry, for the periods played, if the card and the kernel
    // have them; otherwise the worker looks.
    let interrupt = MsiX::find(function).and_then(|msix| {
        let (bar, offset) = msix.table_location();
        let Some(Bar::Memory { address, .. }) = function.bar(bar) else { return None };
        let table = <Arch as Devices>::map_device(address + offset, msix.table_len())?;
        let (vector, message) =
            <Arch as Devices>::allocate_interrupt(function.address(), super::interrupt, index)?;
        msix.enable_masked(function);
        msix.set_entry(&table, 0, message);
        msix.unmask_all(function);
        Some(vector)
    });
    let entry = if interrupt.is_some() { 0 } else { NO_VECTOR };
    let source = function.address();
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(source, pages, false);
    let snd = match Snd::new(transport, entry, &mut alloc, &mut waiter()) {
        Ok(snd) => snd,
        Err(err) => {
            if let Some(vector) = interrupt {
                <Arch as Devices>::free_interrupt(vector);
            }
            return Err(why(err));
        }
    };
    let found = snd.output();
    let caps = Caps {
        rates: found.rates,
        channels_min: found.channels_min,
        channels_max: found.channels_max,
        kind: KIND_VIRTIO,
        model: "virtio sound card",
        hardware_volume: false,
    };
    // From here a failure drops the driver, which lets the card go.
    let driver = Virtio { snd, source, vector: interrupt };
    let driver =
        Box::try_new_in(driver, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    let driver: Box<dyn Driver, Kmalloc> = driver;
    let name = super::add(index, "virtio", caps, driver)?;
    let lowest = RATES.iter().enumerate().find(|&(bit, _)| caps.rates & (1 << bit) != 0);
    let highest = RATES.iter().enumerate().rev().find(|&(bit, _)| caps.rates & (1 << bit) != 0);
    let mut how = TextBuf::<24>::new();
    let _ = match interrupt {
        Some(vector) => {
            core::fmt::Write::write_fmt(&mut how, format_args!("MSI-X vector {vector:#x}"))
        }
        None => core::fmt::Write::write_str(&mut how, "polled"),
    };
    info!(
        "Audio: {name} is a virtio sound card at {source}: stream {}, {} to {} channels, {} to {} \
         frames a second, {how}",
        found.stream,
        caps.channels_min,
        caps.channels_max,
        lowest.map_or(0, |(_, rate)| *rate),
        highest.map_or(0, |(_, rate)| *rate),
    );
    Ok(())
}
