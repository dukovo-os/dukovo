// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sound for the Radix kernel (Phase 14b.1, ADR 0320): the arithmetic
//! behind the files of _/devices/audio_, apart from any device.
//!
//! A program plays by writing signed 16-bit little-endian samples at a
//! rate and channel count the output can play (radix-audio(2) and the
//! `audio` module of the ABI). The kernel converts nothing, so what it
//! needs is small, and all of it is here, where the tests run on the
//! host:
//!
//! - the rates an output plays, as a mask of [`RATES`], and the one
//!   nearest what a program asks ([`nearest_rate`]), with the channel
//!   count likewise ([`nearest_channels`]);
//! - the fragments the samples move in ([`Fragments`]): a period of the
//!   device, about 10 ms of sound unless the program asks for others with
//!   SET_FRAGMENT;
//! - volumes as OSS encodes them ([`Volume`]), and the scaling of samples
//!   by the cube of a volume's fraction, for outputs with no volume of
//!   their own ([`apply_gain`]), and the attenuation a device's own
//!   volume is set to for the same loudness ([`attenuation`]);
//! - the buffer of samples written and not yet played ([`Playback`]): a
//!   ring of bytes not yet handed to the device, and counters of the
//!   bytes written, handed over and played, from which the room for more,
//!   the delay and OSS's OutputSpace follow.
//!
//! The kernel (`radix::audio`) holds a [`Playback`] for each output under
//! a lock, copies writes into it, and has the output's worker move whole
//! fragments to the device and count them played as the device hands
//! them back.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use radix_abi::audio::OutputSpace;

/// The rates a device can play, in frames a second, in the order of the
/// bits of a rate mask: bit `i` is `RATES[i]`, as virtio's sound card
/// numbers them (virtio 1.2, 5.14.6.6.2) and the ABI's RATE_* bits are.
pub const RATES: [u32; 14] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    384000,
];

/// The bit of `rate` in a rate mask, if it is one of [`RATES`].
pub fn rate_bit(rate: u32) -> Option<u32> {
    RATES.iter().position(|&r| r == rate).map(|i| 1 << i)
}

/// The rate of `rates` (a mask of [`RATES`] bits) nearest `asked`, as
/// OSS's SNDCTL_DSP_SPEED takes it: of two as near, the higher, so that
/// nothing asked for is lost to a lower rate. `None` if the mask is
/// empty.
pub fn nearest_rate(rates: u32, asked: u32) -> Option<u32> {
    RATES
        .iter()
        .enumerate()
        .filter(|&(bit, _)| rates & (1 << bit) != 0)
        .map(|(_, &rate)| rate)
        .min_by_key(|&rate| (rate.abs_diff(asked), u32::MAX - rate))
}

/// The channel count between `min` and `max` nearest `asked`, as
/// SNDCTL_DSP_CHANNELS takes it.
pub fn nearest_channels(min: u8, max: u8, asked: u32) -> u8 {
    let max = max.max(min);
    u8::try_from(asked).unwrap_or(u8::MAX).clamp(min, max)
}

/// The format samples are played in: always signed 16-bit little-endian,
/// at a rate and with a number of channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Frames a second.
    pub rate: u32,
    /// Samples in a frame, one for each channel.
    pub channels: u8,
}

impl Format {
    /// What a program plays in until it asks for something else: 48,000
    /// frames a second, in stereo, which nearly every output plays (where
    /// one does not, the kernel takes the nearest it does).
    pub const DEFAULT: Format = Format { rate: 48000, channels: 2 };

    /// The bytes of a frame: two for each channel.
    pub fn frame_bytes(self) -> u32 {
        2 * u32::from(self.channels.max(1))
    }

    /// The bytes of a second of sound.
    pub fn bytes_per_second(self) -> u32 {
        self.rate.saturating_mul(self.frame_bytes())
    }
}

/// The smallest fragment, in bytes.
pub const MIN_FRAGMENT: u32 = 256;
/// The largest fragment, in bytes.
pub const MAX_FRAGMENT: u32 = 65536;
/// The fewest fragments a buffer has: one playing while the next is
/// filled.
pub const MIN_FRAGMENTS: u32 = 2;
/// The most fragments a buffer has.
pub const MAX_FRAGMENTS: u32 = 32;
/// The fragments of a buffer unless the program asks for others.
pub const DEFAULT_FRAGMENTS: u32 = 8;
/// The most bytes a buffer has in all, which is the size of the memory
/// the kernel keeps for an output's samples.
pub const MAX_BUFFER: u32 = 256 * 1024;

/// How the buffer of an output is cut up: `count` fragments of `bytes`
/// each. A fragment is what moves to the device at a time (a period), so
/// its length is how often the device asks for more, and the buffer's is
/// how long the sound written waits before it is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fragments {
    /// The bytes of a fragment: a whole number of frames.
    pub bytes: u32,
    /// How many fragments the buffer has.
    pub count: u32,
}

impl Fragments {
    /// The fragments of a program that asks for none: a power of two of
    /// bytes near 10 ms of sound (2048 bytes, 10.7 ms, at 48,000 frames a
    /// second in stereo), and [`DEFAULT_FRAGMENTS`] of them.
    pub fn default_for(format: Format) -> Fragments {
        let ten_ms = (format.bytes_per_second() / 100).max(1);
        // The power of two nearest 10 ms: the one above, unless the one
        // below is nearer.
        let above = ten_ms.next_power_of_two();
        let below = above / 2;
        let bytes = if below > 0 && ten_ms - below < above - ten_ms { below } else { above };
        Fragments::fit(bytes, DEFAULT_FRAGMENTS, format)
    }

    /// The fragments SET_FRAGMENT's `request` asks for: 2^n bytes, n in
    /// bits 0 to 15, and at most as many as bits 16 to 31 say (0x7fff, as
    /// OSS programs write it, for as many as the kernel likes).
    pub fn requested(request: u32, format: Format) -> Fragments {
        let shift = (request & 0xffff).min(31);
        let bytes = 1u32 << shift;
        let count = request >> 16;
        Fragments::fit(bytes, count, format)
    }

    /// `bytes` and `count` within the limits: a fragment between
    /// [`MIN_FRAGMENT`] and [`MAX_FRAGMENT`] bytes, cut to whole frames,
    /// and between [`MIN_FRAGMENTS`] and [`MAX_FRAGMENTS`] of them, with
    /// no more than [`MAX_BUFFER`] bytes in all.
    fn fit(bytes: u32, count: u32, format: Format) -> Fragments {
        let frame = format.frame_bytes();
        let bytes = bytes.clamp(MIN_FRAGMENT, MAX_FRAGMENT);
        // Whole frames, so that a fragment never ends between two samples
        // of a frame: 252 bytes of 6-byte frames rather than 256.
        let bytes = (bytes / frame * frame).max(frame);
        let count = count.clamp(MIN_FRAGMENTS, MAX_FRAGMENTS).min(MAX_BUFFER / bytes);
        Fragments { bytes, count }
    }

    /// The bytes of the whole buffer.
    pub fn capacity(self) -> u32 {
        self.bytes * self.count
    }
}

/// A volume, a percent for each of the two channels, as OSS encodes it in
/// an int: the left in bits 0 to 7, the right in bits 8 to 15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The left channel's, 0 to 100.
    pub left: u8,
    /// The right channel's, 0 to 100.
    pub right: u8,
}

/// The loudest a channel's volume is.
pub const VOLUME_MAX: u8 = 100;

impl Volume {
    /// Both channels at full volume: the samples as written.
    pub const FULL: Volume = Volume { left: VOLUME_MAX, right: VOLUME_MAX };
    /// Both channels silent.
    pub const MUTED: Volume = Volume { left: 0, right: 0 };

    /// The volume an int encodes, each channel's cut to 100.
    pub fn decode(value: u32) -> Volume {
        let channel = |shift: u32| ((value >> shift) & 0xff).min(u32::from(VOLUME_MAX)) as u8;
        Volume { left: channel(0), right: channel(8) }
    }

    /// The int that encodes the volume.
    pub fn encode(self) -> u32 {
        u32::from(self.left) | u32::from(self.right) << 8
    }

    /// The same volume on both channels.
    pub fn both(percent: u8) -> Volume {
        let percent = percent.min(VOLUME_MAX);
        Volume { left: percent, right: percent }
    }
}

/// The factor a sample is scaled by at `percent` volume, in 16.16 fixed
/// point: the cube of the volume's fraction, so that the steps from 0 to
/// 100 sound evenly spaced, as a fader's do (loudness follows the cube
/// root of power, near enough: 50% is an eighth of the amplitude, about
/// 18 dB down). 100 is 65,536, the sample unchanged.
pub fn gain(percent: u8) -> u32 {
    let p = u64::from(percent.min(VOLUME_MAX));
    (p * p * p * 65536 / 1_000_000) as u32
}

/// The attenuation of each volume from 1 to 100 percent, in quarters of a
/// decibel: 60 log10(v / 100), the decibels of the cube of the fraction,
/// so that a device's own volume sounds as the kernel's scaling does
/// ([`gain`]).
const ATTENUATION: [u16; 100] = [
    480, 408, 365, 336, 312, 293, 277, 263, 251, 240, 230, 221, 213, 205, 198, 191, 185, 179, 173,
    168, 163, 158, 153, 149, 144, 140, 136, 133, 129, 125, 122, 119, 116, 112, 109, 106, 104, 101,
    98, 96, 93, 90, 88, 86, 83, 81, 79, 77, 74, 72, 70, 68, 66, 64, 62, 60, 59, 57, 55, 53, 52, 50,
    48, 47, 45, 43, 42, 40, 39, 37, 36, 34, 33, 31, 30, 29, 27, 26, 25, 23, 22, 21, 19, 18, 17, 16,
    15, 13, 12, 11, 10, 9, 8, 6, 5, 4, 3, 2, 1, 0,
];

/// How far below full a device's own volume sets its sound for `percent`
/// (1 to 100; 0, which mutes, is taken as 1), in quarters of a decibel:
/// as far as [`gain`] scales the samples, so that 50% is 18 dB down. An
/// HD Audio codec's amplifiers and a USB device's feature unit both
/// follow it.
pub fn attenuation(percent: u8) -> u16 {
    ATTENUATION[usize::from(percent.clamp(1, VOLUME_MAX)) - 1]
}

/// Scales the samples of `bytes` (whole frames of `channels` signed
/// 16-bit little-endian samples) by `volume`: the first channel by the
/// left volume, the second by the right, and any other, or the one
/// channel of mono, by the mean of the two. A trailing byte that is not a
/// whole sample is left alone. Full volume changes nothing.
pub fn apply_gain(bytes: &mut [u8], channels: u8, volume: Volume) {
    if volume == Volume::FULL {
        return;
    }
    let left = i64::from(gain(volume.left));
    let right = i64::from(gain(volume.right));
    let middle = i64::from(gain(volume.left.midpoint(volume.right)));
    let channels = usize::from(channels.max(1));
    for (i, sample) in bytes.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let factor = match (channels, i % channels) {
            (1, _) => middle,
            (_, 0) => left,
            (_, 1) => right,
            _ => middle,
        };
        let value = i64::from(i16::from_le_bytes(*sample));
        // At most full scale times 65,536, so the product fits and the
        // result is a sample again.
        let scaled = (value * factor) >> 16;
        *sample = (scaled as i16).to_le_bytes();
    }
}

/// A ring of bytes over memory the caller gives: what was written and
/// not yet taken, oldest first. It holds at most its `limit`, which is
/// at most the memory's length.
pub struct Ring<S> {
    storage: S,
    limit: usize,
    start: usize,
    len: usize,
}

impl<S: AsRef<[u8]> + AsMut<[u8]>> Ring<S> {
    /// An empty ring over `storage`, holding as much as it does.
    pub fn new(storage: S) -> Ring<S> {
        let limit = storage.as_ref().len();
        Ring { storage, limit, start: 0, len: 0 }
    }

    /// Empties the ring and has it hold at most `limit` bytes (no more
    /// than its memory).
    pub fn reset(&mut self, limit: usize) {
        self.limit = limit.min(self.storage.as_ref().len());
        self.start = 0;
        self.len = 0;
    }

    /// The most it holds.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// The bytes it holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Adds as much of `data` as fits; returns how much did.
    pub fn push(&mut self, data: &[u8]) -> usize {
        let count = data.len().min(self.limit - self.len);
        let mut done = 0;
        while done < count {
            // The next free byte, and how far the free space runs before
            // the end of the ring's memory.
            let at = (self.start + self.len) % self.limit;
            let run = (count - done).min(self.limit - at);
            self.storage.as_mut()[at..at + run].copy_from_slice(&data[done..done + run]);
            self.len += run;
            done += run;
        }
        count
    }

    /// Adds `count` zero bytes, or as many as fit; returns how many did.
    pub fn push_silence(&mut self, count: usize) -> usize {
        let count = count.min(self.limit - self.len);
        let mut done = 0;
        while done < count {
            let at = (self.start + self.len) % self.limit;
            let run = (count - done).min(self.limit - at);
            self.storage.as_mut()[at..at + run].fill(0);
            self.len += run;
            done += run;
        }
        count
    }

    /// Takes the oldest bytes into `out`, as many as it holds or fit;
    /// returns how many.
    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let count = out.len().min(self.len);
        let mut done = 0;
        while done < count {
            let run = (count - done).min(self.limit - self.start);
            out[done..done + run]
                .copy_from_slice(&self.storage.as_ref()[self.start..self.start + run]);
            self.start = (self.start + run) % self.limit;
            self.len -= run;
            done += run;
        }
        if self.len == 0 {
            self.start = 0;
        }
        count
    }
}

/// The samples of one output between the program that plays and the
/// device: written to the ring, moved out to the device a fragment at a
/// time, and counted played as the device hands each back.
///
/// Three counters run from the last [`Playback::configure`] or
/// [`Playback::clear`]: the bytes written (silence added to fill out a
/// fragment included), the bytes handed to the device, and the bytes it
/// has played, each at most the one before. The ring holds what was
/// written and not handed over, and the room for more is what keeps the
/// bytes written and not played within the buffer, so that a program
/// never runs further ahead of the sound than its fragments say.
pub struct Playback<S> {
    ring: Ring<S>,
    format: Format,
    fragments: Fragments,
    written: u64,
    submitted: u64,
    played: u64,
}

impl<S: AsRef<[u8]> + AsMut<[u8]>> Playback<S> {
    /// An empty buffer over `storage`, which should be [`MAX_BUFFER`]
    /// bytes: at least the capacity of any fragments it is configured
    /// with.
    pub fn new(storage: S, format: Format) -> Playback<S> {
        let fragments = Fragments::default_for(format);
        let mut ring = Ring::new(storage);
        ring.reset(fragments.capacity() as usize);
        Playback { ring, format, fragments, written: 0, submitted: 0, played: 0 }
    }

    /// Empties the buffer and cuts it up anew. A buffer whose memory is
    /// smaller than the fragments ask for has fewer of them.
    pub fn configure(&mut self, format: Format, fragments: Fragments) {
        let room = self.ring.storage.as_ref().len() as u32;
        let count = fragments.count.min(room / fragments.bytes.max(1)).max(1);
        self.format = format;
        self.fragments = Fragments { bytes: fragments.bytes, count };
        self.clear();
    }

    /// Drops everything written and starts the counters again, as when
    /// the device was stopped and has handed back nothing it will play.
    pub fn clear(&mut self) {
        self.ring.reset(self.fragments.capacity() as usize);
        self.written = 0;
        self.submitted = 0;
        self.played = 0;
    }

    /// The format the samples are in.
    pub fn format(&self) -> Format {
        self.format
    }

    /// How the buffer is cut up.
    pub fn fragments(&self) -> Fragments {
        self.fragments
    }

    /// The bytes written and not yet played: how long a sample written
    /// now waits to be heard, in bytes (GET_OUTPUT_DELAY).
    pub fn delay(&self) -> u32 {
        (self.written - self.played) as u32
    }

    /// The bytes that can be written without waiting.
    pub fn room(&self) -> u32 {
        self.fragments.capacity().saturating_sub(self.delay())
    }

    /// What GET_OUTPUT_SPACE answers.
    pub fn space(&self) -> OutputSpace {
        let room = self.room();
        OutputSpace {
            fragments: (room / self.fragments.bytes) as i32,
            total: self.fragments.count as i32,
            fragment_bytes: self.fragments.bytes as i32,
            bytes: room as i32,
        }
    }

    /// Copies as much of `data` as there is room for; returns how much.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let count = data.len().min(self.room() as usize);
        let done = self.ring.push(&data[..count]);
        self.written += done as u64;
        done
    }

    /// Fills the fragment last written in part out with silence, so that
    /// it can be handed over (SYNC, POST, and a player that closes);
    /// returns the bytes of silence added. The room is always there: the
    /// bytes in flight and in the ring are whole fragments but for this
    /// one, within a buffer of whole fragments.
    pub fn fill_out(&mut self) -> u32 {
        let part = (self.ring.len() % self.fragments.bytes as usize) as u32;
        if part == 0 {
            return 0;
        }
        let silence = self.fragments.bytes - part;
        let added = self.ring.push_silence(silence as usize);
        self.written += added as u64;
        added as u32
    }

    /// Whether a whole fragment waits to be handed to the device.
    pub fn has_fragment(&self) -> bool {
        self.ring.len() >= self.fragments.bytes as usize
    }

    /// Bytes written and not yet handed to the device.
    pub fn queued(&self) -> u32 {
        self.ring.len() as u32
    }

    /// Takes the next whole fragment into `out` (at least a fragment
    /// long) to hand to the device; returns its length, or `None` while
    /// less than a fragment waits.
    pub fn take(&mut self, out: &mut [u8]) -> Option<usize> {
        let bytes = self.fragments.bytes as usize;
        if !self.has_fragment() || out.len() < bytes {
            return None;
        }
        let taken = self.ring.pop(&mut out[..bytes]);
        self.submitted += taken as u64;
        Some(taken)
    }

    /// Fragments handed to the device and not yet played.
    pub fn in_flight(&self) -> u32 {
        ((self.submitted - self.played) / u64::from(self.fragments.bytes)) as u32
    }

    /// Counts a fragment the device handed back: played. Returns false if
    /// none was in flight, which the device should not do.
    pub fn played_one(&mut self) -> bool {
        if self.submitted - self.played < u64::from(self.fragments.bytes) {
            return false;
        }
        self.played += u64::from(self.fragments.bytes);
        true
    }

    /// Whether everything written has been played.
    pub fn is_drained(&self) -> bool {
        self.written == self.played
    }
}
