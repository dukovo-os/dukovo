// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Framebuffer text console for the Radix kernel.
//!
//! A machine without a serial port, such as the reference laptop, shows the
//! kernel's output only on its screen. Under UEFI there is no text mode, so
//! the kernel draws text itself into the linear framebuffer the boot loader
//! set up. [`Console`] keeps a grid of character cells, draws them with a
//! [PSF](psf) bitmap font, and on each [`Console::flush`] redraws only the
//! cells that changed since the last one. Scrolling moves the grid in RAM;
//! the framebuffer is never read, because reads from video memory are slow.
//!
//! A screen may have no framebuffer: a virtual machine's graphics card
//! is sent its pixels. A console for one ([`Console::without_framebuffer`])
//! keeps the same grid and draws nothing. Whoever sends the pixels asks
//! it what changed ([`Console::next_draw`]) and draws that a row of
//! pixels at a time ([`Painter::row`]), so neither keeps a picture of the
//! screen. Both kinds draw with the same code and show the same pixels.
//!
//! The built-in fonts are Spleen by Frederic Cambus (BSD-2-Clause), in three
//! sizes; [`choose_font`] picks one for the screen. The grid's storage is
//! passed in, so the crate needs no heap and its tests run on the host.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod psf;

pub use psf::{Font, Glyph};

use radix_hal::{Framebuffer, PixelFormat};
use radix_klog::Style;

/// The built-in fonts, smallest first, as (name, PSF file).
pub const FONTS: [(&str, &[u8]); 3] = [
    ("8x16", include_bytes!("../fonts/spleen-8x16.psfu")),
    ("12x24", include_bytes!("../fonts/spleen-12x24.psfu")),
    ("16x32", include_bytes!("../fonts/spleen-16x32.psfu")),
];

/// The built-in font called `name`, such as `12x24`.
pub fn builtin_font(name: &str) -> Option<Font<'static>> {
    FONTS.iter().find(|(font, _)| *font == name).and_then(|(_, bytes)| Font::parse(bytes).ok())
}

/// Columns and rows the console should have at least, if a font allows.
pub const MIN_GRID: (usize, usize) = (100, 40);

/// Picks the largest built-in font that still gives a grid of at least 100
/// columns by 40 rows on a `width` x `height` screen, or the smallest font
/// if none does. Returns the font's name.
pub fn choose_font(width: usize, height: usize) -> &'static str {
    choose_font_for(width, height, MIN_GRID)
}

/// Picks the largest built-in font that still gives a grid of at least
/// `grid`'s columns and rows on a `width` x `height` screen, or the
/// smallest font if none does: for a program that needs fewer cells than
/// the console, such as the installer's screens of 100 by 30. Returns the
/// font's name.
pub fn choose_font_for(width: usize, height: usize, grid: (usize, usize)) -> &'static str {
    FONTS
        .iter()
        .rev()
        .filter_map(|&(name, bytes)| Some((name, Font::parse(bytes).ok()?)))
        .find(|(_, font)| width / font.width >= grid.0 && height / font.height >= grid.1)
        .map_or(FONTS[0].0, |(name, _)| name)
}

/// Cells of storage [`Console::new`] needs for a full grid: two per
/// character position, one for the text and one for what is on screen.
pub fn storage_cells(width: usize, height: usize, font: &Font<'_>) -> usize {
    2 * (width / font.width) * (height / font.height)
}

/// Number of distinct [`Style`]s.
const STYLES: usize = 6;

fn style_index(style: Style) -> u32 {
    match style {
        Style::Plain => 0,
        Style::Error => 1,
        Style::Warning => 2,
        Style::Debug => 3,
        Style::Emphasis => 4,
        Style::Banner => 5,
    }
}

/// A colour as (r, g, b).
type Rgb = (u8, u8, u8);

/// Foreground and background colour of each style.
const PALETTE: [(Rgb, Rgb); STYLES] = [
    ((0xc8, 0xc8, 0xc8), (0, 0, 0)),
    ((0xff, 0x60, 0x60), (0, 0, 0)),
    ((0xff, 0xd0, 0x40), (0, 0, 0)),
    ((0x88, 0x88, 0x88), (0, 0, 0)),
    ((0xff, 0xff, 0xff), (0, 0, 0)),
    ((0xff, 0xff, 0xff), (0xa0, 0x10, 0x10)),
];

/// Widest glyph the renderer supports, in pixels.
const MAX_GLYPH_WIDTH: usize = 32;

/// A cell holds a character in its low 21 bits and a style index above
/// them. Zero is a blank in the plain style, which is how cells start.
fn encode_cell(c: char, style: u32) -> u32 {
    if c == ' ' && PALETTE[style as usize].1 == PALETTE[0].1 {
        0
    } else {
        u32::from(c) | (style << 24)
    }
}

fn decode_cell(cell: u32) -> (char, usize) {
    let c = char::from_u32(cell & 0x1f_ffff).filter(|&c| c != '\0').unwrap_or(' ');
    (c, ((cell >> 24) as usize).min(STYLES - 1))
}

/// A rectangle of the screen: x, y, width and height in pixels.
pub type Rect = (usize, usize, usize, usize);

/// What it takes to draw a console's cells and its startup screen: the
/// font, the colours in the screen's pixel layout, and where the grid is
/// on the screen. The console draws on a framebuffer with it, and a copy
/// of it draws rows of pixels with no console at hand ([`Painter::row`]),
/// for a screen whose pixels are sent to it.
#[derive(Clone, Copy, Debug)]
pub struct Painter {
    font: Font<'static>,
    format: PixelFormat,
    /// The screen's size in pixels.
    size: (usize, usize),
    /// Columns in the grid.
    cols: usize,
    /// Pixel position of the grid's top-left corner.
    origin: (usize, usize),
    /// Encoded pixel values: (foreground, background) per style.
    colors: [(u32, u32); STYLES],
}

/// A part of the screen of a console with no framebuffer, to be drawn by
/// whoever sends its pixels ([`Console::next_draw`], [`Painter::row`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    /// All of the screen, in the plain background.
    Blank,
    /// All of the screen: the startup screen, its bar at `per_mille` of
    /// 1000.
    Splash {
        /// The name in large letters.
        title: &'static str,
        /// The line near the bottom.
        hint: &'static str,
        /// How far the bar is.
        per_mille: u32,
    },
    /// The startup screen's bar, at so many of 1000.
    Bar(u32),
    /// Cells of one row of the grid.
    Cells {
        /// The row.
        row: usize,
        /// The column of the first.
        first: usize,
        /// How many.
        count: usize,
    },
}

/// Where the startup screen's name and hint go.
struct Splash<'a> {
    title: &'a str,
    hint: &'a str,
    /// The name's top-left corner, and the pixels to a pixel of the font.
    title_at: (usize, usize),
    scale: usize,
    hint_at: (usize, usize),
}

impl Painter {
    fn bpp(&self) -> usize {
        usize::from(self.format.bytes_per_pixel)
    }

    /// The pixel value of `rgb` in the screen's layout.
    fn encode(&self, (r, g, b): Rgb) -> u32 {
        self.format.encode(r, g, b)
    }

    /// The screen's size in pixels, as (width, height).
    pub fn screen(&self) -> (usize, usize) {
        self.size
    }

    /// Columns in the grid: the most cells a [`Draw::Cells`] has.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The rectangle of the screen that `draw` covers.
    pub fn area(&self, draw: &Draw) -> Rect {
        match *draw {
            Draw::Blank | Draw::Splash { .. } => (0, 0, self.size.0, self.size.1),
            Draw::Bar(_) => self.splash_bar(),
            Draw::Cells { row, first, count } => (
                self.origin.0 + first * self.font.width,
                self.origin.1 + row * self.font.height,
                count * self.font.width,
                self.font.height,
            ),
        }
    }

    /// Draws row `index` of `draw`'s rectangle into `out`, which is a
    /// row of it. `cells` are the cells of a [`Draw::Cells`], as
    /// [`Console::next_draw`] gave them.
    pub fn row(&self, draw: &Draw, cells: &[u32], index: usize, out: &mut [u8]) {
        match *draw {
            Draw::Blank => self.fill(out, self.colors[0].1),
            Draw::Splash { title, hint, per_mille } => {
                self.fill(out, self.encode(SPLASH_BACKGROUND));
                let splash = self.splash(title, hint);
                self.splash_row(&splash, per_mille, index, &mut |x, width, color| {
                    self.run(out, x, width, color);
                });
            }
            Draw::Bar(per_mille) => {
                let (left, top, ..) = self.splash_bar();
                self.bar_row(per_mille, top + index, &mut |x, width, color| {
                    self.run(out, x - left, width, color);
                });
            }
            Draw::Cells { .. } => self.cells_row(cells, index, out),
        }
    }

    /// Paints every pixel of `out` with the value `color`.
    fn fill(&self, out: &mut [u8], color: u32) {
        let (bpp, color) = (self.bpp(), color.to_le_bytes());
        if color == [0; 4] {
            out.fill(0);
            return;
        }
        for pixel in out.chunks_exact_mut(bpp) {
            pixel.copy_from_slice(&color[..bpp]);
        }
    }

    /// Paints `width` pixels of the row `out` from pixel `x`, as far as
    /// the row goes.
    fn run(&self, out: &mut [u8], x: usize, width: usize, color: u32) {
        let bpp = self.bpp();
        let end = (x + width).min(out.len() / bpp);
        if let Some(pixels) = out.get_mut(x * bpp..end.max(x) * bpp) {
            self.fill(pixels, color);
        }
    }

    /// Draws row `gy` of the glyphs of `cells`, side by side from the
    /// start of `out`.
    fn cells_row(&self, cells: &[u32], gy: usize, out: &mut [u8]) {
        let glyph_bytes = self.font.width * self.bpp();
        for (&cell, out) in cells.iter().zip(out.chunks_exact_mut(glyph_bytes)) {
            let (c, style) = decode_cell(cell);
            self.glyph_row(self.font.glyph(c).row_bits(gy), self.colors[style], out);
        }
    }

    /// Draws `bits`, a row of a glyph with its left pixel in the highest
    /// bit, at the start of `out`: its lit pixels in `fg`, the rest in
    /// `bg`.
    fn glyph_row(&self, bits: u32, (fg, bg): (u32, u32), out: &mut [u8]) {
        let (width, bpp) = (self.font.width, self.bpp());
        let (fg, bg) = (fg.to_le_bytes(), bg.to_le_bytes());
        for (gx, pixel) in out.chunks_exact_mut(bpp).take(width).enumerate() {
            let lit = bits & (1 << (width - 1 - gx)) != 0;
            pixel.copy_from_slice(&if lit { fg } else { bg }[..bpp]);
        }
    }

    /// Where the startup screen (ADR 0426) has `title`, in large letters
    /// a little above the middle, and `hint`, small near the bottom.
    fn splash<'a>(&self, title: &'a str, hint: &'a str) -> Splash<'a> {
        let (width, height) = self.size;
        let (font_width, font_height) = (self.font.width, self.font.height);
        let letters = title.chars().count().max(1);
        let scale = (width * SPLASH_EIGHTHS / 8 / (letters * font_width))
            .clamp(1, SPLASH_MAX_SCALE)
            .min((height / 4 / font_height).max(1));
        let title_width = letters * font_width * scale;
        let title_y = (height * 2 / 5).saturating_sub(font_height * scale / 2);
        let hint_width = hint.chars().count() * font_width;
        let hint_y = height.saturating_sub(font_height * 3);
        Splash {
            title,
            hint,
            title_at: (width.saturating_sub(title_width) / 2, title_y),
            scale,
            hint_at: (width.saturating_sub(hint_width) / 2, hint_y),
        }
    }

    /// Where the startup screen's bar is: centred a line below the name,
    /// whose letters are at most a quarter of the screen high, a third of
    /// the screen wide and a third of a line high.
    fn splash_bar(&self) -> Rect {
        let (width, height) = self.size;
        let bar_width = (width * SPLASH_EIGHTHS / 8).max(self.font.width * 10).min(width);
        let bar_height = (self.font.height / 3).max(4);
        let y = height * 2 / 5 + height / 8 + self.font.height;
        ((width - bar_width) / 2, y.min(height.saturating_sub(bar_height)), bar_width, bar_height)
    }

    /// Calls `run` with each stretch of row `y` of the startup screen
    /// that is not its background, as (x, width, pixel value): the
    /// name's, then the hint's, then the bar's, each over those before.
    fn splash_row(
        &self,
        splash: &Splash<'_>,
        per_mille: u32,
        y: usize,
        run: &mut dyn FnMut(usize, usize, u32),
    ) {
        let title = self.encode(SPLASH_TITLE);
        let mut lit = |x, width| run(x, width, title);
        self.text_row(splash.title, splash.title_at, splash.scale, y, &mut lit);
        let hint = self.encode(SPLASH_HINT);
        self.text_row(splash.hint, splash.hint_at, 1, y, &mut |x, width| run(x, width, hint));
        self.bar_row(per_mille, y, run);
    }

    /// Calls `run` with the stretches of row `y` that are the startup
    /// screen's bar at `per_mille` of 1000: what is filled and the track
    /// that is left, or nothing on a row the bar is not on.
    fn bar_row(&self, per_mille: u32, y: usize, run: &mut dyn FnMut(usize, usize, u32)) {
        let (x, top, width, height) = self.splash_bar();
        if !(top..top + height).contains(&y) {
            return;
        }
        let filled = width * per_mille.min(1000) as usize / 1000;
        run(x, filled, self.encode(SPLASH_BAR));
        run(x + filled, width - filled, self.encode(SPLASH_TRACK));
    }

    /// Calls `run` with each lit stretch, as (x, width), of row `y` of
    /// the screen where `text` is drawn with its top-left corner at `at`
    /// and each pixel of the font as a square of `scale` pixels.
    fn text_row(
        &self,
        text: &str,
        at: (usize, usize),
        scale: usize,
        y: usize,
        run: &mut dyn FnMut(usize, usize),
    ) {
        let (font_width, font_height) = (self.font.width, self.font.height);
        let Some(gy) = y.checked_sub(at.1).map(|down| down / scale).filter(|&gy| gy < font_height)
        else {
            return;
        };
        for (index, c) in text.chars().enumerate() {
            let bits = self.font.glyph(c).row_bits(gy);
            let left = at.0 + index * font_width * scale;
            for gx in 0..font_width {
                if bits & (1 << (font_width - 1 - gx)) != 0 {
                    run(left + gx * scale, scale);
                }
            }
        }
    }
}

/// Where a console's pixels go.
enum Screen {
    /// Into the firmware's framebuffer, as they are drawn.
    Framebuffer(Framebuffer),
    /// To whoever sends them to the screen, which asks what waits to be
    /// drawn ([`Console::next_draw`]).
    Sent(Sent),
}

/// What of the screen of a console with no framebuffer waits to be
/// drawn.
#[derive(Clone, Copy)]
struct Sent {
    /// All of it, before anything else.
    whole: Option<Whole>,
    /// The startup screen's bar.
    bar: bool,
    /// How far the bar is, of 1000.
    per_mille: u32,
    /// Whether a cell may differ from what was drawn of it.
    touched: bool,
    /// The row the search for such a cell goes on from.
    next: usize,
}

/// All of a screen, as it waits to be drawn.
#[derive(Clone, Copy)]
enum Whole {
    Blank,
    Splash { title: &'static str, hint: &'static str },
}

/// A text console drawn into a linear framebuffer, or with no
/// framebuffer of its own for a screen whose pixels are sent to it.
pub struct Console {
    screen: Screen,
    paint: Painter,
    rows: usize,
    /// The text, `cols * rows` cells row by row, then what the screen
    /// shows in the same layout; the rest is not used.
    storage: &'static mut [u32],
    col: usize,
    row: usize,
    /// The cursor is past the last column; the next character wraps.
    wrap_pending: bool,
    cells_drawn: u64,
}

impl core::fmt::Debug for Console {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Console")
            .field("framebuffer", &self.framebuffer())
            .field("paint", &self.paint)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl Console {
    /// Takes over `fb`, clears it and returns an empty console. The grid
    /// is centred on the screen. If `storage` holds fewer than
    /// [`storage_cells`] cells, the grid gets fewer rows. Returns `None`
    /// for framebuffers this renderer cannot draw on (fewer than 16
    /// columns or 4 rows, or an unusual pixel size).
    pub fn new(fb: Framebuffer, font: Font<'static>, storage: &'static mut [u32]) -> Option<Self> {
        let bpp = usize::from(fb.format.bytes_per_pixel);
        if !(2..=4).contains(&bpp) || fb.pitch < fb.width * bpp {
            return None;
        }
        let size = (fb.width, fb.height.min(fb.memory.len() / fb.pitch.max(1)));
        let format = fb.format;
        Console::with(Screen::Framebuffer(fb), format, size, &font, storage).ok()
    }

    /// An empty console for a screen of `size` pixels, wide and high,
    /// that has no framebuffer: nothing is drawn, and whoever sends the
    /// screen its pixels asks what to draw ([`Console::next_draw`]) and
    /// draws it in the usual 32-bit layout ([`Console::painter`]). The
    /// grid is as [`Console::new`] makes it.
    ///
    /// # Errors
    ///
    /// `storage` again, for a screen of fewer than 16 columns or 4 rows.
    pub fn without_framebuffer(
        size: (usize, usize),
        font: &Font<'static>,
        storage: &'static mut [u32],
    ) -> Result<Self, &'static mut [u32]> {
        let sent = Sent { whole: None, bar: false, per_mille: 0, touched: false, next: 0 };
        Console::with(Screen::Sent(sent), PixelFormat::XRGB8888, size, font, storage)
    }

    fn with(
        screen: Screen,
        format: PixelFormat,
        size: (usize, usize),
        font: &Font<'static>,
        storage: &'static mut [u32],
    ) -> Result<Self, &'static mut [u32]> {
        let cols = size.0 / font.width;
        let rows = (size.1 / font.height).min(storage.len() / 2 / cols.max(1));
        if font.width > MAX_GLYPH_WIDTH || cols < 16 || rows < 4 {
            return Err(storage);
        }
        let origin = ((size.0 - cols * font.width) / 2, (size.1 - rows * font.height) / 2);
        let colors = PALETTE.map(|((fr, fg, fb), (br, bg, bb))| {
            (format.encode(fr, fg, fb), format.encode(br, bg, bb))
        });
        let paint = Painter { font: *font, format, size, cols, origin, colors };
        let mut console = Console {
            screen,
            paint,
            rows,
            storage,
            col: 0,
            row: 0,
            wrap_pending: false,
            cells_drawn: 0,
        };
        console.clear();
        Ok(console)
    }

    /// Ends the console and returns the storage it was given.
    pub fn into_storage(self) -> &'static mut [u32] {
        self.storage
    }

    /// Columns in the grid.
    pub fn cols(&self) -> usize {
        self.paint.cols
    }

    /// Rows in the grid.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Glyph size in pixels, as (width, height).
    pub fn font_size(&self) -> (usize, usize) {
        (self.paint.font.width, self.paint.font.height)
    }

    /// Pixel position of the grid's top-left corner.
    pub fn origin(&self) -> (usize, usize) {
        self.paint.origin
    }

    /// The framebuffer the console draws on, if it has one.
    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        match &self.screen {
            Screen::Framebuffer(fb) => Some(fb),
            Screen::Sent(_) => None,
        }
    }

    /// The framebuffer's pixels, if the console has one, for the
    /// kernel's display layer, whose first card the screen is: what its
    /// holder in front shows is drawn here.
    pub fn pixels_mut(&mut self) -> Option<&mut [u8]> {
        match &mut self.screen {
            Screen::Framebuffer(fb) => Some(&mut *fb.memory),
            Screen::Sent(_) => None,
        }
    }

    /// What draws the console's cells and its startup screen as rows of
    /// pixels.
    pub fn painter(&self) -> Painter {
        self.paint
    }

    /// The cells of the grid: one for each character position.
    fn len(&self) -> usize {
        self.paint.cols * self.rows
    }

    /// The text, row by row.
    fn cells(&mut self) -> &mut [u32] {
        let len = self.len();
        &mut self.storage[..len]
    }

    /// What the screen shows, in the layout of the text.
    fn drawn(&mut self) -> &mut [u32] {
        let len = self.len();
        &mut self.storage[len..2 * len]
    }

    /// Notes that a cell may differ from what the screen shows, for a
    /// console with no framebuffer.
    fn touch(&mut self) {
        if let Screen::Sent(sent) = &mut self.screen {
            sent.touched = true;
        }
    }

    /// Blanks the screen and moves the cursor to the top-left corner.
    pub fn clear(&mut self) {
        self.blank();
        self.cells().fill(0);
        self.drawn().fill(0);
        self.col = 0;
        self.row = 0;
        self.wrap_pending = false;
    }

    /// Draws the whole screen again from the grid, for when something else
    /// has drawn on it: a holder of the display layer, before the front
    /// was the log's again.
    pub fn repaint(&mut self) {
        self.blank();
        // No cell holds the first value, so every one is drawn. With no
        // framebuffer the blank is sent first, and after it a cell that
        // is blank shows as it is.
        let unseen = if self.framebuffer().is_some() { u32::MAX } else { 0 };
        self.drawn().fill(unseen);
        self.touch();
        self.flush();
    }

    /// Paints every pixel in the plain background.
    fn blank(&mut self) {
        let background = self.paint.colors[0].1;
        let (width, height) = self.paint.size;
        match &mut self.screen {
            Screen::Framebuffer(fb) => {
                for y in 0..height {
                    fill_rect(fb, (0, y, width, 1), background);
                }
            }
            Screen::Sent(sent) => (sent.whole, sent.bar) = (Some(Whole::Blank), false),
        }
    }

    /// The cursor position, as (column, row).
    pub fn position(&self) -> (usize, usize) {
        (self.col, self.row)
    }

    /// Moves the cursor, clamped to the grid.
    pub fn set_position(&mut self, col: usize, row: usize) {
        self.col = col.min(self.paint.cols - 1);
        self.row = row.min(self.rows - 1);
        self.wrap_pending = false;
    }

    /// Paints row `row` blank in `style`'s background, for a title band.
    pub fn fill_row(&mut self, row: usize, style: Style) {
        let blank = encode_cell(' ', style_index(style));
        let cols = self.paint.cols;
        if let Some(cells) = self.cells().get_mut(row * cols..(row + 1) * cols) {
            cells.fill(blank);
        }
        self.touch();
    }

    /// Writes text at the cursor. Lines wrap at the right edge and the
    /// grid scrolls at the bottom. `\n` starts a new line, `\r` returns to
    /// its start and `\t` moves to the next multiple of eight columns, as
    /// on a terminal; other control characters are skipped. Nothing reaches the screen
    /// until [`Console::flush`].
    pub fn write(&mut self, text: &str, style: Style) {
        // Keep going sensibly even if a previous writer was interrupted
        // half-way (the panic path may take the console over).
        let cols = self.paint.cols;
        self.col = self.col.min(cols - 1);
        self.row = self.row.min(self.rows - 1);
        self.touch();
        let style = style_index(style);
        for c in text.chars() {
            match c {
                '\n' => self.newline(),
                '\r' => {
                    self.col = 0;
                    self.wrap_pending = false;
                }
                '\t' if !self.wrap_pending => {
                    let next = (self.col / 8 + 1) * 8;
                    if next < cols {
                        self.col = next;
                    } else {
                        self.col = cols - 1;
                        self.wrap_pending = true;
                    }
                }
                c if c.is_control() => {}
                c => self.put(c, style),
            }
        }
    }

    fn put(&mut self, c: char, style: u32) {
        if self.wrap_pending {
            self.newline();
        }
        let (cols, at) = (self.paint.cols, self.row * self.paint.cols + self.col);
        self.cells()[at] = encode_cell(c, style);
        if self.col + 1 == cols {
            self.wrap_pending = true;
        } else {
            self.col += 1;
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        self.wrap_pending = false;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            let (cols, last) = (self.paint.cols, (self.rows - 1) * self.paint.cols);
            self.cells().copy_within(cols.., 0);
            self.cells()[last..].fill(0);
        }
    }

    /// Draws every cell that changed since the last flush. A console
    /// with no framebuffer draws nothing: [`Console::next_draw`] says
    /// what changed.
    pub fn flush(&mut self) {
        if self.framebuffer().is_none() {
            return;
        }
        let len = self.len();
        for index in 0..len {
            if self.storage[index] != self.storage[len + index] {
                self.draw_cell(index);
                self.storage[len + index] = self.storage[index];
            }
        }
    }

    /// Cells drawn since the console was created, for tests and statistics.
    pub fn cells_drawn(&self) -> u64 {
        self.cells_drawn
    }

    fn draw_cell(&mut self, index: usize) {
        let Screen::Framebuffer(fb) = &mut self.screen else { return };
        self.cells_drawn += 1;
        let paint = &self.paint;
        let (c, style) = decode_cell(self.storage[index]);
        let glyph = paint.font.glyph(c);
        let bpp = paint.bpp();
        let (width, height) = (paint.font.width, paint.font.height);
        let x = paint.origin.0 + (index % paint.cols) * width;
        let y = paint.origin.1 + (index / paint.cols) * height;
        let mut line = [0u8; MAX_GLYPH_WIDTH * 4];
        for gy in 0..height {
            paint.glyph_row(glyph.row_bits(gy), paint.colors[style], &mut line);
            let start = (y + gy) * fb.pitch + x * bpp;
            if let Some(target) = fb.memory.get_mut(start..start + width * bpp) {
                target.copy_from_slice(&line[..width * bpp]);
            }
        }
    }

    /// Calls `f` with the text of row `row`, trailing blanks removed.
    pub fn row_text(&self, row: usize, f: &mut dyn FnMut(char)) {
        let cols = self.paint.cols;
        let Some(cells) = self.storage[..self.len()].get(row * cols..(row + 1) * cols) else {
            return;
        };
        let used = cells.iter().rposition(|&cell| decode_cell(cell).0 != ' ').map_or(0, |i| i + 1);
        cells[..used].iter().for_each(|&cell| f(decode_cell(cell).0));
    }

    /// What to draw next on the screen of a console with no framebuffer,
    /// which counts as drawn from then on: all of the screen if that
    /// waits, then the startup screen's bar, then, if `text` (the log
    /// shows, and not the startup screen), cells of one row that differ
    /// from what was drawn: as many as `cells` holds, copied into it.
    /// `None` when nothing waits, and for a console that draws on a
    /// framebuffer itself.
    pub fn next_draw(&mut self, text: bool, cells: &mut [u32]) -> Option<Draw> {
        let Screen::Sent(sent) = &mut self.screen else { return None };
        if let Some(whole) = sent.whole.take() {
            sent.bar = false;
            return Some(match whole {
                Whole::Blank => Draw::Blank,
                Whole::Splash { title, hint } => {
                    Draw::Splash { title, hint, per_mille: sent.per_mille }
                }
            });
        }
        if core::mem::take(&mut sent.bar) {
            return Some(Draw::Bar(sent.per_mille));
        }
        if !text || !sent.touched || cells.is_empty() {
            return None;
        }
        let (cols, rows) = (self.paint.cols, self.rows);
        let (now, was) = self.storage.split_at_mut(cols * rows);
        for step in 0..rows {
            let row = (sent.next + step) % rows;
            let (now, was) = (&now[row * cols..][..cols], &mut was[row * cols..][..cols]);
            let Some(first) = (0..cols).find(|&at| now[at] != was[at]) else { continue };
            let last = (first..cols).rfind(|&at| now[at] != was[at]).unwrap_or(first);
            let count = (last + 1 - first).min(cells.len());
            cells[..count].copy_from_slice(&now[first..first + count]);
            was[first..first + count].copy_from_slice(&now[first..first + count]);
            sent.next = row;
            return Some(Draw::Cells { row, first, count });
        }
        sent.touched = false;
        None
    }
}

// The startup screen's colours (ADR 0426), the installer's: slate behind,
// cream for the name, sand for the bar on a darker track, and grey for
// the hint.

/// The startup screen's background, which the boot tests look for.
pub const SPLASH_BACKGROUND: (u8, u8, u8) = (0x15, 0x18, 0x1c);
const SPLASH_TITLE: Rgb = (0xd9, 0xd4, 0xca);
/// The startup screen's bar, which the boot tests measure.
pub const SPLASH_BAR: (u8, u8, u8) = (0xe9, 0xc4, 0x8b);
const SPLASH_TRACK: Rgb = (0x2c, 0x31, 0x38);
const SPLASH_HINT: Rgb = (0x7c, 0x88, 0x96);

/// How much of the screen's width the name and the bar take, in
/// eighths: about a third.
const SPLASH_EIGHTHS: usize = 3;
/// The largest the name's letters are drawn, in times the font's size.
const SPLASH_MAX_SCALE: usize = 6;

impl Console {
    /// Draws the startup screen of a quiet boot (ADR 0426) over the whole
    /// screen: `title` in large letters a little above the middle,
    /// the progress bar under it at `per_mille` of 1000, and `hint` small
    /// near the bottom. The grid keeps its text, unseen until
    /// [`Console::repaint`].
    pub fn draw_splash(&mut self, title: &'static str, hint: &'static str, per_mille: u32) {
        let paint = &self.paint;
        let fb = match &mut self.screen {
            Screen::Framebuffer(fb) => fb,
            Screen::Sent(sent) => {
                sent.whole = Some(Whole::Splash { title, hint });
                (sent.bar, sent.per_mille) = (false, per_mille);
                return;
            }
        };
        let splash = paint.splash(title, hint);
        let background = paint.encode(SPLASH_BACKGROUND);
        for y in 0..paint.size.1 {
            fill_rect(fb, (0, y, paint.size.0, 1), background);
            paint.splash_row(&splash, per_mille, y, &mut |x, width, color| {
                fill_rect(fb, (x, y, width, 1), color);
            });
        }
    }

    /// Draws the startup screen's bar at `per_mille` of 1000.
    pub fn draw_progress(&mut self, per_mille: u32) {
        let paint = &self.paint;
        let fb = match &mut self.screen {
            Screen::Framebuffer(fb) => fb,
            Screen::Sent(sent) => {
                (sent.bar, sent.per_mille) = (true, per_mille);
                return;
            }
        };
        let (_, top, _, height) = paint.splash_bar();
        for y in top..top + height {
            paint.bar_row(per_mille, y, &mut |x, width, color| {
                fill_rect(fb, (x, y, width, 1), color);
            });
        }
    }

    /// Where the startup screen's bar is.
    #[cfg(test)]
    fn splash_bar(&self) -> Rect {
        self.paint.splash_bar()
    }
}

/// Paints `rect` of `fb` with the pixel value `color`, clipped to the
/// screen and to the rows its memory holds.
fn fill_rect(fb: &mut Framebuffer, (x, y, width, height): Rect, color: u32) {
    let bpp = usize::from(fb.format.bytes_per_pixel);
    let color = color.to_le_bytes();
    let right = (x + width).min(fb.width);
    for row in y..(y + height).min(fb.height) {
        let start = row * fb.pitch;
        let Some(line) = fb.memory.get_mut(start + x * bpp..start + right.max(x) * bpp) else {
            continue;
        };
        if color == [0; 4] {
            line.fill(0);
            continue;
        }
        for pixel in line.chunks_exact_mut(bpp) {
            pixel.copy_from_slice(&color[..bpp]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framebuffer(width: usize, height: usize) -> Framebuffer {
        let pitch = width * 4 + 8;
        Framebuffer {
            memory: Box::leak(vec![0x55u8; pitch * height].into_boxed_slice()),
            phys_addr: 0xfd00_0000,
            width,
            height,
            pitch,
            format: PixelFormat::XRGB8888,
        }
    }

    fn storage(cells: usize) -> &'static mut [u32] {
        Box::leak(vec![0u32; cells].into_boxed_slice())
    }

    fn console(width: usize, height: usize) -> Console {
        let font = builtin_font("8x16").unwrap();
        let cells = storage_cells(width, height, &font);
        Console::new(framebuffer(width, height), font, storage(cells)).unwrap()
    }

    fn text(console: &Console, row: usize) -> String {
        let mut out = String::new();
        console.row_text(row, &mut |c| out.push(c));
        out
    }

    fn pixel(console: &Console, x: usize, y: usize) -> u32 {
        let fb = console.framebuffer().unwrap();
        let at = y * fb.pitch + x * 4;
        u32::from_le_bytes(fb.memory[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn builtin_fonts_parse_and_cover_ascii() {
        for (name, _) in FONTS {
            let font = builtin_font(name).unwrap();
            assert_eq!(format!("{}x{}", font.width, font.height), name);
            for c in ' '..='~' {
                assert!(font.lookup(c).is_some(), "{name} lacks {c:?}");
            }
            assert!(font.lookup('│').is_some(), "{name} lacks box drawing");
        }
    }

    #[test]
    fn font_choice_follows_screen_size() {
        assert_eq!(choose_font(800, 600), "8x16");
        assert_eq!(choose_font(1366, 768), "8x16");
        assert_eq!(choose_font(1920, 1080), "12x24");
        assert_eq!(choose_font(3840, 2160), "16x32");
        // The installer's 100 by 30 fits larger fonts.
        let screens = (100, 30);
        assert_eq!(choose_font_for(1024, 768, screens), "8x16");
        assert_eq!(choose_font_for(1280, 800, screens), "12x24");
        assert_eq!(choose_font_for(1920, 1080, screens), "16x32");
    }

    #[test]
    fn every_font_draws_a_check_mark() {
        for (name, _) in FONTS {
            let font = builtin_font(name).unwrap();
            // Spleen has none: Dukovo's own is drawn, not `?`.
            assert!(font.lookup('\u{2713}').is_none(), "{name} has \u{2713} now: drop ours");
            let check = font.glyph('\u{2713}');
            let question = font.glyph('?');
            let rows =
                |glyph: Glyph<'_>| (0..font.height).map(|y| glyph.row_bits(y)).collect::<Vec<_>>();
            let lit = rows(check).iter().filter(|&&bits| bits != 0).count();
            assert!(lit >= font.height / 3, "{name}: \u{2713} is {lit} rows high");
            assert_ne!(rows(check), rows(question), "{name}: \u{2713} is drawn as ?");
            // Its top right and its bottom left of centre, as a tick.
            let top = rows(check).into_iter().position(|bits| bits != 0).unwrap();
            assert!(check.pixel(font.width - 2, top), "{name}: \u{2713} starts at the right");
        }
    }

    #[test]
    fn every_font_draws_arrows_and_lists_what_it_draws() {
        for (name, _) in FONTS {
            let font = builtin_font(name).unwrap();
            let question = font.glyph('?');
            let mut chars = Vec::new();
            font.each_char(|c| chars.push(c));
            for c in ['\u{2191}', '\u{2193}', '\u{2713}', 'A', '\u{2500}'] {
                assert!(chars.contains(&c), "{name} does not list {c}");
                let drawn = (0..font.height).map(|y| font.glyph(c).row_bits(y));
                assert!(!drawn.eq((0..font.height).map(|y| question.row_bits(y))), "{name}: {c}");
            }
        }
    }

    #[test]
    fn every_font_draws_half_blocks_and_lines() {
        for (name, _) in FONTS {
            let font = builtin_font(name).unwrap();
            let full = u32::MAX >> (32 - font.width);
            let (top, bottom) = (0, font.height - 1);
            let upper = font.glyph('\u{2580}');
            assert_eq!(
                (upper.row_bits(top), upper.row_bits(bottom)),
                (full, 0),
                "{name}: \u{2580}"
            );
            let lower = font.glyph('\u{2584}');
            assert_eq!(
                (lower.row_bits(top), lower.row_bits(bottom)),
                (0, full),
                "{name}: \u{2584}"
            );
            let block = font.glyph('\u{2588}');
            assert_eq!(
                (block.row_bits(top), block.row_bits(bottom)),
                (full, full),
                "{name}: \u{2588}"
            );
            let line = font.glyph('\u{2500}');
            let lit = (0..font.height).filter(|&y| line.row_bits(y) != 0).count();
            assert!(lit > 0 && lit <= font.height / 8, "{name}: \u{2500} is {lit} rows thick");
        }
    }

    #[test]
    fn clears_and_centres_the_grid() {
        let console = console(804, 600);
        assert_eq!((console.cols(), console.rows()), (100, 37));
        assert_eq!(console.origin(), (2, 4));
        assert_eq!(pixel(&console, 0, 0), 0);
        assert_eq!(pixel(&console, 803, 599), 0);
    }

    #[test]
    fn draws_glyphs_in_style_colours() {
        let mut console = console(640, 480);
        console.write("A", Style::Error);
        console.flush();
        let font = builtin_font("8x16").unwrap();
        let glyph = font.glyph('A');
        let red = PixelFormat::XRGB8888.encode(0xff, 0x60, 0x60);
        for y in 0..16 {
            for x in 0..8 {
                let expected = if glyph.pixel(x, y) { red } else { 0 };
                assert_eq!(pixel(&console, x, y), expected, "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn wraps_scrolls_and_redraws_only_changes() {
        let mut console = console(16 * 8, 4 * 16);
        console.write("0123456789abcdefXY\n", Style::Plain);
        assert_eq!(text(&console, 0), "0123456789abcdef");
        assert_eq!(text(&console, 1), "XY");
        assert_eq!(console.position(), (0, 2));
        console.flush();
        assert_eq!(console.cells_drawn(), 18);
        console.flush();
        assert_eq!(console.cells_drawn(), 18, "nothing changed");

        console.write("two\nthree\nfour", Style::Plain);
        assert_eq!(text(&console, 0), "XY");
        assert_eq!(text(&console, 3), "four");
        console.write("\rFO\tx", Style::Plain);
        assert_eq!(text(&console, 3), "FOur    x");
    }

    #[test]
    fn exact_width_lines_do_not_leave_blank_lines() {
        let mut console = console(16 * 8, 4 * 16);
        console.write("0123456789abcdef\nnext", Style::Plain);
        assert_eq!(text(&console, 1), "next");
    }

    #[test]
    fn banner_rows_keep_their_background() {
        let mut console = console(16 * 8, 4 * 16);
        console.fill_row(0, Style::Banner);
        console.set_position(1, 0);
        console.write("PANIC", Style::Banner);
        console.flush();
        let band = PixelFormat::XRGB8888.encode(0xa0, 0x10, 0x10);
        assert_eq!(pixel(&console, 0, 0), band);
        assert_eq!(pixel(&console, 127, 15), band);
        assert_eq!(pixel(&console, 0, 16), 0);
    }

    #[test]
    fn repaints_over_what_others_drew() {
        let mut console = console(16 * 8, 4 * 16);
        console.write("A", Style::Plain);
        console.flush();
        let drawn = console.cells_drawn();
        console.pixels_mut().unwrap().fill(0x77);
        console.repaint();
        assert_eq!(console.cells_drawn(), drawn + 64, "every cell again");
        assert_ne!(pixel(&console, 0, 0) & 0xff, 0x77);
        assert_eq!(pixel(&console, 127, 63), 0);
    }

    #[test]
    fn the_startup_screen_hides_the_text_and_fills_its_bar() {
        let mut console = console(640, 480);
        console.write("a line of the log", Style::Plain);
        console.flush();
        console.draw_splash("Dukovo", "Esc shows the messages", 250);
        let slate = PixelFormat::XRGB8888.encode(0x15, 0x18, 0x1c);
        let (sand, track) = (
            PixelFormat::XRGB8888.encode(0xe9, 0xc4, 0x8b),
            PixelFormat::XRGB8888.encode(0x2c, 0x31, 0x38),
        );
        assert_eq!(pixel(&console, 0, 0), slate, "the log is hidden");
        assert_eq!(pixel(&console, 639, 479), slate);
        let (x, y, width, height) = console.splash_bar();
        assert_eq!((width, height), (240, 5));
        assert_eq!(pixel(&console, x, y), sand);
        assert_eq!(pixel(&console, x + width / 4 - 1, y + height - 1), sand);
        assert_eq!(pixel(&console, x + width / 4, y), track);
        assert_eq!(pixel(&console, x + width - 1, y), track);
        // The name, in cream, somewhere above the bar.
        let cream = PixelFormat::XRGB8888.encode(0xd9, 0xd4, 0xca);
        assert!((0..y).any(|row| (0..640).any(|col| pixel(&console, col, row) == cream)));
        console.draw_progress(1000);
        assert_eq!(pixel(&console, x + width - 1, y), sand);
        // The grid kept the log, which a repaint shows again.
        assert_eq!(text(&console, 0), "a line of the log");
        console.repaint();
        assert_eq!(pixel(&console, 0, 0) & 0xff_ffff, 0);
    }

    #[test]
    fn the_startup_screen_fits_a_small_screen() {
        let mut console = console(16 * 8, 4 * 16);
        console.draw_splash("Dukovo", "Esc shows the messages", 1000);
        let (x, y, width, height) = console.splash_bar();
        assert!(x + width <= 128 && y + height <= 64);
    }

    /// A console with no framebuffer for a screen of `width` by `height`.
    fn unseen(width: usize, height: usize, name: &str) -> Console {
        let font = builtin_font(name).unwrap();
        let cells = storage_cells(width, height, &font);
        Console::without_framebuffer((width, height), &font, storage(cells)).unwrap()
    }

    /// Draws all that waits of `console`, which has no framebuffer, on
    /// `image`, its screen in rows of four bytes a pixel, as whoever
    /// sends its pixels does, and returns what was drawn.
    fn send(console: &mut Console, text: bool, image: &mut [u8]) -> Vec<Draw> {
        let paint = console.painter();
        let (width, _) = paint.screen();
        let mut cells = vec![0u32; console.cols()];
        let mut draws = Vec::new();
        while let Some(draw) = console.next_draw(text, &mut cells) {
            let (x, y, wide, high) = paint.area(&draw);
            // What a row held before is never left in it.
            let mut row = vec![0xaa; wide * 4];
            for index in 0..high {
                paint.row(&draw, &cells, index, &mut row);
                let at = ((y + index) * width + x) * 4;
                image[at..at + wide * 4].copy_from_slice(&row);
            }
            draws.push(draw);
        }
        draws
    }

    /// The pixels a console's framebuffer shows, row after row.
    fn shown(console: &Console) -> Vec<u8> {
        let fb = console.framebuffer().unwrap();
        let rows = (0..fb.height).map(|y| &fb.memory[y * fb.pitch..][..fb.width * 4]);
        rows.flatten().copied().collect()
    }

    /// Runs `step` on a console with a framebuffer and on one without,
    /// and checks that the pixels sent for the second are those the
    /// first drew.
    fn both(pair: &mut (Console, Console, Vec<u8>), text: bool, step: impl Fn(&mut Console)) {
        step(&mut pair.0);
        step(&mut pair.1);
        send(&mut pair.1, text, &mut pair.2);
        assert!(pair.2 == shown(&pair.0), "the pixels sent are not those drawn");
    }

    #[test]
    fn a_console_with_no_framebuffer_sends_the_pixels_one_with_draws() {
        for (width, height, name) in [(640, 480, "8x16"), (805, 603, "12x24"), (1300, 700, "16x32")]
        {
            let font = builtin_font(name).unwrap();
            let cells = storage_cells(width, height, &font);
            let drawn = Console::new(framebuffer(width, height), font, storage(cells)).unwrap();
            let mut pair = (drawn, unseen(width, height, name), vec![0x55; width * height * 4]);
            assert_eq!(pair.0.cols(), pair.1.cols());
            assert_eq!((pair.0.rows(), pair.0.origin()), (pair.1.rows(), pair.1.origin()));
            both(&mut pair, true, |_| {});
            // Enough lines to scroll the grid.
            for line in 0..pair.0.rows() + 5 {
                let styles = [Style::Plain, Style::Error, Style::Warning, Style::Debug];
                both(&mut pair, true, |console| {
                    let text =
                        format!("{line}: a line of the log \u{2713}\t\u{2588} and its end\n");
                    console.write(&text, styles[line % 4]);
                    console.flush();
                });
            }
            both(&mut pair, true, |console| {
                console.fill_row(0, Style::Banner);
                console.set_position(1, 0);
                console.write("RADIX KERNEL PANIC", Style::Banner);
                console.write("\rover", Style::Emphasis);
                console.flush();
            });
            both(&mut pair, true, Console::repaint);
            both(&mut pair, true, Console::clear);
        }
    }

    #[test]
    fn the_startup_screen_is_sent_as_it_is_drawn() {
        for (width, height, name) in [(640, 480, "8x16"), (1283, 801, "12x24"), (130, 70, "8x16")] {
            let font = builtin_font(name).unwrap();
            let cells = storage_cells(width, height, &font);
            let drawn = Console::new(framebuffer(width, height), font, storage(cells)).unwrap();
            let mut pair = (drawn, unseen(width, height, name), vec![0x55; width * height * 4]);
            both(&mut pair, false, |console| {
                console.write("a line of the log\n", Style::Plain);
                console.draw_splash("Dukovo", "Esc shows the messages", 250);
            });
            for per_mille in [0, 1, 500, 999, 1000, 4000] {
                both(&mut pair, false, |console| console.draw_progress(per_mille));
            }
            // The log is kept behind it, and shows when it goes.
            both(&mut pair, false, |console| console.write("unseen\n", Style::Error));
            both(&mut pair, true, Console::repaint);
            both(&mut pair, false, |console| {
                console.draw_splash("A longer name than a third of it holds", "", 1000);
            });
        }
    }

    #[test]
    fn only_what_changed_is_asked_for() {
        let mut console = unseen(640, 480, "8x16");
        let mut cells = [0u32; 80];
        assert_eq!(console.next_draw(true, &mut cells), Some(Draw::Blank));
        assert_eq!(console.next_draw(true, &mut cells), None);
        console.write("abc", Style::Plain);
        // The startup screen shows: the text waits behind it.
        assert_eq!(console.next_draw(false, &mut cells), None);
        assert_eq!(
            console.next_draw(true, &mut cells),
            Some(Draw::Cells { row: 0, first: 0, count: 3 })
        );
        assert_eq!(cells[..3], [u32::from('a'), u32::from('b'), u32::from('c')]);
        assert_eq!(console.next_draw(true, &mut cells), None);
        // The same letters again change nothing.
        console.write("\rabc", Style::Plain);
        assert_eq!(console.next_draw(true, &mut cells), None);
        // Two changes far apart in a row are one stretch of it.
        console.write("\rAbc\t\tx", Style::Plain);
        assert_eq!(
            console.next_draw(true, &mut cells),
            Some(Draw::Cells { row: 0, first: 0, count: 17 })
        );
        assert_eq!(console.next_draw(true, &mut cells), None);
        // A stretch longer than there is room for comes in parts.
        console.write("\r0123456789", Style::Plain);
        let part = |console: &mut Console| console.next_draw(true, &mut [0; 4]);
        assert_eq!(part(&mut console), Some(Draw::Cells { row: 0, first: 0, count: 4 }));
        assert_eq!(part(&mut console), Some(Draw::Cells { row: 0, first: 4, count: 4 }));
        assert_eq!(part(&mut console), Some(Draw::Cells { row: 0, first: 8, count: 2 }));
        assert_eq!(part(&mut console), None);
        assert_eq!(console.next_draw(true, &mut []), None);
        assert_eq!(console.cells_drawn(), 0, "it draws nothing itself");
    }

    #[test]
    fn the_search_for_changes_goes_on_from_the_row_it_found_one_in() {
        let mut console = unseen(640, 480, "8x16");
        let mut cells = [0u32; 80];
        let mut rows = |console: &mut Console| {
            let mut rows = Vec::new();
            while let Some(draw) = console.next_draw(true, &mut cells) {
                match draw {
                    Draw::Cells { row, .. } => rows.push(row),
                    other => assert_eq!(other, Draw::Blank),
                }
            }
            rows
        };
        let write = |console: &mut Console, row: usize, text: &str| {
            console.set_position(0, row);
            console.write(text, Style::Plain);
        };
        write(&mut console, 5, "five");
        write(&mut console, 2, "two");
        assert_eq!(rows(&mut console), [2, 5]);
        write(&mut console, 1, "one");
        write(&mut console, 5, "FIVE");
        write(&mut console, 29, "last");
        assert_eq!(rows(&mut console), [5, 29, 1]);
    }

    #[test]
    fn what_waits_whole_replaces_what_waited() {
        let mut console = unseen(640, 480, "8x16");
        let mut cells = [0u32; 80];
        console.draw_progress(100);
        console.draw_splash("Dukovo", "hint", 200);
        console.draw_progress(300);
        // The bar comes with the startup screen, where it is by then.
        let splash = Draw::Splash { title: "Dukovo", hint: "hint", per_mille: 300 };
        assert_eq!(console.next_draw(false, &mut cells), Some(splash));
        assert_eq!(console.next_draw(false, &mut cells), None);
        console.draw_progress(400);
        assert_eq!(console.next_draw(false, &mut cells), Some(Draw::Bar(400)));
        console.draw_progress(500);
        console.write("log", Style::Plain);
        console.repaint();
        assert_eq!(console.next_draw(true, &mut cells), Some(Draw::Blank));
        assert_eq!(
            console.next_draw(true, &mut cells),
            Some(Draw::Cells { row: 0, first: 0, count: 3 })
        );
        assert_eq!(console.next_draw(true, &mut cells), None);
        // One that draws on a framebuffer has nothing to ask for.
        let mut drawn = super::tests::console(640, 480);
        drawn.write("log", Style::Plain);
        assert_eq!(drawn.next_draw(true, &mut cells), None);
        assert_eq!(drawn.painter().screen(), (640, 480));
    }

    #[test]
    fn the_storage_comes_back() {
        let font = builtin_font("8x16").unwrap();
        let console = Console::without_framebuffer((640, 480), &font, storage(10_000)).unwrap();
        assert!(console.framebuffer().is_none());
        let again = console.into_storage();
        assert_eq!(again.len(), 10_000);
        // A screen too small for text gives it back at once.
        let small = Console::without_framebuffer((64, 480), &font, again).unwrap_err();
        assert_eq!(small.len(), 10_000);
        let mut console = Console::without_framebuffer((1280, 800), &font, small).unwrap();
        assert_eq!((console.cols(), console.rows()), (160, 31), "as many rows as it has cells for");
        assert!(console.pixels_mut().is_none());
    }

    #[test]
    fn small_storage_limits_rows() {
        let font = builtin_font("8x16").unwrap();
        let console = Console::new(framebuffer(640, 480), font, storage(2 * 80 * 10)).unwrap();
        assert_eq!((console.cols(), console.rows()), (80, 10));
        let tiny = Console::new(framebuffer(64, 480), font, storage(10_000));
        assert!(tiny.is_none());
    }
}
