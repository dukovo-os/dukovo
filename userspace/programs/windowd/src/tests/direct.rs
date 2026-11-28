// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A full-screen window's own pixels as its screen's picture
//! (ADR 0728): windowd draws no frame while all a screen shows is one
//! buffer of a program's, and draws its own again when anything else
//! is to show.

use display_pattern::pattern;
use input::abi::{KEY_ESC, KEY_LEFTALT, KEY_SPACE};
use window_test::Client;

use super::full::asks;
use super::screens::set;
use super::{Program, World, framed, look, middle};
use crate::link::Source;
use crate::screens::BACKGROUND;

/// Shows a window of `program` and has it fill the first screen, of
/// 1024 by 768, with the pattern of `seed`.
pub(super) fn fills(world: &mut World, program: &Program, seed: u32) {
    world.show(program, (320, 200), 5);
    assert_eq!(asks(world, program, true, None), (1024, 768));
    world.show(program, (1024, 768), seed);
    world.settle();
}

/// How many buffers the window server has given back to `program`.
fn given_back(world: &mut World, program: &Program) -> u32 {
    world.ask(program, |client| {
        client.caught_up().unwrap();
        client.given_back()
    })
}

/// Has `program` draw its window anew in one `shade` of grey, in the
/// buffer that does not show.
fn repaints(world: &mut World, program: &Program, shade: u8) {
    world.ask(program, move |client| client.repaint(shade)).unwrap();
    world.settle();
}

#[test]
fn a_full_screen_window_as_large_as_its_screen_shows_from_where_its_program_drew_it() {
    let mut world = World::one_screen();
    let program = world.program();
    let own = world.showed_since(0).last().unwrap().picture;
    fills(&mut world, &program, 6);
    // The screen's picture is the program's buffer, where it is in the
    // program's file, and that buffer is not the program's meanwhile.
    let (picture, kept) = world.desktop.straight(0);
    assert!(picture.is_some());
    assert_eq!(kept, 1);
    {
        let brought = world.card.brought.borrow();
        let sources: Vec<Source> = brought.iter().map(|brought| brought.source).collect();
        assert_eq!(sources, [Source { offset: 0, size: (1024, 768), row_bytes: 4096 }]);
    }
    let last = world.showed_since(0).last().cloned().unwrap();
    assert!(last.main && last.picture != own, "{last:?}");
    for (x, y) in [(0, 0), (1023, 767), (500, 20)] {
        assert_eq!(world.colour(x, y), pattern::colour(6, x, y), "({x}, {y})");
    }
    // windowd drew no frame of it: its own picture is as it was.
    let (_, drawn) = world.desktop.shows(0);
    assert_eq!(drawn[..3], [BACKGROUND[2], BACKGROUND[1], BACKGROUND[0]]);
}

#[test]
fn each_frame_is_the_program_s_next_buffer_and_the_one_before_is_given_back_when_it_shows_no_more()
{
    let mut world = World::one_screen();
    let program = world.program();
    fills(&mut world, &program, 6);
    let before = given_back(&mut world, &program);
    let first = world.desktop.straight(0).0;
    repaints(&mut world, &program, 0x40);
    let second = world.desktop.straight(0);
    assert!(second.0.is_some() && second.0 != first, "{second:?}");
    assert_eq!(second.1, 1);
    assert_eq!(world.colour(700, 300), [0x40; 3]);
    assert_eq!(given_back(&mut world, &program), before + 1);
    // The first buffer again: its picture is made no second time.
    repaints(&mut world, &program, 0x80);
    assert_eq!(world.desktop.straight(0), (first, 1));
    assert_eq!(world.colour(700, 300), [0x80; 3]);
    assert_eq!(world.card.brought.borrow().len(), 2);
    assert_eq!(given_back(&mut world, &program), before + 2);
    // Each was a change of the main plane's picture, and none a frame
    // of windowd's.
    let showed = world.showed_since(world.showed() - 2);
    let last: Vec<bool> = showed.iter().map(|showed| showed.main).collect();
    assert_eq!(last, [true, true]);
}

#[test]
fn a_box_of_windowd_s_over_the_window_is_drawn_in_a_frame_of_windowd_s_own() {
    let mut world = World::one_screen();
    let program = world.program();
    fills(&mut world, &program, 6);
    let before = given_back(&mut world, &program);
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.settle();
    // The window's menu shows over the window, and the buffer is drawn
    // from as any other: it is the program's again.
    assert_eq!(world.desktop.straight(0), (None, 0));
    assert_ne!(world.colour(8, 8), pattern::colour(6, 8, 8));
    assert_eq!(world.colour(1023, 767), pattern::colour(6, 1023, 767));
    assert_eq!(world.card.brought.borrow().len(), 1);
    world.tap(KEY_ESC);
    world.settle();
    assert!(world.desktop.straight(0).0.is_some());
    assert_eq!(world.colour(8, 8), pattern::colour(6, 8, 8));
    assert_eq!(given_back(&mut world, &program), before);
}

#[test]
fn a_window_in_front_of_the_full_screen_one_is_drawn_and_shows_no_more_once_it_is_gone() {
    let mut world = World::one_screen();
    let program = world.program();
    fills(&mut world, &program, 6);
    let other = world.program();
    world.show(&other, (320, 200), 9);
    world.settle();
    assert_eq!(world.desktop.straight(0), (None, 0));
    // The second window shows a step down and to the right of where
    // the first did.
    let (layout, corner) = framed((320, 200));
    world.shows_own(&layout, (corner.0 + 32, corner.1 + 32), 9);
    let (left, top) = middle((320, 200));
    let (left, top) = (left + 32, top + 32);
    assert_eq!(world.colour(0, 0), pattern::colour(6, 0, 0));
    world.ask(&other, Client::minimize).unwrap();
    world.settle();
    assert!(world.desktop.straight(0).0.is_some());
    assert_eq!(world.colour(left, top), pattern::colour(6, left, top));
}

#[test]
fn a_window_put_back_shows_in_a_frame_of_windowd_s_own_on_either_kind_of_card() {
    for shared in [false, true] {
        let mut world = World::new(&[Some((1024, 768))], shared);
        let program = world.program();
        fills(&mut world, &program, 6);
        assert!(world.desktop.straight(0).0.is_some());
        assert_eq!(asks(&mut world, &program, false, None), (320, 200));
        world.show(&program, (320, 200), 7);
        world.settle();
        assert_eq!(world.desktop.straight(0), (None, 0), "shared: {shared}");
        let (layout, corner) = framed((320, 200));
        world.shows_frame(&layout, corner, &look(true));
        world.shows_own(&layout, corner, 7);
        assert_eq!(world.colour(3, 3), BACKGROUND);
        assert_eq!(world.colour(1020, 760), BACKGROUND);
    }
}

#[test]
fn a_card_that_takes_no_picture_of_a_program_s_is_asked_once_for_each_buffer() {
    let mut world = World::one_screen();
    world.card.takes_none.set(true);
    let program = world.program();
    fills(&mut world, &program, 6);
    assert_eq!(world.desktop.straight(0), (None, 0));
    assert_eq!(world.colour(1023, 767), pattern::colour(6, 1023, 767));
    assert_eq!(world.card.asked.get(), 1);
    repaints(&mut world, &program, 0x40);
    repaints(&mut world, &program, 0x80);
    repaints(&mut world, &program, 0xc0);
    assert_eq!(world.colour(700, 300), [0xc0; 3]);
    assert_eq!(world.card.asked.get(), 2);
}

#[test]
fn pixels_that_say_how_much_they_cover_show_from_where_they_are_if_the_window_hides_what_is_under()
{
    for hides_all in [false, true] {
        let mut world = World::one_screen();
        let program = world.program();
        world.ask(&program, move |client| client.cover(hides_all));
        fills(&mut world, &program, 6);
        assert_eq!(world.desktop.straight(0).0.is_some(), hides_all);
        assert_eq!(world.card.asked.get(), u32::from(hides_all));
    }
    // What the window says hides everything shows as it is.
    let mut world = World::one_screen();
    let program = world.program();
    world.ask(&program, |client| client.cover(true));
    fills(&mut world, &program, 6);
    assert_eq!(world.colour(1023, 767), pattern::colour(6, 1023, 767));
}

#[test]
fn a_screen_that_is_turned_or_zoomed_shows_frames_of_windowd_s_own() {
    let mut world = World::one_screen();
    set(&mut world, r#"{"screens": {"virtio0/0": {"turn": 180}}}"#);
    let program = world.program();
    fills(&mut world, &program, 6);
    assert_eq!(world.desktop.straight(0), (None, 0));
    assert_eq!(world.colour(0, 0), pattern::colour(6, 1023, 767));
    assert_eq!(world.card.asked.get(), 0);
    // At a zoom of 2 the buffer is a quarter of the screen's pixels.
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 2}}}"#);
    assert_eq!(world.desktop.straight(0), (None, 0));
    assert_eq!(world.card.asked.get(), 0);
}
