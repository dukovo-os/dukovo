// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sound in user mode (Phase 14b.1, ADR 0320): what programs need to
//! play through `/devices/audio`, which takes only signed 16-bit
//! little-endian samples at a rate and channel count the output plays.
//!
//! - [`wav`] reads WAV files (PCM of 8, 16, 24 or 32 bits, and 32-bit
//!   floats) and writes their header.
//! - [`convert`] turns any of those samples into 16-bit ones, and maps
//!   channels: mono to both sides, stereo to mono, more to fewer.
//! - [`resample`] changes the rate, as eSpeak NG's 22,050 frames a second
//!   to an output's 48,000.
//! - [`tone`] makes test tones, and [`detect`] finds them again: the
//!   strength of one frequency in a recording (Goertzel's algorithm) and
//!   its loudness, which the boot test and the lab check the sound with.
//! - [`settings`] reads and writes audio.toml(5), the output, volume and
//!   mute that audiod(8) keeps.
//!
//! None of it does any I/O, so its tests run on the host; audio(1) and
//! audiod(8), in systools, do the rest.

pub mod convert;
pub mod detect;
pub mod resample;
pub mod settings;
pub mod tone;
pub mod wav;

#[cfg(test)]
mod tests;
