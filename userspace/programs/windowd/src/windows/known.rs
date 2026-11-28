// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Windows that are known again show where they were (ADR 0729).
//!
//! A window with a key ([`crate::places`]) has its place kept, and one
//! that shows with a key the table has a place for shows there: on the
//! screen it was on, at the corner and size its frame had, filling what
//! it filled. Its program is asked for that size and shape in the
//! window's first configure, so that the first frame it draws is the
//! one that shows.
//!
//! It shows on the desktop that shows, and never minimized: the person
//! who starts a program looks for its window where they are. A screen
//! that is not there is the screen's absence as everywhere
//! ([`Windows::screens_changed`]): the window shows as far across and
//! down the screen the pointer is on, and goes back when its own is
//! there again.

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::shell::xdg::ToplevelSurface;
use window_frame::{Bar, Shape};

use super::zones::Fills;
use super::{Home, Restore, Screen, Windows, inside, tiled};
use crate::places::{self, Key, Place};
use crate::screens::layout;

impl Windows {
    /// The screen a window that was at `place` shows on: the one it was
    /// on if that is there, and the one the pointer, at `pointer`, is
    /// on otherwise.
    pub(super) fn screen_for(&self, place: &Place, pointer: Point<f64, Logical>) -> Screen {
        let own = self.screens().into_iter().find(|(name, _)| *name == place.screen);
        own.unwrap_or_else(|| {
            (self.screen_name_at(pointer).unwrap_or_default(), self.screen_at(pointer))
        })
    }

    /// Where the corner of the frame of a window that was at `place`
    /// is on the screen whose area is `screen`: as far across and down
    /// it as it was, with some of a title bar `width` wide on the
    /// screen. Nothing, for a window that never filled nothing.
    pub(super) fn corner_for(
        &self,
        place: &Place,
        screen: Rectangle<i32, Logical>,
        width: i32,
    ) -> Option<Point<i32, Logical>> {
        let then = Rectangle::from_size(Size::from(place.screen_size));
        let at = layout::across(then, screen, Point::from(place.at?));
        Some(self.reachable_on(screen, at, width))
    }

    /// Asks the program of `toplevel`, in the window's first configure,
    /// to draw the window as it was at `place`: at that size, and
    /// maximized or tiled if it was. The window's title bar is `bar`'s.
    /// Returns a place on the screen the window will show on.
    pub fn ask_as_before(
        &self,
        toplevel: &ToplevelSurface,
        place: &Place,
        pointer: Point<f64, Logical>,
        bar: Bar,
    ) -> Point<f64, Logical> {
        let (name, area) = self.screen_for(place, pointer);
        let room = inside(&self.theme, Shape::Normal, bar, area.size);
        let size = match place.fills {
            Fills::All => Some(inside(&self.theme, Shape::Maximized, bar, area.size)),
            Fills::Zone(zone) => {
                let fills = self.splits_of(&name, self.row(&name).shown()).area(area, zone);
                Some(inside(&self.theme, Shape::Normal, bar, fills.size))
            }
            Fills::Nothing | Fills::Screen => place.frame.map(|frame| {
                let own = inside(&self.theme, Shape::Normal, bar, Size::from(frame));
                Size::from((own.w.min(room.w), own.h.min(room.h)))
            }),
        };
        toplevel.with_pending_state(|state| {
            state.bounds = Some(room);
            state.size = size;
            match place.fills {
                Fills::All => {
                    state.states.set(State::Maximized);
                }
                Fills::Zone(zone) => tiled(&mut state.states, Some(zone)),
                Fills::Nothing | Fills::Screen => {}
            }
        });
        area.loc.to_f64() + area.size.to_f64().downscale(2.0).to_point()
    }

    /// The window at `index` first shows, on `screen`, and was at
    /// `place` before: it fills what it filled, and goes back to where
    /// it was before that when it fills it no more.
    pub(super) fn settle_known(&mut self, index: usize, place: &Place, screen: &Screen) {
        let (name, area) = screen;
        let frame = place.frame.map(Size::from);
        let width =
            frame.map_or(self.framed[index].layout.frame.w, |frame: Size<i32, Logical>| frame.w);
        let before = Restore { at: self.corner_for(place, *area, width), frame };
        let shape = self.framed[index].layout.shape;
        match place.fills {
            _ if shape == Shape::Full => {}
            Fills::All => {
                // A program that drew the window maximized as it was
                // asked has it so already.
                if shape == Shape::Normal {
                    self.maximize_on(index, *area);
                }
                self.framed[index].restore = before;
            }
            Fills::Zone(zone) if shape == Shape::Normal => {
                self.snap_on(index, screen, zone);
                self.framed[index].restore = before;
            }
            Fills::Zone(_) | Fills::Nothing | Fills::Screen => {}
        }
        if *name != place.screen {
            // Its own screen is not there: it goes back when it is.
            let then = Rectangle::from_size(Size::from(place.screen_size));
            let desk = self.row(&place.screen).shown();
            let home = Home {
                screen: place.screen.clone(),
                area: then,
                at: place.at.map(Point::from),
                desk,
            };
            self.framed[index].home = Some(home);
        }
    }

    /// Where each window with a key is now. A window that is full
    /// screen, or on another screen than its own while that is away, is
    /// left out: where it was before is what is kept of it.
    pub fn placed(&self) -> Vec<(Key, Place)> {
        let keyed =
            self.framed.iter().filter_map(|framed| Some((framed, places::key_of(&framed.window)?)));
        let keyed: Vec<_> = keyed.collect();
        // This is asked whenever a window is drawn anew, and most
        // desktops have no window with a key.
        if keyed.is_empty() {
            return Vec::new();
        }
        let screens = self.screens();
        let mut placed: Vec<(Key, Place)> = Vec::new();
        for (framed, key) in keyed {
            let area =
                screens.iter().find(|(name, _)| *name == framed.screen).map(|(_, area)| area);
            let Some(area) = area.filter(|_| !framed.full && framed.home.is_none()) else {
                continue;
            };
            // A key is one window's: the first that has it.
            if placed.iter().any(|(other, _)| *other == key) {
                continue;
            }
            let fills = framed.fills();
            let (at, frame) = match fills {
                Fills::Nothing => (Some(framed.at), Some(framed.frame().size)),
                _ => (framed.restore.at, framed.restore.frame),
            };
            let place = Place {
                screen: framed.screen.clone(),
                screen_size: area.size.into(),
                desk: framed.desk,
                at: at.map(|at| (at - area.loc).into()),
                frame: frame.map(Into::into),
                fills,
                minimized: framed.minimized,
            };
            placed.push((key, place));
        }
        placed
    }
}
