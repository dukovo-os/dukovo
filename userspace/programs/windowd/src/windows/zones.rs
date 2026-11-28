// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The zones of a screen a window snaps to (ADR 0727).
//!
//! A screen has a left and a right side, and each side an upper and a
//! lower part: a window snaps to a side, a half of the screen, or to
//! a part of one, a quarter. Where the sides meet, and where each
//! side's parts do, are the screen's splits: in the middle until the
//! person drags an edge two snapped windows share.

use smithay::utils::{Logical, Point, Rectangle};

/// A side of a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The left.
    Left,
    /// The right.
    Right,
}

/// A part of a side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The upper.
    Upper,
    /// The lower.
    Lower,
}

/// A zone of a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// A side, from the top to the bottom.
    Half(Side),
    /// A part of a side.
    Quarter(Side, Level),
}

impl Zone {
    /// The six zones, the halves first.
    pub const ALL: [Zone; 6] = [
        Zone::Half(Side::Left),
        Zone::Half(Side::Right),
        Zone::Quarter(Side::Left, Level::Upper),
        Zone::Quarter(Side::Right, Level::Upper),
        Zone::Quarter(Side::Left, Level::Lower),
        Zone::Quarter(Side::Right, Level::Lower),
    ];

    /// The side it is on.
    pub fn side(self) -> Side {
        match self {
            Zone::Half(side) | Zone::Quarter(side, _) => side,
        }
    }

    /// Whether it and `other` have a place in common, wherever the
    /// splits are.
    pub fn overlaps(self, other: Zone) -> bool {
        match (self, other) {
            (Zone::Quarter(side, level), Zone::Quarter(other, at)) => side == other && level == at,
            _ => self.side() == other.side(),
        }
    }
}

/// The one part in this many of a screen's side that a split is told
/// in.
const WHOLE: i64 = 1 << 16;

/// The least and the most of a screen a split leaves on one side: an
/// eighth and seven.
const LEAST: i64 = WHOLE / 8;

/// Where a screen's zones meet, each as a share of the screen's width
/// or height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splits {
    /// How far across the sides meet.
    across: i64,
    /// How far down the left side's parts meet.
    left: i64,
    /// How far down the right side's do.
    right: i64,
}

impl Default for Splits {
    fn default() -> Splits {
        Splits { across: WHOLE / 2, left: WHOLE / 2, right: WHOLE / 2 }
    }
}

impl Splits {
    /// The column of `screen` the sides meet at.
    fn column(self, screen: Rectangle<i32, Logical>) -> i32 {
        screen.loc.x + (i64::from(screen.size.w) * self.across / WHOLE) as i32
    }

    /// The row of `screen` the parts of `side` meet at.
    fn row(self, screen: Rectangle<i32, Logical>, side: Side) -> i32 {
        let down = if side == Side::Left { self.left } else { self.right };
        screen.loc.y + (i64::from(screen.size.h) * down / WHOLE) as i32
    }

    /// The area `zone` has of a screen whose area is `screen`.
    pub fn area(self, screen: Rectangle<i32, Logical>, zone: Zone) -> Rectangle<i32, Logical> {
        let (left, right) = (screen.loc.x, screen.loc.x + screen.size.w);
        let (top, bottom) = (screen.loc.y, screen.loc.y + screen.size.h);
        let column = self.column(screen);
        let (from, to) = match zone.side() {
            Side::Left => (left, column),
            Side::Right => (column, right),
        };
        let (above, below) = match zone {
            Zone::Half(_) => (top, bottom),
            Zone::Quarter(side, Level::Upper) => (top, self.row(screen, side)),
            Zone::Quarter(side, Level::Lower) => (self.row(screen, side), bottom),
        };
        Rectangle::new(Point::from((from, above)), (to - from, below - above).into())
    }

    /// Has the sides of a screen whose area is `screen` meet at the
    /// column `x`, or as near to it as leaves each side an eighth.
    pub fn meet_across(&mut self, screen: Rectangle<i32, Logical>, x: i32) {
        self.across = share(x - screen.loc.x, screen.size.w);
    }

    /// Has the parts of `side` of such a screen meet at the row `y`,
    /// or as near to it as leaves each part an eighth.
    pub fn meet_down(&mut self, screen: Rectangle<i32, Logical>, side: Side, y: i32) {
        let down = share(y - screen.loc.y, screen.size.h);
        match side {
            Side::Left => self.left = down,
            Side::Right => self.right = down,
        }
    }
}

/// `part` of `whole` as a split, held to what a split may be. Rounded
/// up, so that the split is at `part` again for the same `whole`.
fn share(part: i32, whole: i32) -> i64 {
    let whole = i64::from(whole.max(1));
    let share = (i64::from(part) * WHOLE + whole - 1).div_euclid(whole);
    share.clamp(LEAST, WHOLE - LEAST)
}

/// What a window fills of its screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fills {
    /// What its program chose, where the person put it.
    Nothing,
    /// A zone.
    Zone(Zone),
    /// All of it.
    All,
}

/// An arrow key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrow {
    /// Left.
    Left,
    /// Right.
    Right,
    /// Up.
    Up,
    /// Down.
    Down,
}

/// What Super with an arrow has a window do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// Fill this.
    Fill(Fills),
    /// Go off the screens, into the list.
    Minimize,
    /// Stay as it is.
    Stay,
}

/// What Super with `arrow` has a window that fills `now` do. Left and
/// right snap it to that side, a part of a side to the same part of
/// the other. Up and down go through a side's parts: from a half to a
/// quarter, from a quarter back to the half; up from the top is the
/// whole screen, and down from the bottom is the window as it was.
pub fn after(now: Fills, arrow: Arrow) -> Then {
    let to = match (now, arrow) {
        (Fills::Zone(Zone::Quarter(_, level)), Arrow::Left) => Zone::Quarter(Side::Left, level),
        (Fills::Zone(Zone::Quarter(_, level)), Arrow::Right) => Zone::Quarter(Side::Right, level),
        (_, Arrow::Left) => Zone::Half(Side::Left),
        (_, Arrow::Right) => Zone::Half(Side::Right),
        (Fills::Nothing, Arrow::Up) => return Then::Fill(Fills::All),
        (Fills::Nothing, Arrow::Down) => return Then::Minimize,
        (Fills::All, Arrow::Up) => return Then::Stay,
        (Fills::All, Arrow::Down) => return Then::Fill(Fills::Nothing),
        (Fills::Zone(Zone::Half(side)), Arrow::Up) => Zone::Quarter(side, Level::Upper),
        (Fills::Zone(Zone::Half(side)), Arrow::Down) => Zone::Quarter(side, Level::Lower),
        (Fills::Zone(Zone::Quarter(_, Level::Upper)), Arrow::Up) => {
            return Then::Fill(Fills::All);
        }
        (Fills::Zone(Zone::Quarter(_, Level::Lower)), Arrow::Down) => {
            return Then::Fill(Fills::Nothing);
        }
        (Fills::Zone(Zone::Quarter(side, _)), Arrow::Up | Arrow::Down) => Zone::Half(side),
    };
    if now == Fills::Zone(to) { Then::Stay } else { Then::Fill(Fills::Zone(to)) }
}

/// How near an edge of a screen, in points, the pointer is at it.
const AT_EDGE: f64 = 2.0;

/// How near a corner along an edge the pointer means the quarter.
const AT_CORNER: f64 = 32.0;

/// What a window dragged by its title bar fills if it is let go with
/// the pointer at `at` on a screen whose area is `screen`: the whole
/// screen at the top edge, a side at the left or the right edge, and a
/// quarter near a corner.
pub fn dragged_to(screen: Rectangle<i32, Logical>, at: Point<f64, Logical>) -> Fills {
    let screen = screen.to_f64();
    let (from_left, from_top) = (at.x - screen.loc.x, at.y - screen.loc.y);
    let (from_right, from_bottom) =
        (screen.size.w - 1.0 - from_left, screen.size.h - 1.0 - from_top);
    let side = if from_left < AT_EDGE {
        Some(Side::Left)
    } else if from_right < AT_EDGE {
        Some(Side::Right)
    } else {
        None
    };
    let level = if from_top < AT_CORNER {
        Some(Level::Upper)
    } else if from_bottom < AT_CORNER {
        Some(Level::Lower)
    } else {
        None
    };
    let near_side = if from_left < AT_CORNER {
        Some(Side::Left)
    } else if from_right < AT_CORNER {
        Some(Side::Right)
    } else {
        None
    };
    let at_top = from_top < AT_EDGE;
    let at_bottom = from_bottom < AT_EDGE;
    match (side, level) {
        (Some(side), Some(level)) => Fills::Zone(Zone::Quarter(side, level)),
        (Some(side), None) => Fills::Zone(Zone::Half(side)),
        (None, _) => match near_side {
            Some(side) if at_top => Fills::Zone(Zone::Quarter(side, Level::Upper)),
            Some(side) if at_bottom => Fills::Zone(Zone::Quarter(side, Level::Lower)),
            _ if at_top => Fills::All,
            _ => Fills::Nothing,
        },
    }
}

/// The largest zone that none of `taken` has a place in: what windowd
/// offers the other windows after a snap.
pub fn free(taken: &[Zone]) -> Option<Zone> {
    Zone::ALL.into_iter().find(|zone| taken.iter().all(|taken| !taken.overlaps(*zone)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), (w, h).into())
    }

    const LEFT: Zone = Zone::Half(Side::Left);
    const RIGHT: Zone = Zone::Half(Side::Right);
    const fn quarter(side: Side, level: Level) -> Zone {
        Zone::Quarter(side, level)
    }

    #[test]
    fn the_zones_meet_in_the_middle_until_a_split_is_moved() {
        let (splits, screen) = (Splits::default(), rect(100, 50, 1025, 768));
        assert_eq!(splits.area(screen, LEFT), rect(100, 50, 512, 768));
        assert_eq!(splits.area(screen, RIGHT), rect(612, 50, 513, 768));
        let upper = splits.area(screen, quarter(Side::Right, Level::Upper));
        let lower = splits.area(screen, quarter(Side::Right, Level::Lower));
        assert_eq!((upper, lower), (rect(612, 50, 513, 384), rect(612, 434, 513, 384)));
    }

    #[test]
    fn a_split_is_where_it_was_put_and_each_side_s_parts_meet_on_their_own() {
        let (mut splits, screen) = (Splits::default(), rect(0, 0, 1024, 768));
        splits.meet_across(screen, 700);
        splits.meet_down(screen, Side::Left, 301);
        assert_eq!(splits.area(screen, LEFT), rect(0, 0, 700, 768));
        assert_eq!(splits.area(screen, RIGHT), rect(700, 0, 324, 768));
        assert_eq!(splits.area(screen, quarter(Side::Left, Level::Upper)), rect(0, 0, 700, 301));
        assert_eq!(splits.area(screen, quarter(Side::Left, Level::Lower)), rect(0, 301, 700, 467));
        assert_eq!(splits.area(screen, quarter(Side::Right, Level::Upper)), rect(700, 0, 324, 384));
        // On a screen of another size it is as far across in share.
        assert_eq!(splits.area(rect(0, 0, 2048, 768), LEFT).size.w, 1400);
    }

    #[test]
    fn a_split_leaves_each_side_an_eighth() {
        let (mut splits, screen) = (Splits::default(), rect(0, 0, 1024, 768));
        splits.meet_across(screen, 10);
        assert_eq!(splits.area(screen, LEFT).size.w, 128);
        splits.meet_across(screen, 5000);
        assert_eq!(splits.area(screen, RIGHT).size.w, 128);
        splits.meet_down(screen, Side::Right, -3);
        assert_eq!(splits.area(screen, quarter(Side::Right, Level::Upper)).size.h, 96);
    }

    #[test]
    fn the_arrows_go_through_the_sides_and_their_parts() {
        let zone = |zone| Then::Fill(Fills::Zone(zone));
        let (upper, lower) = (quarter(Side::Left, Level::Upper), quarter(Side::Left, Level::Lower));
        assert_eq!(after(Fills::Nothing, Arrow::Left), zone(LEFT));
        assert_eq!(after(Fills::All, Arrow::Right), zone(RIGHT));
        assert_eq!(after(Fills::Zone(LEFT), Arrow::Left), Then::Stay);
        assert_eq!(after(Fills::Zone(LEFT), Arrow::Right), zone(RIGHT));
        assert_eq!(after(Fills::Zone(LEFT), Arrow::Up), zone(upper));
        assert_eq!(after(Fills::Zone(LEFT), Arrow::Down), zone(lower));
        assert_eq!(after(Fills::Zone(upper), Arrow::Down), zone(LEFT));
        assert_eq!(after(Fills::Zone(lower), Arrow::Up), zone(LEFT));
        assert_eq!(after(Fills::Zone(upper), Arrow::Up), Then::Fill(Fills::All));
        assert_eq!(after(Fills::Zone(lower), Arrow::Down), Then::Fill(Fills::Nothing));
        assert_eq!(
            after(Fills::Zone(upper), Arrow::Right),
            zone(quarter(Side::Right, Level::Upper))
        );
        assert_eq!(after(Fills::Zone(lower), Arrow::Left), Then::Stay);
    }

    #[test]
    fn the_arrows_maximize_minimize_and_restore_a_window_that_is_not_snapped() {
        assert_eq!(after(Fills::Nothing, Arrow::Up), Then::Fill(Fills::All));
        assert_eq!(after(Fills::Nothing, Arrow::Down), Then::Minimize);
        assert_eq!(after(Fills::All, Arrow::Up), Then::Stay);
        assert_eq!(after(Fills::All, Arrow::Down), Then::Fill(Fills::Nothing));
    }

    #[test]
    fn a_drag_to_an_edge_or_a_corner_names_what_the_window_fills() {
        let screen = rect(1024, 0, 800, 600);
        let at = |x: f64, y: f64| dragged_to(screen, Point::from((1024.0 + x, y)));
        assert_eq!(at(400.0, 300.0), Fills::Nothing);
        assert_eq!(at(400.0, 0.5), Fills::All);
        assert_eq!(at(400.0, 599.5), Fills::Nothing);
        assert_eq!(at(0.5, 300.0), Fills::Zone(LEFT));
        assert_eq!(at(799.5, 300.0), Fills::Zone(RIGHT));
        assert_eq!(at(0.5, 20.0), Fills::Zone(quarter(Side::Left, Level::Upper)));
        assert_eq!(at(20.0, 0.5), Fills::Zone(quarter(Side::Left, Level::Upper)));
        assert_eq!(at(799.5, 580.0), Fills::Zone(quarter(Side::Right, Level::Lower)));
        assert_eq!(at(780.0, 599.5), Fills::Zone(quarter(Side::Right, Level::Lower)));
        assert_eq!(at(3.0, 300.0), Fills::Nothing);
    }

    #[test]
    fn the_largest_free_zone_is_offered() {
        assert_eq!(free(&[LEFT]), Some(RIGHT));
        assert_eq!(free(&[RIGHT]), Some(LEFT));
        let upper = quarter(Side::Left, Level::Upper);
        assert_eq!(free(&[upper]), Some(RIGHT));
        assert_eq!(free(&[upper, RIGHT]), Some(quarter(Side::Left, Level::Lower)));
        assert_eq!(
            free(&[LEFT, quarter(Side::Right, Level::Lower)]),
            Some(quarter(Side::Right, Level::Upper))
        );
        assert_eq!(free(&[LEFT, RIGHT]), None);
        assert_eq!(free(&[]), Some(LEFT));
    }
}
