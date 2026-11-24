// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rows of pixels on their way into device memory (ADR 0705).
//!
//! A picture's pixels are FORMAT_XRGB8888, or FORMAT_ARGB8888 for a
//! pointer. A virtio card takes them as they are. The firmware's
//! framebuffer has the layout the firmware chose, which is XRGB8888 on
//! nearly every machine and something else on a few: 16 bits a pixel, 24,
//! or red and blue the other way round. [`store`] writes a row in that
//! [`Layout`]; for XRGB8888 it is one copy of the row.
//!
//! The firmware's card has no pointer of its own either, so the kernel
//! draws one: [`blend`] lays a row of the pointer's picture over the same
//! row of the frame before it is stored. The frame's own pixels are never
//! altered, and the framebuffer is never read: what was under the pointer
//! comes back from the frame when the pointer moves on.

/// Where one colour is in a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    /// The place of its lowest bit.
    pub shift: u8,
    /// How many bits it has.
    pub bits: u8,
}

/// How a pixel of device memory is laid out, as a little-endian number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Red.
    pub red: Channel,
    /// Green.
    pub green: Channel,
    /// Blue.
    pub blue: Channel,
    /// The bytes of a pixel: 2, 3 or 4.
    pub bytes: u8,
}

impl Layout {
    /// The layout of a picture's own pixels: blue in the lowest byte,
    /// then green, then red, and a byte unused.
    pub const XRGB8888: Layout = Layout {
        red: Channel { shift: 16, bits: 8 },
        green: Channel { shift: 8, bits: 8 },
        blue: Channel { shift: 0, bits: 8 },
        bytes: 4,
    };

    /// Whether [`store`] writes this layout: 2 to 4 bytes a pixel, each
    /// colour of 1 to 16 bits and inside the pixel.
    pub fn is_usable(self) -> bool {
        let fits = |channel: Channel| {
            (1..=16).contains(&channel.bits)
                && u32::from(channel.shift) + u32::from(channel.bits) <= u32::from(self.bytes) * 8
        };
        (2..=4).contains(&self.bytes) && fits(self.red) && fits(self.green) && fits(self.blue)
    }

    /// The pixel of this layout that shows the XRGB8888 pixel `pixel`.
    fn encode(self, pixel: u32) -> u32 {
        scale(pixel >> 16, self.red) | scale(pixel >> 8, self.green) | scale(pixel, self.blue)
    }
}

/// The low 8 bits of `value`, as a colour, at `channel`'s place and
/// width. A narrower channel keeps the top bits. A wider one repeats
/// them below, so that full white stays full white.
fn scale(value: u32, channel: Channel) -> u32 {
    let value = value & 0xff;
    let bits = u32::from(channel.bits);
    let wide = match bits {
        0 => return 0,
        1..=8 => value >> (8 - bits),
        9..=16 => (value << (bits - 8)) | (value >> (16 - bits)),
        _ => return 0,
    };
    wide.checked_shl(u32::from(channel.shift)).unwrap_or(0)
}

/// Writes the pixels of `row`, which are XRGB8888, into `target` in
/// `layout`: as many whole pixels as both hold. Returns how many.
pub fn store(layout: Layout, row: &[u8], target: &mut [u8]) -> usize {
    if layout == Layout::XRGB8888 {
        let pixels = row.len().min(target.len()) / 4;
        target[..pixels * 4].copy_from_slice(&row[..pixels * 4]);
        return pixels;
    }
    let bytes = usize::from(layout.bytes);
    if !(2..=4).contains(&bytes) {
        return 0;
    }
    let mut pixels = 0;
    for (source, out) in row.as_chunks::<4>().0.iter().zip(target.chunks_exact_mut(bytes)) {
        let pixel = u32::from_le_bytes(*source);
        out.copy_from_slice(&layout.encode(pixel).to_le_bytes()[..bytes]);
        pixels += 1;
    }
    pixels
}

/// `value` over 255, to the nearest, for a `value` up to 255 times 255.
fn over_255(value: u32) -> u32 {
    let rounded = value + 128;
    (rounded + (rounded >> 8)) >> 8
}

/// Lays the pixels of `over`, a row of a pointer's picture, on those of
/// `row`, a row of the frame, as many as both hold. `over` is
/// FORMAT_ARGB8888: its colours are already multiplied by how solid the
/// pixel is, so each colour of the frame is dimmed by what is left and
/// the pointer's is added.
pub fn blend(row: &mut [u8], over: &[u8]) {
    for (under, pointer) in row.as_chunks_mut::<4>().0.iter_mut().zip(over.as_chunks::<4>().0) {
        let clear = 255 - u32::from(pointer[3]);
        if clear == 255 && pointer[..3] == [0, 0, 0] {
            continue;
        }
        for (colour, &added) in under.iter_mut().zip(pointer).take(3) {
            let mixed = u32::from(added) + over_255(u32::from(*colour) * clear);
            *colour = mixed.min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB565: Layout = Layout {
        red: Channel { shift: 11, bits: 5 },
        green: Channel { shift: 5, bits: 6 },
        blue: Channel { shift: 0, bits: 5 },
        bytes: 2,
    };
    const BGR888: Layout = Layout {
        red: Channel { shift: 0, bits: 8 },
        green: Channel { shift: 8, bits: 8 },
        blue: Channel { shift: 16, bits: 8 },
        bytes: 3,
    };
    const XRGB2101010: Layout = Layout {
        red: Channel { shift: 20, bits: 10 },
        green: Channel { shift: 10, bits: 10 },
        blue: Channel { shift: 0, bits: 10 },
        bytes: 4,
    };

    /// A row of XRGB8888 pixels from colours written as 0xRRGGBB.
    fn row(colours: &[u32]) -> Vec<u8> {
        colours.iter().flat_map(|colour| colour.to_le_bytes()).collect()
    }

    #[test]
    fn a_row_in_the_usual_layout_is_copied_as_it_is() {
        let source = row(&[0x12_3456, 0xff_ffff, 0xab_cdef]);
        let mut target = [0xee; 12];
        assert_eq!(store(Layout::XRGB8888, &source, &mut target), 3);
        assert_eq!(target[..], source[..]);
    }

    #[test]
    fn only_whole_pixels_that_both_hold_are_written() {
        let source = row(&[0x11_1111, 0x22_2222, 0x33_3333]);
        let mut target = [0xee; 10];
        assert_eq!(store(Layout::XRGB8888, &source, &mut target), 2);
        assert_eq!(target[8..], [0xee, 0xee]);
        let mut target = [0xee; 5];
        assert_eq!(store(RGB565, &source, &mut target), 2);
        assert_eq!(target[4], 0xee);
        assert_eq!(store(RGB565, &source[..7], &mut [0; 16]), 1);
        assert_eq!(store(RGB565, &source, &mut []), 0);
    }

    #[test]
    fn sixteen_bits_keep_the_top_of_each_colour() {
        let source = row(&[0xff_0000, 0x00_ff00, 0x00_00ff, 0xff_ffff, 0x84_8284]);
        let mut target = [0; 10];
        assert_eq!(store(RGB565, &source, &mut target), 5);
        let pixels: Vec<u16> =
            target.chunks(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        assert_eq!(pixels, [0xf800, 0x07e0, 0x001f, 0xffff, 0x8410]);
    }

    #[test]
    fn three_bytes_with_red_first_are_written_in_that_order() {
        let source = row(&[0x12_3456, 0xab_cdef]);
        let mut target = [0; 6];
        assert_eq!(store(BGR888, &source, &mut target), 2);
        assert_eq!(target, [0x12, 0x34, 0x56, 0xab, 0xcd, 0xef]);
    }

    #[test]
    fn ten_bits_a_colour_keep_white_white_and_black_black() {
        let source = row(&[0xff_ffff, 0x00_0000, 0x80_0000]);
        let mut target = [0; 12];
        assert_eq!(store(XRGB2101010, &source, &mut target), 3);
        let pixel = |at: usize| u32::from_le_bytes(target[at * 4..][..4].try_into().unwrap());
        assert_eq!(pixel(0), 0x3fff_ffff);
        assert_eq!(pixel(1), 0);
        // 0x80 of 255 is 0x202 of 1023.
        assert_eq!(pixel(2), 0x202 << 20);
    }

    #[test]
    fn a_layout_that_cannot_be_written_is_known() {
        assert!(Layout::XRGB8888.is_usable());
        assert!(RGB565.is_usable() && BGR888.is_usable() && XRGB2101010.is_usable());
        assert!(!Layout { bytes: 1, ..RGB565 }.is_usable());
        assert!(!Layout { bytes: 5, ..Layout::XRGB8888 }.is_usable());
        // Red past the end of a 16-bit pixel.
        assert!(!Layout { red: Channel { shift: 12, bits: 5 }, ..RGB565 }.is_usable());
        assert!(!Layout { green: Channel { shift: 8, bits: 0 }, ..Layout::XRGB8888 }.is_usable());
        // And storing in one writes nothing where it cannot, and never
        // panics.
        let source = row(&[0xff_ffff]);
        assert_eq!(store(Layout { bytes: 9, ..RGB565 }, &source, &mut [0; 16]), 0);
        let odd = Layout { red: Channel { shift: 200, bits: 77 }, ..Layout::XRGB8888 };
        let mut target = [0; 4];
        assert_eq!(store(odd, &source, &mut target), 1);
        assert_eq!(target, [0xff, 0xff, 0, 0]);
    }

    #[test]
    fn dividing_by_255_is_exact_to_the_nearest() {
        for value in 0..=255 * 255 {
            assert_eq!(over_255(value), (value + 127) / 255, "{value}");
        }
    }

    #[test]
    fn a_solid_pointer_covers_and_a_clear_one_shows_the_frame() {
        let mut frame = row(&[0x10_2030, 0x10_2030, 0x10_2030]);
        // Solid white, wholly clear, and half-solid red.
        let pointer = [0xffff_ffffu32, 0x0000_0000, 0x8080_0000];
        let pointer: Vec<u8> = pointer.iter().flat_map(|pixel| pixel.to_le_bytes()).collect();
        blend(&mut frame, &pointer);
        let pixels: Vec<u32> = frame
            .chunks(4)
            .map(|pixel| u32::from_le_bytes(pixel.try_into().unwrap()) & 0xff_ffff)
            .collect();
        // Half of 0x10, 0x20, 0x30 is 0x08, 0x10, 0x18, and red gets
        // 0x80 more.
        assert_eq!(pixels, [0xff_ffff, 0x10_2030, 0x88_1018]);
    }

    #[test]
    fn a_blend_never_wraps_and_leaves_the_fourth_byte() {
        // A pointer whose colour is more than its solidness allows, on a
        // white frame.
        let mut frame = [0xff, 0xff, 0xff, 0x5a];
        blend(&mut frame, &[0xf0, 0xf0, 0xf0, 0x10]);
        assert_eq!(frame, [0xff, 0xff, 0xff, 0x5a]);
        // A pointer narrower than the row touches only its own pixels.
        let mut frame = row(&[0x11_1111, 0x22_2222]);
        blend(&mut frame, &[0, 0, 0xff, 0xff]);
        assert_eq!(frame, row(&[0xff_0000, 0x22_2222]));
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Whatever layout the firmware reports and however long the row and
    /// the device's memory are, storing a row writes whole pixels inside
    /// both and never panics.
    #[kani::proof]
    #[kani::unwind(4)]
    fn a_row_is_stored_within_both_slices() {
        let layout = Layout {
            red: Channel { shift: kani::any(), bits: kani::any() },
            green: Channel { shift: kani::any(), bits: kani::any() },
            blue: Channel { shift: kani::any(), bits: kani::any() },
            bytes: kani::any(),
        };
        let row: [u8; 9] = kani::any();
        let mut target = [0u8; 9];
        let row_len: usize = kani::any_where(|&len: &usize| len <= 9);
        let target_len: usize = kani::any_where(|&len: &usize| len <= 9);
        let pixels = store(layout, &row[..row_len], &mut target[..target_len]);
        assert!(pixels * 4 <= row_len);
        assert!(pixels * usize::from(layout.bytes) <= target_len || pixels == 0);
    }

    /// A pointer's pixel whose colours are no more than its solidness
    /// allows, as multiplied colours are, blends without the clamp: the
    /// frame's colour is dimmed to no more than what the pointer leaves
    /// clear.
    #[kani::proof]
    fn a_true_pointer_pixel_needs_no_clamp() {
        let (under, added, solid): (u8, u8, u8) = (kani::any(), kani::any(), kani::any());
        kani::assume(added <= solid);
        let dimmed = over_255(u32::from(under) * (255 - u32::from(solid)));
        assert!(dimmed <= u32::from(under));
        assert!(u32::from(added) + dimmed <= 255);
    }
}
