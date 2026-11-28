// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pixels a frame is drawn in.
//!
//! A [`Canvas`] is a rectangle of the frame's plane and the memory of
//! its pixels, which its owner gives: windowd draws straight into the
//! buffers it hands the renderer. A pixel is four bytes, blue, green,
//! red and cover, with the colours already multiplied by the cover, as
//! pixman and the display layer's `FORMAT_ARGB8888` have them.
//!
//! Everything drawn is cut to the canvas, so the frame is drawn the
//! same way into each of its strips and only what lies in a strip
//! costs anything. A fill writes whole runs of a row; only the pixels
//! of a round corner and of a stroke are reckoned one by one, by how
//! much of each the shape covers.

use radix_fbcon::Font;

/// The bytes of a pixel.
pub const PIXEL: usize = 4;

/// How many times across and down a pixel is sampled to find how much
/// of it a round corner or a stroke covers.
const SAMPLES: i32 = 4;

/// A colour, and how much of what is under it it covers: 255 is all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colour {
    /// Red.
    pub red: u8,
    /// Green.
    pub green: u8,
    /// Blue.
    pub blue: u8,
    /// How much it covers.
    pub cover: u8,
}

impl Colour {
    /// White.
    pub const WHITE: Colour = Colour::rgb(0xff_ffff);

    /// Black.
    pub const BLACK: Colour = Colour::rgb(0);

    /// The colour a web page writes as `#RRGGBB`, which covers all.
    pub const fn rgb(value: u32) -> Colour {
        Colour {
            red: (value >> 16) as u8,
            green: (value >> 8) as u8,
            blue: value as u8,
            cover: 255,
        }
    }

    /// The colour covering `cover` of 255.
    #[must_use]
    pub const fn covering(self, cover: u8) -> Colour {
        Colour { cover, ..self }
    }

    /// The colour `share` of 255 of the way from this one to `other`.
    #[must_use]
    pub fn towards(self, other: Colour, share: u8) -> Colour {
        let part = |from: u8, to: u8| {
            let (from, to, share) = (u32::from(from), u32::from(to), u32::from(share));
            ((from * (255 - share) + to * share + 127) / 255) as u8
        };
        Colour {
            red: part(self.red, other.red),
            green: part(self.green, other.green),
            blue: part(self.blue, other.blue),
            cover: part(self.cover, other.cover),
        }
    }
}

/// A rectangle of the frame's plane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Its left column.
    pub x: i32,
    /// Its top row.
    pub y: i32,
    /// Its width.
    pub w: i32,
    /// Its height.
    pub h: i32,
}

impl Rect {
    /// The rectangle of `w` by `h` whose top left corner is (`x`, `y`).
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The column after its last.
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    /// The row after its last.
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Whether it holds no pixel.
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Whether the pixel at (`x`, `y`) is in it.
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// What it shares with `other`: empty if nothing.
    #[must_use]
    pub fn shared(&self, other: &Rect) -> Rect {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let w = self.right().min(other.right()) - x;
        let h = self.bottom().min(other.bottom()) - y;
        Rect { x, y, w: w.max(0), h: h.max(0) }
    }

    /// The rectangle `by` larger on every side, or smaller for a
    /// negative `by`.
    #[must_use]
    pub const fn grown(&self, by: i32) -> Rect {
        Rect { x: self.x - by, y: self.y - by, w: self.w + 2 * by, h: self.h + 2 * by }
    }

    /// The rectangle moved (`dx`, `dy`) right and down.
    #[must_use]
    pub const fn moved(&self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
}

/// The radius of each corner of a box: top left, top right, bottom
/// right, bottom left. 0 is a square corner.
pub type Radii = [i32; 4];

/// A box whose every corner has the radius `radius`.
pub const fn all_round(radius: i32) -> Radii {
    [radius; 4]
}

/// A straight stroke from one point to another, in pixels and parts of
/// pixels: a pixel's middle is half a pixel in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Where it starts.
    pub from: (f32, f32),
    /// Where it ends.
    pub to: (f32, f32),
}

impl Line {
    /// Whether the point (`x`, `y`) is in the stroke of `width` along
    /// the line, which ends square at both ends.
    fn covers(&self, width: f32, x: f32, y: f32) -> bool {
        let along = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        let length = along.0.hypot(along.1);
        if length == 0.0 {
            return false;
        }
        let to_point = (x - self.from.0, y - self.from.1);
        let forward = (to_point.0 * along.0 + to_point.1 * along.1) / length;
        let aside = (to_point.0 * along.1 - to_point.1 * along.0) / length;
        (0.0..=length).contains(&forward) && aside.abs() <= width / 2.0
    }
}

/// How much of the pixel `across` columns and `down` rows in from the
/// corner of a box a corner of `radius` covers, of 255.
fn corner_cover(radius: i32, across: i32, down: i32) -> u8 {
    let radius_units = radius * 2 * SAMPLES;
    let mut inside = 0;
    for sample_down in 0..SAMPLES {
        for sample_across in 0..SAMPLES {
            // In half samples, so that a sample's middle is a whole
            // number.
            let from_centre_x = radius_units - (across * 2 * SAMPLES + 2 * sample_across + 1);
            let from_centre_y = radius_units - (down * 2 * SAMPLES + 2 * sample_down + 1);
            let far = from_centre_x * from_centre_x + from_centre_y * from_centre_y;
            inside += i32::from(far <= radius_units * radius_units);
        }
    }
    (inside * 255 / (SAMPLES * SAMPLES)) as u8
}

/// Puts `colour`, covering `cover` of 255 of the pixel, over the pixel
/// `pixel`.
fn blend(pixel: &mut [u8], colour: Colour, cover: u8) {
    const WHOLE: u32 = 255 * 255;
    let over = u32::from(colour.cover) * u32::from(cover);
    if over == 0 {
        return;
    }
    if over == WHOLE {
        pixel.copy_from_slice(&[colour.blue, colour.green, colour.red, 255]);
        return;
    }
    let under = WHOLE - over;
    let mixed = |new: u8, old: u8| {
        ((u32::from(new) * over + u32::from(old) * under + WHOLE / 2) / WHOLE) as u8
    };
    pixel[0] = mixed(colour.blue, pixel[0]);
    pixel[1] = mixed(colour.green, pixel[1]);
    pixel[2] = mixed(colour.red, pixel[2]);
    pixel[3] = mixed(255, pixel[3]);
}

/// What a pixel of a canvas shows as over a pixel that covers all,
/// whose red, green and blue are `under`: as the renderer reckons it
/// (pixman's `OVER`), so that a test can say what a screen shows to
/// the last bit.
pub fn shown_over(pixel: [u8; PIXEL], under: [u8; 3]) -> [u8; 3] {
    let [blue, green, red, cover] = pixel;
    let through = 255 - u16::from(cover);
    let over = |own: u8, under: u8| {
        // The share of what is under, to the nearest 255th.
        let share = u16::from(under) * through + 128;
        own.saturating_add(((share + (share >> 8)) >> 8) as u8)
    };
    [over(red, under[0]), over(green, under[1]), over(blue, under[2])]
}

/// A rectangle of the frame's plane, and its pixels.
#[derive(Debug)]
pub struct Canvas<'a> {
    rect: Rect,
    pixels: &'a mut [u8],
}

impl<'a> Canvas<'a> {
    /// The canvas of `rect` whose pixels are `pixels`, row after row
    /// with nothing between them. What the pixels hold stays.
    ///
    /// # Panics
    ///
    /// If `pixels` is not the size of `rect`'s pixels.
    pub fn over(rect: Rect, pixels: &'a mut [u8]) -> Canvas<'a> {
        assert_eq!(pixels.len(), Canvas::bytes_of(&rect), "a canvas of {rect:?}");
        Canvas { rect, pixels }
    }

    /// How many bytes the pixels of `rect` are.
    pub fn bytes_of(rect: &Rect) -> usize {
        rect.w.max(0) as usize * rect.h.max(0) as usize * PIXEL
    }

    /// The canvas's rectangle.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Makes every pixel see-through.
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// The pixels of row `y` from column `from` up to `to`, which are
    /// in the canvas.
    fn run(&mut self, y: i32, from: i32, to: i32) -> &mut [u8] {
        let row = (y - self.rect.y) as usize * self.rect.w as usize;
        let start = (row + (from - self.rect.x) as usize) * PIXEL;
        let end = (row + (to - self.rect.x) as usize) * PIXEL;
        &mut self.pixels[start..end]
    }

    /// Puts `colour` over the pixels of row `y` from column `from` up
    /// to `to`, where they are in the canvas.
    fn span(&mut self, y: i32, from: i32, to: i32, colour: Colour) {
        let (from, to) = (from.max(self.rect.x), to.min(self.rect.right()));
        if from >= to || y < self.rect.y || y >= self.rect.bottom() {
            return;
        }
        for pixel in self.run(y, from, to).as_chunks_mut::<PIXEL>().0 {
            blend(pixel, colour, 255);
        }
    }

    /// Puts `colour`, covering `cover` of 255 of the pixel, over the
    /// pixel at (`x`, `y`), if it is in the canvas.
    fn put(&mut self, x: i32, y: i32, colour: Colour, cover: u8) {
        if self.rect.contains(x, y) {
            blend(self.run(y, x, x + 1), colour, cover);
        }
    }

    /// Makes the pixels of `rect` see-through.
    pub fn open(&mut self, rect: Rect) {
        let open = rect.shared(&self.rect);
        for y in open.y..open.bottom() {
            self.run(y, open.x, open.right()).fill(0);
        }
    }

    /// Fills `rect` with `colour`.
    pub fn fill(&mut self, rect: Rect, colour: Colour) {
        self.fill_rows(rect, |_| colour);
    }

    /// Fills `rect`, each row with the colour `colour_of` gives for it.
    pub fn fill_rows(&mut self, rect: Rect, colour_of: impl Fn(i32) -> Colour) {
        self.round(rect, all_round(0), colour_of);
    }

    /// Fills the box `rect` whose corners have the radii `radii`, each
    /// row with the colour `colour_of` gives for it.
    pub fn round(&mut self, rect: Rect, radii: Radii, colour_of: impl Fn(i32) -> Colour) {
        let rows = rect.shared(&self.rect);
        let [top_left, top_right, bottom_right, bottom_left] = radii;
        for y in rows.y..rows.bottom() {
            let colour = colour_of(y);
            let (from_top, from_bottom) = (y - rect.y, rect.bottom() - 1 - y);
            // The corner this row crosses at each end, and how far in
            // from the corner's own edge the row is.
            let corner = |above: i32, below: i32| {
                if from_top < above {
                    (above, from_top)
                } else if from_bottom < below {
                    (below, from_bottom)
                } else {
                    (0, 0)
                }
            };
            let (left, left_down) = corner(top_left, bottom_left);
            let (right, right_down) = corner(top_right, bottom_right);
            for across in 0..left {
                self.put(rect.x + across, y, colour, corner_cover(left, across, left_down));
            }
            for across in 0..right {
                let cover = corner_cover(right, across, right_down);
                self.put(rect.right() - 1 - across, y, colour, cover);
            }
            self.span(y, rect.x + left, rect.right() - right, colour);
        }
    }

    /// Draws `lines` as strokes of `width` in `colour`, as one shape:
    /// where two strokes cross, the colour is put once.
    pub fn stroke(&mut self, lines: &[Line], width: f32, colour: Colour) {
        let reach = width / 2.0;
        let ends = lines.iter().flat_map(|line| [line.from, line.to]);
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in ends {
            (left, top) = (left.min(x - reach), top.min(y - reach));
            (right, bottom) = (right.max(x + reach), bottom.max(y + reach));
        }
        if left > right {
            return;
        }
        let around = Rect::new(
            left.floor() as i32,
            top.floor() as i32,
            (right.ceil() - left.floor()) as i32,
            (bottom.ceil() - top.floor()) as i32,
        );
        let drawn = around.shared(&self.rect);
        let step = 1.0 / SAMPLES as f32;
        for y in drawn.y..drawn.bottom() {
            for x in drawn.x..drawn.right() {
                let mut inside = 0;
                for sample in 0..SAMPLES * SAMPLES {
                    let at_x = x as f32 + ((sample % SAMPLES) as f32 + 0.5) * step;
                    let at_y = y as f32 + ((sample / SAMPLES) as f32 + 0.5) * step;
                    inside += i32::from(lines.iter().any(|line| line.covers(width, at_x, at_y)));
                }
                self.put(x, y, colour, (inside * 255 / (SAMPLES * SAMPLES)) as u8);
            }
        }
    }

    /// Writes `text` in `font` and `colour`, its first letter's top
    /// left corner at `at`.
    pub fn text(&mut self, font: &Font<'_>, at: (i32, i32), text: &str, colour: Colour) {
        let (width, height) = (font.width as i32, font.height as i32);
        let mut left = at.0;
        for letter in text.chars() {
            let cell = Rect::new(left, at.1, width, height);
            if !cell.shared(&self.rect).is_empty() {
                let glyph = font.glyph(letter);
                for down in 0..height {
                    let bits = glyph.row_bits(down as usize);
                    for across in (0..width).filter(|across| bits >> (width - 1 - across) & 1 != 0)
                    {
                        self.put(left + across, at.1 + down, colour, 255);
                    }
                }
            }
            left += width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canvas of `rect` on memory of its own, drawn on by `draw`: its
    /// pixels.
    fn drawn(rect: Rect, draw: impl FnOnce(&mut Canvas<'_>)) -> Vec<u8> {
        let mut pixels = vec![0; Canvas::bytes_of(&rect)];
        draw(&mut Canvas::over(rect, &mut pixels));
        pixels
    }

    /// The pixel at (`x`, `y`) of the pixels of `rect`.
    fn at(pixels: &[u8], rect: Rect, x: i32, y: i32) -> [u8; 4] {
        let index = ((y - rect.y) * rect.w + (x - rect.x)) as usize * PIXEL;
        pixels[index..index + PIXEL].try_into().unwrap()
    }

    #[test]
    fn a_pixel_shows_over_another_by_how_much_it_covers() {
        // One that covers all hides what is under, one that covers
        // nothing shows it.
        assert_eq!(shown_over([3, 2, 1, 255], [9, 9, 9]), [1, 2, 3]);
        assert_eq!(shown_over([0, 0, 0, 0], [7, 8, 9]), [7, 8, 9]);
        // Black that covers 35 of 100 leaves 65 of what is under, to
        // the nearest: 166 of 255 of 0x1e is 19.53.
        assert_eq!(shown_over([0, 0, 0, 89], [0x12, 0x16, 0x1e]), [12, 14, 20]);
        // The colours of a pixel that covers a part are less by that
        // part already.
        assert_eq!(shown_over([100, 50, 25, 128], [255, 255, 255]), [152, 177, 227]);
    }

    #[test]
    fn a_colour_is_its_web_number_and_goes_towards_another_by_shares() {
        let colour = Colour::rgb(0x3e_5c7e);
        assert_eq!((colour.red, colour.green, colour.blue, colour.cover), (0x3e, 0x5c, 0x7e, 255));
        assert_eq!(Colour::BLACK.towards(Colour::WHITE, 0), Colour::BLACK);
        assert_eq!(Colour::BLACK.towards(Colour::WHITE, 255), Colour::WHITE);
        assert_eq!(Colour::rgb(0x10_2030).towards(Colour::rgb(0x30_2010), 128).red, 0x20);
        assert_eq!(Colour::WHITE.towards(Colour::BLACK, 51), Colour::rgb(0xcc_cccc));
    }

    #[test]
    fn rectangles_share_what_both_hold() {
        let one = Rect::new(0, 0, 10, 10);
        assert_eq!(one.shared(&Rect::new(5, -5, 10, 10)), Rect::new(5, 0, 5, 5));
        assert!(one.shared(&Rect::new(10, 0, 5, 5)).is_empty());
        assert_eq!(one.grown(1), Rect::new(-1, -1, 12, 12));
        assert_eq!(one.moved(3, -2), Rect::new(3, -2, 10, 10));
        assert!(one.contains(9, 9) && !one.contains(10, 9) && !one.contains(-1, 0));
    }

    #[test]
    fn a_fill_is_cut_to_the_canvas_and_leaves_the_rest() {
        let rect = Rect::new(-2, -2, 4, 4);
        let pixels =
            drawn(rect, |canvas| canvas.fill(Rect::new(0, 0, 100, 1), Colour::rgb(0x01_0203)));
        assert_eq!(at(&pixels, rect, 0, 0), [3, 2, 1, 255]);
        assert_eq!(at(&pixels, rect, 1, 0), [3, 2, 1, 255]);
        assert_eq!(at(&pixels, rect, -1, 0), [0; 4]);
        assert_eq!(at(&pixels, rect, 0, 1), [0; 4]);
    }

    #[test]
    fn a_colour_that_covers_part_lets_the_rest_show() {
        let rect = Rect::new(0, 0, 1, 1);
        let pixels = drawn(rect, |canvas| {
            canvas.fill(rect, Colour::WHITE);
            canvas.fill(rect, Colour::BLACK.covering(51));
        });
        assert_eq!(pixels, [204, 204, 204, 255]);
        // On nothing, the pixel holds the colour times its cover.
        let pixels = drawn(rect, |canvas| canvas.fill(rect, Colour::WHITE.covering(51)));
        assert_eq!(pixels, [51, 51, 51, 51]);
    }

    #[test]
    fn opening_a_rectangle_takes_what_was_drawn_there() {
        let rect = Rect::new(0, 0, 3, 3);
        let pixels = drawn(rect, |canvas| {
            canvas.fill(rect, Colour::WHITE);
            canvas.open(Rect::new(1, 1, 5, 1));
        });
        assert_eq!(at(&pixels, rect, 0, 1), [255; 4]);
        assert_eq!(at(&pixels, rect, 1, 1), [0; 4]);
        assert_eq!(at(&pixels, rect, 2, 1), [0; 4]);
        assert_eq!(at(&pixels, rect, 1, 2), [255; 4]);
    }

    #[test]
    fn each_row_of_a_fill_has_its_own_colour() {
        let rect = Rect::new(0, 0, 2, 3);
        let pixels =
            drawn(rect, |canvas| canvas.fill_rows(rect, |y| Colour::rgb(y as u32 * 0x10_0000)));
        assert_eq!(at(&pixels, rect, 1, 0), [0, 0, 0, 255]);
        assert_eq!(at(&pixels, rect, 1, 2), [0, 0, 0x20, 255]);
    }

    #[test]
    fn a_round_corner_leaves_its_tip_open_and_fills_its_inside() {
        let rect = Rect::new(0, 0, 32, 32);
        let pixels = drawn(rect, |canvas| canvas.round(rect, all_round(8), |_| Colour::WHITE));
        for (x, y) in [(0, 0), (31, 0), (0, 31), (31, 31)] {
            assert_eq!(at(&pixels, rect, x, y), [0; 4], "the tip at ({x}, {y})");
        }
        for (x, y) in [(8, 0), (0, 8), (23, 31), (31, 23), (16, 16), (7, 7), (24, 24)] {
            assert_eq!(at(&pixels, rect, x, y), [255; 4], "inside at ({x}, {y})");
        }
        // The edge of the arc covers part of a pixel, alike at all four.
        let edge = at(&pixels, rect, 2, 2);
        assert!(edge[3] > 0 && edge[3] < 255, "{edge:?}");
        assert_eq!(edge, at(&pixels, rect, 29, 2));
        assert_eq!(edge, at(&pixels, rect, 2, 29));
        assert_eq!(edge, at(&pixels, rect, 29, 29));
    }

    #[test]
    fn only_the_corners_named_are_round() {
        let rect = Rect::new(0, 0, 16, 16);
        let pixels = drawn(rect, |canvas| canvas.round(rect, [0, 0, 5, 0], |_| Colour::WHITE));
        assert_eq!(at(&pixels, rect, 0, 0), [255; 4]);
        assert_eq!(at(&pixels, rect, 15, 0), [255; 4]);
        assert_eq!(at(&pixels, rect, 0, 15), [255; 4]);
        assert_eq!(at(&pixels, rect, 15, 15), [0; 4]);
    }

    #[test]
    fn a_stroke_is_as_wide_as_asked_and_ends_square() {
        let rect = Rect::new(0, 0, 12, 6);
        let line = Line { from: (2.0, 3.0), to: (10.0, 3.0) };
        let pixels = drawn(rect, |canvas| canvas.stroke(&[line], 2.0, Colour::WHITE));
        for x in 2..10 {
            assert_eq!(at(&pixels, rect, x, 2), [255; 4]);
            assert_eq!(at(&pixels, rect, x, 3), [255; 4]);
            assert_eq!(at(&pixels, rect, x, 1), [0; 4]);
            assert_eq!(at(&pixels, rect, x, 4), [0; 4]);
        }
        assert_eq!(at(&pixels, rect, 1, 3), [0; 4]);
        assert_eq!(at(&pixels, rect, 10, 3), [0; 4]);
    }

    #[test]
    fn strokes_that_cross_put_their_colour_once() {
        let rect = Rect::new(0, 0, 8, 8);
        let cross =
            [Line { from: (0.0, 4.0), to: (8.0, 4.0) }, Line { from: (4.0, 0.0), to: (4.0, 8.0) }];
        let half = Colour::WHITE.covering(128);
        let pixels = drawn(rect, |canvas| canvas.stroke(&cross, 2.0, half));
        assert_eq!(at(&pixels, rect, 4, 4), at(&pixels, rect, 1, 4));
    }

    #[test]
    fn text_is_the_fonts_bits_in_the_colour() {
        let font = radix_fbcon::builtin_font("8x16").unwrap();
        let rect = Rect::new(0, 0, 16, 16);
        let pixels = drawn(rect, |canvas| canvas.text(&font, (0, 0), "I ", Colour::rgb(0x00_00ff)));
        let lit = pixels.as_chunks::<PIXEL>().0.iter().filter(|pixel| pixel[3] != 0).count();
        let glyph = font.glyph('I');
        let bits: u32 = (0..16).map(|row| glyph.row_bits(row).count_ones()).sum();
        assert!(bits > 0);
        assert_eq!(lit as u32, bits);
        assert!(
            pixels
                .as_chunks::<PIXEL>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 0 || *pixel == [255, 0, 0, 255])
        );
    }
}
