// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A title bar shared with a window's program (ADR 0719), the person's
//! say in it, and a program that asks who decorates its window.

use input::abi::{BTN_LEFT, KEY_ENTER, KEY_LEFTALT, KEY_SPACE, KEY_UP};
use window_frame::panel::{Entry, Mark};
use window_frame::{Bar, Layout, Rect, Shape, Theme};
use window_test::{BarSaid, Client, Decoration, Heard, Mode, Open};

use super::{Program, World, framed, look};
use crate::screens::BACKGROUND;

/// The size of a window's surface under a title bar that is windowd's
/// whole, and under one that is shared: the bar's rows higher.
const FULL: (u32, u32) = (600, 400);
const SHARED: (u32, u32) = (600, 436);

/// What a program is told of a shared bar in the Basic look.
const SHARED_BAR: BarSaid = BarSaid { mode: Mode::Shared, height: 36, left: 30, right: 226 };
const FULL_BAR: BarSaid = BarSaid { mode: Mode::Full, ..SHARED_BAR };

/// The part of the bar the test program asks for, from its window's
/// corner.
const REGION: (i32, i32, i32, i32) = (40, 4, 200, 28);

/// How wide the window menu is.
const MENU_WIDTH: i32 = 330;

/// The frame of a window whose surface is `size`, with a title bar
/// that is `bar`'s and the program's `region` of it.
fn layout_of(size: (u32, u32), shape: Shape, bar: Bar, region: Option<Rect>) -> Layout {
    Layout::of(&Theme::basic(), (size.0 as i32, size.1 as i32), shape, bar, region)
}

/// The frame of a window that shares its bar and draws [`REGION`] of
/// it.
fn shared(size: (u32, u32), shape: Shape) -> Layout {
    let (x, y, width, height) = REGION;
    layout_of(size, shape, Bar::Shared, Some(Rect::new(x, y, width, height)))
}

/// The window menu's lines for a window that shares its bar or may,
/// with the line for the full bar ticked or not.
fn menu(full: bool) -> [Entry<'static>; 9] {
    let line = |words, key, mark, off| Entry::Line { words, key, mark, off };
    let mark = if full { Mark::Tick } else { Mark::None };
    [
        line("Restore", "", Mark::None, true),
        line("Move", "", Mark::None, false),
        line("Size", "", Mark::None, false),
        line("Minimize", "", Mark::None, false),
        line("Maximize", "", Mark::None, false),
        Entry::Gap,
        line("Full title bar for this app", "", mark, false),
        Entry::Gap,
        line("Close", "Alt+F4", Mark::None, false),
    ]
}

impl World {
    /// A program whose window asks for the bar from the first and
    /// shows with [`REGION`] of it, the pattern of the seed 7 all over
    /// its surface.
    fn sharing(&mut self) -> Program {
        let program = self.program();
        let said = self.ask(&program, |client| {
            client.open(Open { shares_bar: true, ..Open::default() }).unwrap();
            client.region(REGION).unwrap();
            client.bar().unwrap()
        });
        assert_eq!(said, Some(SHARED_BAR));
        self.show(&program, SHARED, 7);
        program
    }

    /// Chooses the line for the full bar in the menu of the window in
    /// front, by the keys.
    fn choose_full_bar(&mut self) {
        self.chord(KEY_LEFTALT, KEY_SPACE);
        // Up from no line is the last, Close; the one above it is the
        // bar's.
        self.tap(KEY_UP);
        self.tap(KEY_UP);
        self.tap(KEY_ENTER);
        self.settle();
    }
}

#[test]
fn a_program_draws_its_region_of_a_shared_bar_and_windowd_the_rest() {
    let mut world = World::one_screen();
    world.sharing();
    // The frame's box is what it is around a window of the same
    // content under windowd's whole bar.
    let (full, corner) = framed(FULL);
    let layout = shared(SHARED, Shape::Normal);
    assert_eq!(layout.frame, full.frame);
    assert_eq!(layout.region, Some(Rect::new(45, 4, 200, 28)));
    // The bar is windowd's but for the region, with no title in it;
    // the region and the content are the program's own pixels, each
    // where the program drew it.
    world.shows_frame(&layout, corner, &look(true));
    world.shows_own(&layout, corner, 7);
    // Outside the frame's round corners the desktop shows, not the
    // window's own top rows.
    assert_eq!(world.colour(corner.0 as u32, corner.1 as u32), BACKGROUND);
    assert_eq!(world.colour((corner.0 + 609) as u32, corner.1 as u32), BACKGROUND);
}

#[test]
fn a_region_is_cut_to_what_windowd_keeps_and_changes_with_a_commit() {
    let mut world = World::one_screen();
    let program = world.sharing();
    let (_, corner) = framed(FULL);
    // All of the bar asked for: the icon, the buttons and the strip to
    // move the window by stay windowd's. It is the window's when the
    // program next draws.
    world.ask(&program, |client| client.region((0, 0, 600, 36)).unwrap());
    world.settle();
    world.shows_frame(&shared(SHARED, Shape::Normal), corner, &look(true));
    world.show(&program, SHARED, 8);
    let all = layout_of(SHARED, Shape::Normal, Bar::Shared, Some(Rect::new(0, 0, 600, 36)));
    assert_eq!(all.region, Some(Rect::new(35, 1, 600 - 30 - 226, 35)));
    world.shows_frame(&all, corner, &look(true));
    world.shows_own(&all, corner, 8);
    // A region of no size, or out of all range, is none.
    for none in [(40, 4, 0, 28), (40, 4, -5, 28), (i32::MAX, i32::MIN, i32::MAX, i32::MAX)] {
        world.ask(&program, move |client| client.region(none).unwrap());
        world.show(&program, SHARED, 9);
        let bare = layout_of(SHARED, Shape::Normal, Bar::Shared, None);
        world.shows_frame(&bare, corner, &look(true));
        world.shows_own(&bare, corner, 9);
    }
}

#[test]
fn the_pointer_is_the_program_s_in_its_region_and_windowd_s_in_the_rest_of_the_bar() {
    let mut world = World::one_screen();
    let program = world.sharing();
    let (_, (left, top)) = framed(FULL);
    let layout = shared(SHARED, Shape::Normal);
    let region = layout.region.unwrap();
    world.told(&program);
    // In the region, the program hears the pointer and its button, at
    // the place of its own surface.
    world.click(left + region.x + 10, top + region.y + 5);
    world.settle();
    let own = (f64::from(REGION.0) + 10.5, f64::from(REGION.1) + 5.5);
    let button = |pressed| Heard::Button { code: u32::from(BTN_LEFT), pressed };
    assert_eq!(world.told(&program), [Heard::Motion(own.0, own.1), button(true), button(false)]);
    // Beside it the bar is windowd's: the pointer leaves the program,
    // and a drag there moves the window.
    world.point(left + region.right() + 10, top + 20);
    world.settle();
    assert_eq!(world.told(&program), [Heard::Leave]);
    world.button(BTN_LEFT, true);
    world.point(left + region.right() + 110, top + 70);
    world.button(BTN_LEFT, false);
    world.settle();
    assert_eq!(world.told(&program), []);
    world.shows_frame(&layout, (left + 100, top + 50), &look(true));
    world.shows_own(&layout, (left + 100, top + 50), 7);
}

#[test]
fn a_bar_changes_hands_when_the_program_has_drawn_for_it_and_the_frame_stays() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, FULL, 7);
    let (full, corner) = framed(FULL);
    // The program asks for the bar once its window shows: it is told
    // the bar is shared, and the frame is as it was until the program
    // has drawn the window the bar's rows higher.
    let said = world.ask(&program, |client| {
        client.share_bar().unwrap();
        client.region(REGION).unwrap();
        client.bar().unwrap()
    });
    assert_eq!(said, Some(SHARED_BAR));
    world.settle();
    world.shows_frame(&full, corner, &look(true));
    world.shows_own(&full, corner, 7);
    world.show(&program, SHARED, 8);
    let layout = shared(SHARED, Shape::Normal);
    world.shows_frame(&layout, corner, &look(true));
    world.shows_own(&layout, corner, 8);
    // It gives the bar back: again the frame waits for the program.
    world.ask(&program, |client| client.give_bar_back().unwrap());
    world.settle();
    world.shows_frame(&layout, corner, &look(true));
    world.show(&program, FULL, 9);
    world.shows_frame(&full, corner, &look(true));
    world.shows_own(&full, corner, 9);
}

#[test]
fn a_window_of_a_size_windowd_gave_is_asked_for_the_bar_s_rows_more_or_fewer() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, FULL, 7);
    world.ask(&program, |client| client.maximize(true).unwrap());
    assert_eq!(world.follow(&program, FULL, 7), ((1024, 732), true));
    // Maximized under a shared bar, the window's surface is the whole
    // screen.
    world.ask(&program, |client| {
        client.share_bar().unwrap();
        client.region(REGION).unwrap();
    });
    assert_eq!(world.follow(&program, FULL, 8), ((1024, 768), true));
    let layout = shared((1024, 768), Shape::Maximized);
    assert_eq!(layout.region, Some(Rect::new(40, 4, 200, 28)));
    world.shows_frame(&layout, (0, 0), &look(true));
    world.shows_own(&layout, (0, 0), 8);
    // Restored, it has the frame it had, around a surface the bar's
    // rows higher than before.
    world.ask(&program, |client| client.maximize(false).unwrap());
    assert_eq!(world.follow(&program, FULL, 9), (SHARED, false));
    let (_, corner) = framed(FULL);
    world.shows_frame(&shared(SHARED, Shape::Normal), corner, &look(true));
    world.ask(&program, |client| client.give_bar_back().unwrap());
    assert_eq!(world.follow(&program, FULL, 10), (FULL, false));
    world.shows_frame(&framed(FULL).0, corner, &look(true));
    world.shows_own(&framed(FULL).0, corner, 10);
}

#[test]
fn a_window_that_asks_to_be_maximized_and_for_the_bar_first_shows_over_the_whole_screen() {
    let mut world = World::one_screen();
    let program = world.program();
    world.ask(&program, |client| {
        client.open(Open { maximized: true, shares_bar: true, ..Open::default() }).unwrap();
        client.region(REGION).unwrap();
    });
    assert_eq!(world.follow(&program, FULL, 7), ((1024, 768), true));
    let layout = shared((1024, 768), Shape::Maximized);
    world.shows_frame(&layout, (0, 0), &look(true));
    world.shows_own(&layout, (0, 0), 7);
}

#[test]
fn a_window_under_a_shared_bar_is_resized_by_its_surface_and_keeps_a_row_below_the_bar() {
    let mut world = World::one_screen();
    let program = world.sharing();
    let (full, (left, top)) = framed(FULL);
    // The right edge: the surface is as much wider, and as high as it
    // was, the bar's rows with it.
    let edge = left + full.frame.w - 2;
    world.point(edge, top + 200);
    world.button(BTN_LEFT, true);
    world.point(edge + 40, top + 200);
    assert_eq!(world.follow(&program, SHARED, 8), ((640, 436), false));
    world.button(BTN_LEFT, false);
    world.settle();
    let wider = shared((640, 436), Shape::Normal);
    world.shows_frame(&wider, (left, top), &look(true));
    world.shows_own(&wider, (left, top), 8);
    // The bottom edge, taken above the top: the bar stays whole, with
    // a row of content below it.
    let bottom = top + full.frame.h - 2;
    world.point(left + 300, bottom);
    world.button(BTN_LEFT, true);
    world.point(left + 300, 0);
    assert_eq!(world.follow(&program, SHARED, 9), ((640, 37), false));
    world.button(BTN_LEFT, false);
    world.settle();
    let least = shared((640, 37), Shape::Normal);
    world.shows_frame(&least, (left, top), &look(true));
    world.shows_own(&least, (left, top), 9);
}

#[test]
fn the_person_gives_an_application_the_full_bar_from_the_window_menu_and_takes_it_back() {
    let mut world = World::one_screen();
    let program = world.sharing();
    let (full, corner) = framed(FULL);
    // The menu of a window that shares its bar has a line for it.
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.settle();
    let under_bar = (corner.0, corner.1 + 36);
    world.shows_panel(MENU_WIDTH, &menu(false), None, under_bar);
    world.tap(KEY_UP);
    world.tap(KEY_UP);
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(false), Some(6), under_bar);
    world.tap(KEY_ENTER);
    world.settle();
    // Chosen, the program is told the bar is windowd's whole, and
    // asked to draw its window so.
    assert_eq!(world.ask(&program, |client| client.bar().unwrap()), Some(FULL_BAR));
    world.show(&program, FULL, 8);
    world.shows_frame(&full, corner, &look(true));
    world.shows_own(&full, corner, 8);
    // The line is ticked now, and choosing it again lets the
    // application share.
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(true), None, under_bar);
    world.tap(KEY_UP);
    world.tap(KEY_UP);
    world.tap(KEY_ENTER);
    world.settle();
    assert_eq!(world.ask(&program, |client| client.bar().unwrap()), Some(SHARED_BAR));
    world.show(&program, SHARED, 9);
    world.shows_frame(&shared(SHARED, Shape::Normal), corner, &look(true));
}

#[test]
fn the_person_s_choice_holds_for_every_window_of_the_application_and_those_to_come() {
    let mut world = World::one_screen();
    let first = world.sharing();
    world.choose_full_bar();
    assert_eq!(world.ask(&first, |client| client.bar().unwrap()), Some(FULL_BAR));
    // A window of the same application that asks afterwards is told
    // the bar is windowd's from the first.
    let second = world.program();
    let said = world.ask(&second, |client| {
        client.open(Open { shares_bar: true, ..Open::default() }).unwrap();
        client.bar().unwrap()
    });
    assert_eq!(said, Some(FULL_BAR));
    world.show(&second, FULL, 8);
    // Both are told when the person lets the application share again:
    // the second is in front and has the menu.
    world.choose_full_bar();
    assert_eq!(world.ask(&first, |client| client.bar().unwrap()), Some(SHARED_BAR));
    assert_eq!(world.ask(&second, |client| client.bar().unwrap()), Some(SHARED_BAR));
}

#[test]
fn a_second_title_bar_object_for_a_window_ends_the_program() {
    let mut world = World::one_screen();
    let program = world.sharing();
    let again = world.ask(&program, Client::share_bar);
    let said = again.unwrap_err();
    assert!(said.contains("already"), "{said}");
    world.settle();
    let (_, corner) = framed(FULL);
    assert_eq!(world.colour((corner.0 + 300) as u32, (corner.1 + 200) as u32), BACKGROUND);
}

#[test]
fn a_program_that_asks_who_decorates_its_window_is_told_windowd_does() {
    let mut world = World::one_screen();
    let decorated = |heard: &Heard| matches!(heard, Heard::Decorated { .. });
    // A toolkit says how it would have the window decorated before the
    // window is first configured, and is answered with that configure.
    let early = world.program();
    world.ask(&early, |client| {
        client.open(Open { decoration: Some(Decoration::Own), ..Open::default() }).unwrap();
    });
    let heard = world.heard(&early, decorated);
    assert_eq!(heard.last(), Some(&Heard::Decorated { by_server: true }));
    world.show(&early, FULL, 7);
    let (full, corner) = framed(FULL);
    world.shows_frame(&full, corner, &look(true));
    world.shows_own(&full, corner, 7);
    // Whatever it asks afterwards, nothing changes for it to hear.
    for how in [Decoration::Either, Decoration::Own, Decoration::Servers] {
        world.ask(&early, move |client| client.decorate(how).unwrap());
        let heard = world.told(&early);
        assert!(!heard.iter().any(decorated), "{how:?}: {heard:?}");
    }
    world.shows_frame(&full, corner, &look(true));
    // One that asks once its window shows is answered then.
    let late = world.program();
    world.show(&late, FULL, 8);
    world.ask(&late, |client| client.decorate(Decoration::Own).unwrap());
    let heard = world.heard(&late, decorated);
    assert_eq!(heard.last(), Some(&Heard::Decorated { by_server: true }));
}
