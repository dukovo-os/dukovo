// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The boxes windowd shows over the windows: the window menu and the
//! list of windows.
//!
//! A panel is lines one under another, some set apart by a line across.
//! [`place`] says how large a panel of given entries is and where each
//! entry lies in it, from the panel's own top left corner; [`draw`]
//! draws it. What a line does when it is chosen is windowd's to know.

use crate::canvas::{Canvas, Rect, all_round};
use crate::draw::{font_of, icon, middle, tick, words};
use crate::theme::Theme;

/// What stands at the left of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Nothing.
    None,
    /// A tick: what the line names is on.
    Tick,
    /// The icon of a window.
    Icon,
}

/// One entry of a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry<'a> {
    /// A line the person can choose, unless it is off.
    Line {
        /// What it says.
        words: &'a str,
        /// The key that does the same, or nothing.
        key: &'a str,
        /// What stands at its left.
        mark: Mark,
        /// Whether it cannot be chosen now.
        off: bool,
    },
    /// The room between two groups of lines, with a line across.
    Gap,
}

impl Entry<'_> {
    /// Whether the person can choose the entry.
    pub fn can_be_chosen(&self) -> bool {
        matches!(self, Entry::Line { off: false, .. })
    }
}

/// How large a panel is, and where its entries are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// The panel's width and height.
    pub size: (i32, i32),
    /// Each entry's rectangle, in the entries' order.
    pub entries: Vec<Rect>,
}

impl Placed {
    /// The entry the point (`x`, `y`) is in, if any.
    pub fn entry_at(&self, x: i32, y: i32) -> Option<usize> {
        self.entries.iter().position(|entry| entry.contains(x, y))
    }
}

/// Places `entries` in a panel `width` wide.
pub fn place(theme: &Theme, width: i32, entries: &[Entry<'_>]) -> Placed {
    let panel = &theme.panel;
    // The line around the panel is 1.
    let edge = 1 + panel.padding;
    let mut top = edge;
    let entries = entries
        .iter()
        .map(|entry| {
            let rows = match entry {
                Entry::Line { .. } => panel.row,
                Entry::Gap => panel.gap,
            };
            let rect = Rect::new(edge, top, width - 2 * edge, rows);
            top += rows;
            rect
        })
        .collect();
    Placed { size: (width, top + edge), entries }
}

/// Draws the panel of `entries`, `width` wide, with the entry `chosen`
/// shown as the one the pointer or the keys are on, where it lies in
/// `canvas`. What the canvas held is gone.
pub fn draw(
    theme: &Theme,
    width: i32,
    entries: &[Entry<'_>],
    chosen: Option<usize>,
    canvas: &mut Canvas<'_>,
) {
    let panel = &theme.panel;
    let placed = place(theme, width, entries);
    let around = Rect::new(0, 0, placed.size.0, placed.size.1);
    canvas.clear();
    canvas.round(around, all_round(panel.radius), |_| panel.line);
    canvas.round(around.grown(-1), all_round(panel.radius - 1), |y| {
        panel.fill.at(y - 1, around.h - 2)
    });
    let font = font_of(theme);
    for (index, (entry, rect)) in entries.iter().zip(&placed.entries).enumerate() {
        let Entry::Line { words: said, key, mark, off } = entry else {
            let line = Rect::new(
                rect.x + panel.gap_inset,
                rect.y + rect.h / 2,
                rect.w - 2 * panel.gap_inset,
                1,
            );
            canvas.fill(line, panel.line);
            continue;
        };
        if chosen == Some(index) && !off {
            canvas.round(*rect, all_round(panel.row_radius), |_| panel.chosen_line);
            canvas.round(rect.grown(-1), all_round(panel.row_radius - 1), |_| panel.chosen);
        }
        let colour = if *off { panel.words_off } else { panel.words };
        let room = Rect::new(rect.x, rect.y, panel.indent, rect.h);
        match mark {
            Mark::None => {}
            Mark::Tick => tick(canvas, theme, room, colour),
            Mark::Icon => {
                let side = theme.icon_side;
                icon(canvas, theme, (room.x + (room.w - side) / 2, room.y + (room.h - side) / 2));
            }
        }
        let Some(font) = &font else { continue };
        let row = middle(font, rect.y, rect.h);
        let key_width = key.chars().count() as i32 * font.width as i32;
        let key_from = rect.right() - panel.margin - key_width;
        words(canvas, font, (key_from, row), key_width, key, panel.key, None);
        let from = rect.x + panel.indent;
        let room = key_from - if key.is_empty() { 0 } else { panel.margin } - from;
        words(canvas, font, (from, row), room, said, colour, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::PIXEL;
    use crate::draw::tests::at;

    const MENU: [Entry<'static>; 5] = [
        Entry::Line { words: "Restore", key: "", mark: Mark::None, off: true },
        Entry::Line { words: "Move", key: "", mark: Mark::None, off: false },
        Entry::Gap,
        Entry::Line { words: "Use the full title bar", key: "", mark: Mark::Tick, off: false },
        Entry::Line { words: "Close", key: "Alt+F4", mark: Mark::None, off: false },
    ];

    fn drawn(chosen: Option<usize>) -> (Rect, Vec<u8>) {
        let theme = Theme::basic();
        let placed = place(&theme, 330, &MENU);
        let rect = Rect::new(0, 0, placed.size.0, placed.size.1);
        let mut pixels = vec![0x55; Canvas::bytes_of(&rect)];
        draw(&theme, 330, &MENU, chosen, &mut Canvas::over(rect, &mut pixels));
        (rect, pixels)
    }

    fn rgb(value: u32) -> [u8; 4] {
        [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
    }

    #[test]
    fn lines_are_thirty_high_and_a_gap_eleven_inside_the_padding() {
        let placed = place(&Theme::basic(), 330, &MENU);
        assert_eq!(placed.size, (330, 12 + 4 * 30 + 11));
        assert_eq!(placed.entries[0], Rect::new(6, 6, 318, 30));
        assert_eq!(placed.entries[2], Rect::new(6, 66, 318, 11));
        assert_eq!(placed.entries[4], Rect::new(6, 107, 318, 30));
        assert_eq!(placed.entry_at(6, 6), Some(0));
        assert_eq!(placed.entry_at(323, 136), Some(4));
        assert_eq!(placed.entry_at(5, 6), None);
        assert_eq!(placed.entry_at(100, 70), Some(2));
        assert!(!MENU[0].can_be_chosen() && MENU[1].can_be_chosen() && !MENU[2].can_be_chosen());
    }

    #[test]
    fn the_panel_is_a_lined_box_with_its_words() {
        let (rect, pixels) = drawn(None);
        let at = |x, y| at(&pixels, rect, x, y);
        assert_eq!(at(0, 0), [0; 4]);
        assert_eq!(at(0, 70), rgb(0x3a_5677));
        assert_eq!(at(100, 1), rgb(0x1e_3a5b));
        assert_eq!(at(100, 141), rgb(0x17_2e4a));
        assert_eq!(at(100, 142), rgb(0x3a_5677));
        // The line across a gap, in from the sides.
        assert_eq!(at(10, 71), rgb(0x3a_5677));
        assert_ne!(at(9, 71), rgb(0x3a_5677));
        // Words in their colours: a line that is off is greyer, a key
        // greyer than a line.
        let has = |row: usize, colour: u32, from: i32, to: i32| {
            let top = [6, 36, 66, 77, 107][row];
            (from..to).any(|x| (top..top + 30).any(|y| at(x, y) == rgb(colour)))
        };
        assert!(has(0, 0x82_96ae, 40, 100));
        assert!(has(1, 0xe8_f0fa, 40, 100));
        assert!(has(4, 0xa2_b5cc, 260, 314));
        assert!(!has(4, 0xa2_b5cc, 40, 260));
        // The tick is in the room left of the words; no other line has
        // anything there.
        assert!(has(3, 0xe8_f0fa, 6, 40));
        assert!(!has(1, 0xe8_f0fa, 6, 40));
    }

    #[test]
    fn the_chosen_line_is_lit_and_a_line_that_is_off_is_not() {
        let (rect, plain) = drawn(None);
        let (_, chosen) = drawn(Some(1));
        assert_eq!(at(&chosen, rect, 8, 37 + 14), rgb(0x2b_5c97));
        assert_eq!(at(&chosen, rect, 6, 36 + 14), rgb(0x6c_b2ff));
        let differ = plain
            .as_chunks::<PIXEL>()
            .0
            .iter()
            .zip(chosen.as_chunks::<PIXEL>().0.iter())
            .enumerate()
            .filter(|(_, (one, other))| one != other)
            .map(|(index, _)| index as i32 / 330);
        assert!(differ.clone().count() > 0);
        assert!(differ.into_iter().all(|row| (36..66).contains(&row)));
        let (_, off) = drawn(Some(0));
        assert_eq!(off, plain);
    }
}
