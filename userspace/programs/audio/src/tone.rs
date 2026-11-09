// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Test tones: sine waves on chosen channels.

/// The tone audio(1)'s `test` plays: A above middle C.
pub const TEST_FREQUENCY: f64 = 440.0;

/// The left channel's tone in the boot test and the lab, and the right's:
/// apart enough that neither is a harmonic of the other.
pub const LEFT_FREQUENCY: f64 = 440.0;
/// The right channel's.
pub const RIGHT_FREQUENCY: f64 = 660.0;

/// `frames` frames of `channels` channels at `rate` frames a second: a
/// sine of `frequency` at `amplitude` (0 to 1 of full scale) on the
/// channels `on` says (by number), silence on the others. The first and
/// last 5 ms fade in and out, so that the tone starts and ends without a
/// click.
pub fn sine(
    frequency: f64,
    rate: u32,
    channels: usize,
    frames: usize,
    amplitude: f64,
    on: &dyn Fn(usize) -> bool,
) -> Vec<i16> {
    let rate = f64::from(rate.max(1));
    let fade = (rate * 0.005) as usize;
    let mut out = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        let edge = frame.min(frames - 1 - frame);
        let gain = if fade > 0 && edge < fade { edge as f64 / fade as f64 } else { 1.0 };
        let phase = 2.0 * core::f64::consts::PI * frequency * frame as f64 / rate;
        let value = (phase.sin() * amplitude * gain * 32767.0).round() as i16;
        for channel in 0..channels {
            out.push(if on(channel) { value } else { 0 });
        }
    }
    out
}
