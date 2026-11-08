// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Samples to the signed 16-bit ones an output takes, and channels to the
//! count it plays.

use crate::wav::Encoding;

/// The sample at the start of `bytes`, stored as `encoding`, as a signed
/// 16-bit one: wider integers keep their top 16 bits, floats are scaled
/// and clipped to full scale.
pub fn sample(bytes: &[u8], encoding: Encoding) -> i16 {
    match encoding {
        Encoding::U8 => (i16::from(bytes[0]) - 128) << 8,
        Encoding::S16 => i16::from_le_bytes([bytes[0], bytes[1]]),
        Encoding::S24 => i16::from_le_bytes([bytes[1], bytes[2]]),
        Encoding::S32 => i16::from_le_bytes([bytes[2], bytes[3]]),
        Encoding::F32 => {
            let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            // NaN becomes silence; the rest is clipped.
            let scaled = if value.is_nan() { 0.0 } else { value * 32768.0 };
            scaled.clamp(-32768.0, 32767.0) as i16
        }
    }
}

/// Decodes `data` (whole frames of `channels` samples stored as
/// `encoding`) into 16-bit samples, appended to `out`.
pub fn to_s16(data: &[u8], encoding: Encoding, out: &mut Vec<i16>) {
    let size = encoding.bytes();
    out.extend(data.chunks_exact(size).map(|bytes| sample(bytes, encoding)));
}

/// Maps interleaved frames of `from` channels to frames of `to`: mono is
/// copied to every channel; to mono, the first two channels are averaged;
/// otherwise each channel takes the one of its number, or silence where
/// there is none (stereo to four channels plays on the front two).
pub fn channels(samples: &[i16], from: usize, to: usize) -> Vec<i16> {
    if from == to || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let frames = samples.len() / from;
    let mut out = Vec::with_capacity(frames * to);
    for frame in samples.chunks_exact(from) {
        for channel in 0..to {
            let value = if from == 1 {
                frame[0]
            } else if to == 1 {
                ((i32::from(frame[0]) + i32::from(frame[1])) / 2) as i16
            } else {
                frame.get(channel).copied().unwrap_or(0)
            };
            out.push(value);
        }
    }
    out
}

/// 16-bit samples as the little-endian bytes an output takes.
pub fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

/// Little-endian 16-bit sample bytes as samples; a trailing odd byte is
/// dropped.
pub fn from_bytes(bytes: &[u8]) -> Vec<i16> {
    bytes.as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect()
}
