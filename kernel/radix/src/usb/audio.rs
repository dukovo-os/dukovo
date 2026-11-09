// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB speakers, headsets and DACs (Phase 14, ADR 0323): the output
//! `usb0` and on, as the controller's thread drives it
//! (`radix_usb::audio`). The output's worker is on the other side of the
//! link (`crate::audio::usb`).
//!
//! Once a device is configured, [`Controller::start_audio`] reads the
//! range of its feature unit's volume, the rates its clock takes (UAC2),
//! and which of the kernel's rates its endpoint has room for, gives it a
//! ring of samples, and adds the output, which takes the place of a USB
//! output unplugged before, with its name. The thread then does what the
//! worker asks, at each look ([`Controller::audio`]):
//!
//! - A format: the streaming interface goes to its playing setting (from
//!   the one with no endpoint, which drops whatever was queued), the rate
//!   is set (UAC1 on the endpoint, UAC2 on the clock, through a clock
//!   selector), and a pacer sizes the packets.
//! - While the worker wants it to play, every service interval's packet
//!   is queued, about 16 ms ahead, as an isochronous TD of the ring's
//!   bytes, an event asked for every 2 ms or so and on the last. A packet
//!   the samples written do not fill is filled with silence when nothing
//!   else is queued: devices take only whole packets.
//! - An asynchronous endpoint's feedback endpoint is read once per
//!   interval of its own, and the pacer follows it.
//! - The volume: each channel's level on the feature unit, the attenuation
//!   radix-audio says for the percent; mute at 0 if the unit can.
//!
//! Each TD's event says what has played, so the worker is woken to hand
//! the periods back. Unplugging the device takes the output away, and
//! its ring goes once both sides have let go.

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::fmt::Write;

use radix_abi::audio::KIND_USB;
use radix_audio::{RATES, Volume, attenuation};
use radix_hw::Dma;
use radix_klog::{TextBuf, info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_usb::Speed;
use radix_usb::audio::{
    Kind, MAX_PIECES, Output, Pacer, Range, Rates, Sync, Version, level, request,
};
use radix_xhci::{code, endpoint_index, interval};

use super::controller::{Controller, Name, Why, alloc};
use crate::audio::usb::{Link, Output as Proxy, Wanted};
use crate::audio::{self, Caps};

/// The pages of the ring of samples: room for two of the largest periods
/// of 32-bit samples, or, if memory is short, a quarter of that.
const RING_PAGES: [usize; 2] = [64, 16];
/// How far ahead packets are queued, in microseconds.
const AHEAD_US: u32 = 16_000;
/// How often a TD asks for an event, in microseconds.
const EVENT_US: u32 = 2_000;
/// The bytes a feedback endpoint's report takes, at most.
const FEEDBACK_BYTES: usize = 4;

/// What the thread keeps of a USB output it drives.
pub(super) struct Sound {
    link: Arc<Link, Kmalloc>,
    /// What the device's descriptors say of the output.
    output: Output,
    /// The output's index.
    index: usize,
    /// The data endpoint's and the feedback endpoint's device context
    /// indices.
    data: u8,
    feedback: Option<u8>,
    /// The feedback TD in flight, 0 for none, and where its report lands.
    feedback_trb: u64,
    feedback_memory: Dma,
    /// Packets' sizes, once a format is set up.
    pacer: Option<Pacer>,
    /// Microframes a service interval.
    microframes: u32,
    /// The most bytes a service interval's packet holds.
    room: u32,
    full_speed: bool,
    /// Whether the streaming interface is in its playing setting.
    streaming: bool,
    /// The last of the worker's asks done.
    seen: u64,
    /// The range of each channel's volume, the master's first.
    ranges: [Option<Range>; 3],
    /// TDs queued since the last that asked for an event.
    since_event: u32,
    /// Errors in a row on the data endpoint.
    errors: u32,
}

/// What `kind` is, in words, for `model`: a static name.
fn model(kind: Kind) -> &'static str {
    match kind {
        Kind::Speaker => "USB speaker",
        Kind::Headphones => "USB headphones",
        Kind::Headset => "USB headset",
        Kind::Line => "USB line out",
        Kind::Digital => "USB digital out",
        Kind::Other => "USB audio",
    }
}

/// The bytes a service interval of `endpoint` can carry at `speed`: its
/// packet, times its transactions a microframe (high speed) or its burst
/// and Mult (SuperSpeed).
fn interval_room(output: &Output, speed: Speed) -> u32 {
    let endpoint = output.setting.endpoint;
    let packet = u32::from(endpoint.max_packet());
    match speed {
        Speed::High => packet * (u32::from(endpoint.extra_transactions()) + 1),
        Speed::Super | Speed::SuperPlus => {
            let mult = u32::from(endpoint.companion_attributes & 3) + 1;
            packet * (u32::from(endpoint.max_burst) + 1) * mult
        }
        _ => packet,
    }
}

impl Controller {
    /// Starts the sound output `output` of the configured device in slot
    /// `slot`: reads what it can do and adds it.
    #[inline(never)]
    pub(super) fn start_audio(&mut self, slot: u8, output: Output) -> Result<(), Why> {
        let at = self.find(slot).ok_or(Why::Device("it is gone"))?;
        let Controller { devices, xhci, host, buffer, source, bus, .. } = self;
        let attached = &mut devices[at];
        let path = attached.path;
        let device = &mut attached.device;
        let speed = device.place().speed;
        let setting = output.setting;
        let microframes = 1u32 << interval(&setting.endpoint, speed);
        let room = interval_room(&output, speed);
        let frame = setting.frame_bytes();

        // The rates the setting plays, that the clock takes (UAC2), and
        // that a packet has room for.
        let clock_ranges = match (output.version, output.clock) {
            (Version::Two, Some(clock)) => {
                let setup = request::rate_ranges(output.control, clock.source);
                let length = usize::from(setup.length).min(buffer.len());
                device.control_in(xhci, host, setup, &mut buffer[..length]).ok()
            }
            _ => None,
        };
        let mut rates = 0;
        for (bit, &rate) in RATES.iter().enumerate() {
            let fits = (rate * microframes).div_ceil(8000) * frame <= room;
            let plays = match (setting.rates, clock_ranges) {
                (Rates::Clock, Some(came)) => request::ranges_hold(&buffer[..came], rate),
                // A clock that does not say plays 48 kHz, as most do.
                (Rates::Clock, None) => rate == 48000,
                (rates, _) => rates.plays(rate),
            };
            if fits && plays {
                rates |= 1 << bit;
            }
        }
        if rates == 0 {
            return Err(Why::Device("it plays none of the kernel's rates"));
        }

        // The volume's range on each channel that has one.
        let mut ranges = [None; 3];
        if let Some(feature) = output.feature {
            for channel in feature.volume_channels(setting.channels).filter(|&c| c < 3) {
                ranges[usize::from(channel)] =
                    volume_range(device, xhci, host, buffer, &output, feature.unit, channel);
            }
        }
        let hardware_volume = ranges.iter().any(Option::is_some);

        let memory = RING_PAGES
            .iter()
            .find_map(|&pages| alloc(*source)(pages, xhci.below_4g()))
            .ok_or(Why::Device("out of memory"))?;
        let feedback_memory =
            alloc(*source)(1, xhci.below_4g()).ok_or(Why::Device("out of memory"))?;
        let link = Link::new(*bus - 1, memory, frame, setting.subframe).map_err(Why::Device)?;
        let proxy = Proxy::new(link.clone(), hardware_volume).map_err(Why::Device)?;
        let proxy = Box::try_new_in(proxy, HEAP.with(Flags::MAY_SLEEP))
            .map_err(|_| Why::Device("out of memory"))?;
        let caps = Caps {
            rates,
            channels_min: setting.channels,
            channels_max: setting.channels,
            kind: KIND_USB,
            model: model(output.kind),
            hardware_volume,
        };
        // The device's side, on the heap before the output is added, so
        // that nothing can fail once it has been.
        let sound = Sound {
            link: link.clone(),
            output,
            index: usize::MAX,
            data: endpoint_index(setting.endpoint.address),
            feedback: setting.feedback.map(|f| endpoint_index(f.address)),
            feedback_trb: 0,
            feedback_memory,
            pacer: None,
            microframes,
            room,
            full_speed: matches!(speed, Speed::Full | Speed::Low),
            streaming: false,
            seen: 0,
            ranges,
            since_event: 0,
            errors: 0,
        };
        let mut sound = Box::try_new_in(sound, HEAP.with(Flags::MAY_SLEEP))
            .map_err(|_| Why::Device("out of memory"))?;
        let (index, name) = audio::attach("usb", caps, proxy).map_err(Why::Device)?;
        sound.index = index;
        link.set_output(index);

        let lowest = RATES.iter().enumerate().find(|&(bit, _)| rates & (1 << bit) != 0);
        let highest = RATES.iter().enumerate().rev().find(|&(bit, _)| rates & (1 << bit) != 0);
        let mut how = TextBuf::<32>::new();
        let _ = match setting.sync {
            Sync::Asynchronous if setting.feedback.is_some() => {
                how.write_str("its own clock, fed back")
            }
            Sync::Asynchronous => how.write_str("its own clock"),
            Sync::Adaptive => how.write_str("the host's clock"),
            Sync::Synchronous | Sync::None => how.write_str("the bus's clock"),
        };
        info!(
            "Audio: {name} is a {} at {}, USB Audio Class {}: {} channel{} of {} bits, {} to {} \
             frames a second on {how}, volume {}",
            model(output.kind),
            Name(*bus, path),
            if output.version == Version::One { 1 } else { 2 },
            setting.channels,
            if setting.channels == 1 { "" } else { "s" },
            setting.bits,
            lowest.map_or(0, |(_, rate)| *rate),
            highest.map_or(0, |(_, rate)| *rate),
            if hardware_volume { "on the device" } else { "scaled by the kernel" },
        );
        attached.audio = Some(sound);
        Ok(())
    }

    /// Does what each USB output's worker asks, and queues its packets;
    /// says whether anything moved.
    pub(super) fn audio(&mut self) -> bool {
        let mut any = false;
        for at in 0..self.devices.len() {
            if self.devices[at].audio.is_none() {
                continue;
            }
            any |= self.audio_ask(at);
            any |= self.audio_volume(at);
            any |= self.audio_queue(at);
        }
        any
    }

    /// Does the worker's last ask of the output of the device at `at`, if
    /// it is not done: sets the stream up for a format, or lets go of it.
    fn audio_ask(&mut self, at: usize) -> bool {
        let Some(sound) = self.devices[at].audio.as_ref() else { return false };
        let (ask, wanted) = {
            let shared = sound.link.shared.lock();
            (shared.asked, shared.wanted)
        };
        if ask == sound.seen {
            return false;
        }
        let answer = self.audio_setup(at, wanted);
        let place = Name(self.bus, self.devices[at].path);
        let Some(sound) = self.devices[at].audio.as_mut() else { return true };
        sound.seen = ask;
        if let Err(why) = &answer {
            warn!("USB: {place}: {} cannot play as asked: {why}", model(sound.output.kind));
        }
        let mut shared = sound.link.shared.lock();
        shared.answered = ask;
        shared.answer = answer.map_err(|why| match why {
            Why::Device(why) => why,
            _ => "the device refused it",
        });
        drop(shared);
        sound.link.wake_worker();
        true
    }

    /// Puts the streaming interface of the output of the device at `at` in
    /// the setting with no endpoint, dropping what was queued, and then,
    /// for `wanted`, in its playing setting at the rate wanted.
    #[inline(never)]
    fn audio_setup(&mut self, at: usize, wanted: Option<Wanted>) -> Result<(), Why> {
        let Controller { devices, xhci, host, source, .. } = self;
        let attached = &mut devices[at];
        let Some(sound) = attached.audio.as_mut() else { return Ok(()) };
        let device = &mut attached.device;
        let output = sound.output;
        let mut alloc = alloc(*source);
        if sound.streaming {
            let mut dropped = [sound.data, 0];
            let count = if let Some(feedback) = sound.feedback {
                dropped[1] = feedback;
                2
            } else {
                1
            };
            sound.streaming = false;
            device.set_interface(
                xhci,
                host,
                &mut alloc,
                (output.streaming, 0),
                &dropped[..count],
                &[],
            )?;
        }
        {
            let mut shared = sound.link.shared.lock();
            shared.stream.clear();
            shared.periods.clear();
        }
        sound.feedback_trb = 0;
        sound.pacer = None;
        sound.since_event = 0;
        let Some(wanted) = wanted else { return Ok(()) };
        let setting = output.setting;
        if wanted.format.channels != setting.channels {
            return Err(Why::Device("it plays another number of channels"));
        }
        let rate = wanted.format.rate;
        let endpoints = [setting.endpoint, setting.feedback.unwrap_or_default()];
        let count = if setting.feedback.is_some() { 2 } else { 1 };
        device.set_interface(
            xhci,
            host,
            &mut alloc,
            (output.streaming, setting.alternate),
            &[],
            &endpoints[..count],
        )?;
        sound.streaming = true;
        match (output.version, output.clock) {
            (Version::One, _) if setting.rate_control => {
                let (setup, data) = request::set_rate_v1(setting.endpoint.address, rate);
                device.control_out(xhci, host, setup, &data)?;
            }
            (Version::Two, Some(clock)) => {
                if let Some(selector) = clock.selector {
                    let (setup, data) = request::select_clock(output.control, selector, 1);
                    // A selector that cannot be set is on its input.
                    let _ = device.control_out(xhci, host, setup, &data);
                }
                if clock.programmable {
                    let (setup, data) = request::set_rate_v2(output.control, clock.source, rate);
                    device.control_out(xhci, host, setup, &data)?;
                }
            }
            _ => {}
        }
        let most = sound.room / setting.frame_bytes();
        sound.pacer = Some(Pacer::new(rate, sound.microframes, most));
        Ok(())
    }

    /// Sends the volume the worker set, if it set one, to the output of
    /// the device at `at`.
    fn audio_volume(&mut self, at: usize) -> bool {
        let Controller { devices, xhci, host, bus, .. } = self;
        let attached = &mut devices[at];
        let Some(sound) = attached.audio.as_mut() else { return false };
        let Some(volume) = sound.link.shared.lock().volume.take() else { return false };
        let Some(feature) = sound.output.feature else { return false };
        let channels = sound.output.setting.channels;
        let control = sound.output.control;
        let device = &mut attached.device;
        let mut failed = false;
        let muted = volume == Volume::MUTED;
        for channel in feature.mute_channels(channels) {
            let (setup, data) = request::set_mute(control, feature.unit, channel, muted);
            failed |= device.control_out(xhci, host, setup, &data).is_err();
        }
        for (channel, range) in sound.ranges.iter().enumerate() {
            let Some(range) = *range else { continue };
            // The master follows the louder side; a mono channel, both.
            let percent = match (channel, channels) {
                (1, 2) => volume.left,
                (2, _) => volume.right,
                _ => volume.left.max(volume.right),
            };
            // In 1/256 dB, from quarters.
            let down = u32::from(attenuation(percent)) * 64;
            let level = level(range, down);
            let (setup, data) = request::set_volume(control, feature.unit, channel as u8, level);
            failed |= device.control_out(xhci, host, setup, &data).is_err();
        }
        if failed {
            warn!(
                "USB: {}: {} did not take the volume",
                Name(*bus, attached.path),
                model(sound.output.kind)
            );
        }
        true
    }

    /// Queues the packets of the output of the device at `at` that its
    /// ring holds, as far ahead as [`AHEAD_US`], and reads its feedback.
    fn audio_queue(&mut self, at: usize) -> bool {
        let Controller { devices, xhci, .. } = self;
        let attached = &mut devices[at];
        let Some(sound) = attached.audio.as_mut() else { return false };
        let device = &mut attached.device;
        let Some(pacer) = sound.pacer.as_mut() else { return false };
        let mut shared = sound.link.shared.lock();
        if !shared.running || !sound.streaming {
            return false;
        }
        let interval_us = sound.microframes * 125;
        let ahead = (AHEAD_US / interval_us).max(2) as usize;
        let every = (EVENT_US / interval_us).max(1);
        let frame = shared.stream.frame();
        let mut queued = 0;
        let mut pieces = [(0, 0); MAX_PIECES];
        while shared.stream.in_flight() < ahead {
            let frames = pacer.peek_frames();
            let mut next = shared.stream.next_packet(frames, false, &mut pieces);
            if next.is_none() && shared.stream.in_flight() == 0 && shared.stream.waiting() > 0 {
                // The last of the sound: made a whole packet with silence.
                let missing = (frames * frame).saturating_sub(shared.stream.waiting());
                if shared.stream.room() >= missing {
                    let at = shared.stream.write_offset() as usize;
                    let size = shared.stream.size() as usize;
                    let first = (missing as usize).min(size - at);
                    zero(&shared.memory, at, first);
                    zero(&shared.memory, 0, missing as usize - first);
                    shared.stream.wrote(missing);
                    next = shared.stream.next_packet(frames, false, &mut pieces);
                }
            }
            let Some((count, bytes)) = next else { break };
            // An event every few milliseconds, and on the last packet
            // there is for now.
            let after = shared.stream.waiting() - bytes;
            let last = after < frames * frame || shared.stream.in_flight() + 1 >= ahead;
            let event = sound.since_event + 1 >= every || last;
            match device.queue_isoch(xhci, sound.data, &pieces[..count], event) {
                Ok(td) => {
                    shared.stream.queued_packet(bytes, count, td.last);
                    pacer.next_frames();
                    sound.since_event = if event { 0 } else { sound.since_event + 1 };
                    queued += 1;
                }
                Err(_) => break,
            }
        }
        drop(shared);
        if queued > 0 {
            device.doorbell(xhci, sound.data);
        }
        // The device's rate, once an interval of the feedback endpoint.
        if let Some(feedback) = sound.feedback
            && sound.feedback_trb == 0
        {
            let report = [(sound.feedback_memory.phys(), FEEDBACK_BYTES as u32)];
            if let Ok(td) = device.queue_isoch(xhci, feedback, &report, true) {
                sound.feedback_trb = td.last;
                device.doorbell(xhci, feedback);
            }
        }
        queued > 0
    }

    /// Whether the transfer at `trb` on `endpoint` of the device at `at`
    /// is its sound output's; if so, notes what it played, or reads the
    /// rate it fed back.
    pub(super) fn audio_transfer(
        &mut self,
        at: usize,
        trb: u64,
        endpoint: u8,
        code: u8,
        residual: u32,
    ) -> bool {
        let bus = self.bus;
        let attached = &mut self.devices[at];
        let Some(sound) = attached.audio.as_mut() else { return false };
        if Some(endpoint) == sound.feedback && (trb == sound.feedback_trb || trb == 0) {
            sound.feedback_trb = 0;
            if matches!(code, code::SUCCESS | code::SHORT_PACKET)
                && let Some(pacer) = sound.pacer.as_mut()
            {
                let came = FEEDBACK_BYTES.saturating_sub(residual as usize);
                let mut report = [0u8; FEEDBACK_BYTES];
                sound.feedback_memory.read(0, &mut report[..came]);
                pacer.feedback(&report[..came], sound.full_speed);
            }
            return true;
        }
        if endpoint != sound.data {
            return false;
        }
        {
            let mut shared = sound.link.shared.lock();
            match code {
                // The controller ran out of TDs, or let them go.
                code::RING_UNDERRUN | code::RING_OVERRUN => shared.stream.all_done(),
                code::STOPPED | code::STOPPED_LENGTH_INVALID | code::STOPPED_SHORT_PACKET => {}
                code::SUCCESS | code::SHORT_PACKET | code::MISSED_SERVICE => {
                    sound.errors = 0;
                    shared.stream.done_through(trb);
                }
                // A packet lost on the bus is lost: the TD is done.
                failed => {
                    sound.errors += 1;
                    shared.stream.done_through(trb);
                    if sound.errors == 1 {
                        warn!(
                            "USB: {}: {} lost a packet: {}",
                            Name(bus, attached.path),
                            model(sound.output.kind),
                            code::name(failed)
                        );
                    }
                }
            }
        }
        sound.link.wake_worker();
        true
    }
}

/// Zeroes `len` bytes of `memory` from `at`.
fn zero(memory: &Dma, at: usize, len: usize) {
    const SILENCE: [u8; 256] = [0; 256];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(SILENCE.len());
        memory.write(at + done, &SILENCE[..n]);
        done += n;
    }
}

/// The range of the volume of channel `channel` of feature unit `unit` of
/// `output`, as the device says it: UAC1's minimum, maximum and
/// resolution, or UAC2's ranges.
fn volume_range(
    device: &mut radix_xhci::Device,
    xhci: &mut radix_xhci::Xhci,
    host: &mut super::controller::KernelHost,
    buffer: &mut [u8],
    output: &Output,
    unit: u8,
    channel: u8,
) -> Option<Range> {
    let control = output.control;
    match output.version {
        Version::One => {
            let mut ask = |what: u8| {
                let setup = request::volume_v1(what, control, unit, channel);
                let came = device.control_in(xhci, host, setup, &mut buffer[..2]).ok()?;
                (came == 2).then(|| i16::from_le_bytes([buffer[0], buffer[1]]))
            };
            let min = ask(request::GET_MIN)?;
            let max = ask(request::GET_MAX)?;
            let resolution = ask(request::GET_RES).unwrap_or(1);
            (min <= max).then_some(Range { min, max, resolution })
        }
        Version::Two => {
            let setup = request::volume_range(control, unit, channel);
            let length = usize::from(setup.length).min(buffer.len());
            let came = device.control_in(xhci, host, setup, &mut buffer[..length]).ok()?;
            Range::parse_v2(&buffer[..came])
        }
    }
}

/// The output of a device that is gone, if it had one: the output goes,
/// and its worker lets go of the link. Called once its slot is given
/// back, so that the controller reads its ring no more.
pub(super) fn gone(sound: Option<Box<Sound, Kmalloc>>) {
    let Some(sound) = sound else { return };
    audio::detach(sound.index);
    sound.link.shared.lock().gone = true;
    sound.link.wake_worker();
}
