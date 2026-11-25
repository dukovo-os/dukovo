// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The test pictures: every pixel a colour worked out from its column,
//! its row and the picture's number, its _seed_.
//!
//! Neighbouring pixels have unrelated colours, and so have the same
//! pixel of two seeds. A screen that shows the picture of seed 7 where
//! the picture of seed 8 should be, shows it a pixel to the side, or
//! reads its rows at the wrong length differs from what is expected in
//! almost every pixel.

/// The colour of the pixel at column `x` and row `y` of the picture of
/// `seed`: red, green and blue.
pub fn colour(seed: u32, x: u32, y: u32) -> [u8; 3] {
    // The three numbers stirred into one, each bit of which depends on
    // every bit of all three.
    let mut mixed = seed.wrapping_mul(0x9e6c_63d1) ^ x.wrapping_mul(0x3c79_ac49);
    mixed = mixed.rotate_left(13) ^ y.wrapping_mul(0x1b56_c4e9);
    mixed ^= mixed >> 15;
    mixed = mixed.wrapping_mul(0x2c1b_3c6d);
    mixed ^= mixed >> 12;
    mixed = mixed.wrapping_mul(0x297a_2d39);
    mixed ^= mixed >> 15;
    let [red, green, blue, _] = mixed.to_le_bytes();
    [red, green, blue]
}

/// Paints part of the picture of `seed` into `rows`, as FORMAT_XRGB8888
/// pixels: `width` pixels a row, from the pixel at column `x` and row
/// `y` of the picture, with `row_bytes` from one row to the next. The
/// last row may end with its last pixel.
pub fn paint(seed: u32, (x, y): (u32, u32), width: u32, row_bytes: usize, rows: &mut [u8]) {
    for (down, row) in rows.chunks_mut(row_bytes.max(4)).enumerate() {
        let pixels = row.as_chunks_mut::<4>().0.iter_mut().take(width as usize);
        for (across, pixel) in pixels.enumerate() {
            let [red, green, blue] = colour(seed, x + across as u32, y + down as u32);
            *pixel = [blue, green, red, 0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbours_and_seeds_have_unrelated_colours() {
        let mut same = 0;
        for y in 0..64 {
            for x in 0..64 {
                let here = colour(7, x, y);
                same += u32::from(here == colour(7, x + 1, y));
                same += u32::from(here == colour(7, x, y + 1));
                same += u32::from(here == colour(8, x, y));
            }
        }
        // A colour is one of 16 million: none of 12 thousand pairs match.
        assert_eq!(same, 0);
        // Every value of a colour's parts turns up: no bit is stuck.
        let mut seen = [0u8; 3];
        let mut unseen = [0xffu8; 3];
        for x in 0..256 {
            for (part, value) in colour(1, x, 0).into_iter().enumerate() {
                seen[part] |= value;
                unseen[part] &= value;
            }
        }
        assert_eq!((seen, unseen), ([0xff; 3], [0; 3]));
    }

    #[test]
    fn a_part_is_painted_from_its_own_corner_with_blue_first() {
        // Two rows of three pixels, each row with four bytes to spare.
        let mut rows = [0xaau8; 16 + 12];
        paint(5, (10, 20), 3, 16, &mut rows);
        for (down, across) in [(0, 0), (0, 2), (1, 0), (1, 2)] {
            let at = down * 16 + across * 4;
            let [red, green, blue] = colour(5, 10 + across as u32, 20 + down as u32);
            assert_eq!(rows[at..at + 4], [blue, green, red, 0]);
        }
        // What is past a row's pixels is not touched.
        assert_eq!(rows[12..16], [0xaa; 4]);
    }
}
