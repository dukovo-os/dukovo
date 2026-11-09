// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Intel High Definition Audio (the High Definition Audio Specification,
//! revision 1.0a; ADR 0322): the controllers of Intel's and AMD's
//! chipsets, PCI class 04/03, and the codecs on their link, played
//! through as one output.
//!
//! [`Hda::new`] resets the controller, which wakes the codecs on its
//! link, starts its command rings ([`controller`]), and takes the first
//! codec with an analog output the firmware says is connected
//! ([`codec`]): its speakers, headphones and line outs, routed to its
//! converters. The output plays one stream ([`stream`]) of signed 16-bit
//! samples in two channels, at the rates every converter takes, in the
//! periods the kernel hands it, and the kernel's worker looks at it every
//! 20 ms while it plays. Its volume is the codec's own amplifiers' where
//! every way has one.
//!
//! The kernel gives the driver its registers, DMA memory and a way to
//! wait ([`Host`]), and drives it as it drives a virtio sound card:
//! configure, submit periods, start, collect what played, stop, release.
//! The codec's specifics come from the specification and, for the few
//! codecs that need more, FreeBSD's BSD-licensed hdaa(4) ([`quirks`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod codec;
pub mod controller;
pub mod quirks;
mod regs;
pub mod stream;
pub mod verb;

#[cfg(test)]
mod tests;

use core::fmt;

use radix_hw::pci::{Function, capability};
use radix_hw::{Dma, Mmio};

pub use codec::{Codec, Role, Route};
pub use controller::Controller;
pub use stream::Stream;

/// The PCI class and subclass of an HDA controller: multimedia, audio
/// device (04/03).
pub const PCI_CLASS: (u8, u8) = (0x04, 0x03);

/// Whether a PCI function of `class` (class, subclass, interface) is an
/// HDA controller.
pub fn is_hda(class: (u8, u8, u8)) -> bool {
    (class.0, class.1) == PCI_CLASS
}

/// Intel's PCI vendor ID.
const INTEL: u16 = 0x8086;

/// The PCI configuration register of Intel's controllers that picks the
/// traffic class of their DMA (TCSEL, bits 2:0).
const TCSEL: usize = 0x44;

/// The PCI configuration registers that make ATI's, AMD's and NVIDIA's
/// controllers snoop the CPU's caches: vendor, register, the bits kept,
/// the bits set (FreeBSD's hdac(4), `hdac_pcie_snoop`).
const SNOOP: [(u16, usize, u8, u8); 3] =
    [(0x1002, 0x42, 0xf8, 0x02), (0x1022, 0x42, 0xf8, 0x02), (0x10de, 0x4e, 0xf0, 0x0f)];

/// PCI Express Device Control (in the PCI Express capability): the
/// function may mark its requests No Snoop.
const DEVICE_CONTROL: usize = 0x08;
const ENABLE_NO_SNOOP: u16 = 1 << 11;

/// Readies the controller's PCI function for DMA memory the CPU caches,
/// as the kernel's is: Intel's controllers move their DMA to traffic
/// class 0 (TCSEL; FreeBSD's hdac(4) does, as some chipsets crackle or
/// stay silent otherwise), ATI's, AMD's and NVIDIA's are told to snoop,
/// and none may mark a request No Snoop, which would let it read memory
/// past the CPU's caches.
pub fn prepare(function: &Function) {
    let vendor = function.vendor_id();
    if vendor == INTEL {
        let tcsel = function.read8(TCSEL);
        function.write8(TCSEL, tcsel & 0xf8);
    }
    for (snooper, register, keep, set) in SNOOP {
        if vendor == snooper {
            let value = function.read8(register);
            function.write8(register, (value & keep) | set);
        }
    }
    if let Some(express) = function.find_capability(capability::EXPRESS) {
        let control = function.read16(express + DEVICE_CONTROL);
        function.write16(express + DEVICE_CONTROL, control & !ENABLE_NO_SNOOP);
    }
}

/// What the driver needs from the kernel.
pub trait Host {
    /// Waits `us` microseconds.
    fn delay_us(&mut self, us: u32);
    /// `pages` pages of zeroed, physically contiguous memory the
    /// controller reads and writes, below 4 GiB if `low`.
    fn alloc(&mut self, pages: usize, low: bool) -> Option<Dma>;
}

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The controller's registers read as all ones, or are fewer than it
    /// says.
    Gone,
    /// The controller did not leave its reset, or its command ring did not
    /// start.
    StuckInReset,
    /// There was no memory for the rings or the stream.
    NoMemory,
    /// A codec did not answer a command.
    NoAnswer,
    /// No codec on the link has an output the driver plays through.
    NoOutput,
    /// The controller has no stream that plays.
    NoStream,
    /// The format or the periods are not ones the output plays.
    BadFormat,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Gone => "the controller is not there",
            Error::StuckInReset => "the controller did not come out of reset",
            Error::NoMemory => "out of memory",
            Error::NoAnswer => "the codec did not answer",
            Error::NoOutput => "no codec has an analog output",
            Error::NoStream => "the controller has no output stream",
            Error::BadFormat => "the format is not the output's",
        })
    }
}

/// An HDA controller and the codec it plays through.
pub struct Hda {
    controller: Controller,
    codec: Codec,
    /// The stream, once it is set up.
    stream: Option<Stream>,
    /// The format the stream is set up for: rate, channels.
    format: (u32, u8),
    failed: bool,
}

impl Hda {
    /// Sets up the controller at `mmio` and the first codec on its link
    /// with an output. With none, the error is [`Error::NoAnswer`] if a
    /// codec did not answer, else [`Error::NoOutput`].
    pub fn new(mmio: Mmio, host: &mut dyn Host) -> Result<Hda, Error> {
        let mut controller = Controller::new(mmio, host)?;
        if controller.output_stream().is_none() {
            controller.halt(host);
            return Err(Error::NoStream);
        }
        let mut codec = None;
        let mut silent = false;
        for cad in 0..15u8 {
            if controller.codecs() & (1 << cad) == 0 {
                continue;
            }
            // A codec that does not answer is passed over (an HDMI codec
            // asleep with its display, say), but said if no other plays.
            match Codec::probe(&mut codec::Link::new(&mut controller, host, cad)) {
                Ok(Some(found)) => {
                    codec = Some(found);
                    break;
                }
                Ok(None) => {}
                Err(_) => silent = true,
            }
        }
        let Some(codec) = codec else {
            controller.halt(host);
            return Err(if silent { Error::NoAnswer } else { Error::NoOutput });
        };
        Ok(Hda { controller, codec, stream: None, format: (0, 0), failed: false })
    }

    /// The codec.
    pub fn codec(&self) -> &Codec {
        &self.codec
    }

    /// Whether the controller reaches memory above 4 GiB.
    pub fn wide(&self) -> bool {
        self.controller.wide()
    }

    /// Whether the output plays `rate` frames a second.
    pub fn plays(&self, rate: u32) -> bool {
        verb::PCM_RATES
            .iter()
            .position(|&each| each == rate)
            .is_some_and(|bit| self.codec.rates & (1 << bit) != 0)
    }

    /// The channels the output plays: two, or one if a converter is mono.
    pub fn channels(&self) -> u8 {
        if self.codec.stereo { 2 } else { 1 }
    }

    /// The most periods the stream holds.
    pub fn max_periods(&self) -> u32 {
        (regs::BDL_MAX as u32) - 2
    }

    /// Notes a codec's failure to answer, which leaves the output failed
    /// once the controller has missed three in a row.
    fn note<T>(&mut self, result: Result<T, Error>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(_) => {
                if self.controller.failed() {
                    self.failed = true;
                }
                None
            }
        }
    }

    /// Sets the output up to play `rate` frames a second in `channels`,
    /// in `count` periods of `bytes`, letting go of what it played
    /// before.
    pub fn configure(
        &mut self,
        rate: u32,
        channels: u8,
        bytes: u32,
        count: u32,
        host: &mut dyn Host,
    ) -> Result<(), Error> {
        self.release(host);
        if !self.plays(rate) || channels != self.channels() {
            return Err(Error::BadFormat);
        }
        let format = verb::format(rate, channels).ok_or(Error::BadFormat)?;
        let slot = self.controller.output_stream().ok_or(Error::NoStream)?;
        let per_second = rate * 2 * u32::from(channels);
        let mmio = self.controller.mmio().clone();
        let wide = self.controller.wide();
        let stream = Stream::new(&mmio, slot, format, bytes, count, per_second, wide, host)?;
        let codec = &self.codec;
        let set =
            codec.set_format(&mut codec::Link::new(&mut self.controller, host, codec.cad), format);
        self.note(set).ok_or(Error::NoAnswer)?;
        self.stream = Some(stream);
        self.format = (rate, channels);
        Ok(())
    }

    /// Whether a period can be handed over now.
    pub fn can_submit(&self) -> bool {
        !self.failed && self.stream.as_ref().is_some_and(Stream::can_submit)
    }

    /// Hands over a period of samples; false if it cannot be taken now.
    pub fn submit(&mut self, samples: &[u8]) -> bool {
        !self.failed && self.stream.as_mut().is_some_and(|stream| stream.submit(samples))
    }

    /// The next period that has played, at `now_ns`: Some(true) for each.
    pub fn collect(&mut self, now_ns: u64) -> Option<bool> {
        let stream = self.stream.as_mut()?;
        let played = stream.collect(self.controller.mmio(), now_ns);
        if stream.stalled() {
            self.failed = true;
        }
        played
    }

    /// Periods handed over and not yet played.
    pub fn in_flight(&self) -> u32 {
        self.stream.as_ref().map_or(0, Stream::in_flight)
    }

    /// Starts playing, at `now_ns`.
    pub fn start(&mut self, host: &mut dyn Host, now_ns: u64) -> bool {
        let mmio = self.controller.mmio().clone();
        !self.failed && self.stream.as_mut().is_some_and(|stream| stream.start(&mmio, host, now_ns))
    }

    /// Stops playing, at `now_ns`; the periods not yet played stay.
    pub fn stop(&mut self, host: &mut dyn Host, now_ns: u64) -> bool {
        let mmio = self.controller.mmio().clone();
        self.stream.as_mut().is_none_or(|stream| stream.stop(&mmio, host, now_ns))
    }

    /// Lets go of the stream and every period in it.
    pub fn release(&mut self, host: &mut dyn Host) {
        if let Some(mut stream) = self.stream.take() {
            stream.release(self.controller.mmio(), host);
        }
    }

    /// Whether the output has failed and plays nothing more.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Sets the volume, `left` and `right` percent; false if the output
    /// has no volume of its own.
    pub fn set_volume(&mut self, left: u8, right: u8, host: &mut dyn Host) -> bool {
        if !self.codec.hardware_volume {
            return false;
        }
        let codec = &self.codec;
        let mut link = codec::Link::new(&mut self.controller, host, codec.cad);
        let set = codec.set_volume(&mut link, left, right);
        self.note(set).unwrap_or(false)
    }

    /// Reads the jacks and turns the pins on or off to match: what plays
    /// now, if it changed.
    pub fn look(&mut self, host: &mut dyn Host) -> Option<Route> {
        if self.failed {
            return None;
        }
        let cad = self.codec.cad;
        let mut link = codec::Link::new(&mut self.controller, host, cad);
        let changed = self.codec.refresh(&mut link);
        let changed = self.note(changed)?;
        changed.then(|| self.codec.route_now())
    }

    /// Stops the stream and puts the controller in reset: what the kernel
    /// does before it lets the driver go.
    pub fn halt(&mut self, host: &mut dyn Host) {
        self.release(host);
        self.controller.halt(host);
    }
}
