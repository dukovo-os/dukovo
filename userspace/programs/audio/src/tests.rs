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

/// Interleaved stereo samples of every kind a track has: tones, digital
/// silence, noise, and full scale both ways, `frames` of them.
fn track(frames: usize) -> Vec<i16> {
    let mut seed = 0x1234_5678_u32;
    let mut samples = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let (left, right) = match n * 4 / frames.max(1) {
            0 => {
                let t = n as f64 / 44_100.0;
                let tone =
                    |hz: f64| (12_000.0 * (2.0 * std::f64::consts::PI * hz * t).sin()) as i16;
                (tone(440.0), tone(660.0))
            }
            1 => (0, 0),
            2 => {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 16) as i16, (seed >> 8) as i16)
            }
            _ => {
                if n % 2 == 0 {
                    (i16::MAX, i16::MIN)
                } else {
                    (i16::MIN, i16::MAX)
                }
            }
        };
        samples.extend([left, right]);
    }
    samples
}

#[test]
fn flac_files_read_back_as_the_samples_they_were_made_of() {
    use crate::flac;
    let tags = [("TITLE", "First Light"), ("ARTIST", "Dukovo Lab"), ("TRACKNUMBER", "1")];
    // Blocks whole and not, a last block of a few hundred frames and one
    // of fewer than 256, and a stream shorter than a block.
    for frames in [3 * flac::BLOCK + 1234, 2 * flac::BLOCK + 100, 1000, flac::BLOCK] {
        let samples = track(frames);
        let file = flac::encode(&samples, 44_100, 2, &tags);
        let decoded = flac::decode(&file).expect("the file reads back");
        assert_eq!(decoded.samples, samples, "{frames} frames");
        assert_eq!(decoded.info.frames, frames as u64);
        assert_eq!((decoded.info.rate, decoded.info.channels, decoded.info.bits), (44_100, 2, 16));
        assert_ne!(decoded.info.md5, [0; 16]);
        assert!(decoded.info.min_frame > 0 && decoded.info.min_frame <= decoded.info.max_frame);
        assert_eq!(decoded.tags, ["TITLE=First Light", "ARTIST=Dukovo Lab", "TRACKNUMBER=1"]);
        // Smaller than the samples: the tones and the silence compress.
        assert!(
            file.len() < samples.len() * 2,
            "{} bytes of {} samples",
            file.len(),
            samples.len()
        );
    }
}

#[test]
fn flac_files_are_the_same_made_a_piece_at_a_time() {
    use crate::flac::{self, Encoder, STREAMINFO_AT, StreamInfo, header};
    let samples = track(5 * flac::BLOCK + 777);
    let whole = flac::encode(&samples, 44_100, 2, &[("ALBUM", "Lab Sessions")]);
    // As a program writes it: the header with what it knows first, the
    // frames as they come, odd pieces of the samples at a time, and then
    // the STREAMINFO again.
    let frames = (samples.len() / 2) as u64;
    let mut file = header(&StreamInfo::before(44_100, 2, frames), &[("ALBUM", "Lab Sessions")]);
    let mut encoder = Encoder::new(44_100, 2);
    for piece in samples.chunks(2 * 1001) {
        encoder.push(piece, &mut file);
    }
    let info = encoder.finish(&mut file);
    let at = STREAMINFO_AT as usize;
    file[at..at + 34].copy_from_slice(&info.bytes());
    assert_eq!(file, whole);
}

#[test]
fn flac_codes_silence_mono_and_long_streams() {
    use crate::flac;
    // Silence is a constant a block: a few bytes each.
    let silence = vec![0i16; 2 * 10 * flac::BLOCK];
    let file = flac::encode(&silence, 44_100, 2, &[]);
    assert!(file.len() < 400, "{} bytes of silence", file.len());
    assert_eq!(flac::decode(&file).expect("silence").samples, silence);
    // One channel, at a rate the frame header does not name.
    let mono: Vec<i16> = track(3 * flac::BLOCK).into_iter().step_by(2).collect();
    let file = flac::encode(&mono, 22_050, 1, &[]);
    let decoded = flac::decode(&file).expect("mono");
    assert_eq!((decoded.info.rate, decoded.info.channels), (22_050, 1));
    assert_eq!(decoded.samples, mono);
    // More than 127 frames: frame numbers of two bytes and more.
    let long = track(200 * flac::BLOCK + 5);
    assert_eq!(flac::decode(&flac::encode(&long, 48_000, 2, &[])).expect("long").samples, long);
}

#[test]
fn flac_files_damaged_are_refused() {
    use crate::flac::{self, Error};
    let samples = track(2 * flac::BLOCK);
    let file = flac::encode(&samples, 44_100, 2, &[]);
    assert_eq!(flac::decode(b"RIFF").unwrap_err(), Error::NotFlac);
    assert_eq!(flac::decode(&file[..file.len() - 3]).unwrap_err(), Error::Truncated);
    // A bit of the audio changed: the frame's CRC no longer holds.
    let mut damaged = file.clone();
    let last = damaged.len() - 40;
    damaged[last] ^= 0x10;
    assert!(matches!(
        flac::decode(&damaged),
        Err(Error::BadCrc(_) | Error::Corrupt(_) | Error::Truncated)
    ));
    // The MD5 of other samples.
    let mut other = file.clone();
    other[8 + 18] ^= 1;
    assert_eq!(flac::decode(&other).unwrap_err(), Error::Md5);
}

/// A READ TOC answer: its length, two bytes of the format's own, then
/// `body`.
fn toc_answer(first: u8, last: u8, body: &[u8]) -> Vec<u8> {
    let mut answer = ((body.len() + 2) as u16).to_be_bytes().to_vec();
    answer.extend([first, last]);
    answer.extend(body);
    answer
}

/// A track descriptor of a READ TOC of the tracks.
fn descriptor(number: u8, control: u8, start: u32) -> Vec<u8> {
    let mut d = vec![0, 0x10 | control, number, 0];
    d.extend(start.to_be_bytes());
    d
}

#[test]
fn cd_tracks_span_up_to_the_next_or_the_lead_out() {
    use crate::cd::{Track, tracks};
    let body: Vec<u8> =
        [descriptor(1, 0, 0), descriptor(2, 0, 375), descriptor(0xaa, 0, 525)].concat();
    let toc = toc_answer(1, 2, &body);
    let found = tracks(&toc, None).expect("the tracks");
    assert_eq!(
        found,
        [
            Track { number: 1, audio: true, start: 0, sectors: 375 },
            Track { number: 2, audio: true, start: 375, sectors: 150 }
        ]
    );
    assert_eq!((found[0].seconds(), found[0].frames()), (5, 375 * 588));
    // An Enhanced CD: a data track starting the second session, after the
    // first's lead-out and the second's lead-in and pregap.
    let body: Vec<u8> = [
        descriptor(1, 0, 0),
        descriptor(2, 0, 20_000),
        descriptor(3, 0x04, 50_000),
        descriptor(0xaa, 0x04, 60_000),
    ]
    .concat();
    let sessions = toc_answer(1, 2, &descriptor(3, 0x04, 50_000));
    let found = tracks(&toc_answer(1, 3, &body), Some(&sessions)).expect("the tracks");
    assert_eq!(found[1], Track { number: 2, audio: true, start: 20_000, sectors: 30_000 - 11_400 });
    assert_eq!(found[2], Track { number: 3, audio: false, start: 50_000, sectors: 10_000 });
    // One session: the data track (a mixed-mode disc's last) follows on.
    let one = toc_answer(1, 1, &descriptor(1, 0x04, 0));
    assert_eq!(
        tracks(&toc_answer(1, 3, &body), Some(&one)).expect("the tracks")[1].sectors,
        30_000
    );
    assert!(tracks(&toc_answer(1, 1, &descriptor(1, 0, 0)), None).is_err(), "no lead-out");
    assert!(tracks(&[0, 2], None).is_err(), "too short");
}

/// CD-Text packs of `kind` holding `texts`, the disc's then each track's
/// from track 1, with their CRCs.
fn text_packs(kind: u8, texts: &[&str], first_sequence: u8) -> Vec<[u8; 18]> {
    let mut bytes: Vec<(u8, u8)> = Vec::new();
    for (track, text) in texts.iter().enumerate() {
        // ISO 8859-1: each character a byte.
        bytes.extend(text.chars().map(|c| c as u8).chain([0]).map(|b| (track as u8, b)));
    }
    bytes
        .chunks(12)
        .enumerate()
        .map(|(n, chunk)| {
            let mut pack = [0u8; 18];
            pack[0] = kind;
            pack[1] = chunk[0].0;
            pack[2] = first_sequence + n as u8;
            for (i, &(_, b)) in chunk.iter().enumerate() {
                pack[4 + i] = b;
            }
            let crc = !crate_cd_crc(&pack[..16]);
            pack[16..].copy_from_slice(&crc.to_be_bytes());
            pack
        })
        .collect()
}

/// CRC-16 of polynomial 0x1021, from 0, as CD-Text's.
fn crate_cd_crc(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

#[test]
fn cd_text_names_the_disc_and_its_tracks() {
    use crate::cd::{CdText, Names, cd_text};
    let names = |title: &str, performer: Option<&str>| Names {
        title: Some(title.to_string()),
        performer: performer.map(str::to_string),
    };
    let titles = text_packs(0x80, &["Lab Sessions", "First Light", "Static", "Caf\u{e9}"], 0);
    let performers = text_packs(0x81, &["Dukovo Lab", "", "Noise Unit", "\t"], titles.len() as u8);
    let mut body: Vec<u8> = titles.iter().chain(&performers).flatten().copied().collect();
    let text = cd_text(&toc_answer(0, 0, &body));
    assert_eq!(text.disc, names("Lab Sessions", Some("Dukovo Lab")));
    assert_eq!(text.track(1), names("First Light", None));
    assert_eq!(text.track(2), names("Static", Some("Noise Unit")));
    // ISO 8859-1, and a tab for the track before's.
    assert_eq!(text.track(3), names("Caf\u{e9}", Some("Noise Unit")));
    assert_eq!(text.track(9), Names::default());
    // A pack whose CRC does not hold is left out: the disc's title goes.
    body[16] ^= 0xff;
    assert_eq!(cd_text(&toc_answer(0, 0, &body)).disc.title, None);
    // Characters of two bytes (MS-JIS) are not read.
    let mut size = [0u8; 18];
    size[0] = 0x8f;
    size[4] = 0x80;
    let crc = !crate_cd_crc(&size[..16]);
    size[16..].copy_from_slice(&crc.to_be_bytes());
    let jis: Vec<u8> = titles.iter().flatten().chain(&size).copied().collect();
    assert_eq!(cd_text(&toc_answer(0, 0, &jis)), CdText::default());
    assert_eq!(cd_text(&[]), CdText::default());
}
