// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A box windowd shows over the windows: the window menu, or the list
//! of windows.
//!
//! A [`Panel`] is lines one under another, one of which may be chosen.
//! The pointer and the keys choose; what a chosen line does is its
//! owner's to know, by the line's place among the rows. The panel's
//! look and the lines' places are the frame library's
//! ([`window_frame::panel`]).

use std::convert::Infallible;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Transform};
use window_frame::panel::{self, Entry, Mark, Placed};
use window_frame::{Canvas, Rect, Theme};

/// One row of a panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A line the person can choose, unless it is off.
    Line {
        /// What it says.
        words: String,
        /// The key that does the same, or nothing.
        key: &'static str,
        /// What stands at its left.
        mark: Mark,
        /// Whether it cannot be chosen now.
        off: bool,
    },
    /// The room between two groups of lines.
    Gap,
}

impl Row {
    fn entry(&self) -> Entry<'_> {
        match self {
            Row::Line { words, key, mark, off } => {
                Entry::Line { words, key, mark: *mark, off: *off }
            }
            Row::Gap => Entry::Gap,
        }
    }
}

/// A panel on the desktop.
#[derive(Debug)]
pub struct Panel {
    at: Point<i32, Logical>,
    width: i32,
    /// How high the panel may be: its screen's height.
    room: i32,
    rows: Vec<Row>,
    /// The first row that shows. A panel with more rows than its screen
    /// has room for shows as many as fit, the chosen one among them.
    first: usize,
    /// Where the rows that show lie.
    placed: Placed,
    chosen: Option<usize>,
    pixels: MemoryRenderBuffer,
}

impl Panel {
    /// A panel of `rows`, `width` wide, with the row `chosen` chosen,
    /// its top left corner as near to `at` as leaves it whole on
    /// `screen`.
    pub fn new(
        theme: &Theme,
        width: i32,
        rows: Vec<Row>,
        chosen: Option<usize>,
        at: Point<i32, Logical>,
        screen: Rectangle<i32, Logical>,
    ) -> Panel {
        let width = width.min(screen.size.w).max(1);
        let pixels = MemoryRenderBuffer::new(Fourcc::Argb8888, (1, 1), 1, Transform::Normal, None);
        let placed = Placed { size: (width, 0), entries: Vec::new() };
        let room = screen.size.h;
        let mut panel = Panel { at, width, room, rows, first: 0, placed, chosen: None, pixels };
        panel.chosen = chosen.filter(|chosen| panel.can_choose(*chosen));
        panel.show_from(theme, 0);
        panel.draw(theme);
        let size = panel.area().size;
        let within =
            |at: i32, size: i32, from: i32, room: i32| at.min(from + room - size).max(from);
        panel.at = Point::from((
            within(at.x, size.w, screen.loc.x, screen.size.w),
            within(at.y, size.h, screen.loc.y, screen.size.h),
        ));
        panel
    }

    /// A panel in the middle of `screen`.
    pub fn in_the_middle(
        theme: &Theme,
        width: i32,
        rows: Vec<Row>,
        chosen: Option<usize>,
        screen: Rectangle<i32, Logical>,
    ) -> Panel {
        let mut panel = Panel::new(theme, width, rows, chosen, screen.loc, screen);
        panel.at = screen.loc + (screen.size - panel.area().size).downscale(2).to_point();
        panel
    }

    /// Places the rows from `first` on that the panel has room for, and
    /// from further on if that takes the chosen row to show.
    fn show_from(&mut self, theme: &Theme, first: usize) {
        self.first = first.min(self.rows.len().saturating_sub(1));
        loop {
            let entries: Vec<Entry<'_>> = self.rows[self.first..].iter().map(Row::entry).collect();
            // At least one row shows, however low the screen.
            let mut shown = entries.len();
            let mut placed = panel::place(theme, self.width, &entries);
            while placed.size.1 > self.room && shown > 1 {
                shown -= 1;
                placed = panel::place(theme, self.width, &entries[..shown]);
            }
            self.placed = placed;
            if self.chosen.is_none_or(|chosen| chosen < self.first + shown) {
                return;
            }
            self.first += 1;
        }
    }

    fn draw(&mut self, theme: &Theme) {
        let shown = self.first..self.first + self.placed.entries.len();
        let chosen = self.chosen.filter(|chosen| shown.contains(chosen)).map(|at| at - self.first);
        let entries: Vec<Entry<'_>> = self.rows[shown].iter().map(Row::entry).collect();
        let (width, size) = (self.width, self.placed.size);
        let all = Rect::new(0, 0, size.0, size.1);
        let mut context = self.pixels.render();
        context.resize(size);
        let drawn = context.draw(|pixels| {
            panel::draw(theme, width, &entries, chosen, &mut Canvas::over(all, pixels));
            Ok::<_, Infallible>(vec![Rectangle::from_size(size.into())])
        });
        let Ok(()) = drawn;
    }

    /// The panel's area of the desktop.
    pub fn area(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.at, self.placed.size.into())
    }

    /// The panel's pixels.
    pub fn pixels(&self) -> &MemoryRenderBuffer {
        &self.pixels
    }

    /// The row that is chosen.
    pub fn chosen(&self) -> Option<usize> {
        self.chosen
    }

    fn can_choose(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|row| row.entry().can_be_chosen())
    }

    /// Whether the place `at` of the desktop is in the panel.
    pub fn holds(&self, at: Point<f64, Logical>) -> bool {
        self.area().to_f64().contains(at)
    }

    /// The row the person can choose at the place `at` of the desktop.
    pub fn row_at(&self, at: Point<f64, Logical>) -> Option<usize> {
        let within = (at - self.at.to_f64()).to_i32_floor::<i32>();
        let row = self.placed.entry_at(within.x, within.y).map(|entry| entry + self.first);
        row.filter(|row| self.can_choose(*row))
    }

    /// Chooses `row`, or none, and shows it if it did not. Returns
    /// whether that is another than before.
    pub fn choose(&mut self, theme: &Theme, row: Option<usize>) -> bool {
        let row = row.filter(|row| self.can_choose(*row));
        if row == self.chosen {
            return false;
        }
        self.chosen = row;
        self.show_from(theme, self.first.min(row.unwrap_or(self.first)));
        self.draw(theme);
        true
    }

    /// Chooses the next row that can be chosen after the one that is,
    /// or before it for `back`, going round at the ends.
    pub fn step(&mut self, theme: &Theme, back: bool) {
        let count = self.rows.len();
        let from = self.chosen.unwrap_or(if back { 0 } else { count.saturating_sub(1) });
        let next = (1..=count)
            .map(|by| if back { (from + count - by % count) % count } else { (from + by) % count })
            .find(|row| self.can_choose(*row));
        self.choose(theme, next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        let line =
            |words: &str, off| Row::Line { words: words.into(), key: "", mark: Mark::None, off };
        vec![line("Restore", true), line("Move", false), Row::Gap, line("Close", false)]
    }

    fn screen() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (1024, 768).into())
    }

    #[test]
    fn a_panel_stays_whole_on_its_screen() {
        let theme = Theme::basic();
        let panel = Panel::new(&theme, 330, rows(), None, (1000, 760).into(), screen());
        let area = panel.area();
        assert_eq!(area.loc + area.size.to_point(), Point::from((1024, 768)));
        let panel = Panel::new(&theme, 330, rows(), None, (-5, 20).into(), screen());
        assert_eq!(panel.area().loc, Point::from((0, 20)));
        let panel = Panel::in_the_middle(&theme, 330, rows(), None, screen());
        let area = panel.area();
        assert_eq!(area.loc.x, (1024 - 330) / 2);
        assert_eq!(area.loc.y, (768 - area.size.h) / 2);
    }

    #[test]
    fn only_a_line_that_is_on_is_chosen() {
        let theme = Theme::basic();
        let mut panel = Panel::new(&theme, 330, rows(), Some(0), (100, 100).into(), screen());
        assert_eq!(panel.chosen(), None);
        assert_eq!(panel.row_at((120.0, 110.0).into()), None);
        assert_eq!(panel.row_at((120.0, 140.0).into()), Some(1));
        assert_eq!(panel.row_at((120.0, 170.0).into()), None);
        assert_eq!(panel.row_at((120.0, 185.0).into()), Some(3));
        assert_eq!(panel.row_at((90.0, 140.0).into()), None);
        assert!(panel.choose(&theme, Some(1)));
        assert!(!panel.choose(&theme, Some(1)));
        assert!(panel.choose(&theme, Some(2)));
        assert_eq!(panel.chosen(), None);
    }

    #[test]
    fn the_keys_step_over_what_cannot_be_chosen_and_go_round() {
        let theme = Theme::basic();
        let mut panel = Panel::new(&theme, 330, rows(), None, (100, 100).into(), screen());
        panel.step(&theme, false);
        assert_eq!(panel.chosen(), Some(1));
        panel.step(&theme, false);
        assert_eq!(panel.chosen(), Some(3));
        panel.step(&theme, false);
        assert_eq!(panel.chosen(), Some(1));
        panel.step(&theme, true);
        assert_eq!(panel.chosen(), Some(3));
        let mut panel = Panel::new(&theme, 330, rows(), None, (100, 100).into(), screen());
        panel.step(&theme, true);
        assert_eq!(panel.chosen(), Some(3));
    }

    #[test]
    fn a_panel_with_more_rows_than_fit_shows_those_about_the_chosen_one() {
        let theme = Theme::basic();
        let line = |at: usize| Row::Line {
            words: format!("Window {at}"),
            key: "",
            mark: Mark::Icon,
            off: false,
        };
        // Rows of 30 with 6 above and below them: a screen 200 high
        // has room for six.
        let low = Rectangle::new((0, 0).into(), (640, 200).into());
        let rows: Vec<Row> = (0..10).map(line).collect();
        let mut panel = Panel::in_the_middle(&theme, 480, rows.clone(), Some(1), low);
        assert_eq!(panel.area(), Rectangle::new((80, 4).into(), (480, 192).into()));
        assert_eq!(panel.row_at((100.0, 4.0 + 6.0 + 15.0).into()), Some(0));
        // The keys go past the last that shows, and the rows follow.
        for _ in 0..5 {
            panel.step(&theme, false);
        }
        assert_eq!(panel.chosen(), Some(6));
        assert_eq!(panel.row_at((100.0, 4.0 + 6.0 + 15.0).into()), Some(1));
        assert_eq!(panel.row_at((100.0, 4.0 + 6.0 + 5.0 * 30.0 + 15.0).into()), Some(6));
        // And round, to the first.
        for _ in 0..4 {
            panel.step(&theme, false);
        }
        assert_eq!(panel.chosen(), Some(0));
        assert_eq!(panel.row_at((100.0, 4.0 + 6.0 + 15.0).into()), Some(0));
        panel.step(&theme, true);
        assert_eq!(panel.chosen(), Some(9));
        assert_eq!(panel.row_at((100.0, 4.0 + 6.0 + 5.0 * 30.0 + 15.0).into()), Some(9));
        // One that begins with a row far down shows it.
        let panel = Panel::in_the_middle(&theme, 480, rows, Some(8), low);
        assert_eq!(panel.row_at((100.0, 4.0 + 6.0 + 5.0 * 30.0 + 15.0).into()), Some(8));
        assert_eq!(panel.area().size.h, 192);
    }
}
