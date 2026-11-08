// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! WAV files: a RIFF file of form `WAVE` with a `fmt ` chunk, which says
//! how the samples are stored, and a `data` chunk, which holds them
//! (Microsoft's Multimedia Programming Interface and Data Specifications
//! 1.0, and WAVE_FORMAT_EXTENSIBLE for more than two channels or more
//! than 16 bits). Other chunks are skipped.

use std::fmt;

/// How the samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Unsigned 8-bit integers, silence at 128.
    U8,
    /// Signed little-endian integers of 16 bits.
    S16,
    /// Of 24 bits, in three bytes.
    S24,
    /// Of 32 bits.
    S32,
    /// 32-bit IEEE floats, full scale at 1.0.
    F32,
}

impl Encoding {
    /// The bytes of one sample.
    pub fn bytes(self) -> usize {
        match self {
            Encoding::U8 => 1,
            Encoding::S16 => 2,
            Encoding::S24 => 3,
            Encoding::S32 | Encoding::F32 => 4,
        }
    }
}

/// A WAV file's samples and how to play them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wav<'a> {
    /// Frames a second.
    pub rate: u32,
    /// Samples in a frame.
    pub channels: u16,
    /// How each sample is stored.
    pub encoding: Encoding,
    /// The samples, interleaved, whole frames only.
    pub data: &'a [u8],
}

/// Why a file is not a WAV file the player can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// It is not a RIFF WAVE file.
    NotWav,
    /// A chunk runs past the end of the file, or `fmt ` is too short.
    Truncated,
    /// It has no `fmt ` chunk before its `data`, or no `data`.
    MissingChunk(&'static str),
    /// Its samples are compressed, or of a size it cannot read.
    Unsupported(u16, u16),
    /// It says it has no channels, or no rate.
    Empty,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotWav => write!(f, "not a WAV file"),
            Error::Truncated => write!(f, "the file is cut short"),
            Error::MissingChunk(name) => write!(f, "the file has no {name} chunk"),
            Error::Unsupported(tag, bits) => {
                write!(f, "samples of format {tag:#06x} and {bits} bits are not supported")
            }
            Error::Empty => write!(f, "the file has no channels or no rate"),
        }
    }
}

/// The format tags of PCM and of floats, and the one that says the real
/// format follows (whose subformat GUID starts with the real tag).
const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xfffe;

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Reads a WAV file. A `data` chunk that says it runs past the end of the
/// file (as a recording cut off before its header was written) is taken
/// to the end.
pub fn parse(file: &[u8]) -> Result<Wav<'_>, Error> {
    if file.len() < 12 || &file[0..4] != b"RIFF" || &file[8..12] != b"WAVE" {
        return Err(Error::NotWav);
    }
    let mut at = 12;
    let mut format: Option<(u32, u16, Encoding)> = None;
    while at + 8 <= file.len() {
        let id = &file[at..at + 4];
        let size = u32_at(file, at + 4) as usize;
        let body = at + 8;
        if id == b"fmt " {
            if size < 16 || body + size > file.len() {
                return Err(Error::Truncated);
            }
            let fmt = &file[body..body + size];
            let mut tag = u16_at(fmt, 0);
            let channels = u16_at(fmt, 2);
            let rate = u32_at(fmt, 4);
            let bits = u16_at(fmt, 14);
            if tag == FORMAT_EXTENSIBLE {
                if size < 40 {
                    return Err(Error::Truncated);
                }
                tag = u16_at(fmt, 24);
            }
            let encoding = match (tag, bits) {
                (FORMAT_PCM, 8) => Encoding::U8,
                (FORMAT_PCM, 16) => Encoding::S16,
                (FORMAT_PCM, 24) => Encoding::S24,
                (FORMAT_PCM, 32) => Encoding::S32,
                (FORMAT_FLOAT, 32) => Encoding::F32,
                _ => return Err(Error::Unsupported(tag, bits)),
            };
            if channels == 0 || rate == 0 {
                return Err(Error::Empty);
            }
            format = Some((rate, channels, encoding));
        } else if id == b"data" {
            let (rate, channels, encoding) = format.ok_or(Error::MissingChunk("fmt"))?;
            let end = body.saturating_add(size).min(file.len());
            let frame = usize::from(channels) * encoding.bytes();
            let whole = (end - body) / frame * frame;
            return Ok(Wav { rate, channels, encoding, data: &file[body..body + whole] });
        }
        // Chunks are padded to an even length.
        at = body.saturating_add(size).saturating_add(size & 1);
    }
    Err(Error::MissingChunk("data"))
}

/// The 44-byte header of a WAV file of `frames` frames of 16-bit samples.
pub fn header(rate: u32, channels: u16, frames: u32) -> [u8; 44] {
    let block = u32::from(channels) * 2;
    let data = frames.saturating_mul(block);
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&data.saturating_add(36).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&FORMAT_PCM.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * block).to_le_bytes());
    h[32..34].copy_from_slice(&(block as u16).to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}
