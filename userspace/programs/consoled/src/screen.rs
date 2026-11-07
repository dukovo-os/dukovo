// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screen: _/devices/fb0_ (radix-framebuffer(2)), taken from the kernel's
//! log, and the console drawn on it.
//!
//! The grid is centred on the screen in the Spleen font the kernel's log
//! used, chosen the same way (radix-fbcon), so the screen looks the same
//! when consoled takes it over; the installer's screens ask for a larger
//! one where it fits them. Each row of cells that changed since it was
//! last drawn is drawn into a buffer as tall as a row and the width of
//! the screen, and written to the framebuffer in one write; the
//! framebuffer is never read. The colours are those of the shown
//! console's palette, and the margins round the grid are its colour 0, the
//! background, so that a program that changes the palette colours the
//! screen edge to edge.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;

use radix_abi::framebuffer::{self as abi, FramebufferInfo};
use radix_fbcon::{Font, builtin_font, choose_font_for};

use crate::sys;
use crate::term::{
    BOLD, Cell, DIM, Palette, REVERSE, Terminal, UNDERLINE, WIDE_TAIL, default_palette,
};

/// The layout of the framebuffer's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Width and height in pixels.
    pub width: usize,
    pub height: usize,
    /// Bytes from one line of pixels to the next.
    pub pitch: usize,
    /// Bytes in a pixel: 2, 3 or 4.
    pub bpp: usize,
    /// Where each colour's bits are, and how many there are.
    pub red: (u8, u8),
    pub green: (u8, u8),
    pub blue: (u8, u8),
}

impl Format {
    /// Reads the radix-framebuffer(2) FramebufferInfo in `bytes`.
    pub fn parse(bytes: &[u8; FramebufferInfo::SIZE]) -> Format {
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let byte = |at: usize| bytes[at];
        Format {
            width: word(FramebufferInfo::OFFSET_WIDTH) as usize,
            height: word(FramebufferInfo::OFFSET_HEIGHT) as usize,
            pitch: word(FramebufferInfo::OFFSET_PITCH) as usize,
            bpp: usize::from(byte(FramebufferInfo::OFFSET_BYTES_PER_PIXEL)),
            red: (byte(FramebufferInfo::OFFSET_RED_SHIFT), byte(FramebufferInfo::OFFSET_RED_BITS)),
            green: (
                byte(FramebufferInfo::OFFSET_GREEN_SHIFT),
                byte(FramebufferInfo::OFFSET_GREEN_BITS),
            ),
            blue: (
                byte(FramebufferInfo::OFFSET_BLUE_SHIFT),
                byte(FramebufferInfo::OFFSET_BLUE_BITS),
            ),
        }
    }

    /// The bytes of a pixel of colour (`r`, `g`, `b`).
    pub fn encode(&self, (r, g, b): (u8, u8, u8)) -> [u8; 4] {
        let part = |value: u8, (shift, bits): (u8, u8)| -> u32 {
            let bits = bits.min(8);
            (u32::from(value) >> (8 - bits)) << shift
        };
        (part(r, self.red) | part(g, self.green) | part(b, self.blue)).to_le_bytes()
    }
}

/// The console on the screen.
pub struct Screen {
    file: File,
    format: Format,
    font: Font<'static>,
    font_name: &'static str,
    cols: usize,
    rows: usize,
    origin: (usize, usize),
    /// The palette the screen is drawn in: the shown console's.
    palette: Palette,
    /// Each colour of `palette` as pixel bytes.
    colours: Vec<[u8; 4]>,
    /// One row of cells' pixels, the screen's width.
    band: Vec<u8>,
    /// What each cell shows now, to draw only what changed.
    drawn: Vec<Option<Cell>>,
    drawn_cursor: Option<(usize, usize)>,
    drawn_reverse: bool,
}

/// The cells of a framebuffer of `format` in `font`: columns, rows and
/// the top-left cell's pixel, the grid centred, as radix-fbcon's.
pub fn layout(format: &Format, font: &Font<'_>) -> (usize, usize, (usize, usize)) {
    let cols = format.width / font.width;
    let rows = format.height / font.height;
    let origin = ((format.width - cols * font.width) / 2, (format.height - rows * font.height) / 2);
    (cols, rows, origin)
}

impl Screen {
    /// Opens _/devices/fb0_ and takes the screen from the kernel's log,
    /// with the largest font that gives a grid of `grid`'s columns and rows
    /// at least, or the smallest.
    pub fn open(path: &str, grid: (usize, usize)) -> io::Result<Screen> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        let mut info = [0u8; FramebufferInfo::SIZE];
        sys::control(fd, abi::GET_INFO, 0, &mut info)?;
        let format = Format::parse(&info);
        if !(2..=4).contains(&format.bpp) || format.pitch < format.width * format.bpp {
            return Err(io::Error::other(format!("an unusual framebuffer: {format:?}")));
        }
        let font_name = choose_font_for(format.width, format.height, grid);
        let font = builtin_font(font_name).ok_or_else(|| io::Error::other("no built-in font"))?;
        let (cols, rows, origin) = layout(&format, &font);
        if cols < 16 || rows < 4 {
            return Err(io::Error::other("the screen is too small for a console"));
        }
        sys::control(fd, abi::TAKE, 0, &mut [])?;
        let palette = default_palette();
        let colours = palette.iter().map(|&rgb| format.encode(rgb)).collect();
        let mut screen = Screen {
            file,
            format,
            font,
            font_name,
            cols,
            rows,
            origin,
            palette,
            colours,
            band: vec![0; format.pitch * font.height],
            drawn: vec![None; cols * rows],
            drawn_cursor: None,
            drawn_reverse: false,
        };
        screen.clear()?;
        Ok(screen)
    }

    /// Columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// What the console's geometry log line says, in the words of the
    /// kernel's own (the boot tests read the screen by it).
    pub fn describe(&self) -> String {
        format!(
            "console of {}x{} cells with font {} at ({}, {})",
            self.cols, self.rows, self.font_name, self.origin.0, self.origin.1
        )
    }

    /// Paints the whole screen in the palette's background, colour 0, and
    /// forgets what was drawn. The row buffer is painted too, so that the
    /// margins beside each row of cells, which each row's write covers,
    /// stay that colour.
    fn clear(&mut self) -> io::Result<()> {
        let (bpp, pitch, width) = (self.format.bpp, self.format.pitch, self.format.width);
        let pixel = self.colours[0];
        let paint = |buffer: &mut [u8]| {
            for line in buffer.chunks_exact_mut(pitch) {
                for px in line[..width * bpp].chunks_exact_mut(bpp) {
                    px.copy_from_slice(&pixel[..bpp]);
                }
            }
        };
        paint(&mut self.band);
        let mut lines = vec![0u8; pitch * 64];
        paint(&mut lines);
        let total = pitch * self.format.height;
        let mut at = 0;
        while at < total {
            let len = lines.len().min(total - at);
            self.file.write_all_at(&lines[..len], at as u64)?;
            at += len;
        }
        self.drawn.fill(None);
        self.drawn_cursor = None;
        Ok(())
    }

    /// Draws in `palette` from now on: every cell again, and the margins in
    /// its background.
    fn set_palette(&mut self, palette: &Palette) -> io::Result<()> {
        self.palette = *palette;
        self.colours = palette.iter().map(|&rgb| self.format.encode(rgb)).collect();
        self.clear()
    }

    /// Forgets what is on the screen, so the next draw draws every cell:
    /// for another console, or after someone else drew on it.
    pub fn invalidate(&mut self) {
        self.drawn.fill(None);
        self.drawn_cursor = None;
    }

    /// Draws what changed of `term` since the last draw, with `bottom`'s
    /// cells over its last row if there are any: a notification
    /// (`notices`).
    pub fn draw(&mut self, term: &Terminal, bottom: Option<&[Cell]>) -> io::Result<()> {
        // Another console's colours, or the program changed them: rare, so
        // the whole screen is painted again.
        if term.palette() != &self.palette {
            self.set_palette(term.palette())?;
        }
        let reverse = term.modes().reverse_screen;
        if reverse != self.drawn_reverse {
            self.invalidate();
            self.drawn_reverse = reverse;
        }
        let (cols, rows) = (self.cols.min(term.cols()), self.rows.min(term.rows()));
        let last = rows - 1;
        let shown = |x: usize, y: usize| match bottom {
            Some(cells) if y == last => cells.get(x).copied().unwrap_or(Cell::BLANK),
            _ => term.shown(x, y),
        };
        // No cursor under a notification.
        let cursor = term.cursor().filter(|&(_, cy)| bottom.is_none() || cy != last);
        for y in 0..rows {
            let cursor_here = cursor.filter(|&(_, cy)| cy == y).map(|(cx, _)| cx);
            let cursor_was = self.drawn_cursor.as_ref().is_some_and(|&(_, cy)| cy == y);
            let changed = cursor_here.is_some() != cursor_was
                || (cursor_here.is_some() && self.drawn_cursor != cursor)
                || (0..cols).any(|x| self.drawn[y * self.cols + x] != Some(shown(x, y)));
            if changed {
                self.draw_row(|x| shown(x, y), y, cols, cursor_here, reverse)?;
            }
        }
        self.drawn_cursor = cursor;
        Ok(())
    }

    fn draw_row(
        &mut self,
        shown: impl Fn(usize) -> Cell,
        y: usize,
        cols: usize,
        cursor: Option<usize>,
        reverse: bool,
    ) -> io::Result<()> {
        let (fw, fh, bpp) = (self.font.width, self.font.height, self.format.bpp);
        let pitch = self.format.pitch;
        for x in 0..cols {
            let cell = shown(x);
            self.drawn[y * self.cols + x] = Some(cell);
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if cell.flags & BOLD != 0 && fg < 8 {
                fg += 8;
            }
            if (cell.flags & REVERSE != 0) != reverse {
                (fg, bg) = (bg, fg);
            }
            let mut fg_px = self.colours[usize::from(fg)];
            if cell.flags & DIM != 0 {
                let (r, g, b) = self.palette[usize::from(fg)];
                fg_px = self.format.encode((r / 2, g / 2, b / 2));
            }
            let bg_px = self.colours[usize::from(bg)];
            // The second half of a wide character: the first drew it.
            let glyph = (cell.flags & WIDE_TAIL == 0).then(|| self.font.glyph(cell.c));
            let left = (self.origin.0 + x * fw) * bpp;
            for gy in 0..fh {
                let bits = glyph.map_or(0, |glyph| glyph.row_bits(gy));
                let underline = cell.flags & UNDERLINE != 0 && gy == fh - 1;
                let caret = cursor == Some(x) && gy + 2 >= fh;
                let line = &mut self.band[gy * pitch + left..gy * pitch + left + fw * bpp];
                for gx in 0..fw {
                    let lit = bits & (1 << (fw - 1 - gx)) != 0 || underline || caret;
                    let px = if lit { &fg_px } else { &bg_px };
                    line[gx * bpp..(gx + 1) * bpp].copy_from_slice(&px[..bpp]);
                }
            }
        }
        let top = (self.origin.1 + y * fh) * pitch;
        self.file.write_all_at(&self.band, top as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_pixels_of_each_layout() {
        let xrgb = Format {
            width: 640,
            height: 480,
            pitch: 2560,
            bpp: 4,
            red: (16, 8),
            green: (8, 8),
            blue: (0, 8),
        };
        assert_eq!(xrgb.encode((0x12, 0x34, 0x56)), [0x56, 0x34, 0x12, 0]);
        let rgb565 = Format { bpp: 2, red: (11, 5), green: (5, 6), blue: (0, 5), ..xrgb };
        assert_eq!(
            u16::from_le_bytes(rgb565.encode((0xff, 0xff, 0xff))[..2].try_into().unwrap()),
            0xffff
        );
    }

    #[test]
    fn lays_out_as_the_kernel_does() {
        let format = Format {
            width: 1280,
            height: 800,
            pitch: 5120,
            bpp: 4,
            red: (16, 8),
            green: (8, 8),
            blue: (0, 8),
        };
        let name = choose_font_for(format.width, format.height, radix_fbcon::MIN_GRID);
        let font = builtin_font(name).unwrap();
        assert_eq!(
            layout(&format, &font),
            (1280 / font.width, 800 / font.height, (0, (800 % font.height) / 2))
        );
    }
}
