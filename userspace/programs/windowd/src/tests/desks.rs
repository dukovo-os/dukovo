// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A screen's desktops (ADR 0726): made, closed and stepped through
//! by the keys, each with the windows that are on it.

use display_pattern::pattern;
use input::abi::{
    KEY_D, KEY_F4, KEY_LEFT, KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_RIGHT,
    KEY_TAB,
};

use super::screens::{pixel, to_second};
use super::{Program, World, framed, middle};
use crate::screens::BACKGROUND;

/// Presses `key` with `held` down, and lets all go.
fn chord(world: &mut World, held: &[u16], key: u16) {
    for held in held {
        world.key(*held, true);
    }
    world.tap(key);
    for held in held.iter().rev() {
        world.key(*held, false);
    }
    world.settle();
}

const SUPER_CTRL: [u16; 2] = [KEY_LEFTMETA, KEY_LEFTCTRL];
const SUPER_CTRL_SHIFT: [u16; 3] = [KEY_LEFTMETA, KEY_LEFTCTRL, KEY_LEFTSHIFT];

/// Whether the window of `program` is the one the person works in, as
/// the program was last told.
fn active(world: &mut World, program: &Program) -> bool {
    world.ask(program, |client| {
        client.caught_up().unwrap();
        client.active()
    })
}

/// The colour the first screen shows where the own pixels of a window
/// of 320 by 200 begin that shows in the middle, a step down and right
/// for each window before it.
fn at_step(world: &mut World, step: u32) -> [u8; 3] {
    let (left, top) = middle((320, 200));
    world.colour(left + step * 32, top + step * 32)
}

#[test]
fn a_new_desktop_is_empty_and_the_one_before_keeps_its_windows() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    assert!(active(&mut world, &program));
    chord(&mut world, &SUPER_CTRL, KEY_D);
    assert_eq!(at_step(&mut world, 0), BACKGROUND);
    // No window has the keyboard on a desktop with none.
    assert!(!active(&mut world, &program));
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
    assert!(active(&mut world, &program));
    // The first is the row's end.
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
}

#[test]
fn a_window_shows_on_the_desktop_it_came_to_and_the_keyboard_follows_the_desktops() {
    let mut world = World::one_screen();
    let first = world.program();
    world.show(&first, (320, 200), 5);
    chord(&mut world, &SUPER_CTRL, KEY_D);
    let second = world.program();
    world.show(&second, (320, 200), 6);
    assert_eq!(at_step(&mut world, 1), pattern::colour(6, 0, 0));
    assert_eq!(at_step(&mut world, 0), BACKGROUND);
    assert!(active(&mut world, &second) && !active(&mut world, &first));
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
    let (left, top) = middle((320, 200));
    // Where the second alone would show.
    assert_eq!(world.colour(left + 32 + 310, top + 32 + 190), BACKGROUND);
    assert!(active(&mut world, &first) && !active(&mut world, &second));
    // The list of windows holds those of the desktop that shows: Alt
    // with Tab finds no other window.
    chord(&mut world, &[KEY_LEFTALT], KEY_TAB);
    assert!(active(&mut world, &first) && !active(&mut world, &second));
    assert_eq!(world.colour(left + 32 + 310, top + 32 + 190), BACKGROUND);
}

#[test]
fn with_shift_the_window_goes_along_to_the_next_desktop() {
    let mut world = World::one_screen();
    let first = world.program();
    world.show(&first, (320, 200), 5);
    let second = world.program();
    world.show(&second, (320, 200), 6);
    chord(&mut world, &SUPER_CTRL, KEY_D);
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    // The second window, which the person works in, goes to the new
    // desktop; the first stays.
    chord(&mut world, &SUPER_CTRL_SHIFT, KEY_RIGHT);
    assert_eq!(at_step(&mut world, 1), pattern::colour(6, 0, 0));
    assert_eq!(at_step(&mut world, 0), BACKGROUND);
    assert!(active(&mut world, &second));
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
    assert_eq!(at_step(&mut world, 1), pattern::colour(5, 32, 32));
    assert!(active(&mut world, &first));
}

#[test]
fn a_desktop_that_closes_leaves_its_windows_on_the_one_before() {
    let mut world = World::one_screen();
    let first = world.program();
    world.show(&first, (320, 200), 5);
    chord(&mut world, &SUPER_CTRL, KEY_D);
    let second = world.program();
    world.show(&second, (320, 200), 6);
    chord(&mut world, &SUPER_CTRL, KEY_F4);
    // Both show, the second in front, as it was last used.
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
    assert_eq!(at_step(&mut world, 1), pattern::colour(6, 0, 0));
    assert!(active(&mut world, &second));
    // The only desktop does not close, and there is none after it.
    chord(&mut world, &SUPER_CTRL, KEY_F4);
    chord(&mut world, &SUPER_CTRL, KEY_RIGHT);
    assert_eq!(at_step(&mut world, 1), pattern::colour(6, 0, 0));
}

#[test]
fn the_first_desktop_that_closes_leaves_its_windows_on_the_one_after() {
    let mut world = World::one_screen();
    let first = world.program();
    world.show(&first, (320, 200), 5);
    chord(&mut world, &SUPER_CTRL, KEY_D);
    let second = world.program();
    world.show(&second, (320, 200), 6);
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    chord(&mut world, &SUPER_CTRL, KEY_F4);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
    // The first window, on its own desktop until now, was used last.
    assert_eq!(at_step(&mut world, 1), pattern::colour(5, 32, 32));
    assert!(active(&mut world, &first));
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
}

#[test]
fn each_screen_has_desktops_of_its_own() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let first = world.program();
    world.show(&first, (320, 200), 5);
    // The pointer goes to the second screen, which gets a desktop.
    to_second(&mut world);
    chord(&mut world, &SUPER_CTRL, KEY_D);
    assert_eq!(at_step(&mut world, 0), pattern::colour(5, 0, 0));
    // A window carried there joins the desktop that shows there.
    chord(&mut world, &[KEY_LEFTMETA, KEY_LEFTSHIFT], KEY_RIGHT);
    let (layout, (left, top)) = framed((320, 200));
    let carried = (left * 800 / 1024 + layout.surface.x, top * 600 / 768 + layout.surface.y);
    assert_eq!(pixel(&mut world, 1, carried), pattern::colour(5, 0, 0));
    assert_eq!(at_step(&mut world, 0), BACKGROUND);
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(pixel(&mut world, 1, carried), BACKGROUND);
    chord(&mut world, &SUPER_CTRL, KEY_RIGHT);
    assert_eq!(pixel(&mut world, 1, carried), pattern::colour(5, 0, 0));
}

#[test]
fn a_window_goes_back_to_its_desktop_when_its_screen_returns() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    to_second(&mut world);
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let (layout, _) = framed((320, 200));
    let home = (
        (800 - layout.frame.w) / 2 + layout.surface.x,
        (600 - layout.frame.h) / 2 + layout.surface.y,
    );
    assert_eq!(pixel(&mut world, 1, home), pattern::colour(5, 0, 0));
    chord(&mut world, &SUPER_CTRL, KEY_D);
    assert_eq!(pixel(&mut world, 1, home), BACKGROUND);
    // The screen goes: the window shows on the one that is left,
    // whichever desktop of its screen it was on.
    world.kernel.borrow_mut().found(1, None);
    world.settle();
    let (left, top) = ((800 - layout.frame.w) / 2, (600 - layout.frame.h) / 2);
    let carried = (left * 1024 / 800 + layout.surface.x, top * 768 / 600 + layout.surface.y);
    assert_eq!(world.colour(carried.0 as u32, carried.1 as u32), pattern::colour(5, 0, 0));
    // The screen returns with its desktops: the window is on the first
    // of them again, and the second shows.
    world.kernel.borrow_mut().found(1, Some((800, 600)));
    world.settle();
    assert_eq!(world.colour(carried.0 as u32, carried.1 as u32), BACKGROUND);
    assert_eq!(pixel(&mut world, 1, home), BACKGROUND);
    to_second(&mut world);
    chord(&mut world, &SUPER_CTRL, KEY_LEFT);
    assert_eq!(pixel(&mut world, 1, home), pattern::colour(5, 0, 0));
}
