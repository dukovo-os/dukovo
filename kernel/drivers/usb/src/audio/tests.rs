// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::vec::Vec;

use proptest::prelude::*;

use super::*;
use crate::{Configuration, Setup};

/// A configuration of `body`, its descriptors after the configuration's
/// own, with `interfaces` interfaces.
fn configuration(interfaces: u8, body: &[&[u8]]) -> Vec<u8> {
    let mut bytes = std::vec![9, 2, 0, 0, interfaces, 1, 0, 0xc0, 50];
    for d in body {
        assert_eq!(usize::from(d[0]), d.len(), "{d:?}");
        bytes.extend_from_slice(d);
    }
    let total = bytes.len() as u16;
    bytes[2..4].copy_from_slice(&total.to_le_bytes());
    bytes
}

/// The output of `bytes`, read as a configuration.
fn output(bytes: &[u8]) -> Option<Output> {
    find(&Configuration::parse(bytes).unwrap())
}

/// The interface descriptor of interface `number`, setting `alternate`,
/// of the audio class's `subclass`, with `endpoints` endpoints.
fn interface(number: u8, alternate: u8, endpoints: u8, subclass: u8, protocol: u8) -> [u8; 9] {
    [9, 4, number, alternate, endpoints, 1, subclass, protocol, 0]
}

/// QEMU's `usb-audio` as it is by default: a UAC1 speaker at full speed.
/// Its input terminal (1) is the USB stream, its feature unit (2) has a
/// mute on the master channel and a volume on each of the two, and its
/// output terminal (3) is a speaker; the streaming interface's setting 1
/// plays 16-bit stereo at 48 kHz, 192 bytes a millisecond, on a
/// synchronous endpoint.
fn qemu_usb_audio() -> Vec<u8> {
    configuration(
        2,
        &[
            &interface(0, 0, 0, 1, 0),
            &[9, 0x24, 1, 0x00, 0x01, 0x2b, 0x00, 1, 1],
            &[12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 0x03, 0x00, 0, 0],
            &[13, 0x24, 6, 2, 1, 2, 0x01, 0x00, 0x02, 0x00, 0x02, 0x00, 0],
            &[9, 0x24, 3, 3, 0x01, 0x03, 0, 2, 0],
            &interface(1, 0, 0, 2, 0),
            &interface(1, 1, 1, 2, 0),
            &[7, 0x24, 1, 1, 0, 0x01, 0x00],
            &[11, 0x24, 2, 1, 2, 2, 16, 1, 0x80, 0xbb, 0x00],
            &[9, 5, 0x01, 0x0d, 192, 0, 1, 0, 0],
            &[7, 0x25, 1, 0, 0, 0, 0],
        ],
    )
}

#[test]
fn qemus_speaker_plays_16_bit_stereo_at_48_khz() {
    let out = output(&qemu_usb_audio()).unwrap();
    assert_eq!(
        (out.version, out.control, out.streaming, out.kind),
        (Version::One, 0, 1, Kind::Speaker)
    );
    let s = out.setting;
    assert_eq!((s.alternate, s.channels, s.subframe, s.bits, s.frame_bytes()), (1, 2, 2, 16, 4));
    assert!(s.rates.plays(48000));
    assert!(!s.rates.plays(44100));
    assert_eq!((s.endpoint.address, s.endpoint.max_packet(), s.endpoint.interval), (0x01, 192, 1));
    // Synchronous: its clock is the bus's, and it has no feedback; nor
    // does its endpoint take a rate.
    assert_eq!((s.sync, s.feedback, s.rate_control), (Sync::Synchronous, None, false));
    let feature = out.feature.unwrap();
    assert_eq!(feature, Feature { unit: 2, mute: 0b001, volume: 0b110 });
    // The master mutes; each channel has its own volume.
    assert_eq!(feature.mute_channels(2).collect::<Vec<_>>(), [0]);
    assert_eq!(feature.volume_channels(2).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(feature.volume_channels(1).collect::<Vec<_>>(), [1]);
    assert_eq!(out.clock, None);
}

/// A UAC2 headset at high speed, as most USB headsets of the last years
/// are: an interface association of three interfaces; a clock source
/// (0x29) whose rate the host sets, behind a clock selector (0x28); the
/// output path from the USB stream (1) through a feature unit (0x0a,
/// whose master has a volume and a mute) to the headset (0x0b); and the
/// microphone's path, from the microphone (4) to a USB stream (5), which
/// the second streaming interface sends. The first streaming interface's
/// setting 1 is 24-bit and its setting 2 16-bit, on an asynchronous
/// endpoint with a feedback endpoint.
fn uac2_headset() -> Vec<u8> {
    let general = |link: u8| [16, 0x24, 1, link, 0, 1, 1, 0, 0, 0, 2, 3, 0, 0, 0, 0];
    configuration(
        3,
        &[
            &[8, 0x0b, 0, 3, 1, 0, 0x20, 0],
            &interface(0, 0, 0, 1, 0x20),
            &[9, 0x24, 1, 0x00, 0x02, 0x04, 0x60, 0x00, 0],
            &[8, 0x24, 0x0a, 0x29, 0x03, 0x07, 0, 0],
            &[8, 0x24, 0x0b, 0x28, 1, 0x29, 0x03, 0],
            &[17, 0x24, 2, 0x01, 0x01, 0x01, 0, 0x28, 2, 3, 0, 0, 0, 0, 0, 0, 0],
            &[18, 0x24, 6, 0x0a, 0x01, 0x0f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[12, 0x24, 3, 0x0b, 0x02, 0x04, 0, 0x0a, 0x28, 0, 0, 0],
            &[17, 0x24, 2, 0x04, 0x01, 0x02, 0, 0x28, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            &[12, 0x24, 3, 0x05, 0x01, 0x01, 0, 0x04, 0x28, 0, 0, 0],
            &interface(1, 0, 0, 2, 0x20),
            &interface(1, 1, 2, 2, 0x20),
            &general(0x01),
            &[6, 0x24, 2, 1, 4, 24],
            &[7, 5, 0x01, 0x05, 0x40, 0x00, 1],
            &[8, 0x25, 1, 0, 0, 0, 0, 0],
            &[7, 5, 0x81, 0x11, 4, 0, 4],
            &interface(1, 2, 2, 2, 0x20),
            &general(0x01),
            &[6, 0x24, 2, 1, 2, 16],
            &[7, 5, 0x01, 0x05, 0x20, 0x00, 1],
            &[8, 0x25, 1, 0, 0, 0, 0, 0],
            &[7, 5, 0x81, 0x11, 4, 0, 4],
            &interface(2, 0, 0, 2, 0x20),
            &interface(2, 1, 1, 2, 0x20),
            &[16, 0x24, 1, 0x05, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0],
            &[6, 0x24, 2, 1, 2, 16],
            &[7, 5, 0x82, 0x05, 0x10, 0x00, 1],
        ],
    )
}

#[test]
fn a_uac2_headset_plays_through_its_clock_and_feedback() {
    let out = output(&uac2_headset()).unwrap();
    assert_eq!(
        (out.version, out.control, out.streaming, out.kind),
        (Version::Two, 0, 1, Kind::Headset)
    );
    // The 16-bit setting, of the two the headset's output has; the
    // microphone's interface is passed over.
    let s = out.setting;
    assert_eq!((s.alternate, s.channels, s.subframe, s.bits), (2, 2, 2, 16));
    assert_eq!(s.rates, Rates::Clock);
    assert_eq!((s.endpoint.address, s.endpoint.max_packet()), (0x01, 0x20));
    assert_eq!(s.sync, Sync::Asynchronous);
    assert_eq!(s.feedback.map(|f| f.address), Some(0x81));
    assert!(!s.rate_control);
    assert_eq!(out.feature, Some(Feature { unit: 0x0a, mute: 1, volume: 1 }));
    assert_eq!(out.feature.unwrap().volume_channels(2).collect::<Vec<_>>(), [0]);
    assert_eq!(out.clock, Some(Clock { source: 0x29, programmable: true, selector: Some(0x28) }));
}

/// A UAC1 DAC at full speed: its output path runs from the USB stream (1)
/// through a feature unit (2) and a mixer (3) that also takes the line in
/// (4, through its own feature unit 7), and a selector (5), to the line
/// out (6). Its setting plays 24-bit stereo in three bytes a sample at
/// three rates, on an asynchronous endpoint whose bSynchAddress names
/// the feedback endpoint, and the endpoint takes its rate.
fn uac1_dac() -> Vec<u8> {
    configuration(
        2,
        &[
            &interface(0, 0, 0, 1, 0),
            &[9, 0x24, 1, 0x00, 0x01, 0x40, 0x00, 1, 1],
            &[12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 0x03, 0x00, 0, 0],
            &[10, 0x24, 6, 2, 1, 1, 0x01, 0x02, 0x02, 0],
            &[13, 0x24, 4, 3, 2, 2, 7, 2, 0x03, 0x00, 0, 0, 0],
            &[12, 0x24, 2, 4, 0x03, 0x06, 0, 2, 0x03, 0x00, 0, 0],
            &[10, 0x24, 6, 7, 4, 1, 0x03, 0x00, 0x00, 0],
            &[7, 0x24, 5, 5, 1, 3, 0],
            &[9, 0x24, 3, 6, 0x03, 0x06, 0, 5, 0],
            &interface(1, 0, 0, 2, 0),
            &interface(1, 1, 2, 2, 0),
            &[7, 0x24, 1, 1, 1, 0x01, 0x00],
            &[17, 0x24, 2, 1, 2, 3, 24, 3, 0x44, 0xac, 0x00, 0x80, 0xbb, 0x00, 0x00, 0x77, 0x01],
            &[9, 5, 0x01, 0x05, 0x46, 0x02, 1, 0, 0x82],
            &[7, 0x25, 1, 0x01, 0, 0, 0],
            &[9, 5, 0x82, 0x01, 3, 0, 1, 5, 0],
        ],
    )
}

#[test]
fn a_uac1_dac_is_found_through_its_mixer_and_selector() {
    let out = output(&uac1_dac()).unwrap();
    assert_eq!((out.version, out.kind), (Version::One, Kind::Line));
    let s = out.setting;
    assert_eq!((s.channels, s.subframe, s.bits, s.frame_bytes()), (2, 3, 24, 6));
    assert!([44100, 48000, 96000].iter().all(|&r| s.rates.plays(r)));
    assert!(!s.rates.plays(32000));
    assert_eq!(s.endpoint.max_packet(), 0x246);
    assert_eq!(s.sync, Sync::Asynchronous);
    assert_eq!(s.feedback.map(|f| f.address), Some(0x82));
    assert!(s.rate_control);
    // The feature unit on the stream's path, not the line in's.
    assert_eq!(out.feature, Some(Feature { unit: 2, mute: 1, volume: 0b110 }));
    assert_eq!(out.clock, None);
}

#[test]
fn a_uac1_range_of_rates_is_read() {
    let mut bytes = qemu_usb_audio();
    // bSamFreqType 0: from 8 kHz to 48 kHz. The format descriptor grows
    // by three bytes, so the configuration is built again.
    let format = [14, 0x24, 2, 1, 2, 2, 16, 0, 0x40, 0x1f, 0x00, 0x80, 0xbb, 0x00];
    let at = bytes.windows(3).position(|w| w == [11, 0x24, 2]).unwrap();
    bytes.splice(at..at + 11, format);
    let total = bytes.len() as u16;
    bytes[2..4].copy_from_slice(&total.to_le_bytes());
    let rates = output(&bytes).unwrap().setting.rates;
    assert_eq!(rates, Rates::Range(8000, 48000));
    assert!(rates.plays(44100) && rates.plays(8000) && !rates.plays(96000));
}

#[test]
fn a_function_has_only_the_streaming_interfaces_it_lists() {
    // Function A (interfaces 0 and 1) has an output path from its USB
    // stream, terminal 1, but its stream is 8-bit, which is not played.
    // Function B (2 and 3) has one from its own terminal 1. Interface 3
    // links to terminal 1 too, but is B's: A's header lists only 1.
    let entities: [&[u8]; 3] = [
        &[12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 0x03, 0x00, 0, 0],
        &[10, 0x24, 6, 2, 1, 1, 0x03, 0x00, 0x00, 0],
        &[9, 0x24, 3, 3, 0x01, 0x03, 0, 2, 0],
    ];
    let mut body: Vec<&[u8]> = Vec::new();
    body.push(&[9, 4, 0, 0, 0, 1, 1, 0, 0]);
    body.push(&[9, 0x24, 1, 0x00, 0x01, 0x2b, 0x00, 1, 1]);
    body.extend(entities);
    body.push(&[9, 4, 1, 1, 1, 1, 2, 0, 0]);
    body.push(&[7, 0x24, 1, 1, 0, 0x01, 0x00]);
    body.push(&[11, 0x24, 2, 1, 2, 1, 8, 1, 0x80, 0xbb, 0x00]);
    body.push(&[9, 5, 0x01, 0x09, 96, 0, 1, 0, 0]);
    body.push(&[9, 4, 2, 0, 0, 1, 1, 0, 0]);
    body.push(&[9, 0x24, 1, 0x00, 0x01, 0x2b, 0x00, 1, 3]);
    body.extend(entities);
    body.push(&[9, 4, 3, 1, 1, 1, 2, 0, 0]);
    body.push(&[7, 0x24, 1, 1, 0, 0x01, 0x00]);
    body.push(&[11, 0x24, 2, 1, 2, 2, 16, 1, 0x80, 0xbb, 0x00]);
    body.push(&[9, 5, 0x02, 0x09, 192, 0, 1, 0, 0]);
    let out = output(&configuration(4, &body)).unwrap();
    assert_eq!((out.control, out.streaming, out.setting.endpoint.address), (2, 3, 0x02));
    assert_eq!(out.setting.sync, Sync::Adaptive);
}

#[test]
fn what_is_not_an_output_is_passed_over() {
    // No audio function at all: a keyboard.
    let keyboard = configuration(1, &[&[9, 4, 0, 0, 1, 3, 1, 1, 0], &[7, 5, 0x81, 3, 8, 0, 10]]);
    assert_eq!(output(&keyboard), None);
    // QEMU's speaker in 8 bits, or in six channels, or with its feature
    // unit's output terminal a USB stream (a recording's path).
    for (from, to) in [
        (&[11u8, 0x24, 2, 1, 2, 2, 16][..], &[11u8, 0x24, 2, 1, 2, 1, 8][..]),
        (&[11, 0x24, 2, 1, 2, 2, 16], &[11, 0x24, 2, 1, 6, 2, 16]),
        (&[9, 0x24, 3, 3, 0x01, 0x03], &[9, 0x24, 3, 3, 0x01, 0x01]),
    ] {
        let mut bytes = qemu_usb_audio();
        let at = bytes.windows(from.len()).position(|w| w == from).unwrap();
        bytes[at..at + to.len()].copy_from_slice(to);
        assert_eq!(output(&bytes), None, "{to:?}");
    }
    // A streaming interface that only records.
    let mut bytes = qemu_usb_audio();
    let at = bytes.windows(4).position(|w| w == [9, 5, 0x01, 0x0d]).unwrap();
    bytes[at + 2] = 0x81;
    assert_eq!(output(&bytes), None);
}

#[test]
fn requests_name_the_entity_and_the_control() {
    let bytes = |setup: Setup| setup.to_u64().to_le_bytes();
    // UAC1: 48 kHz to endpoint 1.
    let (setup, data) = request::set_rate_v1(0x01, 48000);
    assert_eq!(bytes(setup), [0x22, 0x01, 0x00, 0x01, 0x01, 0x00, 3, 0]);
    assert_eq!(data, [0x80, 0xbb, 0x00]);
    // UAC2: the rate of clock 0x29 of interface 0, and the rates it takes.
    let (setup, data) = request::set_rate_v2(0, 0x29, 96000);
    assert_eq!(bytes(setup), [0x21, 0x01, 0x00, 0x01, 0x00, 0x29, 4, 0]);
    assert_eq!(data, 96000u32.to_le_bytes());
    assert_eq!(bytes(request::rate_ranges(0, 0x29)), [0xa1, 0x02, 0x00, 0x01, 0x00, 0x29, 194, 0]);
    let (setup, data) = request::select_clock(0, 0x28, 1);
    assert_eq!((bytes(setup), data), ([0x21, 0x01, 0x00, 0x01, 0x00, 0x28, 1, 0], [1]));
    // Volume: UAC1's minimum of channel 1 of unit 2, UAC2's range of the
    // master of unit 0x0a, and setting it; and the master's mute.
    let min = request::volume_v1(request::GET_MIN, 0, 2, 1);
    assert_eq!(bytes(min), [0xa1, 0x82, 0x01, 0x02, 0x00, 0x02, 2, 0]);
    assert_eq!(
        bytes(request::volume_range(3, 0x0a, 0)),
        [0xa1, 0x02, 0x00, 0x02, 0x03, 0x0a, 98, 0]
    );
    let (setup, data) = request::set_volume(0, 2, 2, -0x0a00);
    assert_eq!((bytes(setup), data), ([0x21, 0x01, 0x02, 0x02, 0x00, 0x02, 2, 0], [0x00, 0xf6]));
    let (setup, data) = request::set_mute(0, 2, 0, true);
    assert_eq!((bytes(setup), data), ([0x21, 0x01, 0x00, 0x01, 0x00, 0x02, 1, 0], [1]));
}

#[test]
fn a_clocks_ranges_say_its_rates() {
    // 44.1 kHz alone, and 48 kHz to 192 kHz in steps of 48 kHz.
    let mut answer = std::vec![2, 0];
    for (min, max, step) in [(44100u32, 44100, 0), (48000, 192_000, 48000)] {
        for v in [min, max, step] {
            answer.extend(v.to_le_bytes());
        }
    }
    for (rate, held) in [
        (44100, true),
        (48000, true),
        (96000, true),
        (192_000, true),
        (72000, false),
        (32000, false),
    ] {
        assert_eq!(request::ranges_hold(&answer, rate), held, "{rate}");
    }
    // Fewer subranges than it says, or none.
    assert!(!request::ranges_hold(&answer[..13], 48000));
    assert!(!request::ranges_hold(&[0, 0], 48000));
    assert!(!request::ranges_hold(&[], 48000));
}

#[test]
fn volume_levels_follow_the_units_steps() {
    // QEMU's: -127.996 dB to +8 dB in steps of 0.53 dB. Full is the step
    // nearest 0 dB that is not above it.
    let qemu = Range { min: -32767, max: 0x0800, resolution: 0x88 };
    assert_eq!(level(qemu, 0), -127);
    assert!(level(qemu, 0) <= 0 && level(qemu, 0) > -0x88);
    assert_eq!(level(qemu, 0x10_0000), -32767);
    // -64 dB to 0 dB in steps of 1 dB.
    let decibels = Range { min: -64 * 256, max: 0, resolution: 256 };
    assert_eq!(level(decibels, 0), 0);
    assert_eq!(level(decibels, 10 * 256 + 100), -10 * 256);
    assert_eq!(level(decibels, 10 * 256 + 140), -11 * 256);
    assert_eq!(level(decibels, 80 * 256), -64 * 256);
    // A unit whose top is -6 dB plays full there.
    let low = Range { min: -50 * 256, max: -6 * 256, resolution: 256 };
    assert_eq!(level(low, 0), -6 * 256);
    assert_eq!(level(low, 3 * 256), -9 * 256);
    // A resolution of 0 is taken as the finest.
    assert_eq!(level(Range { min: -100, max: 100, resolution: 0 }, 37), -37);

    // UAC2's answer: two subranges.
    let mut answer = std::vec![2, 0];
    for v in [-6400i16, -3200, 128, -3200, 600, 256] {
        answer.extend(v.to_le_bytes());
    }
    assert_eq!(Range::parse_v2(&answer), Some(Range { min: -6400, max: 600, resolution: 128 }));
    // Fewer subranges than it says: those that came.
    assert_eq!(
        Range::parse_v2(&answer[..8]),
        Some(Range { min: -6400, max: -3200, resolution: 128 })
    );
    assert_eq!(Range::parse_v2(&[0, 0]), None);
}

#[test]
fn the_pacer_carries_the_fraction() {
    // 48 kHz at full speed: 48 frames each millisecond.
    let mut pacer = Pacer::new(48000, 8, 48);
    assert!((0..1000).all(|_| pacer.next_frames() == 48));
    // 44.1 kHz: 44 frames, and 45 every tenth; 44100 over a second.
    let mut pacer = Pacer::new(44100, 8, 45);
    let first: Vec<u32> = (0..10).map(|_| pacer.next_frames()).collect();
    assert_eq!(first.iter().sum::<u32>(), 441);
    assert_eq!(first.iter().filter(|&&f| f == 45).count(), 1);
    assert_eq!((0..990).map(|_| pacer.next_frames()).sum::<u32>() + 441, 44100);
    assert_eq!(pacer.rate(), 44100);
    // High speed, every microframe: 6 frames; every second microframe:
    // 12, which a packet of 10 cuts.
    let mut pacer = Pacer::new(48000, 1, 7);
    assert_eq!((pacer.peek_frames(), pacer.next_frames()), (6, 6));
    let mut pacer = Pacer::new(48000, 2, 10);
    assert_eq!(pacer.next_frames(), 10);
}

#[test]
fn the_pacer_follows_feedback_in_either_format() {
    // Full speed, 10.14 in three bytes: 48.25 frames a millisecond.
    let mut pacer = Pacer::new(48000, 8, 52);
    let value = (48u32 << 14) + (1 << 12);
    assert!(pacer.feedback(&value.to_le_bytes()[..3], true));
    assert_eq!(pacer.rate(), 48250);
    assert_eq!((0..4).map(|_| pacer.next_frames()).sum::<u32>(), 193);
    // Its format is kept: the same value in 16.16 now is too far.
    assert!(!pacer.feedback(&(48u32 << 16).to_le_bytes(), true));
    assert_eq!(pacer.rate(), 48250);

    // A full-speed device that sends 16.16 in four bytes.
    let mut pacer = Pacer::new(48000, 8, 52);
    assert!(pacer.feedback(&((47u32 << 16) + (1 << 15)).to_le_bytes(), true));
    assert_eq!(pacer.rate(), 47500);

    // High speed, 16.16 a microframe, every microframe: 6.03125.
    let mut pacer = Pacer::new(48000, 1, 8);
    assert!(pacer.feedback(&((6u32 << 16) + (1 << 11)).to_le_bytes(), false));
    assert_eq!(pacer.rate(), 48250);

    // Nonsense changes nothing: zero, a third of the rate, and too few
    // bytes.
    let mut pacer = Pacer::new(48000, 8, 52);
    assert!(!pacer.feedback(&[0, 0, 0], true));
    assert!(!pacer.feedback(&(16u32 << 14).to_le_bytes()[..3], true));
    assert!(!pacer.feedback(&[1, 2], true));
    assert_eq!(pacer.rate(), 48000);
}

/// A ring of 8 KiB whose first 4 KiB end where 64 KiB of memory do.
const BASE: u64 = 0x1_f000;
const RING: u32 = 0x2000;

#[test]
fn packets_are_cut_where_the_ring_wraps_and_memory_turns_64_kib() {
    let mut stream = Stream::new(BASE, RING, 4);
    let mut out = [(0, 0); MAX_PIECES];
    assert_eq!(stream.room(), RING);
    // Nothing to play yet, but a short packet of what there is when asked.
    assert_eq!(stream.next_packet(48, false, &mut out), None);
    stream.wrote(100);
    assert_eq!(stream.next_packet(48, false, &mut out), None);
    assert_eq!(stream.next_packet(48, true, &mut out), Some((1, 100)));
    stream.wrote(RING);
    assert_eq!((stream.room(), stream.waiting()), (0, RING));

    // Packets of 192 bytes from offset 0: the 22nd crosses 0x1000, where
    // memory turns 64 KiB.
    let mut last = 0;
    for n in 0..21 {
        assert_eq!(stream.next_packet(48, false, &mut out), Some((1, 192)));
        assert_eq!(out[0], (BASE + n * 192, 192));
        last += 1;
        stream.queued_packet(192, 1, last);
    }
    assert_eq!(stream.next_packet(48, false, &mut out), Some((2, 192)));
    assert_eq!(out[..2], [(BASE + 4032, 64), (0x2_0000, 128)]);
    last += 1;
    stream.queued_packet(192, 2, last);
    // The 43rd crosses the ring's end, 8192 = 42 * 192 + 128.
    for _ in 22..42 {
        stream.next_packet(48, false, &mut out).unwrap();
        last += 1;
        stream.queued_packet(192, 1, last);
    }
    assert_eq!(stream.next_packet(48, false, &mut out), None, "only 128 bytes are left");
    // The first 21 TDs play: their bytes are room again.
    assert!(stream.done_through(21));
    assert_eq!((stream.played(), stream.room(), stream.in_flight()), (21 * 192, 21 * 192, 21));
    stream.wrote(64);
    assert_eq!(stream.next_packet(48, false, &mut out), Some((2, 192)));
    assert_eq!(out[..2], [(BASE + 8064, 128), (BASE, 64)]);
    // A TD that is not queued is not done.
    assert!(!stream.done_through(999));
    // An underrun: everything queued is done.
    stream.all_done();
    assert_eq!((stream.in_flight(), stream.played(), stream.unplayed()), (0, 42 * 192, 128 + 64));
    stream.clear();
    assert_eq!((stream.room(), stream.played(), stream.write_offset()), (RING, 0, 0));
}

#[test]
fn the_transfer_ring_is_never_overfilled() {
    let mut stream = Stream::new(0x10_0000, 0x1_0000, 4);
    stream.wrote(0x1_0000);
    let mut out = [(0, 0); MAX_PIECES];
    for n in 0..MAX_TDS as u64 {
        let (pieces, bytes) = stream.next_packet(8, false, &mut out).unwrap();
        stream.queued_packet(bytes, pieces, n);
    }
    // 255 TDs of a TRB each fill the ring's 255 TRBs.
    assert_eq!(stream.next_packet(8, false, &mut out), None);
    assert!(stream.done_through(0));
    assert!(stream.next_packet(8, false, &mut out).is_some());
}

proptest! {
    /// Whatever a device sends, reading it for an output neither panics
    /// nor loops, and what it finds is a setting the kernel plays.
    #[test]
    fn any_configuration_is_read_safely(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let mut patched = bytes.clone();
        if patched.len() >= 9 {
            patched[0] = 9;
            patched[1] = descriptor::CONFIGURATION;
        }
        if let Some(config) = Configuration::parse(&patched)
            && let Some(out) = find(&config)
        {
            prop_assert!((1..=2).contains(&out.setting.channels));
            prop_assert!(out.setting.frame_bytes() <= 8);
        }
        let _ = Range::parse_v2(&bytes);
        let _ = request::ranges_hold(&bytes, 48000);
        let _ = Pacer::new(48000, 8, 48).feedback(&bytes, bytes.len() % 2 == 0);
    }

    /// Every packet's pieces hold its bytes, in the ring, none across 64
    /// KiB of memory, each after the one before.
    #[test]
    fn pieces_stay_in_the_ring(
        base in 0u64..0x4_0000,
        size in 64u32..0x2_0000,
        steps in proptest::collection::vec((0u32..4096, 1u32..200, any::<bool>()), 1..60),
    ) {
        let base = base & !3;
        let size = size & !3;
        let mut stream = Stream::new(base, size, 4);
        let mut out = [(0, 0); MAX_PIECES];
        let mut next = 0u64;
        for (n, &(write, frames, play)) in steps.iter().enumerate() {
            stream.wrote(write & !3);
            if let Some((count, bytes)) = stream.next_packet(frames, true, &mut out) {
                prop_assert_eq!(out[..count].iter().map(|p| p.1).sum::<u32>(), bytes);
                for &(phys, len) in &out[..count] {
                    prop_assert!(phys >= base && phys + u64::from(len) <= base + u64::from(size));
                    prop_assert!((phys & 0xffff) + u64::from(len) <= 0x1_0000);
                    prop_assert_eq!(phys, base + next % u64::from(size));
                    next += u64::from(len);
                }
                stream.queued_packet(bytes, count, n as u64);
            }
            if play {
                stream.all_done();
            }
            prop_assert!(stream.unplayed() <= u64::from(size));
        }
    }
}
