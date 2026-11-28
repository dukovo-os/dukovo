// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Windows snapped to the sides of a screen and to their parts
//! (ADR 0727), by the keys and by the pointer.

use display_pattern::pattern;
use input::abi::{KEY_DOWN, KEY_LEFT, KEY_LEFTMETA, KEY_RIGHT, KEY_UP};
use window_frame::{Bar, Layout, Shape, Theme};

use super::laid::settings;
use super::{Program, World, middle};
use crate::screens::BACKGROUND;

/// The size a window's program is asked to draw for a frame's box of
/// `frame`, and where its own pixels begin in the box.
fn inside((width, height): (i32, i32)) -> ((u32, u32), (i32, i32)) {
    // A frame's sides and its title bar are the same around any size.
    let layout = Layout::of(&Theme::basic(), (100, 100), Shape::Normal, Bar::Full, None);
    let (sides, rows) = (layout.frame.w - 100, layout.frame.h - 100);
    (((width - sides) as u32, (height - rows) as u32), (layout.surface.x, layout.surface.y))
}

/// Presses Super with `arrow`, has `program` draw its window as it is
/// asked to with the pattern of `seed`, and returns the size drawn.
fn arrow(world: &mut World, program: &Program, arrow: u16, seed: u32) -> (u32, u32) {
    world.key(KEY_LEFTMETA, true);
    world.tap(arrow);
    world.key(KEY_LEFTMETA, false);
    let (size, _) = world.follow(program, (320, 200), seed);
    world.settle();
    size
}

/// Presses Super with `arrow`, for a window that is then of the size
/// it was: its program is asked for nothing it draws anew.
fn arrow_alone(world: &mut World, arrow: u16) {
    world.key(KEY_LEFTMETA, true);
    world.tap(arrow);
    world.key(KEY_LEFTMETA, false);
    world.settle();
}

/// Fails unless the first screen shows a window that fills `zone`,
/// its corner and its size, with the pattern of `seed`, and `size` is
/// what its program drew for that.
fn fills(world: &mut World, zone: ((i32, i32), (i32, i32)), size: (u32, u32), seed: u32) {
    let (corner, frame) = zone;
    let (asked, within) = inside(frame);
    assert_eq!(size, asked);
    let (left, top) = ((corner.0 + within.0) as u32, (corner.1 + within.1) as u32);
    assert_eq!(world.colour(left, top), pattern::colour(seed, 0, 0));
    let (right, bottom) = (left + size.0 - 1, top + size.1 - 1);
    assert_eq!(world.colour(right, bottom), pattern::colour(seed, size.0 - 1, size.1 - 1));
}

#[test]
fn super_with_left_and_right_snaps_a_window_to_that_side() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let size = arrow(&mut world, &program, KEY_LEFT, 6);
    fills(&mut world, ((0, 0), (512, 768)), size, 6);
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left + 300, top + 100), BACKGROUND);
    arrow_alone(&mut world, KEY_RIGHT);
    fills(&mut world, ((512, 0), (512, 768)), size, 6);
    assert_eq!(world.colour(100, 300), BACKGROUND);
}

#[test]
fn up_and_down_go_through_a_side_s_parts_and_down_from_the_bottom_restores() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let size = arrow(&mut world, &program, KEY_RIGHT, 6);
    fills(&mut world, ((512, 0), (512, 768)), size, 6);
    let size = arrow(&mut world, &program, KEY_UP, 7);
    fills(&mut world, ((512, 0), (512, 384)), size, 7);
    assert_eq!(world.colour(700, 500), BACKGROUND);
    arrow_alone(&mut world, KEY_LEFT);
    fills(&mut world, ((0, 0), (512, 384)), size, 7);
    let size = arrow(&mut world, &program, KEY_DOWN, 9);
    fills(&mut world, ((0, 0), (512, 768)), size, 9);
    let size = arrow(&mut world, &program, KEY_DOWN, 10);
    fills(&mut world, ((0, 384), (512, 384)), size, 10);
    // The window is as it was before it was snapped, where it was.
    let size = arrow(&mut world, &program, KEY_DOWN, 11);
    assert_eq!(size, (320, 200));
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(11, 0, 0));
    assert_eq!(world.colour(100, 600), BACKGROUND);
}

#[test]
fn up_from_the_upper_part_maximizes_and_a_maximized_window_snaps() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    arrow(&mut world, &program, KEY_LEFT, 6);
    arrow(&mut world, &program, KEY_UP, 7);
    world.key(KEY_LEFTMETA, true);
    world.tap(KEY_UP);
    world.key(KEY_LEFTMETA, false);
    let bar = Theme::basic().bar as u32;
    assert_eq!(world.follow(&program, (320, 200), 8), ((1024, 768 - bar), true));
    world.settle();
    assert_eq!(world.colour(1023, 767), pattern::colour(8, 1023, 767 - bar));
    let size = arrow(&mut world, &program, KEY_RIGHT, 9);
    fills(&mut world, ((512, 0), (512, 768)), size, 9);
    // Maximized and restored from there, it is as it was at first.
    world.key(KEY_LEFTMETA, true);
    world.tap(KEY_UP);
    world.tap(KEY_UP);
    world.key(KEY_LEFTMETA, false);
    world.follow(&program, (320, 200), 10);
    world.follow(&program, (320, 200), 10);
    let size = arrow(&mut world, &program, KEY_DOWN, 11);
    assert_eq!(size, (320, 200));
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(11, 0, 0));
}

#[test]
fn a_snapped_window_fills_its_zone_of_a_screen_that_changes() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let size = arrow(&mut world, &program, KEY_RIGHT, 6);
    fills(&mut world, ((512, 0), (512, 768)), size, 6);
    world.desktop.set(&settings(r#"{"screens": {"virtio0/0": {"scale": 2}}}"#)).unwrap();
    let (size, _) = world.follow(&program, (320, 200), 7);
    world.settle();
    // The right side of 512 by 384 points, each two pixels.
    let (asked, within) = inside((256, 384));
    assert_eq!(size, asked);
    let (left, top) = ((256 + within.0) as u32 * 2, within.1 as u32 * 2);
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
    assert_eq!(world.colour(left + 1, top + 1), pattern::colour(7, 0, 0));
}
