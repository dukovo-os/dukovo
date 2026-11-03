// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The nearest colour of xterm's 256-colour palette to a 24-bit one, for
//! terminals that show no 24-bit colour (ADR 0370).
//!
//! Written for Dukovo from the palette's definition alone: colours 16 to
//! 231 are a cube of six levels a channel, 0, 95, 135, 175, 215 and 255,
//! colour `16 + 36 r + 6 g + b`; colours 232 to 255 are 24 greys, `8 +
//! 10 n` in each channel. Colours 0 to 15 are the terminal's own, which
//! each terminal sets as it likes, so they are never chosen. Of the
//! cube's colour nearest in each channel and the nearest grey, the one
//! nearer as people see it is taken, by the "redmean" weighting of the
//! channels' differences.

#![forbid(unsafe_code)]

/// The cube's six levels of a channel.
const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// A colour given as its red, green and blue.
pub trait AsRGB {
    /// Its red, green and blue, each 0 to 255.
    fn as_rgb(&self) -> (u8, u8, u8);
}

impl AsRGB for (u8, u8, u8) {
    fn as_rgb(&self) -> (u8, u8, u8) {
        *self
    }
}

impl AsRGB for [u8; 3] {
    fn as_rgb(&self) -> (u8, u8, u8) {
        (self[0], self[1], self[2])
    }
}

/// `0xRRGGBB`; the top byte is ignored.
impl AsRGB for u32 {
    fn as_rgb(&self) -> (u8, u8, u8) {
        let [_, r, g, b] = self.to_be_bytes();
        (r, g, b)
    }
}

impl<T: AsRGB + ?Sized> AsRGB for &T {
    fn as_rgb(&self) -> (u8, u8, u8) {
        (**self).as_rgb()
    }
}

/// The index, 16 to 255, of the palette's colour nearest to `colour`.
pub fn ansi256_from_rgb<C: AsRGB>(colour: C) -> u8 {
    let (r, g, b) = colour.as_rgb();
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    // Six levels a channel, so each index is below 6 and this below 216.
    let cube_index = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;
    let grey_n = grey((u32::from(r) + u32::from(g) + u32::from(b)) / 3);
    let grey_level = 8 + 10 * grey_n;
    let grey_colour = (grey_level, grey_level, grey_level);
    if distance((r, g, b), grey_colour) < distance((r, g, b), cube) {
        232 + grey_n
    } else {
        cube_index
    }
}

/// The 24-bit colour of the palette's colour `index`, for indices 16 to
/// 255; colours 0 to 15 as xterm sets them by default.
pub fn rgb_from_ansi256(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => SYSTEM[usize::from(index)],
        16..=231 => {
            let i = usize::from(index - 16);
            (LEVELS[i / 36], LEVELS[i / 6 % 6], LEVELS[i % 6])
        }
        232..=255 => {
            let level = 8 + 10 * (index - 232);
            (level, level, level)
        }
    }
}

/// xterm's default colours 0 to 15.
const SYSTEM: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// The index of the cube's level nearest to `value`.
fn level(value: u8) -> usize {
    let mut best = 0;
    for (i, &level) in LEVELS.iter().enumerate() {
        if value.abs_diff(level) < value.abs_diff(LEVELS[best]) {
            best = i;
        }
    }
    best
}

/// The grey, 0 to 23, whose level `8 + 10 n` is nearest to `value`.
fn grey(value: u32) -> u8 {
    let n = (value.saturating_sub(3) / 10).min(23);
    // Rounded: the level just above may be nearer.
    let n = if n < 23 && value > 8 + 10 * n + 5 { n + 1 } else { n };
    n as u8
}

/// How far apart two colours look, the "redmean" weighting of the
/// squares of their channels' differences (times 256).
fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let mean = (u32::from(a.0) + u32::from(b.0)) / 2;
    let d = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
    (512 + mean) * d(a.0, b.0) + 1024 * d(a.1, b.1) + (767 - mean) * d(a.2, b.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_colours_are_their_own() {
        for index in 16..=255u8 {
            assert_eq!(ansi256_from_rgb(rgb_from_ansi256(index)), index, "{index}");
        }
    }

    #[test]
    fn colours_find_the_nearest() {
        assert_eq!(ansi256_from_rgb((0, 0, 0)), 16);
        assert_eq!(ansi256_from_rgb((255, 255, 255)), 231);
        assert_eq!(ansi256_from_rgb((255, 0, 0)), 196);
        assert_eq!(ansi256_from_rgb((128, 128, 128)), 244);
        assert_eq!(ansi256_from_rgb((95, 135, 175)), 67);
        assert_eq!(ansi256_from_rgb((250, 2, 3)), 196);
        assert_eq!(ansi256_from_rgb(0x00_ff_87_00_u32), 208);
        assert_eq!(ansi256_from_rgb([1, 1, 1]), 16);
        assert_eq!(ansi256_from_rgb((12, 12, 12)), 232);
    }

    #[test]
    fn every_colour_finds_one_nearly_the_nearest() {
        // Against every colour of 16 to 255: never more than a little
        // farther than the nearest there is.
        let mut worst = 0f64;
        for r in (0..=255u8).step_by(7) {
            for g in (0..=255u8).step_by(7) {
                for b in (0..=255u8).step_by(7) {
                    let c = (r, g, b);
                    let found = distance(c, rgb_from_ansi256(ansi256_from_rgb(c)));
                    let best = (16..=255u8).map(|i| distance(c, rgb_from_ansi256(i))).min().unwrap();
                    worst = worst.max(f64::from(found) / f64::from(best.max(1)));
                }
            }
        }
        assert!(worst < 2.0, "{worst}");
    }
}
