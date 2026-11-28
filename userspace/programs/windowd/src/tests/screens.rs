// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens as the person has set them (windowd.json(5),
//! ADR 0725): a screen's mode, zoom and turn, whether it is in use,
//! and where each is on the desktop.

use display_pattern::pattern;
use input::abi::{BTN_LEFT, KEY_LEFT, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_RIGHT};
use window_frame::{Bar, Layout, Shape, Theme};
use window_test::{Drawn, Screen};

use super::laid::settings;
use super::{Program, World};
use crate::screens::BACKGROUND;

/// Has the desktop be as the settings file of `text` says.
pub(super) fn set(world: &mut World, text: &str) {
    world.desktop.set(&settings(text)).unwrap();
    world.settle();
}

/// The colour the screen at place `screen` shows at (`x`, `y`) of its
/// picture.
pub(super) fn pixel(world: &mut World, screen: usize, (x, y): (i32, i32)) -> [u8; 3] {
    let (width, pixels) = world.shows(screen);
    let at = ((y as u32 * width + x as u32) * 4) as usize;
    [pixels[at + 2], pixels[at + 1], pixels[at]]
}

/// Where the corner of the frame of a window of `size` first is on a
/// screen whose area of the desktop is `area` wide and tall, from the
/// area's corner, and where the window's own pixels are from there:
/// the window is in the screen's middle.
fn placed(area: (i32, i32), (width, height): (u32, u32)) -> ((i32, i32), (i32, i32)) {
    let size = (width as i32, height as i32);
    let layout = Layout::of(&Theme::basic(), size, Shape::Normal, Bar::Full, None);
    let corner = ((area.0 - layout.frame.w) / 2, (area.1 - layout.frame.h) / 2);
    (corner, (layout.surface.x, layout.surface.y))
}

/// Where the own pixels of a window of `size` first show on such a
/// screen.
fn middle_in(area: (i32, i32), size: (u32, u32)) -> (i32, i32) {
    let (corner, within) = placed(area, size);
    (corner.0 + within.0, corner.1 + within.1)
}

/// What `program` is told of the screens, once it has heard the news
/// of them, by their names.
fn told(world: &mut World, program: &Program) -> Vec<Screen> {
    let mut screens = world.ask(program, |client| {
        client.caught_up().unwrap();
        client.screens()
    });
    screens.sort_by(|one, other| one.name.cmp(&other.name));
    screens
}

/// Points at the place (`x`, `y`) of the first screen, whose area of
/// the desktop is `area` wide and tall.
fn point_in(world: &mut World, area: (i32, i32), (x, y): (i32, i32)) {
    let now = world.now();
    let share = |at: i32, side: i32| (f64::from(at) + 0.5) / f64::from(side - 1);
    world.desktop.pointed(now, Some(share(x, area.0)), Some(share(y, area.1)));
}

/// Moves the pointer from the first screen to the second, which is to
/// its right.
pub(super) fn to_second(world: &mut World) {
    for _ in 0..4 {
        let now = world.now();
        world.desktop.moved(now, 400, 0);
        world.settle();
    }
}

#[test]
fn at_twice_the_size_a_point_of_the_desktop_is_four_pixels() {
    let mut world = World::one_screen();
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 2}}}"#);
    let program = world.program();
    world.show(&program, (160, 100), 5);
    let (left, top) = middle_in((512, 384), (160, 100));
    for (x, y) in [(0, 0), (37, 21), (159, 99)] {
        let colour = pattern::colour(5, x, y);
        let (x, y) = ((left + x as i32) * 2, (top + y as i32) * 2);
        for pixel_of_four in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
            assert_eq!(pixel(&mut world, 0, pixel_of_four), colour, "{pixel_of_four:?}");
        }
    }
    // The frame's border is as many points wide, each two pixels.
    let border = pixel(&mut world, 0, (left * 2 - 2, top * 2 + 40));
    assert_ne!(border, BACKGROUND);
    assert_eq!(pixel(&mut world, 0, (left * 2 - 1, top * 2 + 41)), border);
    let screens = told(&mut world, &program);
    let screen = &screens[0];
    assert_eq!((screen.width, screen.height, screen.scale, screen.turned), (1024, 768, 2, 0));
    // The pointer is where it is in pixels: the middle of the point
    // (100, 50) is the pixel (201, 101).
    let before = world.showed();
    point_in(&mut world, (512, 384), (100, 50));
    world.settle();
    let showed = world.showed_since(before);
    assert_eq!(showed.last().and_then(|showed| showed.pointer_at), Some((201, 101)), "{showed:?}");
}

#[test]
fn at_a_zoom_between_whole_numbers_programs_draw_at_the_next_and_the_screen_shows_less() {
    let mut world = World::one_screen();
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 1.5}}}"#);
    let program = world.program();
    world.ask(&program, |client| client.show_plain((160, 100), 200).unwrap());
    world.settle();
    // 1024 by 768 pixels are 683 by 512 points.
    let (left, top) = middle_in((683, 512), (160, 100));
    let inside = |x: i32, y: i32| (((left + x) * 3 + 1) / 2 + 1, ((top + y) * 3 + 1) / 2 + 1);
    for at in [inside(0, 0), inside(80, 50), inside(158, 98)] {
        assert_eq!(pixel(&mut world, 0, at), [200, 200, 200], "{at:?}");
    }
    assert_eq!(pixel(&mut world, 0, (1023, 767)), BACKGROUND);
    let screens = told(&mut world, &program);
    assert_eq!((screens[0].width, screens[0].height, screens[0].scale), (1024, 768, 2));
}

#[test]
fn a_turned_screen_shows_the_desktop_turned_and_says_where_it_changed() {
    let mut world = World::one_screen();
    set(&mut world, r#"{"screens": {"virtio0/0": {"turn": 90}}}"#);
    let program = world.program();
    world.show(&program, (160, 100), 5);
    // The desktop is 768 across and 1024 down; its top left corner
    // shows at the picture's top right.
    let (left, top) = middle_in((768, 1024), (160, 100));
    let turned = |x: i32, y: i32| (1023 - y, x);
    for (x, y) in [(0, 0), (37, 21), (159, 99)] {
        let at = turned(left + x as i32, top + y as i32);
        assert_eq!(pixel(&mut world, 0, at), pattern::colour(5, x, y), "{at:?}");
    }
    assert_eq!(told(&mut world, &program)[0].turned, 3);
    let before = world.showed();
    world.ask(&program, |client| client.repaint(90).unwrap());
    world.settle();
    let (x, y) = turned(left + 80, top + 50);
    assert_eq!(pixel(&mut world, 0, (x, y)), [90, 90, 90]);
    let showed = world.showed_since(before);
    let rects: Vec<_> = showed.iter().flat_map(|showed| showed.damage.clone()).collect();
    let right = |rect: &radix_display::Rect| rect.x + rect.width as i32;
    let bottom = |rect: &radix_display::Rect| rect.y + rect.height as i32;
    let has = |rect: &radix_display::Rect| {
        rect.x <= x && x < right(rect) && rect.y <= y && y < bottom(rect)
    };
    assert!(rects.iter().any(has), "{rects:?}");
    let within = |rect: &radix_display::Rect| {
        rect.x >= 0 && rect.y >= 0 && right(rect) <= 1024 && bottom(rect) <= 768
    };
    assert!(rects.iter().all(within), "{rects:?}");
    // The pointer's picture is turned with it: its tip, the arrow's
    // top left corner, is the top right pixel of the picture.
    let before = world.showed();
    point_in(&mut world, (768, 1024), (100, 50));
    world.settle();
    let showed = world.showed_since(before);
    let at = showed.last().and_then(|showed| showed.pointer_at);
    assert_eq!(at, Some((1023 - 50 - 127, 100)), "{showed:?}");
}

#[test]
fn a_screen_shows_in_the_mode_the_person_names_if_its_monitor_has_it() {
    let mut world = World::one_screen();
    world
        .kernel
        .borrow_mut()
        .offers(0, &[(1024, 768, 60_000), (800, 600, 75_000), (800, 600, 60_000)]);
    world.settle();
    let program = world.program();
    set(&mut world, r#"{"screens": {"virtio0/0": {"mode": "800x600@60"}}}"#);
    assert_eq!(world.desktop.said(), Vec::<String>::new());
    let screens = told(&mut world, &program);
    assert_eq!((screens[0].width, screens[0].height), (800, 600));
    assert_eq!(world.shows(0).0, 800);
    set(&mut world, r#"{"screens": {"virtio0/0": {"mode": "1280x1024"}}}"#);
    assert_eq!(
        world.desktop.said(),
        ["the screen virtio0/0 has no mode 1280x1024; it shows in the one its monitor prefers"]
    );
    assert_eq!(world.shows(0).0, 1024);
}

#[test]
fn a_screen_that_is_not_in_use_shows_nothing_unless_it_is_the_only_one() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    assert_eq!(told(&mut world, &program).len(), 2);
    set(&mut world, r#"{"screens": {"virtio0/0": {"use": false}}}"#);
    let screens = told(&mut world, &program);
    assert_eq!(screens.len(), 1, "{screens:?}");
    assert_eq!((screens[0].name.as_str(), screens[0].x), ("virtio0/1", 0));
    assert_eq!(world.desktop.said(), Vec::<String>::new());
    set(&mut world, r#"{"screens": {"virtio0/0": {"use": false}, "virtio0/1": {"use": false}}}"#);
    assert_eq!(told(&mut world, &program).len(), 2);
    assert_eq!(
        world.desktop.said(),
        ["no screen is set to be in use; the desktop shows on every one"]
    );
}

#[test]
fn the_layout_for_the_screens_there_are_says_where_each_is() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    let text = r#"{"layouts": [
        {"screens": {"virtio0/0": [0, 600], "virtio0/1": [100, 0]}},
        {"screens": {"virtio0/0": [0, 0]}}]}"#;
    set(&mut world, text);
    let places = |screens: &[Screen]| -> Vec<(i32, i32)> {
        screens.iter().map(|screen| (screen.x, screen.y)).collect()
    };
    assert_eq!(places(&told(&mut world, &program)), [(0, 600), (100, 0)]);
    // The pointer goes up from the first screen to the second.
    world.settle();
    let before = world.showed();
    let now = world.now();
    world.desktop.moved(now, 0, -700);
    world.settle();
    let showed = world.showed_since(before);
    let on_second: Vec<_> = showed.iter().filter(|showed| showed.pipe == 1).collect();
    assert!(on_second.iter().any(|showed| showed.pointer_at.is_some()), "{showed:?}");
    // With the second gone, the layout for the first alone holds.
    world.kernel.borrow_mut().found(1, None);
    world.settle();
    assert_eq!(places(&told(&mut world, &program)), [(0, 0)]);
    assert_eq!(world.desktop.said(), Vec::<String>::new());
}

#[test]
fn a_layout_that_leaves_a_gap_is_said_and_the_screens_are_side_by_side() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    set(&mut world, r#"{"layouts": [{"screens": {"virtio0/0": [0, 0], "virtio0/1": [1100, 0]}}]}"#);
    let screens = told(&mut world, &program);
    assert_eq!((screens[0].x, screens[1].x, screens[1].y), (0, 1024, 0));
    assert_eq!(
        world.desktop.said(),
        ["the layout for virtio0/0, virtio0/1 leaves a gap between the screens; the screens are \
          side by side"]
    );
}

#[test]
fn settings_that_are_as_built_again_show_the_screens_as_built() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (160, 100), 5);
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 2, "turn": 180}}}"#);
    // The window is as far across and down the screen as it was: at
    // half as many points.
    let (corner, within) = placed((1024, 768), (160, 100));
    let (left, top) = (corner.0 / 2 + within.0, corner.1 / 2 + within.1);
    // Turned by a half, a point's four pixels end at the far corner.
    assert_eq!(pixel(&mut world, 0, (1023 - left * 2, 767 - top * 2)), pattern::colour(5, 0, 0));
    set(&mut world, "{}");
    let (left, top) = (corner.0 / 2 * 2 + within.0, corner.1 / 2 * 2 + within.1);
    assert_eq!(pixel(&mut world, 0, (left, top)), pattern::colour(5, 0, 0));
    let screen = &told(&mut world, &program)[0];
    assert_eq!((screen.scale, screen.turned), (1, 0));
}

#[test]
fn a_window_on_a_screen_that_goes_is_carried_to_the_one_left_and_back_when_it_returns() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    // The pointer is on the second screen, and the window shows there.
    to_second(&mut world);
    world.show(&program, (160, 100), 5);
    let (left, top) = middle_in((800, 600), (160, 100));
    assert_eq!(pixel(&mut world, 1, (left, top)), pattern::colour(5, 0, 0));
    world.kernel.borrow_mut().found(1, None);
    world.settle();
    // As far across and down the first screen as it was the second.
    let (corner, within) = placed((800, 600), (160, 100));
    let carried = (corner.0 * 1024 / 800 + within.0, corner.1 * 768 / 600 + within.1);
    assert_eq!(pixel(&mut world, 0, carried), pattern::colour(5, 0, 0));
    world.kernel.borrow_mut().found(1, Some((800, 600)));
    world.settle();
    assert_eq!(pixel(&mut world, 1, (left, top)), pattern::colour(5, 0, 0));
    assert_eq!(pixel(&mut world, 0, carried), BACKGROUND);
}

#[test]
fn a_window_the_person_moved_stays_when_its_screen_returns() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    to_second(&mut world);
    world.show(&program, (160, 100), 5);
    world.kernel.borrow_mut().found(1, None);
    world.settle();
    let (corner, within) = placed((800, 600), (160, 100));
    let carried = (corner.0 * 1024 / 800, corner.1 * 768 / 600);
    // Dragged by its title bar, ten points right and down.
    world.point(carried.0 + 40, carried.1 + 10);
    world.button(BTN_LEFT, true);
    world.point(carried.0 + 50, carried.1 + 20);
    world.button(BTN_LEFT, false);
    world.settle();
    let moved = (carried.0 + 10 + within.0, carried.1 + 10 + within.1);
    assert_eq!(pixel(&mut world, 0, moved), pattern::colour(5, 0, 0));
    world.kernel.borrow_mut().found(1, Some((800, 600)));
    world.settle();
    assert_eq!(pixel(&mut world, 0, moved), pattern::colour(5, 0, 0));
}

#[test]
fn a_maximized_window_fills_its_screen_at_whatever_size_the_screen_is() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (160, 100), 5);
    world.ask(&program, |client| client.maximize(true).unwrap());
    let bar = Theme::basic().bar;
    assert_eq!(world.follow(&program, (160, 100), 6), ((1024, 768 - bar as u32), true));
    assert_eq!(pixel(&mut world, 0, (1023, 767)), pattern::colour(6, 1023, 767 - bar as u32));
    world.desktop.set(&settings(r#"{"screens": {"virtio0/0": {"scale": 2}}}"#)).unwrap();
    // 512 by 384 points, less the title bar's rows.
    assert_eq!(world.follow(&program, (160, 100), 7), ((512, 384 - bar as u32), true));
    world.settle();
    assert_eq!(pixel(&mut world, 0, (0, 2 * bar)), pattern::colour(7, 0, 0));
    assert_eq!(pixel(&mut world, 0, (1023, 767)), pattern::colour(7, 511, 383 - bar as u32));
}

#[test]
fn super_with_shift_and_an_arrow_carries_the_window_to_the_next_screen() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    world.show(&program, (160, 100), 5);
    let (corner, within) = placed((1024, 768), (160, 100));
    let carried = (corner.0 * 800 / 1024 + within.0, corner.1 * 600 / 768 + within.1);
    let chord = |world: &mut World, arrow: u16| {
        world.key(KEY_LEFTMETA, true);
        world.key(KEY_LEFTSHIFT, true);
        world.tap(arrow);
        world.key(KEY_LEFTSHIFT, false);
        world.key(KEY_LEFTMETA, false);
        world.settle();
    };
    chord(&mut world, KEY_RIGHT);
    assert_eq!(pixel(&mut world, 1, carried), pattern::colour(5, 0, 0));
    // After the last screen comes the first; a place is carried to
    // the whole point at or before it each time.
    chord(&mut world, KEY_RIGHT);
    assert_eq!(pixel(&mut world, 1, carried), BACKGROUND);
    chord(&mut world, KEY_LEFT);
    let back = (corner.0 * 800 / 1024 * 1024 / 800, corner.1 * 600 / 768 * 768 / 600);
    let carried = (back.0 * 800 / 1024 + within.0, back.1 * 600 / 768 + within.1);
    assert_eq!(pixel(&mut world, 1, carried), pattern::colour(5, 0, 0));
    // A maximized window fills the screen it is carried to.
    world.ask(&program, |client| client.maximize(true).unwrap());
    let bar = Theme::basic().bar as u32;
    assert_eq!(world.follow(&program, (160, 100), 6), ((800, 600 - bar), true));
    assert_eq!(pixel(&mut world, 1, (799, 599)), pattern::colour(6, 799, 599 - bar));
    chord(&mut world, KEY_LEFT);
    assert_eq!(world.follow(&program, (160, 100), 7), ((1024, 768 - bar), true));
    world.settle();
    assert_eq!(pixel(&mut world, 0, (1023, 767)), pattern::colour(7, 1023, 767 - bar));
    assert_eq!(pixel(&mut world, 1, (799, 599)), BACKGROUND);
}

#[test]
fn a_program_is_told_its_screen_s_zoom_and_draws_a_pixel_for_a_pixel() {
    let mut world = World::one_screen();
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 2}}}"#);
    let program = world.program();
    // 160 by 100 points are 320 by 200 of the screen's pixels.
    world.ask(&program, |client| client.show_as((320, 200), 5, Drawn::Zoomed(2)).unwrap());
    world.settle();
    let zoom = world.ask(&program, |client| {
        client.caught_up().unwrap();
        client.zoom()
    });
    assert_eq!(zoom, (Some(2), Some(240)));
    let (left, top) = middle_in((512, 384), (160, 100));
    for (x, y) in [(0, 0), (1, 0), (0, 1), (201, 77), (319, 199)] {
        let at = (left * 2 + x as i32, top * 2 + y as i32);
        assert_eq!(pixel(&mut world, 0, at), pattern::colour(5, x, y), "{at:?}");
    }
    // At a zoom between whole numbers it is told both: the whole one
    // above for a program that draws at those alone.
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 1.5}}}"#);
    let zoom = world.ask(&program, |client| {
        client.caught_up().unwrap();
        client.zoom()
    });
    assert_eq!(zoom, (Some(2), Some(180)));
}

#[test]
fn a_window_carried_to_another_screen_is_told_that_screen_s_zoom() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    set(&mut world, r#"{"screens": {"virtio0/1": {"scale": 2}}}"#);
    let program = world.program();
    world.show(&program, (160, 100), 5);
    let zoom = |world: &mut World| {
        world.ask(&program, |client| {
            client.caught_up().unwrap();
            client.zoom()
        })
    };
    // A surface draws a pixel a point until it is told otherwise.
    assert_eq!(zoom(&mut world), (None, Some(120)));
    world.key(KEY_LEFTMETA, true);
    world.key(KEY_LEFTSHIFT, true);
    world.tap(KEY_RIGHT);
    world.key(KEY_LEFTSHIFT, false);
    world.key(KEY_LEFTMETA, false);
    world.settle();
    assert_eq!(zoom(&mut world), (Some(2), Some(240)));
}

#[test]
fn a_picture_shows_at_the_size_its_program_says_whatever_its_own() {
    let mut world = World::one_screen();
    let program = world.program();
    // 80 by 50 pixels shown 160 by 100 points: each pixel four.
    world.ask(&program, |client| client.show_as((80, 50), 5, Drawn::Viewed(160, 100)).unwrap());
    world.settle();
    let (left, top) = middle_in((1024, 768), (160, 100));
    for (x, y) in [(0, 0), (37, 21), (79, 49)] {
        let colour = pattern::colour(5, x, y);
        let (x, y) = (left + x as i32 * 2, top + y as i32 * 2);
        for at in [(x, y), (x + 1, y + 1)] {
            assert_eq!(pixel(&mut world, 0, at), colour, "{at:?}");
        }
    }
    // The frame is around the size that shows.
    assert_eq!(pixel(&mut world, 0, (left + 160 + 6, top + 50)), BACKGROUND);
    assert_ne!(pixel(&mut world, 0, (left + 160 + 2, top + 50)), BACKGROUND);
}

#[test]
fn at_a_zoom_between_whole_numbers_no_line_of_the_desktop_shows_inside_a_frame() {
    let mut world = World::one_screen();
    set(&mut world, r#"{"screens": {"virtio0/0": {"scale": 1.25}}}"#);
    let program = world.program();
    world.ask(&program, |client| client.show_plain((160, 100), 200).unwrap());
    world.settle();
    // 1024 by 768 pixels are 820 by 615 points.
    let (corner, within) = placed((820, 615), (160, 100));
    let layout = Layout::of(&Theme::basic(), (160, 100), Shape::Normal, Bar::Full, None);
    // Dragged a point at a time, the frame's edges fall on every part
    // of a pixel there is.
    point_in(&mut world, (820, 615), (corner.0 + 40, corner.1 + 10));
    world.button(BTN_LEFT, true);
    for step in 1..=8 {
        point_in(&mut world, (820, 615), (corner.0 + 40 + step, corner.1 + 10 + step));
        world.settle();
        let (left, top) = (corner.0 + step, corner.1 + step);
        let pixels = |points: i32| (f64::from(points) * 1.25).round() as i32;
        // A row and a column through the middle of the content, from
        // well inside the frame's one edge to well inside the other.
        let row = pixels(top + within.1 + 50);
        for x in pixels(left) + 2..pixels(left + layout.frame.w) - 2 {
            assert_ne!(pixel(&mut world, 0, (x, row)), BACKGROUND, "step {step}, column {x}");
        }
        let column = pixels(left + within.0 + 80);
        for y in pixels(top) + 2..pixels(top + layout.frame.h) - 2 {
            assert_ne!(pixel(&mut world, 0, (column, y)), BACKGROUND, "step {step}, row {y}");
        }
    }
    world.button(BTN_LEFT, false);
}
