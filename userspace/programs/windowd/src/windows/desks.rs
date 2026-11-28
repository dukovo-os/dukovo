// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A screen's row of desktops (ADR 0726).
//!
//! Each screen has desktops of its own, side by side in a row, one of
//! which shows. A window is on one desktop of the screen it is on. A
//! desktop is known by its place in the row, which changes when one
//! before it is made or closed: [`Row::add`] and [`Row::close`] say
//! which, and [`after_add`] and [`after_close`] where a window's
//! desktop is then.

/// How many desktops a screen has at most.
pub const MOST: usize = 16;

/// A screen's desktops: how many there are, and which shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    count: usize,
    shown: usize,
}

impl Default for Row {
    fn default() -> Row {
        Row { count: 1, shown: 0 }
    }
}

impl Row {
    /// The place of the desktop that shows.
    pub fn shown(self) -> usize {
        self.shown
    }

    /// How many desktops there are.
    #[cfg(test)]
    pub fn count(self) -> usize {
        self.count
    }

    /// Makes a desktop after the one that shows, and shows it. Returns
    /// its place, or `None` if the row is full.
    pub fn add(&mut self) -> Option<usize> {
        if self.count == MOST {
            return None;
        }
        self.count += 1;
        self.shown += 1;
        Some(self.shown)
    }

    /// Closes the desktop that shows, and shows the one before it, or
    /// the one after for the first. Returns the place of the one that
    /// closed, or `None` if it is the only one.
    pub fn close(&mut self) -> Option<usize> {
        if self.count == 1 {
            return None;
        }
        let closed = self.shown;
        self.count -= 1;
        self.shown = closed.saturating_sub(1);
        Some(closed)
    }

    /// Shows the desktop after the one that shows, or the one before
    /// for `back`. Returns its place, or `None` at the row's end.
    pub fn step(&mut self, back: bool) -> Option<usize> {
        let to = if back { self.shown.checked_sub(1)? } else { self.shown + 1 };
        if to == self.count {
            return None;
        }
        self.shown = to;
        Some(to)
    }

    /// The desktop nearest to the one at `desk` that the row has.
    pub fn nearest(self, desk: usize) -> usize {
        desk.min(self.count - 1)
    }
}

/// Where the desktop that was at `desk` is once one was made at
/// `made`.
pub fn after_add(desk: usize, made: usize) -> usize {
    if desk >= made { desk + 1 } else { desk }
}

/// Where a window of the desktop that was at `desk` is once the one
/// at `closed` closed: the windows of that one are on the one before
/// it, or on the one after for the first.
pub fn after_close(desk: usize, closed: usize) -> usize {
    if desk > closed || (desk == closed && closed > 0) { desk - 1 } else { desk }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_starts_with_one_desktop_and_grows_after_the_one_that_shows() {
        let mut row = Row::default();
        assert_eq!((row.count(), row.shown()), (1, 0));
        assert_eq!(row.add(), Some(1));
        assert_eq!(row.step(true), Some(0));
        // Between the first and what was the second.
        assert_eq!(row.add(), Some(1));
        assert_eq!((row.count(), row.shown()), (3, 1));
        assert_eq!([0, 1, 2].map(|desk| after_add(desk, 1)), [0, 2, 3]);
    }

    #[test]
    fn a_row_is_full_at_the_most() {
        let mut row = Row::default();
        for place in 1..MOST {
            assert_eq!(row.add(), Some(place));
        }
        assert_eq!(row.add(), None);
        assert_eq!((row.count(), row.shown()), (MOST, MOST - 1));
    }

    #[test]
    fn a_step_stops_at_the_row_s_ends() {
        let mut row = Row::default();
        assert_eq!(row.step(false), None);
        assert_eq!(row.step(true), None);
        row.add();
        assert_eq!(row.step(false), None);
        assert_eq!(row.step(true), Some(0));
        assert_eq!(row.step(true), None);
        assert_eq!(row.step(false), Some(1));
    }

    #[test]
    fn closing_shows_the_one_before_and_its_windows_go_there() {
        let mut row = Row::default();
        row.add();
        row.add();
        // The third of three shows.
        assert_eq!(row.close(), Some(2));
        assert_eq!((row.count(), row.shown()), (2, 1));
        assert_eq!([0, 1, 2].map(|desk| after_close(desk, 2)), [0, 1, 1]);
        row.step(true);
        // The first closes: what was the second is the first.
        assert_eq!(row.close(), Some(0));
        assert_eq!((row.count(), row.shown()), (1, 0));
        assert_eq!([0, 1].map(|desk| after_close(desk, 0)), [0, 0]);
        assert_eq!(row.close(), None);
    }

    #[test]
    fn the_nearest_desktop_is_the_last_for_one_past_the_end() {
        let mut row = Row::default();
        row.add();
        assert_eq!([0, 1, 5].map(|desk| row.nearest(desk)), [0, 1, 1]);
    }
}
