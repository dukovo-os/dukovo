// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where the screens are on the desktop, and which is which
//! (ADR 0725).
//!
//! A screen's area of the desktop is its mode's size, turned as its
//! picture is and divided by its zoom. Where the areas are is the
//! person's to say, in a layout for the set of screens there are
//! (windowd.json(5)); for a set with no layout they are side by side
//! in the display layer's order, their tops level. A layout must leave
//! the desktop in one piece and no place on two screens, or the
//! pointer could not reach a screen, or a window would show twice.

use display::abi::Mode;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::settings::screens::{ModeSet, Places, Turn, Zoom};

/// The size of the area of the desktop a screen shows in a mode of
/// `mode` pixels, turned by `turn` and at the zoom `zoom`: no pixel of
/// the screen is left without a place on the desktop.
pub fn area(mode: (u32, u32), turn: Turn, zoom: Zoom) -> Size<i32, Logical> {
    let (width, height) = if turn.sideways() { (mode.1, mode.0) } else { mode };
    let points = |pixels: u32| (f64::from(pixels) / zoom.number()).ceil() as i32;
    Size::from((points(width), points(height)))
}

/// The mode of `modes` that `set` names: of its size, and at the rate
/// nearest the one it names, or the fastest if it names none.
pub fn mode(modes: &[Mode], set: ModeSet) -> Option<Mode> {
    let sized = modes.iter().filter(|mode| (mode.width, mode.height) == set.size);
    match set.hz {
        // Of two as near, the one the monitor lists first.
        Some(hz) => sized
            .filter(|mode| mode.refresh_mhz.abs_diff(hz * 1000) <= 500)
            .min_by_key(|mode| mode.refresh_mhz.abs_diff(hz * 1000)),
        None => sized.rev().max_by_key(|mode| mode.refresh_mhz),
    }
    .copied()
}

/// A screen the desktop shows on, as a layout looks at it.
#[derive(Clone, Copy, Debug)]
pub struct Shown<'a> {
    /// Its name, as programs are told it.
    pub name: &'a str,
    /// Its monitor's maker and model, or nothing if it has neither.
    pub monitor: &'a str,
    /// The size of its area of the desktop.
    pub size: Size<i32, Logical>,
}

/// Why the layout for a set of screens is not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unused {
    /// A place would be on two screens.
    Overlap,
    /// A screen would touch no other along an edge.
    Apart,
}

impl core::fmt::Display for Unused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Unused::Overlap => "puts one screen over another",
            Unused::Apart => "leaves a gap between the screens",
        })
    }
}

/// Where each of `shown` is: by the layout of `layouts` that is for
/// exactly these screens, each by its name or else by its monitor, or
/// side by side with no such layout. A layout that cannot be used is
/// said why, and the screens are side by side.
pub fn places(
    shown: &[Shown<'_>],
    layouts: &[Places],
) -> (Vec<Rectangle<i32, Logical>>, Option<Unused>) {
    let laid = layouts.iter().find_map(|layout| by_layout(shown, layout));
    let unused = laid.as_ref().and_then(|areas| check(areas).err());
    match laid {
        Some(areas) if unused.is_none() => (areas, None),
        _ => {
            let mut left = 0;
            let beside = shown.iter().map(|screen| {
                let area = Rectangle::new(Point::from((left, 0)), screen.size);
                left += screen.size.w;
                area
            });
            (beside.collect(), unused)
        }
    }
}

/// The areas `layout` gives `shown`, if it is for exactly them.
fn by_layout(shown: &[Shown<'_>], layout: &Places) -> Option<Vec<Rectangle<i32, Logical>>> {
    if layout.len() != shown.len() {
        return None;
    }
    let mut keys: Vec<&str> = Vec::new();
    let mut areas = Vec::new();
    for screen in shown {
        let key = [screen.name, screen.monitor]
            .into_iter()
            .find(|key| !key.is_empty() && layout.contains_key(*key))?;
        // Two monitors of one model are told apart by their names
        // alone.
        if keys.contains(&key) {
            return None;
        }
        keys.push(key);
        areas.push(Rectangle::new(Point::from(layout[key]), screen.size));
    }
    Some(areas)
}

/// Whether `a` and `b` have a place in common.
fn overlap(a: &Rectangle<i32, Logical>, b: &Rectangle<i32, Logical>) -> bool {
    a.loc.x < b.loc.x + b.size.w
        && b.loc.x < a.loc.x + a.size.w
        && a.loc.y < b.loc.y + b.size.h
        && b.loc.y < a.loc.y + a.size.h
}

/// Whether `a` and `b` share a length of an edge, which the pointer
/// crosses from one to the other.
fn touch(a: &Rectangle<i32, Logical>, b: &Rectangle<i32, Logical>) -> bool {
    let shared = |a: (i32, i32), b: (i32, i32)| a.0.max(b.0) < (a.0 + a.1).min(b.0 + b.1);
    let meet = |a: (i32, i32), b: (i32, i32)| a.0 + a.1 == b.0 || b.0 + b.1 == a.0;
    let (ax, ay) = ((a.loc.x, a.size.w), (a.loc.y, a.size.h));
    let (bx, by) = ((b.loc.x, b.size.w), (b.loc.y, b.size.h));
    (meet(ax, bx) && shared(ay, by)) || (meet(ay, by) && shared(ax, bx))
}

/// Whether the desktop of `areas` is in one piece with no place on two
/// screens.
fn check(areas: &[Rectangle<i32, Logical>]) -> Result<(), Unused> {
    for (at, a) in areas.iter().enumerate() {
        if areas[at + 1..].iter().any(|b| overlap(a, b)) {
            return Err(Unused::Overlap);
        }
    }
    // The screens that are reached from the first, edge by edge.
    let mut reached = vec![false; areas.len()];
    let mut next = vec![0];
    while let Some(at) = next.pop() {
        if core::mem::replace(&mut reached[at], true) {
            continue;
        }
        let beside = |to: &usize| !reached[*to] && touch(&areas[at], &areas[*to]);
        next.extend((0..areas.len()).filter(beside));
    }
    if reached.contains(&false) { Err(Unused::Apart) } else { Ok(()) }
}

/// The place in `to` that is as far across and down it as `at` is
/// across and down `from`.
pub fn across(
    from: Rectangle<i32, Logical>,
    to: Rectangle<i32, Logical>,
    at: Point<i32, Logical>,
) -> Point<i32, Logical> {
    let share = |at: i32, from: (i32, i32), to: (i32, i32)| {
        let along = i64::from(at - from.0) * i64::from(to.1) / i64::from(from.1.max(1));
        to.0 + along as i32
    };
    Point::from((
        share(at.x, (from.loc.x, from.size.w), (to.loc.x, to.size.w)),
        share(at.y, (from.loc.y, from.size.h), (to.loc.y, to.size.h)),
    ))
}

/// Which of `areas` has its middle nearest the middle of `gone`: where
/// what was on a screen that went is put.
pub fn nearest(gone: Rectangle<i32, Logical>, areas: &[Rectangle<i32, Logical>]) -> Option<usize> {
    let middle = |area: &Rectangle<i32, Logical>| {
        (
            i64::from(area.loc.x) * 2 + i64::from(area.size.w),
            i64::from(area.loc.y) * 2 + i64::from(area.size.h),
        )
    };
    let from = middle(&gone);
    let far = |area: &Rectangle<i32, Logical>| {
        let to = middle(area);
        (to.0 - from.0).pow(2) + (to.1 - from.1).pow(2)
    };
    (0..areas.len()).min_by_key(|at| far(&areas[*at]))
}

/// The screen after the one at `from` in `areas`, or before it for
/// `back`, as they come from left to right and then from the top
/// down; after the last comes the first. `None` with one screen.
pub fn beside(areas: &[Rectangle<i32, Logical>], from: usize, back: bool) -> Option<usize> {
    let mut order: Vec<usize> = (0..areas.len()).collect();
    order.sort_by_key(|at| (areas[*at].loc.x, areas[*at].loc.y));
    let place = order.iter().position(|at| *at == from)?;
    let step = if back { order.len() - 1 } else { 1 };
    let to = order[(place + step) % order.len()];
    (to != from).then_some(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    fn shown<'a>(name: &'a str, monitor: &'a str, size: (i32, i32)) -> Shown<'a> {
        Shown { name, monitor, size: Size::from(size) }
    }

    fn layout(pairs: &[(&str, (i32, i32))]) -> Places {
        pairs.iter().map(|(name, at)| ((*name).to_owned(), *at)).collect()
    }

    fn at(width: u32, height: u32, refresh_mhz: u32) -> Mode {
        Mode { width, height, refresh_mhz, ..Mode::default() }
    }

    #[test]
    fn an_area_is_the_mode_turned_and_divided_by_the_zoom() {
        let zoom = |number| Zoom::of(number).expect("a zoom");
        assert_eq!(area((1920, 1080), Turn::None, Zoom::ONE), Size::from((1920, 1080)));
        assert_eq!(area((1920, 1080), Turn::Quarter, zoom(2.0)), Size::from((540, 960)));
        assert_eq!(area((1920, 1080), Turn::Half, zoom(1.5)), Size::from((1280, 720)));
        // 1366 / 1.25 is 1092.8: the last column has a place.
        assert_eq!(area((1366, 768), Turn::ThreeQuarters, zoom(1.25)), Size::from((615, 1093)));
    }

    #[test]
    fn a_mode_is_chosen_by_its_size_and_the_nearest_rate() {
        let modes = [
            at(1920, 1080, 60_000),
            at(1920, 1080, 59_940),
            at(1920, 1080, 143_981),
            at(800, 600, 75_000),
        ];
        let set = |size, hz| ModeSet { size, hz };
        assert_eq!(mode(&modes, set((1920, 1080), None)), Some(modes[2]));
        assert_eq!(mode(&modes, set((1920, 1080), Some(60))), Some(modes[0]));
        assert_eq!(mode(&modes, set((1920, 1080), Some(144))), Some(modes[2]));
        assert_eq!(mode(&modes, set((800, 600), None)), Some(modes[3]));
        assert_eq!(mode(&modes, set((1920, 1080), Some(75))), None);
        assert_eq!(mode(&modes, set((1024, 768), None)), None);
        let same = [at(640, 480, 60_000), at(640, 480, 60_000)];
        assert_eq!(mode(&same, set((640, 480), None)), Some(same[0]));
    }

    #[test]
    fn with_no_layout_the_screens_are_side_by_side() {
        let screens = [shown("a/0", "", (800, 600)), shown("a/1", "", (1280, 720))];
        let (areas, unused) = places(&screens, &[layout(&[("a/0", (0, 0))])]);
        assert_eq!(areas, [rect(0, 0, 800, 600), rect(800, 0, 1280, 720)]);
        assert_eq!(unused, None);
    }

    #[test]
    fn the_layout_for_exactly_these_screens_places_them() {
        let screens = [shown("a/0", "DEL One", (800, 600)), shown("a/1", "DEL Two", (1280, 720))];
        let layouts = [
            layout(&[("a/0", (0, 0)), ("a/1", (800, 0)), ("a/2", (2080, 0))]),
            layout(&[("DEL Two", (0, -720)), ("a/0", (100, 0))]),
        ];
        let (areas, unused) = places(&screens, &layouts);
        assert_eq!(areas, [rect(100, 0, 800, 600), rect(0, -720, 1280, 720)]);
        assert_eq!(unused, None);
    }

    #[test]
    fn two_monitors_of_one_model_need_their_names() {
        let screens = [shown("a/0", "DEL One", (800, 600)), shown("a/1", "DEL One", (800, 600))];
        let by_model = [layout(&[("DEL One", (0, 0)), ("a/1", (0, 600))])];
        assert_eq!(places(&screens, &by_model).0, [rect(0, 0, 800, 600), rect(0, 600, 800, 600)]);
        let twice = [layout(&[("DEL One", (0, 600)), ("x", (0, 0))])];
        assert_eq!(places(&screens, &twice).0, [rect(0, 0, 800, 600), rect(800, 0, 800, 600)]);
    }

    #[test]
    fn a_layout_that_overlaps_or_leaves_a_gap_is_not_used() {
        let screens = [shown("a", "", (800, 600)), shown("b", "", (800, 600))];
        let beside = [rect(0, 0, 800, 600), rect(800, 0, 800, 600)];
        for (second, why) in [
            ((799, 0), Unused::Overlap),
            ((0, 0), Unused::Overlap),
            ((801, 0), Unused::Apart),
            // A corner alone is no way across.
            ((800, 600), Unused::Apart),
            ((-800, -600), Unused::Apart),
        ] {
            let layouts = [layout(&[("a", (0, 0)), ("b", second)])];
            assert_eq!(places(&screens, &layouts), (beside.to_vec(), Some(why)), "{second:?}");
        }
        let layouts = [layout(&[("a", (0, 0)), ("b", (800, 599))])];
        assert_eq!(places(&screens, &layouts).1, None);
    }

    #[test]
    fn three_screens_are_in_one_piece_through_the_middle_one() {
        let screens =
            [shown("a", "", (800, 600)), shown("b", "", (800, 600)), shown("c", "", (800, 600))];
        let chain = [layout(&[("a", (1600, 0)), ("b", (0, 0)), ("c", (800, 0))])];
        assert_eq!(places(&screens, &chain).1, None);
        let apart = [layout(&[("a", (0, 0)), ("b", (800, 0)), ("c", (1700, 0))])];
        assert_eq!(places(&screens, &apart).1, Some(Unused::Apart));
    }

    #[test]
    fn a_place_is_carried_by_its_share_of_the_way_across_and_down() {
        let (from, to) = (rect(0, 0, 800, 600), rect(800, -100, 1600, 1200));
        assert_eq!(across(from, to, Point::from((0, 0))), Point::from((800, -100)));
        assert_eq!(across(from, to, Point::from((200, 300))), Point::from((1200, 500)));
        assert_eq!(across(to, from, Point::from((1200, 500))), Point::from((200, 300)));
        // A place off the screen's left stays as far off, in share.
        assert_eq!(across(from, to, Point::from((-40, 0))), Point::from((720, -100)));
    }

    #[test]
    fn the_nearest_screen_is_by_the_middles() {
        let areas = [rect(0, 0, 800, 600), rect(800, 0, 800, 600), rect(0, 600, 800, 600)];
        assert_eq!(nearest(rect(1600, 0, 800, 600), &areas), Some(1));
        assert_eq!(nearest(rect(0, 1200, 800, 600), &areas), Some(2));
        assert_eq!(nearest(rect(0, 0, 10, 10), &[]), None);
    }

    #[test]
    fn the_screen_beside_one_is_the_next_from_left_to_right() {
        let areas = [rect(800, 0, 800, 600), rect(0, 0, 800, 600), rect(1600, 0, 800, 600)];
        assert_eq!(beside(&areas, 1, false), Some(0));
        assert_eq!(beside(&areas, 0, false), Some(2));
        assert_eq!(beside(&areas, 2, false), Some(1));
        assert_eq!(beside(&areas, 1, true), Some(2));
        assert_eq!(beside(&areas, 0, true), Some(1));
        assert_eq!(beside(&areas[..1], 0, false), None);
    }
}
