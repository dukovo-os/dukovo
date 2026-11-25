// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Cells to pixels: which cells of the grid changed, how each looks, and
//! the rows of FORMAT_XRGB8888 pixels that show them.

use radix_fbcon::Font;

use crate::term::{BOLD, Cell, DIM, Palette, REVERSE, UNDERLINE, WIDE_TAIL};

/// The bytes of a pixel of colour (`r`, `g`, `b`): blue in the lowest
/// byte, the top byte unused.
pub fn pixel((r, g, b): (u8, u8, u8)) -> [u8; 4] {
    [b, g, r, 0]
}

/// The cells of one row of the grid that changed: columns `from` to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// The row.
    pub y: usize,
    /// The first column.
    pub from: usize,
    /// The last column.
    pub to: usize,
}

/// What the grid showed when it was last drawn.
#[derive(Clone, Debug)]
pub struct Drawn {
    cols: usize,
    /// What each cell shows; `None` for one to draw whatever it holds.
    cells: Vec<Option<Cell>>,
    /// Where the cursor is drawn.
    cursor: Option<(usize, usize)>,
}

impl Drawn {
    /// A grid of `cols` by `rows` with nothing drawn yet.
    pub fn new(cols: usize, rows: usize) -> Drawn {
        Drawn { cols, cells: vec![None; cols * rows], cursor: None }
    }

    /// Forgets what is drawn, so that every cell is drawn again.
    pub fn forget(&mut self) {
        self.cells.fill(None);
        self.cursor = None;
    }

    /// Takes in what the grid is to show now, `shown` for each cell and
    /// the cursor at `cursor`, and returns what differs from what is
    /// drawn: for each row, the columns from the first cell that changed
    /// to the last. A cursor that moved changes the cell it left and the
    /// cell it is on.
    pub fn changed(
        &mut self,
        shown: impl Fn(usize, usize) -> Cell,
        cursor: Option<(usize, usize)>,
    ) -> Vec<Span> {
        let rows = self.cells.len().checked_div(self.cols).unwrap_or(0);
        let moved = cursor != self.cursor;
        let mut spans = Vec::new();
        for y in 0..rows {
            let mut span: Option<(usize, usize)> = None;
            let mut add = |x: usize| {
                span = Some(span.map_or((x, x), |(from, to)| (from.min(x), to.max(x))));
            };
            for x in 0..self.cols {
                let cell = Some(shown(x, y));
                let was = &mut self.cells[y * self.cols + x];
                if *was != cell {
                    *was = cell;
                    add(x);
                }
            }
            if moved {
                for (x, _) in [self.cursor, cursor].into_iter().flatten().filter(|at| at.1 == y) {
                    if x < self.cols {
                        add(x);
                    }
                }
            }
            if let Some((from, to)) = span {
                spans.push(Span { y, from, to });
            }
        }
        self.cursor = cursor;
        spans
    }
}

/// How one cell is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// Its character; `None` for the second half of a wide one, which
    /// shows its background alone.
    pub glyph: Option<char>,
    /// The pixel of its character.
    pub ink: [u8; 4],
    /// The pixel behind it.
    pub paper: [u8; 4],
    /// Whether its last rows are a line under it.
    pub underline: bool,
    /// Whether the cursor is on it.
    pub caret: bool,
}

impl Look {
    /// How `cell` looks in `palette`: bold brightens the first eight
    /// colours, dim halves the character's, and the cell's reverse and
    /// the screen's (`reverse`) each swap the two. `caret` puts the
    /// cursor on it.
    pub fn of(cell: Cell, palette: &Palette, reverse: bool, caret: bool) -> Look {
        let (mut fg, mut bg) = (cell.fg, cell.bg);
        if cell.flags & BOLD != 0 && fg < 8 {
            fg += 8;
        }
        if (cell.flags & REVERSE != 0) != reverse {
            (fg, bg) = (bg, fg);
        }
        let ink = match palette[usize::from(fg)] {
            (r, g, b) if cell.flags & DIM != 0 => pixel((r / 2, g / 2, b / 2)),
            colour => pixel(colour),
        };
        Look {
            glyph: (cell.flags & WIDE_TAIL == 0).then_some(cell.c),
            ink,
            paper: pixel(palette[usize::from(bg)]),
            underline: cell.flags & UNDERLINE != 0,
            caret,
        }
    }
}

/// Draws `looks`, cells side by side, into `out` in `font` at `scale`
/// times its size: as many rows as a cell is tall, each as wide as the
/// cells together, four bytes a pixel. The underline is the cell's last
/// row of the font and the cursor its last two, as the kernel's console
/// draws them.
pub fn paint(font: &Font<'_>, scale: usize, looks: &[Look], out: &mut [u8]) {
    let (width, height) = (font.width * scale, font.height * scale);
    if looks.is_empty() || width == 0 {
        return;
    }
    let glyphs: Vec<_> = looks.iter().map(|look| look.glyph.map(|c| font.glyph(c))).collect();
    let rows = out.chunks_exact_mut(looks.len() * width * 4).take(height);
    for (y, row) in rows.enumerate() {
        let cells = looks.iter().zip(&glyphs).zip(row.chunks_exact_mut(width * 4));
        for ((look, glyph), cell) in cells {
            let bits = glyph.as_ref().map_or(0, |glyph| glyph.row_bits(y / scale));
            let line =
                (look.underline && y + scale >= height) || (look.caret && y + 2 * scale >= height);
            for (x, px) in cell.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let lit = line || bits & (1 << (font.width - 1 - x / scale)) != 0;
                *px = if lit { look.ink } else { look.paper };
            }
        }
    }
}

/// The parts of a screen of `screen` pixels that a grid of `grid` pixels
/// at `origin` leaves bare, as (x, y, width, height): above it, below
/// it, left of it and right of it. One of no size is left out.
pub fn margins(
    screen: (usize, usize),
    origin: (usize, usize),
    grid: (usize, usize),
) -> impl Iterator<Item = (usize, usize, usize, usize)> {
    let (right, bottom) = (origin.0 + grid.0, origin.1 + grid.1);
    [
        (0, 0, screen.0, origin.1),
        (0, bottom, screen.0, screen.1.saturating_sub(bottom)),
        (0, origin.1, origin.0, grid.1),
        (right, origin.1, screen.0.saturating_sub(right), grid.1),
    ]
    .into_iter()
    .filter(|&(_, _, width, height)| width > 0 && height > 0)
}

#[cfg(test)]
mod tests {
    use radix_fbcon::builtin_font;

    use super::*;
    use crate::term::default_palette;

    fn cell(c: char) -> Cell {
        Cell { c, ..Cell::BLANK }
    }

    #[test]
    fn only_the_cells_that_changed_are_spans() {
        let mut drawn = Drawn::new(8, 3);
        let blank = |_: usize, _: usize| Cell::BLANK;
        // Nothing drawn yet: every row whole.
        let all = drawn.changed(blank, None);
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|span| (span.from, span.to) == (0, 7)));
        assert!(drawn.changed(blank, None).is_empty());
        // Two cells of the second row, with one between that did not.
        let typed =
            |x: usize, y: usize| if y == 1 && (x == 2 || x == 4) { cell('a') } else { Cell::BLANK };
        assert_eq!(drawn.changed(typed, None), [Span { y: 1, from: 2, to: 4 }]);
        assert!(drawn.changed(typed, None).is_empty());
        drawn.forget();
        assert_eq!(drawn.changed(typed, None).len(), 3);
    }

    #[test]
    fn a_cursor_that_moves_changes_where_it_was_and_where_it_is() {
        let mut drawn = Drawn::new(8, 3);
        let blank = |_: usize, _: usize| Cell::BLANK;
        drawn.changed(blank, Some((1, 0)));
        assert_eq!(drawn.cursor, Some((1, 0)));
        // Along its row, to another row, and away.
        assert_eq!(drawn.changed(blank, Some((5, 0))), [Span { y: 0, from: 1, to: 5 }]);
        assert_eq!(
            drawn.changed(blank, Some((0, 2))),
            [Span { y: 0, from: 5, to: 5 }, Span { y: 2, from: 0, to: 0 }]
        );
        assert_eq!(drawn.changed(blank, None), [Span { y: 2, from: 0, to: 0 }]);
        assert!(drawn.changed(blank, None).is_empty());
        // A cursor past the grid, as a terminal wider than the screen
        // may have, changes nothing.
        assert!(drawn.changed(blank, Some((9, 1))).is_empty());
    }

    #[test]
    fn a_cells_look_follows_its_flags() {
        let palette = default_palette();
        let plain = Look::of(cell('a'), &palette, false, false);
        assert_eq!(plain.glyph, Some('a'));
        assert_eq!(plain.ink, pixel(palette[usize::from(Cell::BLANK.fg)]));
        assert_eq!(plain.paper, pixel(palette[usize::from(Cell::BLANK.bg)]));
        // The cell's reverse and the screen's undo each other.
        let reversed = Cell { flags: REVERSE, ..cell('a') };
        assert_eq!(Look::of(reversed, &palette, false, false).ink, plain.paper);
        assert_eq!(Look::of(reversed, &palette, true, false).ink, plain.ink);
        let bold = Cell { fg: 1, flags: BOLD, ..cell('a') };
        assert_eq!(Look::of(bold, &palette, false, false).ink, pixel(palette[9]));
        let dim = Cell { fg: 7, flags: DIM, ..cell('a') };
        let (r, g, b) = palette[7];
        assert_eq!(Look::of(dim, &palette, false, false).ink, pixel((r / 2, g / 2, b / 2)));
        let tail = Cell { flags: WIDE_TAIL | UNDERLINE, ..cell('a') };
        let look = Look::of(tail, &palette, false, true);
        assert_eq!((look.glyph, look.underline, look.caret), (None, true, true));
        assert_eq!(pixel((0x12, 0x34, 0x56)), [0x56, 0x34, 0x12, 0]);
    }

    #[test]
    fn a_span_is_painted_cell_by_cell() {
        let font = builtin_font("8x16").unwrap();
        let (ink, paper) = ([1, 1, 1, 0], [2, 2, 2, 0]);
        let look = |glyph: Option<char>, underline: bool, caret: bool| Look {
            glyph,
            ink,
            paper,
            underline,
            caret,
        };
        let looks =
            [look(Some('|'), false, false), look(None, true, false), look(Some(' '), false, true)];
        let mut out = vec![0xff; 3 * 8 * 16 * 4];
        paint(&font, 1, &looks, &mut out);
        let at = |x: usize, y: usize| &out[(y * 24 + x) * 4..][..4];
        // The bar's cell has ink in it and paper beside it.
        assert!((0..8).any(|x| at(x, 8) == ink) && (0..8).any(|x| at(x, 8) == paper));
        // The wide character's second half is paper, with its line on
        // the last row only.
        assert!((8..16).all(|x| at(x, 14) == paper) && (8..16).all(|x| at(x, 15) == ink));
        // The cursor is the last two rows.
        assert!((16..24).all(|x| at(x, 13) == paper));
        assert!((16..24).all(|x| at(x, 14) == ink && at(x, 15) == ink));
        // Twice the size: each of the font's pixels is two by two.
        let mut doubled = vec![0xff; 16 * 32 * 4];
        paint(&font, 2, &looks[..1], &mut doubled);
        let twice = |x: usize, y: usize| &doubled[(y * 16 + x) * 4..][..4];
        for (x, y) in (0..8).flat_map(|x| (0..16).map(move |y| (x, y))) {
            let one = at(x, y);
            assert!(
                [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .all(|(dx, dy)| twice(2 * x + dx, 2 * y + dy) == one)
            );
        }
    }

    #[test]
    fn the_margins_are_what_the_grid_leaves_bare() {
        // A grid that fills the screen leaves none.
        assert_eq!(margins((1920, 1080), (0, 0), (1920, 1080)).count(), 0);
        // 1280 by 800 in 8x16 is 160 by 50 cells: no margin; in 12x24 it
        // is 106 by 33, with 4 pixels on each side.
        let all: Vec<_> = margins((1280, 800), (4, 4), (1272, 792)).collect();
        assert_eq!(all, [(0, 0, 1280, 4), (0, 796, 1280, 4), (0, 4, 4, 792), (1276, 4, 4, 792)]);
        // Together with the grid they are the whole screen.
        let area: usize = all.iter().map(|&(_, _, w, h)| w * h).sum();
        assert_eq!(area + 1272 * 792, 1280 * 800);
    }
}
