// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::vec;
use std::vec::Vec;

use radix_abi::audio as abi;

use super::*;

#[test]
fn rate_bits_are_the_abis() {
    let bits = [
        abi::RATE_5512,
        abi::RATE_8000,
        abi::RATE_11025,
        abi::RATE_16000,
        abi::RATE_22050,
        abi::RATE_32000,
        abi::RATE_44100,
        abi::RATE_48000,
        abi::RATE_64000,
        abi::RATE_88200,
        abi::RATE_96000,
        abi::RATE_176400,
        abi::RATE_192000,
        abi::RATE_384000,
    ];
    for (rate, bit) in RATES.iter().zip(bits) {
        assert_eq!(rate_bit(*rate), Some(bit), "{rate}");
    }
    assert_eq!(rate_bit(44000), None);
}

#[test]
fn nearest_rate_prefers_the_closest_then_the_higher() {
    let mask = abi::RATE_44100 | abi::RATE_48000 | abi::RATE_96000;
    assert_eq!(nearest_rate(mask, 48000), Some(48000));
    assert_eq!(nearest_rate(mask, 22050), Some(44100));
    assert_eq!(nearest_rate(mask, 46000), Some(44100));
    assert_eq!(nearest_rate(mask, 47000), Some(48000));
    assert_eq!(nearest_rate(mask, 1_000_000), Some(96000));
    // As near to both: the higher.
    let mask = abi::RATE_8000 | abi::RATE_16000;
    assert_eq!(nearest_rate(mask, 12000), Some(16000));
    assert_eq!(nearest_rate(0, 48000), None);
}

#[test]
fn channels_are_clamped() {
    assert_eq!(nearest_channels(2, 2, 1), 2);
    assert_eq!(nearest_channels(1, 2, 1), 1);
    assert_eq!(nearest_channels(1, 8, 6), 6);
    assert_eq!(nearest_channels(1, 2, 6), 2);
    assert_eq!(nearest_channels(1, 2, 0), 1);
    assert_eq!(nearest_channels(1, 2, u32::MAX), 2);
}

#[test]
fn default_fragments_are_about_ten_milliseconds() {
    // 1,920 bytes are 10 ms at 48 kHz stereo: 2048 is nearer than 1024.
    assert_eq!(Fragments::default_for(Format::DEFAULT), Fragments { bytes: 2048, count: 8 });
    // eSpeak NG's 22,050 Hz mono: 441 bytes, so 512.
    let speech = Format { rate: 22050, channels: 1 };
    assert_eq!(Fragments::default_for(speech), Fragments { bytes: 512, count: 8 });
    // 8 kHz mono is 160 bytes: no less than the smallest fragment.
    let phone = Format { rate: 8000, channels: 1 };
    assert_eq!(Fragments::default_for(phone).bytes, MIN_FRAGMENT);
    // 6-byte frames: whole frames only.
    let three = Format { rate: 48000, channels: 3 };
    assert_eq!(Fragments::default_for(three).bytes % 6, 0);
}

#[test]
fn requested_fragments_are_kept_within_limits() {
    let f = Format::DEFAULT;
    // 2^12 bytes, 4 of them.
    assert_eq!(Fragments::requested(0x0004_000c, f), Fragments { bytes: 4096, count: 4 });
    // Too small and too few.
    assert_eq!(Fragments::requested(0x0001_0004, f), Fragments { bytes: 256, count: 2 });
    // Too large, and "as many as you like": 64 KiB, four of them in
    // 256 KiB.
    assert_eq!(Fragments::requested(0x7fff_0018, f), Fragments { bytes: 65536, count: 4 });
    // Many small ones: at most 32.
    assert_eq!(Fragments::requested(0x7fff_0009, f), Fragments { bytes: 512, count: 32 });
    for request in [0, 0xffff_ffff, 0x0000_001f, 0x0002_0008] {
        let got = Fragments::requested(request, Format { rate: 44100, channels: 6 });
        assert!((MIN_FRAGMENT - 12..=MAX_FRAGMENT).contains(&got.bytes), "{got:?}");
        assert_eq!(got.bytes % 12, 0);
        assert!((MIN_FRAGMENTS..=MAX_FRAGMENTS).contains(&got.count));
        assert!(got.capacity() <= MAX_BUFFER);
    }
}

#[test]
fn volumes_encode_as_oss_does() {
    assert_eq!(Volume::decode(0x3264), Volume { left: 100, right: 50 });
    assert_eq!(Volume { left: 100, right: 50 }.encode(), 0x3264);
    // Each channel is cut to 100, and the bits above 15 are ignored.
    assert_eq!(Volume::decode(0xffff_ffff), Volume::FULL);
    assert_eq!(Volume::both(250), Volume::FULL);
    assert_eq!(Volume::both(70).encode(), 0x4646);
}

#[test]
fn attenuation_is_as_loud_as_the_gain() {
    // Within half a quarter of a decibel of 60 log10(p / 100), the
    // decibels of the gain, and never louder at a lower volume.
    for percent in 1..=100u8 {
        let exact = -60.0 * (f64::from(percent) / 100.0).log10() * 4.0;
        let table = f64::from(attenuation(percent));
        assert!((table - exact).abs() <= 0.5, "{percent}: {table} for {exact}");
        if percent > 1 {
            assert!(attenuation(percent) <= attenuation(percent - 1));
        }
    }
    assert_eq!((attenuation(100), attenuation(50), attenuation(0)), (0, 72, 480));
}

#[test]
fn gain_is_the_cube_of_the_fraction() {
    assert_eq!(gain(100), 65536);
    assert_eq!(gain(0), 0);
    // Half the volume is an eighth of the amplitude.
    assert_eq!(gain(50), 8192);
    assert_eq!(gain(200), 65536);
    // Never louder for a lower volume.
    for p in 1..=100u8 {
        assert!(gain(p) >= gain(p - 1));
    }
}

fn samples(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn gain_scales_each_channel() {
    let mut bytes = samples(&[16000, -16000, 32767, -32768]);
    apply_gain(&mut bytes, 2, Volume { left: 50, right: 100 });
    assert_eq!(bytes, samples(&[2000, -16000, 4095, -32768]));
    // Mono takes the mean of the two volumes.
    let mut bytes = samples(&[8000, 8000]);
    apply_gain(&mut bytes, 1, Volume { left: 0, right: 100 });
    assert_eq!(bytes, samples(&[1000, 1000]));
    // Muted is silence; full is untouched.
    let mut bytes = samples(&[1234, -5678]);
    apply_gain(&mut bytes, 2, Volume::MUTED);
    assert_eq!(bytes, samples(&[0, 0]));
    let mut bytes = samples(&[1234, -5678, 9]);
    apply_gain(&mut bytes, 2, Volume::FULL);
    assert_eq!(bytes, samples(&[1234, -5678, 9]));
}

#[test]
fn ring_wraps() {
    let mut ring = Ring::new(vec![0u8; 8]);
    assert_eq!(ring.push(&[1, 2, 3, 4, 5, 6]), 6);
    let mut out = [0u8; 4];
    assert_eq!(ring.pop(&mut out), 4);
    assert_eq!(out, [1, 2, 3, 4]);
    // Across the end of the memory.
    assert_eq!(ring.push(&[7, 8, 9, 10, 11, 12, 13]), 6);
    assert_eq!(ring.len(), 8);
    let mut out = [0u8; 8];
    assert_eq!(ring.pop(&mut out), 8);
    assert_eq!(out, [5, 6, 7, 8, 9, 10, 11, 12]);
    assert!(ring.is_empty());
    // A limit below the memory's length.
    ring.reset(3);
    assert_eq!(ring.push(&[1, 2, 3, 4]), 3);
    assert_eq!(ring.push_silence(5), 0);
    let mut out = [9u8; 2];
    ring.pop(&mut out);
    assert_eq!(ring.push_silence(5), 2);
    let mut out = [9u8; 3];
    assert_eq!(ring.pop(&mut out), 3);
    assert_eq!(out, [3, 0, 0]);
}

fn playback() -> Playback<Vec<u8>> {
    let mut p = Playback::new(vec![0u8; MAX_BUFFER as usize], Format::DEFAULT);
    p.configure(Format::DEFAULT, Fragments { bytes: 1024, count: 4 });
    p
}

#[test]
fn writes_wait_for_what_is_played() {
    let mut p = playback();
    assert_eq!(p.room(), 4096);
    assert_eq!(p.write(&[1; 5000]), 4096);
    assert_eq!(p.write(&[1; 10]), 0);
    let space = p.space();
    assert_eq!((space.fragments, space.total, space.fragment_bytes, space.bytes), (0, 4, 1024, 0));
    // Two fragments to the device: still no room until they are played.
    let mut period = [0u8; 1024];
    assert_eq!(p.take(&mut period), Some(1024));
    assert_eq!(p.take(&mut period), Some(1024));
    assert_eq!((p.room(), p.in_flight(), p.queued()), (0, 2, 2048));
    assert!(p.played_one());
    assert_eq!((p.room(), p.delay(), p.in_flight()), (1024, 3072, 1));
    assert_eq!(p.space().fragments, 1);
}

#[test]
fn a_part_fragment_is_filled_out_with_silence() {
    let mut p = playback();
    p.write(&[7; 1500]);
    let mut period = [0u8; 1024];
    assert_eq!(p.take(&mut period), Some(1024));
    // 476 bytes wait: not a whole fragment.
    assert_eq!(p.take(&mut period), None);
    assert_eq!(p.fill_out(), 548);
    assert_eq!(p.fill_out(), 0);
    assert_eq!(p.take(&mut period), Some(1024));
    assert_eq!(&period[..476], &[7; 476][..]);
    assert!(period[476..].iter().all(|&b| b == 0));
    assert!(!p.is_drained());
    assert!(p.played_one() && p.played_one());
    assert!(!p.played_one(), "nothing more in flight");
    assert!(p.is_drained());
}

#[test]
fn filling_out_always_fits() {
    // Whatever was written and played, silence to the end of a fragment
    // has room.
    for written in [1, 1023, 1025, 3000, 4095] {
        let mut p = playback();
        assert_eq!(p.write(&vec![1; written]), written);
        let mut period = [0u8; 1024];
        while p.take(&mut period).is_some() {}
        let before = p.queued();
        let added = p.fill_out();
        assert_eq!((before + added) % 1024, 0, "{written}");
        assert!(p.delay() <= 4096);
    }
}

#[test]
fn clear_drops_everything() {
    let mut p = playback();
    p.write(&[1; 3000]);
    let mut period = [0u8; 1024];
    p.take(&mut period);
    p.clear();
    assert_eq!((p.room(), p.delay(), p.queued(), p.in_flight()), (4096, 0, 0, 0));
    assert!(p.is_drained());
}

#[test]
fn configure_fits_the_memory() {
    let mut p = Playback::new(vec![0u8; 8192], Format::DEFAULT);
    p.configure(Format::DEFAULT, Fragments { bytes: 4096, count: 8 });
    assert_eq!(p.fragments(), Fragments { bytes: 4096, count: 2 });
    assert_eq!(p.write(&[0; 10000]), 8192);
}
