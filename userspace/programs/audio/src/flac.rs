// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! FLAC files (RFC 9639), lossless sound: what audiocd(1) rips a music
//! CD's tracks to (ADR 0447).
//!
//! An [`Encoder`] takes 16-bit samples, interleaved as a WAV file holds
//! them, and gives FLAC's frames, a block of [`BLOCK`] frames of samples
//! at a time. Each channel of a block is coded as the shortest of one
//! value repeated (digital silence), a fixed predictor of order 0 to 4
//! whose residual is in Rice codes, or the samples as they are; and the
//! two channels of a stereo block as left and right, one of them and
//! their difference, or their mean and their difference, whichever is
//! shortest. [`header`] lays out the start of a file: the STREAMINFO,
//! which [`Encoder::finish`] gives once every frame is made, and the
//! tags, as Vorbis comments. [`encode`] makes a whole file in memory, as
//! a program that writes one a piece at a time makes it too.
//!
//! [`decode`] reads back the files the encoder writes, checking every
//! CRC and the MD5 of the samples, for the tests and the boot test; it
//! does not read the linear prediction (LPC) that other encoders use.
//!
//! The predictor of a channel is the order whose residual is smallest in
//! sum, and a partition's Rice parameter is chosen from its sum, not by
//! trying each: quick, and a few per cent larger than the best a search
//! of linear predictors finds.

use std::fmt;

use md5::{Digest, Md5};

/// The frames of samples of each block but the last.
pub const BLOCK: usize = 4096;
/// Where the 34 bytes of the STREAMINFO are in a file that [`header`]
/// starts: a program that writes the frames as it makes them writes the
/// STREAMINFO there again when [`Encoder::finish`] gives it.
pub const STREAMINFO_AT: u64 = 8;
/// The bytes of a STREAMINFO.
const STREAMINFO_SIZE: usize = 34;
/// What the Vorbis comments say made the file.
const VENDOR: &str = "Dukovo";

/// The sync code that starts each frame, of blocks of a fixed size.
const SYNC: u64 = 0xfff8;
/// The highest predictor order of the fixed predictors.
const MAX_ORDER: usize = 4;
/// The highest partition order tried: 16 partitions of 256 samples in a
/// block of 4,096.
const MAX_PARTITION_ORDER: u32 = 8;
/// The highest Rice parameter in a residual of 4-bit parameters, whose
/// 15 is an escape that this encoder does not use; and in one of 5-bit
/// parameters.
const MAX_NARROW_PARAMETER: u32 = 14;
const MAX_WIDE_PARAMETER: u32 = 30;

/// The channel assignments of a frame header: channels coded apart, and
/// the three ways of coding a stereo pair with its difference (the side).
const INDEPENDENT_STEREO: u64 = 0b0001;
const LEFT_SIDE: u64 = 0b1000;
const SIDE_RIGHT: u64 = 0b1001;
const MID_SIDE: u64 = 0b1010;

/// What a file says of its stream, in its STREAMINFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Frames of samples a second.
    pub rate: u32,
    /// Channels, 1 to 8.
    pub channels: u8,
    /// Bits of each sample: 16 for what this encoder makes.
    pub bits: u8,
    /// Frames of samples in the whole stream.
    pub frames: u64,
    /// The shortest block but the last, in frames of samples.
    pub min_block: u16,
    /// The longest block.
    pub max_block: u16,
    /// The smallest frame, in bytes; 0 if not known.
    pub min_frame: u32,
    /// The largest frame, in bytes; 0 if not known.
    pub max_frame: u32,
    /// The MD5 of the samples, interleaved, little-endian; zeros if not
    /// known.
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// What a STREAMINFO says before the frames are made: the format and
    /// how many frames of samples will come, nothing of the frames.
    pub fn before(rate: u32, channels: u8, frames: u64) -> StreamInfo {
        let block = block_of(frames);
        StreamInfo {
            rate,
            channels,
            bits: 16,
            frames,
            min_block: block,
            max_block: block,
            min_frame: 0,
            max_frame: 0,
            md5: [0; 16],
        }
    }

    /// The STREAMINFO's 34 bytes.
    pub fn bytes(&self) -> [u8; STREAMINFO_SIZE] {
        let mut bits = Bits::default();
        bits.put(u64::from(self.min_block), 16);
        bits.put(u64::from(self.max_block), 16);
        bits.put(u64::from(self.min_frame), 24);
        bits.put(u64::from(self.max_frame), 24);
        bits.put(u64::from(self.rate), 20);
        bits.put(u64::from(self.channels.saturating_sub(1)), 3);
        bits.put(u64::from(self.bits.saturating_sub(1)), 5);
        bits.put(self.frames >> 32, 4);
        bits.put(self.frames, 32);
        let mut out = [0u8; STREAMINFO_SIZE];
        out[..18].copy_from_slice(&bits.finish());
        out[18..].copy_from_slice(&self.md5);
        out
    }

    /// Reads a STREAMINFO's 34 bytes.
    fn parse(bytes: &[u8]) -> Result<StreamInfo, Error> {
        if bytes.len() != STREAMINFO_SIZE {
            return Err(Error::Corrupt("a STREAMINFO of other than 34 bytes"));
        }
        let mut read = Reader::new(bytes);
        let min_block = read.get(16)? as u16;
        let max_block = read.get(16)? as u16;
        let min_frame = read.get(24)? as u32;
        let max_frame = read.get(24)? as u32;
        let rate = read.get(20)? as u32;
        let channels = read.get(3)? as u8 + 1;
        let bits = read.get(5)? as u8 + 1;
        let frames = (read.get(4)? << 32) | read.get(32)?;
        let mut md5 = [0u8; 16];
        md5.copy_from_slice(&bytes[18..]);
        Ok(StreamInfo {
            rate,
            channels,
            bits,
            frames,
            min_block,
            max_block,
            min_frame,
            max_frame,
            md5,
        })
    }
}

/// The block size a STREAMINFO gives a stream of `frames` frames of
/// samples: [`BLOCK`], or the one block of a shorter stream.
fn block_of(frames: u64) -> u16 {
    if frames < BLOCK as u64 { frames.max(16) as u16 } else { BLOCK as u16 }
}

/// The start of a FLAC file: its marker, its STREAMINFO, and `tags`, as
/// Vorbis comments of a name and a value each (`TITLE`, `ARTIST`). The
/// STREAMINFO is [`STREAMINFO_AT`] bytes in.
pub fn header(info: &StreamInfo, tags: &[(&str, &str)]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    // A STREAMINFO block, not the last of the metadata, and its length.
    out.extend([0, 0, 0, STREAMINFO_SIZE as u8]);
    out.extend(info.bytes());
    let mut comments = Vec::new();
    let mut string = |text: &[u8]| {
        comments.extend((text.len() as u32).to_le_bytes());
        comments.extend(text);
    };
    string(VENDOR.as_bytes());
    let tags: Vec<String> = tags.iter().map(|(name, value)| format!("{name}={value}")).collect();
    comments.extend((tags.len() as u32).to_le_bytes());
    for tag in &tags {
        comments.extend((tag.len() as u32).to_le_bytes());
        comments.extend(tag.as_bytes());
    }
    // A VORBIS_COMMENT block, the last of the metadata. A block is at
    // most 16 MiB: tags from CD-Text are a few hundred bytes.
    out.push(0x80 | 4);
    out.extend(&(comments.len() as u32).to_be_bytes()[1..]);
    out.extend(comments);
    out
}

/// A whole FLAC file of `samples`, interleaved, of `channels` channels at
/// `rate`, tagged with `tags`.
pub fn encode(samples: &[i16], rate: u32, channels: u8, tags: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = Encoder::new(rate, channels);
    let mut frames = Vec::new();
    encoder.push(samples, &mut frames);
    let info = encoder.finish(&mut frames);
    let mut file = header(&info, tags);
    file.extend(frames);
    file
}

/// Turns samples into FLAC's frames, as they come.
pub struct Encoder {
    rate: u32,
    channels: u8,
    /// Samples given that do not yet fill a block.
    pending: Vec<i16>,
    /// The next frame's number.
    number: u64,
    /// Frames of samples given so far.
    frames: u64,
    md5: Md5,
    min_frame: u32,
    max_frame: u32,
}

impl Encoder {
    /// An encoder of `channels` channels, 1 to 8, at `rate` frames of
    /// samples a second.
    ///
    /// # Panics
    ///
    /// If FLAC has no such channels or rate: more than 8 channels, or a
    /// rate of 0 or of more than 20 bits.
    pub fn new(rate: u32, channels: u8) -> Encoder {
        assert!((1..=8).contains(&channels), "FLAC has 1 to 8 channels");
        assert!(rate > 0 && rate < 1 << 20, "FLAC's rates fit in 20 bits");
        Encoder {
            rate,
            channels,
            pending: Vec::with_capacity(BLOCK * usize::from(channels)),
            number: 0,
            frames: 0,
            md5: Md5::new(),
            min_frame: u32::MAX,
            max_frame: 0,
        }
    }

    /// Takes `samples`, interleaved, whole frames of them, and adds the
    /// frames of every block they fill to `out`.
    pub fn push(&mut self, samples: &[i16], out: &mut Vec<u8>) {
        let channels = usize::from(self.channels);
        debug_assert!(samples.len().is_multiple_of(channels), "whole frames of samples");
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.md5.update(&bytes);
        self.frames += (samples.len() / channels) as u64;
        let block = BLOCK * channels;
        let mut rest = samples;
        // Fill what is pending first, then code whole blocks straight from
        // the samples given.
        if !self.pending.is_empty() {
            let take = rest.len().min(block - self.pending.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == block {
                let pending = std::mem::take(&mut self.pending);
                self.frame(&pending, out);
                self.pending = pending;
                self.pending.clear();
            }
        }
        while rest.len() >= block {
            self.frame(&rest[..block], out);
            rest = &rest[block..];
        }
        self.pending.extend_from_slice(rest);
    }

    /// Adds the last, shorter block's frame to `out`, and gives the
    /// STREAMINFO of the stream.
    pub fn finish(mut self, out: &mut Vec<u8>) -> StreamInfo {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.frame(&pending, out);
        }
        let mut info = StreamInfo::before(self.rate, self.channels, self.frames);
        if self.number > 0 {
            info.min_frame = self.min_frame;
            info.max_frame = self.max_frame;
        }
        info.md5 = self.md5.finalize().into();
        info
    }

    /// Codes the block `samples`, interleaved, as a frame added to `out`.
    fn frame(&mut self, samples: &[i16], out: &mut Vec<u8>) {
        let channels = usize::from(self.channels);
        let n = samples.len() / channels;
        let signals: Vec<Vec<i32>> = (0..channels)
            .map(|c| samples.iter().skip(c).step_by(channels).map(|&s| i32::from(s)).collect())
            .collect();
        // The channels as they are coded, with the bits of their samples.
        let (assignment, coded): (u64, Vec<(Vec<i32>, u32)>) = if channels == 2 {
            stereo(&signals[0], &signals[1])
        } else {
            (channels as u64 - 1, signals.into_iter().map(|s| (s, 16)).collect())
        };

        let mut bits = Bits::default();
        bits.put(SYNC, 16);
        let (size_code, size_extra) = match n {
            BLOCK => (0b1100, None),
            ..=256 => (0b0110, Some((n as u64 - 1, 8))),
            _ => (0b0111, Some((n as u64 - 1, 16))),
        };
        let rate_code = match self.rate {
            44_100 => 0b1001,
            48_000 => 0b1010,
            // The STREAMINFO's.
            _ => 0b0000,
        };
        bits.put(size_code, 4);
        bits.put(rate_code, 4);
        bits.put(assignment, 4);
        // 16 bits a sample, and a reserved bit.
        bits.put(0b100, 3);
        bits.put(0, 1);
        put_coded(&mut bits, self.number);
        if let Some((value, width)) = size_extra {
            bits.put(value, width);
        }
        let crc = crc8(&bits.bytes);
        bits.put(u64::from(crc), 8);
        for (signal, depth) in &coded {
            let plan = plan(signal, *depth);
            write_subframe(&mut bits, signal, *depth, &plan);
        }
        bits.align();
        let crc = crc16(&bits.bytes);
        bits.put(u64::from(crc), 16);
        let frame = bits.finish();
        self.min_frame = self.min_frame.min(frame.len() as u32);
        self.max_frame = self.max_frame.max(frame.len() as u32);
        self.number += 1;
        out.extend(frame);
    }
}

/// The cheapest way to code a stereo pair: its channel assignment and the
/// two channels coded, each with the bits of its samples (17 for the
/// difference of two of 16).
fn stereo(left: &[i32], right: &[i32]) -> (u64, Vec<(Vec<i32>, u32)>) {
    let side: Vec<i32> = left.iter().zip(right).map(|(l, r)| l - r).collect();
    let mid: Vec<i32> = left.iter().zip(right).map(|(l, r)| (l + r) >> 1).collect();
    let cost = |signal: &[i32], depth| plan(signal, depth).bits;
    let (l, r, s, m) = (cost(left, 16), cost(right, 16), cost(&side, 17), cost(&mid, 16));
    let choices =
        [(INDEPENDENT_STEREO, l + r), (LEFT_SIDE, l + s), (SIDE_RIGHT, s + r), (MID_SIDE, m + s)];
    let (assignment, _) =
        choices.iter().copied().min_by_key(|&(_, bits)| bits).unwrap_or(choices[0]);
    let channels = match assignment {
        LEFT_SIDE => vec![(left.to_vec(), 16), (side, 17)],
        SIDE_RIGHT => vec![(side, 17), (right.to_vec(), 16)],
        MID_SIDE => vec![(mid, 16), (side, 17)],
        _ => vec![(left.to_vec(), 16), (right.to_vec(), 16)],
    };
    (assignment, channels)
}

/// How a channel of a block is coded.
#[derive(Debug)]
enum Coding {
    /// One value, every sample.
    Constant,
    /// The samples as they are.
    Verbatim,
    /// A fixed predictor's residual, in partitions of Rice codes.
    Fixed { order: usize, residual: Residual },
}

/// A residual's Rice coding: whether its parameters take 5 bits, its
/// partition order, and each partition's parameter.
#[derive(Debug)]
struct Residual {
    wide: bool,
    partition_order: u32,
    parameters: Vec<u32>,
}

/// A coding, and the bits it takes, as near as can be told.
#[derive(Debug)]
struct Plan {
    coding: Coding,
    bits: u64,
}

/// The shortest coding of `signal`, whose samples are of `depth` bits.
fn plan(signal: &[i32], depth: u32) -> Plan {
    let n = signal.len();
    // A subframe's header is 8 bits.
    if signal.iter().all(|&s| s == signal[0]) {
        return Plan { coding: Coding::Constant, bits: 8 + u64::from(depth) };
    }
    let verbatim = Plan { coding: Coding::Verbatim, bits: 8 + n as u64 * u64::from(depth) };
    if n <= MAX_ORDER {
        return verbatim;
    }
    // The order whose residual is smallest in sum.
    let order = (0..=MAX_ORDER)
        .min_by_key(|&order| {
            (order..n).map(|i| fixed_residual(signal, order, i).unsigned_abs()).sum::<u64>()
        })
        .unwrap_or(0);
    let folded: Vec<u64> = (order..n).map(|i| fold(fixed_residual(signal, order, i))).collect();
    let (residual, residual_bits) = rice(&folded, n, order);
    let bits = 8 + order as u64 * u64::from(depth) + residual_bits;
    if bits < verbatim.bits {
        Plan { coding: Coding::Fixed { order, residual }, bits }
    } else {
        verbatim
    }
}

/// The residual of sample `i` of `signal` from the fixed predictor of
/// `order`: what is left of it after the polynomial through the samples
/// before it.
fn fixed_residual(signal: &[i32], order: usize, i: usize) -> i64 {
    let x = |back: usize| i64::from(signal[i - back]);
    match order {
        0 => x(0),
        1 => x(0) - x(1),
        2 => x(0) - 2 * x(1) + x(2),
        3 => x(0) - 3 * x(1) + 3 * x(2) - x(3),
        _ => x(0) - 4 * x(1) + 6 * x(2) - 4 * x(3) + x(4),
    }
}

/// A residual folded to a natural number: 0, -1, 1, -2, 2 as 0, 1, 2, 3,
/// 4, as Rice codes take them.
fn fold(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// A residual folded back.
fn unfold(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// The cheapest Rice coding of the folded residual `folded` of a block of
/// `n` samples after `order` warm-up samples, and its bits.
fn rice(folded: &[u64], n: usize, order: usize) -> (Residual, u64) {
    let mut best: Option<(Residual, u64)> = None;
    for partition_order in 0..=MAX_PARTITION_ORDER {
        let partitions = 1usize << partition_order;
        let length = n >> partition_order;
        // Partitions are the same length, the block divides into them, and
        // the first holds at least one sample after the warm-up.
        if !n.is_multiple_of(partitions) || length <= order {
            break;
        }
        let mut parameters = Vec::with_capacity(partitions);
        let mut bits = 0u64;
        for p in 0..partitions {
            let start = if p == 0 { 0 } else { p * length - order };
            let end = (p + 1) * length - order;
            let part = &folded[start..end];
            let sum: u64 = part.iter().sum();
            let count = part.len() as u64;
            let (parameter, cost) = (0..=MAX_WIDE_PARAMETER)
                .map(|k| (k, count * u64::from(k + 1) + (sum >> k)))
                .min_by_key(|&(_, cost)| cost)
                .unwrap_or((0, 0));
            parameters.push(parameter);
            bits += cost;
        }
        let wide = parameters.iter().any(|&k| k > MAX_NARROW_PARAMETER);
        // The method's two bits, the partition order's four, and each
        // partition's parameter.
        bits += 2 + 4 + partitions as u64 * if wide { 5 } else { 4 };
        if best.as_ref().is_none_or(|(_, b)| bits < *b) {
            best = Some((Residual { wide, partition_order, parameters }, bits));
        }
    }
    best.unwrap_or((Residual { wide: false, partition_order: 0, parameters: vec![0] }, u64::MAX))
}

/// Writes `signal`, of samples of `depth` bits, as `plan` codes it.
fn write_subframe(bits: &mut Bits, signal: &[i32], depth: u32, plan: &Plan) {
    // A zero bit, the type's six, and no wasted bits.
    let kind = match plan.coding {
        Coding::Constant => 0b000000,
        Coding::Verbatim => 0b000001,
        Coding::Fixed { order, .. } => 0b001000 | order as u64,
    };
    bits.put(kind << 1, 8);
    match &plan.coding {
        Coding::Constant => bits.put(signal[0] as u64, depth),
        Coding::Verbatim => signal.iter().for_each(|&s| bits.put(s as u64, depth)),
        Coding::Fixed { order, residual } => {
            for &s in &signal[..*order] {
                bits.put(s as u64, depth);
            }
            bits.put(u64::from(residual.wide), 2);
            bits.put(u64::from(residual.partition_order), 4);
            let n = signal.len();
            let length = n >> residual.partition_order;
            let width = if residual.wide { 5 } else { 4 };
            for (p, &k) in residual.parameters.iter().enumerate() {
                bits.put(u64::from(k), width);
                let start = if p == 0 { *order } else { p * length };
                for i in start..(p + 1) * length {
                    let u = fold(fixed_residual(signal, *order, i));
                    bits.unary(u >> k);
                    bits.put(u, k);
                }
            }
        }
    }
}

/// Writes `number` as a frame header's coded number: as UTF-8 writes a
/// character, in 1 to 7 bytes.
fn put_coded(bits: &mut Bits, number: u64) {
    if number < 0x80 {
        bits.put(number, 8);
        return;
    }
    // A first byte of `total` ones, a zero and 7 - `total` bits, then
    // bytes of 10 and six bits.
    let mut total = 2u32;
    while number >> ((7 - total) + 6 * (total - 1)) != 0 {
        total += 1;
    }
    let lead = u64::from((0xff00u16 >> total) as u8);
    bits.put(lead | (number >> (6 * (total - 1))), 8);
    for i in (0..total - 1).rev() {
        bits.put(0x80 | ((number >> (6 * i)) & 0x3f), 8);
    }
}

/// Bits written the highest first, as FLAC lays them out.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    /// Bits not yet a whole byte, the oldest highest, and how many.
    word: u64,
    used: u32,
}

impl Bits {
    /// Appends the low `count` bits of `value`, at most 32.
    fn put(&mut self, value: u64, count: u32) {
        debug_assert!(count <= 32);
        if count == 0 {
            return;
        }
        self.word = (self.word << count) | (value & ((1u64 << count) - 1));
        self.used += count;
        while self.used >= 8 {
            self.used -= 8;
            self.bytes.push((self.word >> self.used) as u8);
        }
        self.word &= (1u64 << self.used) - 1;
    }

    /// Appends `zeros` zero bits and a one.
    fn unary(&mut self, mut zeros: u64) {
        while zeros >= 32 {
            self.put(0, 32);
            zeros -= 32;
        }
        self.put(1, zeros as u32 + 1);
    }

    /// Fills the last byte with zeros.
    fn align(&mut self) {
        if self.used > 0 {
            self.put(0, 8 - self.used);
        }
    }

    /// The bytes, the last filled with zeros.
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.bytes
    }
}

/// CRC-8 of polynomial x^8 + x^2 + x + 1, from 0: a frame header's.
fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |crc, &byte| CRC8[usize::from(crc ^ byte)])
}

/// CRC-16 of polynomial x^16 + x^15 + x^2 + 1, from 0: a whole frame's.
fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |crc, &byte| (crc << 8) ^ CRC16[usize::from((crc >> 8) as u8 ^ byte)])
}

const CRC8: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

const CRC16: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Why a file was not read.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// It does not start with `fLaC`.
    NotFlac,
    /// It ends in the middle of something.
    Truncated,
    /// It is FLAC, but not what this reads.
    Unsupported(&'static str),
    /// A header's or frame's CRC is wrong.
    BadCrc(&'static str),
    /// Something in it is not as RFC 9639 says.
    Corrupt(&'static str),
    /// The samples are not the ones the STREAMINFO's MD5 is of.
    Md5,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFlac => write!(f, "not a FLAC file"),
            Error::Truncated => write!(f, "the file ends too soon"),
            Error::Unsupported(what) => write!(f, "{what} are not read"),
            Error::BadCrc(what) => write!(f, "a {what}'s CRC is wrong"),
            Error::Corrupt(what) => write!(f, "{what}"),
            Error::Md5 => write!(f, "the samples are not the ones the MD5 is of"),
        }
    }
}

/// A file read back.
#[derive(Debug)]
pub struct Decoded {
    /// What its STREAMINFO says.
    pub info: StreamInfo,
    /// The Vorbis comments, `NAME=value` each.
    pub tags: Vec<String>,
    /// The samples, interleaved.
    pub samples: Vec<i16>,
}

/// Reads a FLAC file of 16-bit samples, checking each CRC, the count of
/// samples and their MD5.
pub fn decode(file: &[u8]) -> Result<Decoded, Error> {
    if !file.starts_with(b"fLaC") {
        return Err(Error::NotFlac);
    }
    let mut at = 4;
    let mut info = None;
    let mut tags = Vec::new();
    loop {
        let head = file.get(at..at + 4).ok_or(Error::Truncated)?;
        let last = head[0] & 0x80 != 0;
        let length = usize::from(head[1]) << 16 | usize::from(head[2]) << 8 | usize::from(head[3]);
        let body = file.get(at + 4..at + 4 + length).ok_or(Error::Truncated)?;
        match head[0] & 0x7f {
            0 => info = Some(StreamInfo::parse(body)?),
            4 => tags = comments(body)?,
            _ => {}
        }
        at += 4 + length;
        if last {
            break;
        }
    }
    let info = info.ok_or(Error::Corrupt("no STREAMINFO"))?;
    if info.bits != 16 {
        return Err(Error::Unsupported("samples of other than 16 bits"));
    }
    let mut samples = Vec::with_capacity(info.frames as usize * usize::from(info.channels));
    while at < file.len() {
        at += frame(&file[at..], &info, &mut samples)?;
    }
    if samples.len() as u64 != info.frames * u64::from(info.channels) {
        return Err(Error::Corrupt("frames of samples other than the STREAMINFO's count"));
    }
    if info.md5 != [0; 16] {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let md5: [u8; 16] = Md5::digest(&bytes).into();
        if md5 != info.md5 {
            return Err(Error::Md5);
        }
    }
    Ok(Decoded { info, tags, samples })
}

/// A VORBIS_COMMENT block's comments, without its vendor string.
fn comments(body: &[u8]) -> Result<Vec<String>, Error> {
    let word = |at: usize| -> Result<usize, Error> {
        let b = body.get(at..at + 4).ok_or(Error::Truncated)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    // The vendor string, then the count of comments, then each.
    let mut at = 4 + word(0)?;
    let count = word(at)?;
    at += 4;
    let mut tags = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let length = word(at)?;
        let text = body.get(at + 4..at + 4 + length).ok_or(Error::Truncated)?;
        tags.push(String::from_utf8_lossy(text).into_owned());
        at += 4 + length;
    }
    Ok(tags)
}

/// Reads the frame at the start of `bytes` into `samples`, and gives its
/// length.
fn frame(bytes: &[u8], info: &StreamInfo, samples: &mut Vec<i16>) -> Result<usize, Error> {
    let mut read = Reader::new(bytes);
    if read.get(15)? != SYNC >> 1 {
        return Err(Error::Corrupt("a frame that does not start with the sync code"));
    }
    let _variable = read.get(1)?;
    let size_code = read.get(4)?;
    let rate_code = read.get(4)?;
    let assignment = read.get(4)?;
    let depth = match read.get(3)? {
        0b000 => u32::from(info.bits),
        0b100 => 16,
        _ => return Err(Error::Unsupported("samples of other than 16 bits")),
    };
    if read.get(1)? != 0 {
        return Err(Error::Corrupt("a frame header's reserved bit set"));
    }
    read.coded()?;
    let n = match size_code {
        0b0001 => 192,
        0b0010..=0b0101 => 576 << (size_code - 2),
        0b0110 => read.get(8)? as usize + 1,
        0b0111 => read.get(16)? as usize + 1,
        0b1000..=0b1111 => 256 << (size_code - 8),
        _ => return Err(Error::Corrupt("a reserved block size")),
    };
    match rate_code {
        0b1100 => drop(read.get(8)?),
        0b1101 | 0b1110 => drop(read.get(16)?),
        0b1111 => return Err(Error::Corrupt("a forbidden sample rate")),
        _ => {}
    }
    let header = read.byte();
    if read.get(8)? as u8 != crc8(&bytes[..header]) {
        return Err(Error::BadCrc("frame header"));
    }
    let channels = match assignment {
        0..=7 => assignment as usize + 1,
        LEFT_SIDE | SIDE_RIGHT | MID_SIDE => 2,
        _ => return Err(Error::Corrupt("a reserved channel assignment")),
    };
    if channels != usize::from(info.channels) {
        return Err(Error::Corrupt("a frame of other channels than the STREAMINFO's"));
    }
    let mut decoded = Vec::with_capacity(channels);
    for c in 0..channels {
        let side = matches!((assignment, c), (LEFT_SIDE | MID_SIDE, 1) | (SIDE_RIGHT, 0));
        decoded.push(subframe(&mut read, n, depth + u32::from(side))?);
    }
    if channels == 2 {
        let (a, b) = (&decoded[0], &decoded[1]);
        let (left, right): (Vec<i64>, Vec<i64>) = match assignment {
            LEFT_SIDE => (a.clone(), a.iter().zip(b).map(|(l, s)| l - s).collect()),
            SIDE_RIGHT => (a.iter().zip(b).map(|(s, r)| s + r).collect(), b.clone()),
            MID_SIDE => a
                .iter()
                .zip(b)
                .map(|(&m, &s)| {
                    let m = (m << 1) | (s & 1);
                    ((m + s) >> 1, (m - s) >> 1)
                })
                .unzip(),
            _ => (a.clone(), b.clone()),
        };
        decoded = vec![left, right];
    }
    for i in 0..n {
        for channel in &decoded {
            let sample =
                i16::try_from(channel[i]).map_err(|_| Error::Corrupt("a sample out of range"))?;
            samples.push(sample);
        }
    }
    read.align();
    let end = read.byte();
    if read.get(16)? as u16 != crc16(&bytes[..end]) {
        return Err(Error::BadCrc("frame"));
    }
    Ok(read.byte())
}

/// Reads a subframe of `n` samples of `depth` bits.
fn subframe(read: &mut Reader<'_>, n: usize, depth: u32) -> Result<Vec<i64>, Error> {
    if read.get(1)? != 0 {
        return Err(Error::Corrupt("a subframe header's first bit set"));
    }
    let kind = read.get(6)?;
    let wasted = if read.get(1)? == 1 { read.unary()? as u32 + 1 } else { 0 };
    let depth = depth
        .checked_sub(wasted)
        .filter(|&d| d > 0)
        .ok_or(Error::Corrupt("too many wasted bits"))?;
    let mut signal = match kind {
        0b000000 => vec![read.signed(depth)?; n],
        0b000001 => (0..n).map(|_| read.signed(depth)).collect::<Result<_, _>>()?,
        0b001000..=0b001100 => {
            let order = (kind - 0b001000) as usize;
            if order > n {
                return Err(Error::Corrupt("a predictor order above the block size"));
            }
            let mut signal: Vec<i64> =
                (0..order).map(|_| read.signed(depth)).collect::<Result<_, _>>()?;
            let residual = residual(read, n, order)?;
            for r in residual {
                let i = signal.len();
                let x = |back: usize| signal[i - back];
                let predicted = match order {
                    0 => 0,
                    1 => x(1),
                    2 => 2 * x(1) - x(2),
                    3 => 3 * x(1) - 3 * x(2) + x(3),
                    _ => 4 * x(1) - 6 * x(2) + 4 * x(3) - x(4),
                };
                signal.push(predicted + r);
            }
            signal
        }
        0b100000..=0b111111 => return Err(Error::Unsupported("linear prediction subframes")),
        _ => return Err(Error::Corrupt("a reserved subframe type")),
    };
    if wasted > 0 {
        signal.iter_mut().for_each(|s| *s <<= wasted);
    }
    Ok(signal)
}

/// Reads the Rice-coded residual of a block of `n` samples after `order`
/// warm-up samples.
fn residual(read: &mut Reader<'_>, n: usize, order: usize) -> Result<Vec<i64>, Error> {
    let width = match read.get(2)? {
        0 => 4,
        1 => 5,
        _ => return Err(Error::Corrupt("a reserved residual coding method")),
    };
    let escape = (1 << width) - 1;
    let partition_order = read.get(4)?;
    let partitions = 1usize << partition_order;
    let length = n >> partition_order;
    if !n.is_multiple_of(partitions) || length < order {
        return Err(Error::Corrupt("a partition order the block does not divide into"));
    }
    let mut residual = Vec::with_capacity(n - order);
    for p in 0..partitions {
        let count = if p == 0 { length - order } else { length };
        let k = read.get(width)?;
        if k == escape {
            let raw = read.get(5)? as u32;
            for _ in 0..count {
                residual.push(if raw == 0 { 0 } else { read.signed(raw)? });
            }
        } else {
            for _ in 0..count {
                let q = read.unary()?;
                let u = (q << k) | read.get(k as u32)?;
                residual.push(unfold(u));
            }
        }
    }
    Ok(residual)
}

/// Bits read the highest first.
struct Reader<'a> {
    bytes: &'a [u8],
    /// The next bit's place, in bits from the start.
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    /// The next `count` bits, at most 32, as a number.
    fn get(&mut self, count: u32) -> Result<u64, Error> {
        let mut value = 0u64;
        for _ in 0..count {
            let byte = *self.bytes.get(self.at / 8).ok_or(Error::Truncated)?;
            value = (value << 1) | u64::from(byte >> (7 - self.at % 8) & 1);
            self.at += 1;
        }
        Ok(value)
    }

    /// The next `count` bits as a signed number, two's complement.
    fn signed(&mut self, count: u32) -> Result<i64, Error> {
        let value = self.get(count)?;
        let shift = 64 - count;
        Ok(((value << shift) as i64) >> shift)
    }

    /// Zeros up to a one: how many.
    fn unary(&mut self) -> Result<u64, Error> {
        let mut zeros = 0;
        while self.get(1)? == 0 {
            zeros += 1;
        }
        Ok(zeros)
    }

    /// A frame header's coded number.
    fn coded(&mut self) -> Result<u64, Error> {
        let first = self.get(8)?;
        let total = (first as u8).leading_ones();
        match total {
            0 => Ok(first),
            1 | 8 => Err(Error::Corrupt("a coded number that does not start as one")),
            _ => {
                let mut value = first & (0x7f >> total);
                for _ in 1..total {
                    let next = self.get(8)?;
                    if next & 0xc0 != 0x80 {
                        return Err(Error::Corrupt("a coded number's byte that does not follow"));
                    }
                    value = (value << 6) | (next & 0x3f);
                }
                Ok(value)
            }
        }
    }

    /// Skips to the next whole byte.
    fn align(&mut self) {
        self.at = self.at.div_ceil(8) * 8;
    }

    /// The whole bytes read.
    fn byte(&self) -> usize {
        self.at.div_ceil(8)
    }
}
