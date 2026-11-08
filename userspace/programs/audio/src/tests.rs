// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::convert::{self, channels, from_bytes, to_bytes, to_s16};
use crate::detect::{self, rms, share, sounding};
use crate::resample::{Resampler, resample};
use crate::settings::{HEADER, Settings};
use crate::tone::{LEFT_FREQUENCY, RIGHT_FREQUENCY, sine};
use crate::wav::{self, Encoding, Error};

/// A WAV file with a `fmt ` chunk of `tag`, `bits` and `channels`, and
/// `data`, after a chunk to skip.
fn file(tag: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    f.extend_from_slice(b"LIST\x03\0\0\0abc\0");
    f.extend_from_slice(b"fmt \x10\0\0\0");
    f.extend_from_slice(&tag.to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&bits.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(data);
    f
}

#[test]
fn reads_wav_files() {
    let f = file(1, 16, 2, 44100, &[1, 0, 2, 0, 3, 0, 4, 0]);
    let w = wav::parse(&f).unwrap();
    assert_eq!((w.rate, w.channels, w.encoding), (44100, 2, Encoding::S16));
    assert_eq!(w.data, &[1, 0, 2, 0, 3, 0, 4, 0]);
    // A data chunk that runs past the end, and half a frame at it.
    let mut f = file(3, 32, 1, 8000, &[0; 10]);
    let len = f.len();
    f[len - 14..len - 10].copy_from_slice(&1000u32.to_le_bytes());
    let w = wav::parse(&f).unwrap();
    assert_eq!((w.encoding, w.data.len()), (Encoding::F32, 8));
    assert_eq!(wav::parse(b"RIFX0000WAVE"), Err(Error::NotWav));
    assert_eq!(wav::parse(&file(2, 4, 1, 8000, &[])), Err(Error::Unsupported(2, 4)));
    assert_eq!(wav::parse(&file(1, 16, 0, 8000, &[])), Err(Error::Empty));
    assert_eq!(wav::parse(b"RIFF\0\0\0\0WAVEdata\0\0\0\0"), Err(Error::MissingChunk("fmt")));
}

#[test]
fn a_written_header_reads_back() {
    let mut f = wav::header(48000, 2, 3).to_vec();
    f.extend_from_slice(&[0; 12]);
    let w = wav::parse(&f).unwrap();
    assert_eq!((w.rate, w.channels, w.encoding, w.data.len()), (48000, 2, Encoding::S16, 12));
}

#[test]
fn samples_become_16_bit() {
    assert_eq!(convert::sample(&[128], Encoding::U8), 0);
    assert_eq!(convert::sample(&[255], Encoding::U8), 127 << 8);
    assert_eq!(convert::sample(&[0], Encoding::U8), -32768);
    assert_eq!(convert::sample(&[0x00, 0x34, 0x12], Encoding::S24), 0x1234);
    assert_eq!(convert::sample(&[0, 0, 0x00, 0x80], Encoding::S32), -32768);
    assert_eq!(convert::sample(&0.5f32.to_le_bytes(), Encoding::F32), 16384);
    assert_eq!(convert::sample(&2.0f32.to_le_bytes(), Encoding::F32), 32767);
    assert_eq!(convert::sample(&(-2.0f32).to_le_bytes(), Encoding::F32), -32768);
    assert_eq!(convert::sample(&f32::NAN.to_le_bytes(), Encoding::F32), 0);
    let mut out = Vec::new();
    to_s16(&[1, 0, 0xff, 0xff], Encoding::S16, &mut out);
    assert_eq!(out, [1, -1]);
    assert_eq!(from_bytes(&to_bytes(&[5, -6, 32767])), [5, -6, 32767]);
}

#[test]
fn channels_are_mapped() {
    assert_eq!(channels(&[1, 2], 1, 2), [1, 1, 2, 2]);
    assert_eq!(channels(&[10, 20, -4, 4], 2, 1), [15, 0]);
    assert_eq!(channels(&[1, 2, 3, 4], 2, 4), [1, 2, 0, 0, 3, 4, 0, 0]);
    assert_eq!(channels(&[1, 2, 3, 4, 5, 6], 6, 2), [1, 2]);
    assert_eq!(channels(&[7, 8], 2, 2), [7, 8]);
}

/// How much of the left channel of `samples` is `frequency`, over its
/// middle (away from the fades).
fn tone_share(samples: &[i16], channels: usize, rate: u32, frequency: f64) -> f64 {
    let left = detect::channel(samples, channels, 0);
    let middle = &left[left.len() / 4..left.len() * 3 / 4];
    share(middle, rate, frequency)
}

#[test]
fn a_tone_is_found_and_another_is_not() {
    let tone = sine(LEFT_FREQUENCY, 48000, 2, 48000, 0.5, &|channel| channel == 0);
    assert!(tone_share(&tone, 2, 48000, LEFT_FREQUENCY) > 0.95);
    assert!(tone_share(&tone, 2, 48000, RIGHT_FREQUENCY) < 0.01);
    // The right channel is silent.
    let right = detect::channel(&tone, 2, 1);
    assert_eq!(rms(&right), 0.0);
    assert_eq!(share(&right, 48000, LEFT_FREQUENCY), 0.0);
    // Its loudness is its amplitude over the square root of two.
    let left = detect::channel(&tone, 2, 0);
    let expected = 0.5 * 32767.0 / 2f64.sqrt();
    assert!((rms(&left[4800..43200]) - expected).abs() < expected * 0.01);
}

#[test]
fn where_the_sound_is() {
    let mut samples = vec![0i16; 4800];
    samples.extend(sine(LEFT_FREQUENCY, 48000, 1, 9600, 0.5, &|_| true));
    samples.extend(vec![0i16; 4800]);
    let mono = detect::channel(&samples, 1, 0);
    let range = sounding(&mono, 48000, 0.05).unwrap();
    // Within a block (10 ms) of where it starts and ends.
    assert!((4800..=5280).contains(&range.start), "{range:?}");
    assert!((13920..=14880).contains(&range.end), "{range:?}");
    assert_eq!(sounding(&[0.0; 1000], 48000, 0.05), None);
}

#[test]
fn speech_rate_up_to_an_outputs_keeps_the_tone() {
    // 22,050 to 48,000, in uneven chunks, as eSpeak NG's would come.
    let tone = sine(LEFT_FREQUENCY, 22050, 1, 22050, 0.5, &|_| true);
    let mut resampler = Resampler::new(22050, 48000, 1);
    let mut out = Vec::new();
    for chunk in tone.chunks(777) {
        resampler.process(chunk, &mut out);
    }
    assert!((out.len() as i64 - 48000).abs() <= 2, "{}", out.len());
    assert!(tone_share(&out, 1, 48000, LEFT_FREQUENCY) > 0.95);
    // In one go, the same.
    assert_eq!(resample(&tone, 22050, 48000, 1), out);
}

#[test]
fn rate_down_keeps_the_tone_and_drops_what_cannot_be_played() {
    let tone = sine(LEFT_FREQUENCY, 96000, 2, 96000, 0.5, &|_| true);
    let out = resample(&tone, 96000, 48000, 2);
    assert!((out.len() as i64 - 96000).abs() <= 4);
    assert!(tone_share(&out, 2, 48000, LEFT_FREQUENCY) > 0.95);
    // 40 kHz cannot be played at 48,000 frames a second: what is left of
    // it is far quieter.
    let high = sine(40000.0, 96000, 1, 96000, 0.5, &|_| true);
    let down = resample(&high, 96000, 48000, 1);
    let before = rms(&detect::channel(&high, 1, 0));
    let after = rms(&detect::channel(&down, 1, 0));
    assert!(after < before * 0.4, "{after} against {before}");
}

#[test]
fn the_same_rate_changes_nothing() {
    let samples = [1i16, 2, 3, 4];
    assert!(Resampler::new(48000, 48000, 2).is_identity());
    assert_eq!(resample(&samples, 48000, 48000, 2), samples);
}

#[test]
fn settings_read_and_keep_their_notes() {
    let text = "# Sound\noutput = \"virtio0\"  \nvolume = 55 # quieter\nother = 1\n";
    let s = Settings::parse(text);
    assert_eq!(s, Settings { output: Some("virtio0".into()), volume: 55, muted: false });
    let changed = Settings { output: None, volume: 80, muted: true }.update(text);
    assert_eq!(changed, "# Sound\noutput = \"\"\nvolume = 80\nother = 1\nmuted = true\n");
    assert_eq!(Settings::parse(&changed), Settings { output: None, volume: 80, muted: true });
    // Bad values are the defaults; too loud is cut to 100.
    let s = Settings::parse("volume = loud\nmuted = yes\n");
    assert_eq!(s, Settings::default());
    assert_eq!(Settings::parse("volume = 250").volume, 100);
    assert_eq!(Settings::parse("muted = true\nvolume = 40").effective_volume(), 0);
    // A new file keeps its header above the values.
    let new = Settings::default().update(HEADER);
    assert!(new.starts_with(HEADER) && new.ends_with("volume = 70\nmuted = false\n"), "{new}");
    assert_eq!(Settings::parse(&new), Settings::default());
    // Quotes in a name survive.
    let odd = Settings { output: Some("a\"b".into()), ..Settings::default() };
    assert_eq!(Settings::parse(&odd.update("")), odd);
}
