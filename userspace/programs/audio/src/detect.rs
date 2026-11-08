// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Finding tones in a recording: what the boot test checks QEMU's capture
//! of the guest's sound with, and the lab its recording on the Mint PC.
//!
//! [`strength`] is Goertzel's algorithm: the power of one frequency in a
//! block of samples, the one bin of a discrete Fourier transform that is
//! wanted, without the rest. Divided by the block's whole power it says
//! how much of the sound is that tone ([`share`]): near 1 for a clean
//! sine, near 0 for silence, noise or another tone. [`rms`] is the
//! loudness, whose ratio between two recordings checks a volume.

/// One channel of interleaved frames.
pub fn channel(samples: &[i16], channels: usize, which: usize) -> Vec<f64> {
    samples.chunks_exact(channels.max(1)).map(|frame| f64::from(frame[which])).collect()
}

/// The power of `frequency` in `block`, sampled at `rate` frames a
/// second: the squared magnitude of its DFT bin, scaled so that a full
/// block of a sine of amplitude A gives about (A·n/2)².
pub fn strength(block: &[f64], rate: u32, frequency: f64) -> f64 {
    let omega = 2.0 * core::f64::consts::PI * frequency / f64::from(rate.max(1));
    let coefficient = 2.0 * omega.cos();
    let (mut previous, mut before) = (0.0f64, 0.0f64);
    for &sample in block {
        let current = sample + coefficient * previous - before;
        before = previous;
        previous = current;
    }
    previous * previous + before * before - coefficient * previous * before
}

/// How much of `block`'s power is `frequency`: its Goertzel power against
/// what a pure tone of the block's whole power would give, 0 to about 1.
pub fn share(block: &[f64], rate: u32, frequency: f64) -> f64 {
    let energy: f64 = block.iter().map(|sample| sample * sample).sum();
    if energy == 0.0 || block.is_empty() {
        return 0.0;
    }
    // A sine's whole power over n samples is n·A²/2, and its bin's
    // n²·A²/4: the ratio of the two is n/2.
    strength(block, rate, frequency) / (energy * block.len() as f64 / 2.0)
}

/// The root mean square of `block`: its loudness.
pub fn rms(block: &[f64]) -> f64 {
    if block.is_empty() {
        return 0.0;
    }
    (block.iter().map(|sample| sample * sample).sum::<f64>() / block.len() as f64).sqrt()
}

/// The frames of `samples` (one channel) whose 10 ms blocks are louder
/// than `floor` (a fraction of full scale): where the sound is, as a
/// range of frames from the first loud block to the end of the last.
pub fn sounding(samples: &[f64], rate: u32, floor: f64) -> Option<core::ops::Range<usize>> {
    let block = (rate as usize / 100).max(1);
    let loud =
        |start: usize| rms(&samples[start..(start + block).min(samples.len())]) > floor * 32768.0;
    let starts: Vec<usize> =
        (0..samples.len()).step_by(block).filter(|&start| loud(start)).collect();
    let first = *starts.first()?;
    let last = *starts.last()?;
    Some(first..(last + block).min(samples.len()))
}
