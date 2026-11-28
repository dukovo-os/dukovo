// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Windows that are known again show where they were
//! (`dukovo_session_v1`): ADR 0729.

use std::path::{Path, PathBuf};
use std::time::Duration;

use display_pattern::pattern;
use input::abi::{BTN_LEFT, KEY_DOWN, KEY_LEFT, KEY_UP};
use window_frame::Theme;
use window_test::{Client, Heard, Open};

use super::screens::{pixel, to_second};
use super::snap::{arrow, fills};
use super::{PATIENCE, Program, World, framed, look};
use crate::screens::BACKGROUND;

/// The size of the windows' own pixels as their programs choose it.
const SIZE: (u32, u32) = (320, 200);

/// How far a window is dragged.
const BY: (i32, i32) = (60, 40);

/// The file the places are kept in for the test named `test`, in a
/// folder of its own, which is not there yet.
fn file(test: &str) -> PathBuf {
    let folder = format!("windowd-kept-{}-{test}", std::process::id());
    let folder = std::env::temp_dir().join(folder);
    let _ = std::fs::remove_dir_all(&folder);
    folder.join(".config/windowd-session.json")
}

/// A desktop on `screens` that keeps its windows' places in `file`.
fn world_on(file: &Path, screens: &[Option<(u32, u32)>]) -> World {
    let mut world = World::new(screens, false);
    world.desktop.keep_places(file.to_owned());
    assert_eq!(world.desktop.said(), Vec::<String>::new());
    world
}

/// Has `program` ask for a window it names `name`, and draw it as it is
/// asked to with the pattern of `seed`: a side the desktop leaves to
/// the program is `own`'s. Returns the size drawn and whether the
/// window was asked for maximized, once the frame that shows it has
/// shown.
fn opens(
    world: &mut World,
    program: &Program,
    name: &'static str,
    own: (u32, u32),
    seed: u32,
) -> ((u32, u32), bool) {
    let drawn = world.ask(program, move |client| {
        client.open(Open { name: Some(name), ..Open::default() }).unwrap();
        // What the first configure asked is read here, and is not for
        // whoever next waits to be asked for a size.
        let size = |heard: &Heard| matches!(heard, Heard::Size { .. }).then_some(());
        let _ = client.wait(Duration::ZERO, size);
        let (width, height, maximized) = client.asked();
        let side = |asked: i32, own: u32| if asked > 0 { asked as u32 } else { own };
        let size = (side(width, own.0), side(height, own.1));
        client.show(size, seed).unwrap();
        let frame = |heard: &Heard| matches!(heard, Heard::Frame(_)).then_some(());
        client.wait(PATIENCE, frame).unwrap();
        (size, maximized)
    });
    world.settle();
    drawn
}

/// Has `program` take its window away.
fn closes(world: &mut World, program: &Program) {
    world.ask(program, Client::close_window).unwrap();
    world.settle();
}

/// Drags the window whose frame's corner is at `corner` by its title
/// bar, [`BY`] to the right and down, and returns where the corner is
/// then.
fn drags(world: &mut World, corner: (i32, i32)) -> (i32, i32) {
    let held = (corner.0 + 100, corner.1 + 15);
    world.point(held.0, held.1);
    world.button(BTN_LEFT, true);
    world.point(held.0 + BY.0 / 2, held.1 + BY.1 / 2);
    world.point(held.0 + BY.0, held.1 + BY.1);
    world.button(BTN_LEFT, false);
    world.settle();
    (corner.0 + BY.0, corner.1 + BY.1)
}

#[test]
fn a_named_window_shows_again_where_it_was_at_the_size_it_had() {
    let file = file("again");
    let mut world = world_on(&file, &[Some((1024, 768))]);
    let program = world.program();
    // Not known yet, it is placed as any new window is, at the size
    // its program chooses.
    assert_eq!(opens(&mut world, &program, "main", SIZE, 7), (SIZE, false));
    let (layout, corner) = framed(SIZE);
    world.shows_frame(&layout, corner, &look(true));
    let moved = drags(&mut world, corner);
    world.shows_frame(&layout, moved, &look(true));
    closes(&mut world, &program);
    assert_eq!(world.colour(moved.0 as u32, moved.1 as u32), BACKGROUND);
    // Known again, it is asked for the size it had, whatever its
    // program would choose now, and shows where it was.
    assert_eq!(opens(&mut world, &program, "main", (200, 100), 8), (SIZE, false));
    world.shows_frame(&layout, moved, &look(true));
    let own = ((moved.0 + layout.surface.x) as u32, (moved.1 + layout.surface.y) as u32);
    assert_eq!(world.colour(own.0, own.1), pattern::colour(8, 0, 0));
    // A window of another name is a new one: at the size its program
    // chooses, a step along from the middle.
    closes(&mut world, &program);
    assert_eq!(opens(&mut world, &program, "other", (200, 100), 9), ((200, 100), false));
    assert_eq!(world.colour(moved.0 as u32, moved.1 as u32), BACKGROUND);
}

#[test]
fn the_places_are_written_a_second_after_the_windows_stay_and_outlive_windowd() {
    let file = file("outlive");
    let (layout, corner) = framed(SIZE);
    let moved = {
        let mut world = world_on(&file, &[Some((1024, 768))]);
        let program = world.program();
        opens(&mut world, &program, "main", SIZE, 7);
        let moved = drags(&mut world, corner);
        assert!(!file.exists());
        // A turn is two milliseconds.
        let mut turns = 0;
        while !file.exists() {
            world.turn();
            turns += 1;
            assert!(turns < 2000, "the places were never written");
        }
        assert!(turns > 300, "written after {turns} turns");
        assert_eq!(world.desktop.said(), Vec::<String>::new());
        moved
    };
    let mut world = world_on(&file, &[Some((1024, 768))]);
    let program = world.program();
    assert_eq!(opens(&mut world, &program, "main", (200, 100), 8), (SIZE, false));
    world.shows_frame(&layout, moved, &look(true));
    std::fs::remove_dir_all(file.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn a_window_that_was_snapped_or_maximized_is_asked_to_be_so_for_its_first_frame() {
    let file = file("filled");
    let mut world = world_on(&file, &[Some((1024, 768))]);
    let program = world.program();
    opens(&mut world, &program, "main", SIZE, 7);
    let (layout, corner) = framed(SIZE);
    let moved = drags(&mut world, corner);
    let left_half = ((0, 0), (512, 768));
    let size = arrow(&mut world, &program, KEY_LEFT, 7);
    fills(&mut world, left_half, size, 7);
    closes(&mut world, &program);
    let (size, maximized) = opens(&mut world, &program, "main", SIZE, 8);
    assert!(!maximized);
    fills(&mut world, left_half, size, 8);
    // Down to the side's lower part and from there restored, it is
    // where it was before it was snapped, as large.
    arrow(&mut world, &program, KEY_DOWN, 9);
    assert_eq!(arrow(&mut world, &program, KEY_DOWN, 9), SIZE);
    world.shows_frame(&layout, moved, &look(true));

    arrow(&mut world, &program, KEY_UP, 10);
    closes(&mut world, &program);
    let bar = Theme::basic().bar as u32;
    let all = ((1024, 768 - bar), true);
    assert_eq!(opens(&mut world, &program, "main", SIZE, 11), all);
    assert_eq!(world.colour(1023, 767), pattern::colour(11, 1023, 767 - bar));
    assert_eq!(arrow(&mut world, &program, KEY_DOWN, 12), SIZE);
    world.shows_frame(&layout, moved, &look(true));
}

#[test]
fn a_name_is_one_window_s_at_a_time_and_has_a_byte_at_the_least() {
    let file = file("one");
    let mut world = world_on(&file, &[Some((1024, 768))]);
    let (first, second) = (world.program(), world.program());
    opens(&mut world, &first, "main", SIZE, 7);
    let (layout, corner) = framed(SIZE);
    let moved = drags(&mut world, corner);
    // While the first has the name, a second window of the application
    // that is given it has none: it is a new window.
    assert_eq!(opens(&mut world, &second, "main", (200, 100), 8), ((200, 100), false));
    closes(&mut world, &second);
    closes(&mut world, &first);
    assert_eq!(opens(&mut world, &second, "main", (200, 100), 9), (SIZE, false));
    world.shows_frame(&layout, moved, &look(true));
    // A name of no bytes is the program's mistake.
    let empty = world.ask(&first, |client| client.open(Open { name: Some(""), ..Open::default() }));
    assert!(empty.is_err(), "{empty:?}");
}

#[test]
fn a_window_whose_screen_is_away_shows_on_the_one_there_is_and_goes_back() {
    let file = file("away");
    let two = [Some((1024, 768)), Some((800, 600))];
    let mut world = world_on(&file, &two);
    let program = world.program();
    // The pointer is on the second screen, and the window shows there.
    to_second(&mut world);
    opens(&mut world, &program, "main", (160, 100), 5);
    let layout = window_frame::Layout::of(
        &Theme::basic(),
        (160, 100),
        window_frame::Shape::Normal,
        window_frame::Bar::Full,
        None,
    );
    let corner = ((800 - layout.frame.w) / 2, (600 - layout.frame.h) / 2);
    let own = (corner.0 + layout.surface.x, corner.1 + layout.surface.y);
    assert_eq!(pixel(&mut world, 1, own), pattern::colour(5, 0, 0));
    closes(&mut world, &program);
    world.kernel.borrow_mut().found(1, None);
    world.settle();
    // As far across and down the first screen as it was the second.
    opens(&mut world, &program, "main", (200, 100), 6);
    let carried =
        (corner.0 * 1024 / 800 + layout.surface.x, corner.1 * 768 / 600 + layout.surface.y);
    assert_eq!(pixel(&mut world, 0, carried), pattern::colour(6, 0, 0));
    // And back on its own when that is there again.
    world.kernel.borrow_mut().found(1, Some((800, 600)));
    world.settle();
    assert_eq!(pixel(&mut world, 1, own), pattern::colour(6, 0, 0));
    assert_eq!(pixel(&mut world, 0, carried), BACKGROUND);
    // Known again while its screen is there, it shows on it wherever
    // the pointer is.
    closes(&mut world, &program);
    world.point(100, 100);
    opens(&mut world, &program, "main", (200, 100), 7);
    assert_eq!(pixel(&mut world, 1, own), pattern::colour(7, 0, 0));
}
