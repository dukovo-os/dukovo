// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Changing the rate of a stream of samples.
//!
//! [`Resampler`] interpolates between neighbouring frames, linearly, with
//! the position kept as a fraction in fixed point so that it never drifts
//! over a long stream. Going up (22,050 to 48,000 frames a second, as
//! speech does) adds nothing that was not there, and linear
//! interpolation's soft roll-off of the top octave costs speech nothing.
//! Going down, frequencies above the new rate's half fold back as
//! aliases, so the samples are first averaged over the span each output
//! frame stands for (a box filter), which takes most of them out. A
//! better filter can come later without changing the interface; the
//! kernel never resamples (ADR 0320).

/// The bits of fraction in a position.
const FRACTION_BITS: u32 = 32;
const ONE: u64 = 1 << FRACTION_BITS;

/// A stream's rate converter: feed it chunks of interleaved frames, in
/// order, and it gives the frames at the new rate.
pub struct Resampler {
    channels: usize,
    /// Input frames per output frame, in fixed point.
    step: u64,
    /// Where the next output frame falls, in input frames from the start
    /// of `last` (which is input frame -1, before the next chunk).
    position: u64,
    /// The last input frame of the previous chunk.
    last: Vec<i32>,
    /// How many input frames each output frame averages, going down.
    span: usize,
    /// The input frames kept from the previous chunk for that average.
    history: Vec<i16>,
}

impl Resampler {
    /// A converter of `channels` channels from `from` frames a second to
    /// `to`.
    pub fn new(from: u32, to: u32, channels: usize) -> Resampler {
        let channels = channels.max(1);
        let (from, to) = (u64::from(from.max(1)), u64::from(to.max(1)));
        let step = (from << FRACTION_BITS) / to;
        let span = if from > to { from.div_ceil(to) as usize } else { 1 };
        Resampler {
            channels,
            step,
            // The first output frame is the first input frame.
            position: ONE,
            last: vec![0; channels],
            span,
            history: Vec::new(),
        }
    }

    /// Whether it changes nothing: the rates are the same.
    pub fn is_identity(&self) -> bool {
        self.step == ONE
    }

    /// Converts the next chunk, `input`, appending the frames it gives to
    /// `out`.
    pub fn process(&mut self, input: &[i16], out: &mut Vec<i16>) {
        let channels = self.channels;
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        let frames = input.len() / channels;
        if frames == 0 {
            return;
        }
        // Going down: each frame the average of the span before it.
        let filtered;
        let input = if self.span > 1 {
            filtered = self.box_filter(&input[..frames * channels]);
            &filtered[..]
        } else {
            &input[..frames * channels]
        };
        // Frame -1 is `last`; frame i of the chunk is input[i].
        let at = |index: usize, channel: usize, last: &[i32]| -> i32 {
            if index == 0 {
                last[channel]
            } else {
                i32::from(input[(index - 1) * channels + channel])
            }
        };
        // Positions are counted from frame -1: whole part `index` is the
        // frame before, the fraction how far toward the next.
        while ((self.position >> FRACTION_BITS) as usize) < frames {
            let index = (self.position >> FRACTION_BITS) as usize;
            let fraction = (self.position & (ONE - 1)) as i64;
            for channel in 0..channels {
                let a = i64::from(at(index, channel, &self.last));
                let b = i64::from(at(index + 1, channel, &self.last));
                let value = a + (((b - a) * fraction) >> FRACTION_BITS);
                out.push(value.clamp(-32768, 32767) as i16);
            }
            self.position += self.step;
        }
        self.position -= (frames as u64) << FRACTION_BITS;
        for channel in 0..channels {
            self.last[channel] = i32::from(input[(frames - 1) * channels + channel]);
        }
    }

    /// Each frame the mean of itself and the `span - 1` before it, with
    /// those of the last chunk carried over.
    fn box_filter(&mut self, input: &[i16]) -> Vec<i16> {
        let channels = self.channels;
        let keep = (self.span - 1) * channels;
        let mut joined = core::mem::take(&mut self.history);
        // The first chunk's start: as if its first frame had been there.
        while joined.len() < keep {
            joined.extend_from_slice(&input[..channels]);
        }
        joined.extend_from_slice(input);
        let frames = input.len() / channels;
        let mut out = Vec::with_capacity(input.len());
        for frame in 0..frames {
            for channel in 0..channels {
                let sum: i32 = (0..self.span)
                    .map(|back| i32::from(joined[(frame + back) * channels + channel]))
                    .sum();
                out.push((sum / self.span as i32) as i16);
            }
        }
        self.history = joined[joined.len() - keep..].to_vec();
        out
    }
}

/// Converts all of `input` at once.
pub fn resample(input: &[i16], from: u32, to: u32, channels: usize) -> Vec<i16> {
    let mut resampler = Resampler::new(from, to, channels);
    let mut out = Vec::new();
    resampler.process(input, &mut out);
    out
}
