// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Intel High Definition Audio controllers (`radix-hda`, ADR 0322), the
//! outputs `hda0` and on.
//!
//! [`bind`] readies the controller's PCI function (snooping, traffic
//! class), maps its registers, turns on bus mastering, and sets it and
//! the first codec on its link with an analog output up. The controller
//! raises no interrupts: the worker's looks, every 20 ms while it plays,
//! find the periods played, and every quarter of a second the jacks are
//! read, so that headphones plugged in take the sound from the speakers.
//! A codec's answer is waited for in steps of 10 microseconds, without
//! sleeping, for at most 100 ms.

use alloc::boxed::Box;

use radix_abi::audio::KIND_HDA;
use radix_audio::{Format, Fragments, RATES, Volume};
use radix_hal::{Devices, Timer};
use radix_hda::{Error, Hda, Host, Role, Route, quirks};
use radix_hw::Dma;
use radix_hw::pci::{Address, Bar, Function};
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};

use super::{Caps, Driver};
use crate::Arch;
use crate::sched;

/// How often the jacks are read, at most.
const JACK_LOOK_NS: u64 = 250_000_000;

/// What the driver needs of the kernel, for the controller `source`.
struct Kernel {
    source: Address,
}

impl Host for Kernel {
    fn delay_us(&mut self, us: u32) {
        if us >= 1000 {
            sched::sleep_ns(u64::from(us) * 1000);
        } else {
            Arch::delay_us(u64::from(us));
        }
    }

    fn alloc(&mut self, pages: usize, low: bool) -> Option<Dma> {
        <Arch as Devices>::alloc_dma(self.source, pages, low)
    }
}

/// An HDA controller as the worker drives it.
struct Output {
    hda: Hda,
    kernel: Kernel,
    /// When the jacks were last read.
    looked_ns: u64,
}

/// What went wrong, in words, for the log.
fn why(err: Error) -> &'static str {
    match err {
        Error::BadFormat => "the format is not the codec's",
        Error::NoMemory => "out of memory",
        Error::NoAnswer => "the codec did not answer",
        _ => "the controller failed",
    }
}

/// What `route` plays through, in words.
fn describe(route: Route) -> &'static str {
    match (route.speakers, route.headphones, route.line) {
        (_, true, _) => "the headphones",
        (true, false, true) => "the speakers and the line out",
        (true, false, false) => "the speakers",
        (false, false, true) => "the line out",
        (false, false, false) => "nothing: no output is connected",
    }
}

impl Driver for Output {
    fn max_periods(&self) -> u32 {
        self.hda.max_periods()
    }

    fn configure(&mut self, format: Format, fragments: Fragments) -> Result<(), &'static str> {
        self.hda
            .configure(
                format.rate,
                format.channels,
                fragments.bytes,
                fragments.count,
                &mut self.kernel,
            )
            .map_err(why)
    }

    fn can_submit(&self) -> bool {
        self.hda.can_submit()
    }

    fn submit(&mut self, samples: &[u8]) -> bool {
        self.hda.submit(samples)
    }

    // The engine plays the buffer round and round: a period written is
    // played when the engine reaches it, with nothing to tell.
    fn notify(&mut self) {}

    fn collect(&mut self) -> Option<bool> {
        self.hda.collect(sched::now_ns())
    }

    fn in_flight(&self) -> u32 {
        self.hda.in_flight()
    }

    fn start(&mut self) -> bool {
        self.hda.start(&mut self.kernel, sched::now_ns())
    }

    fn stop(&mut self) -> bool {
        self.hda.stop(&mut self.kernel, sched::now_ns())
    }

    fn release(&mut self) {
        self.hda.release(&mut self.kernel);
    }

    fn failed(&self) -> bool {
        self.hda.failed()
    }

    fn set_volume(&mut self, volume: Volume) -> bool {
        self.hda.set_volume(volume.left, volume.right, &mut self.kernel)
    }

    fn look(&mut self, name: &str) {
        let now = sched::now_ns();
        if now.saturating_sub(self.looked_ns) < JACK_LOOK_NS {
            return;
        }
        self.looked_ns = now;
        if let Some(route) = self.hda.look(&mut self.kernel) {
            info!("Audio: {name} plays through {}", describe(route));
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // The stream and the rings stop, and the controller goes into
        // reset, before their memory goes.
        self.hda.halt(&mut self.kernel);
    }
}

/// Sets up the HDA controller `function` as an output, saying why in the
/// log if it cannot be.
pub fn bind(function: &Function) {
    match try_bind(function) {
        Ok(()) => {}
        // A display's controller, with HDMI codecs only, is for later.
        Err(why @ "no codec has an analog output") => {
            info!("Audio: the HD Audio controller at {} is not used: {why}", function.address());
        }
        Err(why) => {
            warn!(
                "Audio: could not set up the HD Audio controller at {}: {why}",
                function.address()
            );
        }
    }
}

/// What a pin of `role` is, in words, for the log.
fn role_name(role: Role) -> &'static str {
    match role {
        Role::Speaker => "speaker",
        Role::Headphones => "headphones",
        Role::Line => "line out",
    }
}

/// [`bind`]. Its frame holds the driver before it goes on the heap: kept
/// out of `init`'s (tools/xtask/src/frames.rs allows it).
#[inline(never)]
fn try_bind(function: &Function) -> Result<(), &'static str> {
    radix_hda::prepare(function);
    function.enable_memory_and_dma();
    let Some(Bar::Memory { address, size, .. }) = function.bar(0) else {
        return Err("its registers are not in memory");
    };
    let size = usize::try_from(size).map_err(|_| "its registers are too large")?;
    let mmio = <Arch as Devices>::map_device(address, size).ok_or("could not map its registers")?;
    let source = function.address();
    let mut kernel = Kernel { source };
    let hda = match Hda::new(mmio, &mut kernel) {
        Ok(hda) => hda,
        Err(Error::NoOutput) => return Err("no codec has an analog output"),
        Err(err) => return Err(why(err)),
    };
    // From here a failure drops the driver, which stops the controller.
    let driver = Output { hda, kernel, looked_ns: sched::now_ns() };
    let codec = driver.hda.codec();
    let mut rates = 0;
    for (bit, &rate) in RATES.iter().enumerate() {
        if driver.hda.plays(rate) {
            rates |= 1 << bit;
        }
    }
    let channels = driver.hda.channels();
    let caps = Caps {
        rates,
        channels_min: channels,
        channels_max: channels,
        kind: KIND_HDA,
        model: quirks::model(codec.vendor),
        hardware_volume: codec.hardware_volume,
    };
    // What the log says of it, before the driver goes to the worker.
    let (vendor, subsystem) = (codec.vendor, codec.subsystem);
    let route = codec.route_now();
    let mut pins = TextBuf::<120>::new();
    for (i, pin) in codec.pins().enumerate() {
        let _ = core::fmt::Write::write_fmt(
            &mut pins,
            format_args!(
                "{}{} {:#04x} from {:#04x}",
                if i == 0 { "" } else { ", " },
                role_name(pin.role),
                pin.nid,
                pin.dac()
            ),
        );
    }
    let index = super::reserve().ok_or("too many sound outputs")?;
    let driver =
        Box::try_new_in(driver, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    let driver: Box<dyn Driver, Kmalloc> = driver;
    let name = super::add(index, "hda", caps, driver)?;
    let lowest = RATES.iter().enumerate().find(|&(bit, _)| caps.rates & (1 << bit) != 0);
    let highest = RATES.iter().enumerate().rev().find(|&(bit, _)| caps.rates & (1 << bit) != 0);
    info!(
        "Audio: {name} is an HD Audio codec at {source} ({} {:04x}:{:04x}, card {:04x}:{:04x}): \
         {}; {} to {} frames a second, volume {}, polled",
        quirks::model(vendor),
        vendor >> 16,
        vendor & 0xffff,
        subsystem >> 16,
        subsystem & 0xffff,
        pins.as_str(),
        lowest.map_or(0, |(_, rate)| *rate),
        highest.map_or(0, |(_, rate)| *rate),
        if caps.hardware_volume { "on the codec" } else { "scaled by the kernel" },
    );
    info!("Audio: {name} plays through {}", describe(route));
    Ok(())
}
