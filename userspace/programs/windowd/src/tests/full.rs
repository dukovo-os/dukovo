// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Full-screen windows (ADR 0728): a window fills its screen with no
//! frame when its program asks, and is as it was before after.

use display_pattern::pattern;
use input::abi::{KEY_DOWN, KEY_ENTER, KEY_LEFT, KEY_LEFTALT, KEY_LEFTMETA, KEY_SPACE};
use window_frame::panel::{Entry, Mark};
use window_frame::{Bar, Layout, Shape, Theme};
use window_test::{Heard, Open};

use super::screens::pixel;
use super::{Program, World, framed, look, middle};
use crate::screens::BACKGROUND;

/// Has `program` ask for its window to be full screen on the screen
/// named `on`, or no longer for `full` false, and returns the size the
/// desktop then asks for, having said which of the two.
fn asks(world: &mut World, program: &Program, full: bool, on: Option<&'static str>) -> (i32, i32) {
    world.ask(program, move |client| client.full(full, on)).unwrap();
    let heard = world.heard(program, |heard| matches!(heard, Heard::Size { .. }));
    assert!(heard.contains(&Heard::Full(full)), "{heard:?}");
    match heard.last() {
        Some(Heard::Size { width, height, .. }) => (*width, *height),
        _ => unreachable!(),
    }
}

#[test]
fn a_program_asks_for_its_window_to_fill_the_screen_with_no_frame_and_to_be_as_it_was() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    assert_eq!(asks(&mut world, &program, true, None), (1024, 768));
    // Until the program has drawn so, the window is as it was.
    world.settle();
    let (layout, corner) = framed((320, 200));
    world.shows_frame(&layout, corner, &look(true));
    world.show(&program, (1024, 768), 6);
    world.settle();
    for (x, y) in [(0, 0), (1023, 0), (0, 767), (1023, 767), (500, 20)] {
        assert_eq!(world.colour(x, y), pattern::colour(6, x, y), "({x}, {y})");
    }
    // And back: where it was, at the size it had, with its frame.
    assert_eq!(asks(&mut world, &program, false, None), (320, 200));
    world.show(&program, (320, 200), 7);
    world.settle();
    world.shows_frame(&layout, corner, &look(true));
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
    assert_eq!(world.colour(3, 3), BACKGROUND);
}

#[test]
fn a_window_that_was_snapped_or_maximized_is_that_again_after() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    world.chord(KEY_LEFTMETA, KEY_LEFT);
    let (snapped, _) = world.follow(&program, (320, 200), 6);
    assert_eq!(asks(&mut world, &program, true, None), (1024, 768));
    world.show(&program, (1024, 768), 7);
    world.settle();
    assert_eq!(world.colour(1023, 767), pattern::colour(7, 1023, 767));
    let back = asks(&mut world, &program, false, None);
    assert_eq!((back.0 as u32, back.1 as u32), snapped);
    world.show(&program, snapped, 8);
    world.settle();
    let layout = Layout::of(&Theme::basic(), (back.0, back.1), Shape::Normal, Bar::Full, None);
    assert_eq!((layout.frame.w, layout.frame.h), (512, 768));
    let own = (layout.surface.x as u32, layout.surface.y as u32);
    assert_eq!(world.colour(own.0, own.1), pattern::colour(8, 0, 0));
    assert_ne!(world.colour(own.0, own.1 - 1), pattern::colour(8, 0, 0));
    assert_eq!(world.colour(700, 300), BACKGROUND);
    // Maximized, full screen, and maximized again.
    world.ask(&program, |client| client.maximize(true)).unwrap();
    let bar = Theme::basic().bar as u32;
    assert_eq!(world.follow(&program, (320, 200), 9), ((1024, 768 - bar), true));
    assert_eq!(asks(&mut world, &program, true, None), (1024, 768));
    world.show(&program, (1024, 768), 10);
    world.settle();
    assert_eq!(world.colour(0, 0), pattern::colour(10, 0, 0));
    assert_eq!(asks(&mut world, &program, false, None), (1024, (768 - bar) as i32));
    world.show(&program, (1024, 768 - bar), 11);
    world.settle();
    assert_eq!(world.colour(0, bar), pattern::colour(11, 0, 0));
    assert_eq!(world.colour(1023, 767), pattern::colour(11, 1023, 767 - bar));
}

#[test]
fn a_window_drawn_smaller_than_its_screen_is_in_the_middle_with_black_around_it() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    assert_eq!(asks(&mut world, &program, true, None), (1024, 768));
    world.show(&program, (800, 600), 6);
    world.settle();
    assert_eq!(world.colour(112, 84), pattern::colour(6, 0, 0));
    assert_eq!(world.colour(911, 683), pattern::colour(6, 799, 599));
    for (x, y) in [(0, 0), (111, 84), (112, 83), (912, 683), (1023, 767)] {
        assert_eq!(world.colour(x, y), [0, 0, 0], "({x}, {y})");
    }
}

#[test]
fn a_program_names_the_screen_its_window_fills() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    world.show(&program, (320, 200), 5);
    assert_eq!(asks(&mut world, &program, true, Some("virtio0/1")), (800, 600));
    world.show(&program, (800, 600), 6);
    world.settle();
    assert_eq!(pixel(&mut world, 1, (0, 0)), pattern::colour(6, 0, 0));
    assert_eq!(pixel(&mut world, 1, (799, 599)), pattern::colour(6, 799, 599));
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), BACKGROUND);
    // As it was before, it is on the screen it was on.
    assert_eq!(asks(&mut world, &program, false, None), (320, 200));
    world.show(&program, (320, 200), 7);
    world.settle();
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
    assert_eq!(pixel(&mut world, 1, (0, 0)), BACKGROUND);
}

#[test]
fn a_window_that_first_shows_full_screen_does_on_the_screen_its_program_named() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    world.ask(&program, |client| client.open(Open::default())).unwrap();
    assert_eq!(asks(&mut world, &program, true, Some("virtio0/1")), (800, 600));
    world.show(&program, (800, 600), 5);
    world.settle();
    assert_eq!(pixel(&mut world, 1, (0, 0)), pattern::colour(5, 0, 0));
    assert_eq!(world.colour(512, 384), BACKGROUND);
    // It was nowhere before: it shows in the middle of that screen at
    // the size its program chooses.
    assert_eq!(asks(&mut world, &program, false, None), (0, 0));
    world.show(&program, (320, 200), 6);
    world.settle();
    let layout = Layout::of(&Theme::basic(), (320, 200), Shape::Normal, Bar::Full, None);
    let corner = ((800 - layout.frame.w) / 2, (600 - layout.frame.h) / 2);
    let own = (corner.0 + layout.surface.x, corner.1 + layout.surface.y);
    assert_eq!(pixel(&mut world, 1, own), pattern::colour(6, 0, 0));
}

#[test]
fn the_keys_and_the_window_menu_put_a_full_screen_window_back() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    assert_eq!(asks(&mut world, &program, true, None), (1024, 768));
    world.show(&program, (1024, 768), 6);
    world.settle();
    // The menu, at the screen's corner: Restore and no other change of
    // the window's place.
    let line = |words, key, off| Entry::Line { words, key, mark: Mark::None, off };
    let menu = [
        line("Restore", "", false),
        line("Move", "", true),
        line("Size", "", true),
        line("Minimize", "", false),
        line("Maximize", "", true),
        Entry::Gap,
        line("Close", "Alt+F4", false),
    ];
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.settle();
    world.shows_panel(330, &menu, None, (0, 0));
    world.tap(KEY_DOWN);
    world.tap(KEY_ENTER);
    let heard = world.heard(&program, |heard| matches!(heard, Heard::Size { .. }));
    assert!(heard.contains(&Heard::Full(false)), "{heard:?}");
    world.show(&program, (320, 200), 7);
    world.settle();
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
    // Super and the down arrow do the same.
    assert_eq!(asks(&mut world, &program, true, None), (1024, 768));
    world.show(&program, (1024, 768), 8);
    world.chord(KEY_LEFTMETA, KEY_DOWN);
    let heard = world.heard(&program, |heard| matches!(heard, Heard::Size { .. }));
    assert!(heard.contains(&Heard::Full(false)), "{heard:?}");
    assert_eq!(heard.last(), Some(&Heard::Size { width: 320, height: 200, maximized: false }));
}
