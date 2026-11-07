// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick's boot menu in Dukovo's colours (ADR 0402): a GRUB
//! theme drawn as the approved design draws the menu, in the installer's
//! colours and the console's font, so that the stick looks the same from
//! the menu to the installer's last page.
//!
//! `ob image` writes it into `boot/grub/theme` of the stick and the ISO:
//!
//! - `theme.txt`, the layout: the name spaced out, the release under it,
//!   the entries in a box with the chosen one on tan, the keys' hints,
//!   how long until the chosen entry starts, and GRUB's own credit.
//! - `spleen-24.pf2` and `spleen-32.pf2`, the console's Spleen fonts in
//!   GRUB's font format, made here from the same PSF files the console
//!   draws with (the GRUB built for the images has no `grub-mkfont`).
//! - The box's and the chosen entry's pictures, PNG files made here.
//!
//! The menu is laid out in the cells of the 12x24 font, as the design's
//! is in a terminal's, and centred across the screen. The serial port
//! keeps GRUB's plain menu.

use std::fmt::Write as _;
use std::path::Path;

use radix_fbcon::{Font, builtin_font};

use crate::{Context, Result};

/// A cell of the menu's font, in pixels.
const CELL: (u32, u32) = (12, 24);

/// The menu box's width in cells, as the design has it.
const MENU_CELLS: u32 = 54;

/// The installer's colours (its `look.rs`), as GRUB reads them.
const SLATE: &str = "#15181c";
const CREAM: &str = "#d9d4ca";
const GREY: &str = "#7c8896";
const SAND: &str = "#e9c48b";
const TAN: [u8; 3] = [0xc8, 0x87, 0x3a];
const BLUE_GREY: [u8; 3] = [0x8f, 0xa6, 0xba];

/// The fonts written, from the console's: (file, built-in font, the name
/// GRUB knows it by).
const FONTS: [(&str, &str, &str); 2] = [
    ("spleen-24.pf2", "12x24", "Spleen Regular 24"),
    ("spleen-32.pf2", "16x32", "Spleen Regular 32"),
];

/// Writes the theme into `dir`, for a menu of `entries` entries on the
/// stick of `version`.
///
/// # Errors
///
/// If `dir` cannot be written.
pub fn write(dir: &Path, version: &str, entries: usize) -> Result<()> {
    crate::process::create_dir(dir)?;
    let put = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).context(|| format!("writing {}", path.display()))
    };
    put("theme.txt", theme(version, entries).as_bytes())?;
    for (file, builtin, name) in FONTS {
        let font = builtin_font(builtin).expect("the console's fonts are built in");
        put(file, &pf2(&font, name))?;
    }
    for (name, picture) in pictures() {
        put(&name, &picture.png())?;
    }
    Ok(())
}

/// `theme.txt`: where each part of the menu goes, in pixels from the
/// screen's top and its middle.
fn theme(version: &str, entries: usize) -> String {
    let (cw, ch) = CELL;
    let entries = u32::try_from(entries).unwrap_or(u32::MAX).max(1);
    // The design's rows, counted from a tenth of the way down: the name,
    // the release two rows under it, the box three under that, the box
    // its entries and two rows of border and gap each side high, then the
    // hints, the countdown, and GRUB's credit two rows from the bottom.
    let row = |n: u32| format!("10%+{}", n * ch);
    let menu_rows = entries + 4;
    let hints = 5 + menu_rows + 1;
    let mut text = String::new();
    let _ = write!(
        text,
        "\
# Dukovo's boot menu on the install stick, in the installer's colours
# (ADR 0402). `ob image` writes this file, and the fonts and pictures
# beside it (tools/xtask/src/grub_theme.rs).

title-text: \"\"
desktop-color: \"{SLATE}\"
terminal-font: \"{font}\"
terminal-left: \"0\"
terminal-top: \"0\"
terminal-width: \"100%\"
terminal-height: \"100%\"

+ label {{
    left = 0
    top = {name_top}
    width = 100%
    height = 32
    align = \"center\"
    font = \"{title_font}\"
    color = \"{SAND}\"
    text = \"D U K O V O\"
}}

+ label {{
    left = 0
    top = {release_top}
    width = 100%
    height = {ch}
    align = \"center\"
    font = \"{font}\"
    color = \"{GREY}\"
    text = \"Dukovo {version} · installer on USB\"
}}

+ boot_menu {{
    left = 50%-{half}
    top = {menu_top}
    width = {menu_width}
    height = {menu_height}
    item_font = \"{font}\"
    selected_item_font = \"{font}\"
    item_color = \"{CREAM}\"
    selected_item_color = \"{SLATE}\"
    item_height = {ch}
    item_padding = 0
    item_spacing = 0
    icon_width = 0
    icon_height = 0
    item_icon_space = 0
    menu_pixmap_style = \"menu_*.png\"
    selected_item_pixmap_style = \"select_*.png\"
    scrollbar = false
}}

+ label {{
    left = 0
    top = {hints_top}
    width = 100%
    height = {ch}
    align = \"center\"
    font = \"{font}\"
    color = \"{GREY}\"
    text = \"↑ and ↓ choose · Enter starts · e edits · c command line\"
}}

+ label {{
    id = \"__timeout__\"
    left = 0
    top = {timeout_top}
    width = 100%
    height = {ch}
    align = \"center\"
    font = \"{font}\"
    color = \"{SAND}\"
    text = \"The chosen entry starts in %d s\"
}}

+ label {{
    left = 0
    top = 100%-{credit}
    width = 100%
    height = {ch}
    align = \"center\"
    font = \"{font}\"
    color = \"{GREY}\"
    text = \"GRUB {grub} (GPL-3.0), themed in Dukovo colours\"
}}
",
        grub = crate::grub::VERSION,
        font = FONTS[0].2,
        title_font = FONTS[1].2,
        name_top = row(0),
        release_top = row(2),
        menu_top = row(5),
        half = MENU_CELLS * cw / 2,
        menu_width = MENU_CELLS * cw,
        menu_height = menu_rows * ch,
        hints_top = row(hints),
        timeout_top = row(hints + 1),
        credit = 2 * ch,
    );
    text
}

/// A picture: its width, height and pixels, red, green, blue and alpha.
struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

impl Picture {
    /// A picture of `width` by `height`, clear.
    fn new(width: u32, height: u32) -> Picture {
        Picture { width, height, pixels: vec![[0; 4]; (width * height) as usize] }
    }

    /// A picture of `width` by `height` filled with `rgb`.
    fn filled(width: u32, height: u32, [r, g, b]: [u8; 3]) -> Picture {
        Picture { width, height, pixels: vec![[r, g, b, 0xff]; (width * height) as usize] }
    }

    /// Paints the pixels from (`x0`, `y0`) to before (`x1`, `y1`) in `rgb`.
    fn fill(&mut self, (x0, y0): (u32, u32), (x1, y1): (u32, u32), [r, g, b]: [u8; 3]) {
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                self.pixels[(y * self.width + x) as usize] = [r, g, b, 0xff];
            }
        }
    }

    /// The picture as a PNG file: RGBA, eight bits a channel, its rows in
    /// zlib's stored blocks, uncompressed, which GRUB's reader reads.
    fn png(&self) -> Vec<u8> {
        let mut raw = Vec::new();
        for row in self.pixels.chunks(self.width as usize) {
            // Each row starts with its filter: none.
            raw.push(0);
            raw.extend(row.iter().flatten());
        }
        let mut header = Vec::new();
        header.extend(self.width.to_be_bytes());
        header.extend(self.height.to_be_bytes());
        // Eight bits a channel, RGBA, deflate, adaptive filters, no interlace.
        header.extend([8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut png, *b"IHDR", &header);
        chunk(&mut png, *b"IDAT", &zlib_stored(&raw));
        chunk(&mut png, *b"IEND", &[]);
        png
    }
}

/// The menu box's border, in the design's blue grey, and the chosen
/// entry's tan, as GRUB's nine-part boxes: each part's file and picture.
///
/// The box is two cells deep on each side: its line runs through the
/// middle of the outer cell, as the console's box-drawing characters
/// draw it, two pixels thick, and the inner cell is a gap. The chosen
/// entry is tan from the gap's edge, its text two cells in.
fn pictures() -> Vec<(String, Picture)> {
    let (cw, ch) = CELL;
    // The line, as Spleen 12x24's ─ and │: two pixels from the cell's
    // middle less one.
    let (lx, ly) = (cw / 2 - 1, ch / 2 - 1);
    let (side, deep) = (2 * cw, 2 * ch);
    let line = BLUE_GREY;
    let across = |width: u32, y: u32, from: u32, to: u32| {
        let mut p = Picture::new(width, deep);
        p.fill((from, y), (to, y + 2), line);
        p
    };
    let mut pictures = vec![
        ("menu_n.png".to_string(), across(1, ly, 0, 1)),
        ("menu_s.png".to_string(), across(1, ch + ly, 0, 1)),
    ];
    let mut west = Picture::new(side, 1);
    west.fill((lx, 0), (lx + 2, 1), line);
    let mut east = Picture::new(side, 1);
    east.fill((cw + lx, 0), (cw + lx + 2, 1), line);
    // The corners: each line from the corner's point to the picture's
    // edges toward the box's middle.
    let mut nw = across(side, ly, lx, side);
    nw.fill((lx, ly), (lx + 2, deep), line);
    let mut ne = across(side, ly, 0, cw + lx + 2);
    ne.fill((cw + lx, ly), (cw + lx + 2, deep), line);
    let mut sw = across(side, ch + ly, lx, side);
    sw.fill((lx, 0), (lx + 2, ch + ly + 2), line);
    let mut se = across(side, ch + ly, 0, cw + lx + 2);
    se.fill((cw + lx, 0), (cw + lx + 2, ch + ly + 2), line);
    pictures.extend([
        ("menu_w.png".to_string(), west),
        ("menu_e.png".to_string(), east),
        ("menu_nw.png".to_string(), nw),
        ("menu_ne.png".to_string(), ne),
        ("menu_sw.png".to_string(), sw),
        ("menu_se.png".to_string(), se),
        // The chosen entry: tan, two cells of it before the text and
        // after, the middle stretched to the box's width.
        ("select_w.png".to_string(), Picture::filled(side, ch, TAN)),
        ("select_c.png".to_string(), Picture::filled(1, ch, TAN)),
        ("select_e.png".to_string(), Picture::filled(side, ch, TAN)),
    ]);
    pictures
}

/// Appends a PNG chunk of `kind` holding `data` to `png`.
fn chunk(png: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    png.extend(u32::try_from(data.len()).expect("a small picture").to_be_bytes());
    let start = png.len();
    png.extend(kind);
    png.extend(data);
    let crc = crc32(&png[start..]);
    png.extend(crc.to_be_bytes());
}

/// `data` as a zlib stream of stored, uncompressed, blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // Deflate, a 32 KiB window, and the check bits that make the header
    // a multiple of 31.
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(0xffff).peekable();
    if blocks.peek().is_none() {
        out.extend([1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let len = u16::try_from(block.len()).expect("blocks of 65535 bytes at most");
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(block);
    }
    out.extend(adler32(data).to_be_bytes());
    out
}

/// CRC-32 as PNG checks its chunks with (ISO 3309), bit by bit.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Adler-32, the check zlib ends its stream with.
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// `font` in GRUB's font format, PF2, called `name`: every character the
/// console draws, each a whole cell's bitmap standing on the baseline as
/// the console's does, so that GRUB's command line and editor draw boxes
/// and text as the console does.
///
/// A PF2 file is a run of sections, each a four-letter name, its length
/// (big-endian, as every number in the file) and its data: the format,
/// the font's names and size, its widest and tallest character, its
/// ascent and descent, the index of characters (each its code point,
/// how it is stored, and where its glyph is), then the glyphs to the end
/// of the file, each its size, offsets and advance, then its bits row by
/// row, packed with no padding between rows.
fn pf2(font: &Font<'_>, name: &str) -> Vec<u8> {
    let (width, height) = (font.width, font.height);
    // The baseline is under the lowest row of `H`.
    let h = font.glyph('H');
    let ascent = (0..height).rev().find(|&y| h.row_bits(y) != 0).map_or(height, |y| y + 1);
    let descent = height - ascent;
    let mut chars = Vec::new();
    font.each_char(|c| chars.push(c));
    chars.sort_unstable();
    chars.dedup();

    let short = |value: usize| u16::try_from(value).expect("a console font's size").to_be_bytes();
    let string = |text: &str| [text.as_bytes(), b"\0"].concat();
    let sections: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"FILE", b"PFF2".to_vec()),
        (b"NAME", string(name)),
        (b"FAMI", string("Spleen")),
        (b"WEIG", string("normal")),
        (b"SLAN", string("normal")),
        (b"PTSZ", short(height).to_vec()),
        (b"MAXW", short(width).to_vec()),
        (b"MAXH", short(height).to_vec()),
        (b"ASCE", short(ascent).to_vec()),
        (b"DESC", short(descent).to_vec()),
    ];
    let mut file = Vec::new();
    for (kind, data) in &sections {
        file.extend(*kind);
        file.extend(u32::try_from(data.len()).expect("a short section").to_be_bytes());
        file.extend(data);
    }
    // The index's entries are nine bytes; the glyphs start after it and
    // the data section's header, whose length is "to the end".
    let index_len = chars.len() * 9;
    let glyph_len = 10 + (width * height).div_ceil(8);
    let first_glyph = file.len() + 8 + index_len + 8;
    file.extend(b"CHIX");
    file.extend(u32::try_from(index_len).expect("a font's index").to_be_bytes());
    for (i, &c) in chars.iter().enumerate() {
        file.extend(u32::from(c).to_be_bytes());
        // Stored uncompressed.
        file.push(0);
        let at = first_glyph + i * glyph_len;
        file.extend(u32::try_from(at).expect("a font under 4 GiB").to_be_bytes());
    }
    file.extend(b"DATA");
    file.extend(u32::MAX.to_be_bytes());
    let descent_offset = -i16::try_from(descent).expect("a console font's descent");
    for &c in &chars {
        let glyph = font.glyph(c);
        file.extend(short(width));
        file.extend(short(height));
        // No offset across; the bitmap's bottom `descent` under the
        // baseline; the next character a cell on.
        file.extend(0i16.to_be_bytes());
        file.extend(descent_offset.to_be_bytes());
        file.extend(short(width));
        let mut bits = vec![0u8; (width * height).div_ceil(8)];
        for y in 0..height {
            for x in 0..width {
                if glyph.pixel(x, y) {
                    let n = y * width + x;
                    bits[n / 8] |= 0x80 >> (n % 8);
                }
            }
        }
        file.extend(bits);
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_places_the_menu_for_its_entries() {
        let text = theme("0.14.0", 3);
        assert!(text.contains("text = \"Dukovo 0.14.0 · installer on USB\""), "{text}");
        assert!(text.contains("left = 50%-324\n    top = 10%+120\n    width = 648\n"), "{text}");
        assert!(text.contains("height = 168\n"), "{text}");
        // The hints a row under the box, the countdown under them.
        assert!(text.contains("top = 10%+312\n"), "{text}");
        assert!(text.contains("id = \"__timeout__\"\n    left = 0\n    top = 10%+336\n"));
    }

    #[test]
    fn writes_fonts_grub_reads() {
        let font = builtin_font("12x24").unwrap();
        let file = pf2(&font, "Spleen Regular 24");
        assert!(file.starts_with(b"FILE\0\0\0\x04PFF2NAME\0\0\0\x12Spleen Regular 24\0"));
        // The index, in order of code point, each entry pointing at a
        // glyph of the cell's size, inside the file.
        let at = file.windows(4).position(|w| w == b"CHIX").unwrap();
        let len = u32::from_be_bytes(file[at + 4..at + 8].try_into().unwrap()) as usize;
        let index: Vec<(u32, usize)> = file[at + 8..at + 8 + len]
            .chunks(9)
            .map(|e| {
                let code = u32::from_be_bytes(e[..4].try_into().unwrap());
                (code, u32::from_be_bytes(e[5..9].try_into().unwrap()) as usize)
            })
            .collect();
        assert!(index.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for code in ['A', '·', '↑', '✓', '─'] {
            let &(_, offset) = index.iter().find(|(c, _)| *c == u32::from(code)).unwrap();
            assert_eq!(&file[offset..offset + 4], &[0, 12, 0, 24], "{code}");
            assert!(offset + 10 + 36 <= file.len());
        }
        // `H` stands on the baseline: its descent is under it.
        let descent = file.windows(4).position(|w| w == b"DESC").unwrap();
        assert_eq!(&file[descent + 4..descent + 10], &[0, 0, 0, 2, 0, 5]);
    }

    #[test]
    fn writes_pngs_with_their_checks() {
        let png = Picture::filled(2, 1, TAN).png();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x02\0\0\0\x01\x08\x06"));
        // The IEND chunk's CRC, as every PNG file ends with it.
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let stream = zlib_stored(&[0, 1, 2]);
        assert_eq!(stream, [0x78, 0x01, 1, 3, 0, 0xfc, 0xff, 0, 1, 2, 0, 0x07, 0, 0x04]);
    }
}
