// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reading the kernel's framebuffer console from a QEMU screenshot.
//!
//! The reference laptop has no serial port, so what the kernel draws on the
//! screen is a feature the boot tests check, not a side effect. A screenshot
//! (PPM, from QMP's `screendump`) is cut into the console's character cells,
//! and each cell is matched against the glyphs of the same built-in font the
//! kernel used, which gives back the text exactly; no OCR is involved. The
//! kernel logs the console's geometry on the serial port, which says where
//! the cells are.

use std::collections::HashMap;

use radix_fbcon::Font;

/// A decoded screenshot.
pub struct Image {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// RGB triples, row by row.
    pixels: Vec<[u8; 3]>,
}

impl Image {
    /// Parses a binary PPM (P6) file with 8-bit samples.
    pub fn parse_ppm(bytes: &[u8]) -> Result<Image, String> {
        let mut fields = Vec::new();
        let mut at = 0;
        // Magic, width, height and maximum value, separated by whitespace
        // and comments, then one whitespace byte before the samples.
        while fields.len() < 4 {
            while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'#') {
                if bytes[at] == b'#' {
                    while at < bytes.len() && bytes[at] != b'\n' {
                        at += 1;
                    }
                } else {
                    at += 1;
                }
            }
            let start = at;
            while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if start == at {
                return Err("truncated PPM header".into());
            }
            fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
        }
        at += 1;
        if fields[0] != "P6" || fields[3] != "255" {
            return Err(format!("not an 8-bit binary PPM ({} {})", fields[0], fields[3]));
        }
        let number = |s: &str| s.parse::<usize>().map_err(|_| format!("bad PPM size `{s}`"));
        let (width, height) = (number(&fields[1])?, number(&fields[2])?);
        let data = bytes.get(at..at + width * height * 3).ok_or("truncated PPM data")?;
        let pixels = data.as_chunks::<3>().0.to_vec();
        Ok(Image { width, height, pixels })
    }

    /// The pixel at (`x`, `y`), as RGB.
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        self.pixels[y * self.width + x]
    }
}

/// Where the console's cells are, as the kernel logged it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Columns and rows.
    pub cols: usize,
    /// Rows.
    pub rows: usize,
    /// Font name, such as `8x16`.
    pub font: (usize, usize),
    /// How many times its size the font is drawn: 2 for consoled's
    /// biggest text sizes (ADR 0325), where each pixel is two by two.
    pub scale: usize,
    /// Pixel position of the top-left cell.
    pub origin: (usize, usize),
}

impl Geometry {
    /// Finds the kernel's line
    /// `Framebuffer: console of 160x50 cells with font 8x16 at (0, 0)`,
    /// or consoled's `consoled: console of ...` once it has taken the
    /// screen over, with `font 12x24 doubled` where it draws the font at
    /// twice its size: the last of them. A line the serial port has not
    /// yet ended is not read, as it may be only part of one.
    pub fn from_log(log: &str) -> Option<Geometry> {
        let ended = log.rfind('\n').map_or("", |end| &log[..end]);
        let line = ended.lines().rev().find_map(|l| {
            ["Framebuffer: console of ", "consoled: console of "]
                .iter()
                .find_map(|prefix| l.split(prefix).nth(1))
        })?;
        let (grid, rest) = line.split_once(" cells with font ")?;
        let (font, rest) = rest.split_once(" at (")?;
        let (x, y) = rest.trim_end_matches(')').split_once(", ")?;
        let pair = |s: &str| -> Option<(usize, usize)> {
            let (a, b) = s.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        };
        let (cols, rows) = pair(grid)?;
        let (font, scale) = match font.strip_suffix(" doubled") {
            Some(font) => (font, 2),
            None => (font, 1),
        };
        Some(Geometry {
            cols,
            rows,
            font: pair(font)?,
            scale,
            origin: (x.trim().parse().ok()?, y.trim().parse().ok()?),
        })
    }
}

/// Reads the console's text from a screenshot: one string per row, with
/// trailing blanks removed. A cell that matches no glyph reads as `?`.
pub fn read_text(image: &Image, geometry: Geometry) -> Result<Vec<String>, String> {
    let name = format!("{}x{}", geometry.font.0, geometry.font.1);
    let font = radix_fbcon::builtin_font(&name).ok_or(format!("no built-in font {name}"))?;
    let scale = geometry.scale.max(1);
    let (fw, fh) = (geometry.font.0 * scale, geometry.font.1 * scale);
    let (ox, oy) = geometry.origin;
    if ox + geometry.cols * fw > image.width || oy + geometry.rows * fh > image.height {
        return Err(format!(
            "the console ({}x{} cells of {name} at {ox},{oy}) does not fit a {}x{} screenshot",
            geometry.cols, geometry.rows, image.width, image.height
        ));
    }
    let glyphs = glyph_table(&font);
    let mut rows = Vec::with_capacity(geometry.rows);
    for row in 0..geometry.rows {
        let mut text = String::with_capacity(geometry.cols);
        for col in 0..geometry.cols {
            let bits = cell_bits(image, ox + col * fw, oy + row * fh, geometry.font, scale);
            text.push(*glyphs.get(&bits).unwrap_or(&'?'));
        }
        rows.push(text.trim_end().to_string());
    }
    Ok(rows)
}

/// The lit pixels of every printable ASCII glyph, row by row.
fn glyph_table(font: &Font<'_>) -> HashMap<Vec<u32>, char> {
    let mut table = HashMap::new();
    for c in (' '..='~').rev() {
        let glyph = font.glyph(c);
        let bits: Vec<u32> = (0..font.height).map(|y| glyph.row_bits(y)).collect();
        table.insert(bits, c);
    }
    table
}

/// The pixels of one cell of a font `width` by `height` drawn `scale`
/// times its size that differ from its background, which is the colour
/// most of its pixels have (glyphs light up far fewer than half): one for
/// each of the font's pixels, the top left of its `scale` by `scale`.
fn cell_bits(
    image: &Image,
    x0: usize,
    y0: usize,
    (width, height): (usize, usize),
    scale: usize,
) -> Vec<u32> {
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    for y in 0..height {
        for x in 0..width {
            *counts.entry(image.pixel(x0 + x * scale, y0 + y * scale)).or_default() += 1;
        }
    }
    let background = counts.into_iter().max_by_key(|&(_, n)| n).map_or_default(|(c, _)| c);
    (0..height)
        .map(|y| {
            (0..width).fold(0u32, |bits, gx| {
                (bits << 1) | u32::from(image.pixel(x0 + gx * scale, y0 + y * scale) != background)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws `lines` with the 8x16 font, white on blue, into a PPM.
    fn render(lines: &[&str], width: usize, height: usize) -> Vec<u8> {
        render_scaled(lines, width, height, 1)
    }

    /// Draws `lines` with the 8x16 font at `scale` times its size, from
    /// (2, 4), white on blue, into a PPM.
    fn render_scaled(lines: &[&str], width: usize, height: usize, scale: usize) -> Vec<u8> {
        let font = radix_fbcon::builtin_font("8x16").unwrap();
        let mut pixels = vec![[0u8, 0, 128]; width * height];
        for (row, line) in lines.iter().enumerate() {
            for (col, c) in line.chars().enumerate() {
                let glyph = font.glyph(c);
                for y in 0..16 * scale {
                    for x in 0..8 * scale {
                        if glyph.pixel(x / scale, y / scale) {
                            let at = (4 + row * 16 * scale + y) * width + 2 + col * 8 * scale + x;
                            pixels[at] = [255, 255, 255];
                        }
                    }
                }
            }
        }
        let mut out = format!("P6\n# test\n{width} {height}\n255\n").into_bytes();
        out.extend(pixels.iter().flatten());
        out
    }

    #[test]
    fn reads_back_drawn_text() {
        let ppm =
            render(&["Radix panic: oops", "  #0  0xffffffff80100000  radix::main+0x10"], 400, 40);
        let image = Image::parse_ppm(&ppm).unwrap();
        let geometry = Geometry { cols: 49, rows: 2, font: (8, 16), scale: 1, origin: (2, 4) };
        let rows = read_text(&image, geometry).unwrap();
        assert_eq!(rows, ["Radix panic: oops", "  #0  0xffffffff80100000  radix::main+0x10"]);
        // Drawn twice its size, as consoled's biggest text sizes are.
        let ppm = render_scaled(&["Text larger", "# stty size"], 400, 80, 2);
        let image = Image::parse_ppm(&ppm).unwrap();
        let geometry = Geometry { cols: 24, rows: 2, font: (8, 16), scale: 2, origin: (2, 4) };
        assert_eq!(read_text(&image, geometry).unwrap(), ["Text larger", "# stty size"]);
    }

    #[test]
    fn finds_the_geometry_in_the_log() {
        let log = "[    0.1 cpu0] Framebuffer: console of 160x50 cells with font 8x16 at (0, 0)\n";
        let geometry = Geometry::from_log(log).unwrap();
        assert_eq!(
            geometry,
            Geometry { cols: 160, rows: 50, font: (8, 16), scale: 1, origin: (0, 0) }
        );
        assert!(Geometry::from_log("nothing here").is_none());
        // consoled's, once it has the screen, wins.
        let log = format!(
            "{log}[    2.5 cpu1] consoled: console of 128x48 cells with font 8x16 at (0, 0)\n"
        );
        let geometry = Geometry::from_log(&log).unwrap();
        assert_eq!((geometry.cols, geometry.rows), (128, 48));
        // A line the serial port is part way through is not read.
        let part = format!("{log}[    3.1 cpu1] consoled: console of 160x50 cells with font 8x1");
        let geometry = Geometry::from_log(&part).unwrap();
        assert_eq!((geometry.cols, geometry.rows), (128, 48));
        // A font consoled draws twice its size.
        let log = format!(
            "{log}[    9.0 cpu1] consoled: console of 80x22 cells with font 12x24 doubled at (0, 12)\n"
        );
        assert_eq!(
            Geometry::from_log(&log).unwrap(),
            Geometry { cols: 80, rows: 22, font: (12, 24), scale: 2, origin: (0, 12) }
        );
    }

    #[test]
    fn rejects_other_images() {
        assert!(Image::parse_ppm(b"P5\n1 1\n255\n\0").is_err());
        assert!(Image::parse_ppm(b"P6\n2 2\n255\n\0\0\0").is_err());
    }
}
