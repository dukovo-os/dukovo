// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Windows snapped to the sides of a screen and to their parts
//! (ADR 0727), by the keys and by the pointer.

use display_pattern::pattern;
use input::abi::{
    BTN_LEFT, KEY_A, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_LEFT, KEY_LEFTALT, KEY_LEFTMETA, KEY_RIGHT,
    KEY_SPACE, KEY_UP,
};
use window_frame::panel::{Entry, Mark};
use window_frame::{Bar, Button, Canvas, Layout, Rect, Shape, Theme};
use window_test::Heard;

use super::laid::settings;
use super::screens::pixel;
use super::{Program, World, framed, middle};
use crate::screens::BACKGROUND;

/// The size a window's program is asked to draw for a frame's box of
/// `frame`, and where its own pixels begin in the box.
pub(super) fn inside((width, height): (i32, i32)) -> ((u32, u32), (i32, i32)) {
    // A frame's sides and its title bar are the same around any size.
    let layout = Layout::of(&Theme::basic(), (100, 100), Shape::Normal, Bar::Full, None);
    let (sides, rows) = (layout.frame.w - 100, layout.frame.h - 100);
    (((width - sides) as u32, (height - rows) as u32), (layout.surface.x, layout.surface.y))
}

/// Presses Super with `arrow`, has `program` draw its window as it is
/// asked to with the pattern of `seed`, and returns the size drawn.
pub(super) fn arrow(world: &mut World, program: &Program, arrow: u16, seed: u32) -> (u32, u32) {
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
pub(super) fn fills(
    world: &mut World,
    zone: ((i32, i32), (i32, i32)),
    size: (u32, u32),
    seed: u32,
) {
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

/// The colour a preview's inside shows over the desktop's own.
fn shaded() -> [u8; 3] {
    let chosen = Theme::basic().panel.chosen;
    let over = |colour: u8, under: u8| {
        ((u32::from(colour) * 110 + u32::from(under) * 145 + 127) / 255) as u8
    };
    [
        over(chosen.red, BACKGROUND[0]),
        over(chosen.green, BACKGROUND[1]),
        over(chosen.blue, BACKGROUND[2]),
    ]
}

/// Whether `shows` is `colour`, give or take what a renderer rounds.
fn nearly(shows: [u8; 3], colour: [u8; 3]) -> bool {
    shows.iter().zip(colour).all(|(shows, colour)| shows.abs_diff(colour) <= 2)
}

/// The colour of the line around a preview.
fn line() -> [u8; 3] {
    let line = Theme::basic().panel.chosen_line;
    [line.red, line.green, line.blue]
}

#[test]
fn a_window_dragged_to_an_edge_shows_what_it_fills_there_and_does_when_let_go() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let (_, (left, top)) = framed((320, 200));
    world.point(left + 100, top + 15);
    world.button(BTN_LEFT, true);
    // Away from the edges nothing shows but the window.
    world.point(200, 300);
    world.settle();
    assert_eq!(world.colour(700, 100), BACKGROUND);
    assert_eq!(world.colour(9, 9), BACKGROUND);
    // At the left edge: the left side, a line around it and the
    // desktop through it.
    world.point(0, 300);
    world.settle();
    assert_eq!(world.colour(9, 9), line());
    assert_eq!(world.colour(502, 758), line());
    assert!(nearly(world.colour(300, 100), shaded()), "{:?}", world.colour(300, 100));
    assert_eq!(world.colour(4, 4), BACKGROUND);
    assert_eq!(world.colour(700, 100), BACKGROUND);
    // Near the corner: the upper part of that side.
    world.point(0, 10);
    world.settle();
    assert_eq!(world.colour(502, 374), line());
    assert!(nearly(world.colour(300, 300), shaded()));
    assert_eq!(world.colour(300, 600), BACKGROUND);
    // At the top edge: the whole screen.
    world.point(300, 0);
    world.settle();
    assert_eq!(world.colour(1014, 758), line());
    assert!(nearly(world.colour(700, 600), shaded()));
    // Away again, and nothing.
    world.point(300, 300);
    world.settle();
    assert_eq!(world.colour(700, 600), BACKGROUND);
    assert_eq!(world.colour(9, 9), BACKGROUND);
    // Let go at the right edge, the window fills the right side.
    world.point(1023, 300);
    world.button(BTN_LEFT, false);
    let (size, _) = world.follow(&program, (320, 200), 6);
    world.settle();
    fills(&mut world, ((512, 0), (512, 768)), size, 6);
    assert_eq!(world.colour(300, 100), BACKGROUND);
    // Restored by the keys, it is where it was before it was dragged.
    let size = arrow(&mut world, &program, KEY_DOWN, 7);
    fills(&mut world, ((512, 384), (512, 384)), size, 7);
    let size = arrow(&mut world, &program, KEY_DOWN, 8);
    assert_eq!(size, (320, 200));
    let (own_left, own_top) = middle((320, 200));
    assert_eq!(world.colour(own_left, own_top), pattern::colour(8, 0, 0));
    // Let go at the top edge, it is maximized.
    world.point(left + 100, top + 15);
    world.button(BTN_LEFT, true);
    world.point(400, 100);
    world.point(400, 0);
    world.button(BTN_LEFT, false);
    let bar = Theme::basic().bar as u32;
    assert_eq!(world.follow(&program, (320, 200), 9), ((1024, 768 - bar), true));
    world.settle();
    assert_eq!(world.colour(1023, 767), pattern::colour(9, 1023, 767 - bar));
}

#[test]
fn a_snapped_window_dragged_by_its_bar_is_as_it_was_and_snaps_anew_on_the_screen_it_is_let_go_on() {
    let mut world = World::new(&[Some((1024, 768)), Some((800, 600))], false);
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let size = arrow(&mut world, &program, KEY_LEFT, 6);
    fills(&mut world, ((0, 0), (512, 768)), size, 6);
    // Dragged by its bar, it is of the size it had, under the pointer.
    world.point(256, 15);
    world.button(BTN_LEFT, true);
    world.point(300, 300);
    assert_eq!(world.follow(&program, (320, 200), 7), ((320, 200), false));
    // The edge the second screen lies beyond snaps nothing: the pointer
    // goes on to that screen, and to its far edge.
    world.point(1023, 300);
    world.settle();
    assert_eq!(world.colour(700, 100), BACKGROUND);
    for _ in 0..6 {
        let now = world.now();
        world.desktop.moved(now, 400, 0);
        world.settle();
    }
    assert_eq!(pixel(&mut world, 1, (409, 9)), line());
    assert!(nearly(pixel(&mut world, 1, (600, 100)), shaded()));
    assert_eq!(world.colour(700, 100), BACKGROUND);
    world.button(BTN_LEFT, false);
    let (size, _) = world.follow(&program, (320, 200), 8);
    world.settle();
    let (asked, within) = inside((400, 600));
    assert_eq!(size, asked);
    assert_eq!(pixel(&mut world, 1, (400 + within.0, within.1)), pattern::colour(8, 0, 0));
    assert_eq!(pixel(&mut world, 1, (200, 300)), BACKGROUND);
    // Restored, it is where it was before the first snap, on the
    // first screen.
    arrow(&mut world, &program, KEY_DOWN, 9);
    let size = arrow(&mut world, &program, KEY_DOWN, 10);
    assert_eq!(size, (320, 200));
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(10, 0, 0));
}

/// The rows of a list of windows of `titles`.
fn rows<'a>(titles: &[&'a str]) -> Vec<Entry<'a>> {
    let line = |words: &&'a str| Entry::Line { words, key: "", mark: Mark::Icon, off: false };
    titles.iter().map(line).collect()
}

/// Where the corner of a list of `entries`, 480 wide, is in the middle
/// of `area`, its corner and its size.
fn in_the_middle(entries: &[Entry<'_>], area: ((i32, i32), (i32, i32))) -> (i32, i32) {
    let (width, height) = window_frame::panel::place(&Theme::basic(), 480, entries).size;
    let ((left, top), (across, down)) = area;
    (left + (across - width) / 2, top + (down - height) / 2)
}

#[test]
fn after_a_snap_the_other_windows_are_offered_for_the_room_that_is_left() {
    let mut world = World::one_screen();
    let programs = [world.program(), world.program(), world.program()];
    for (program, (seed, title)) in programs.iter().zip([(7, "One"), (8, "Two"), (9, "Three")]) {
        world.show(program, (320, 200), seed);
        world.ask(program, move |client| client.title(title)).unwrap();
    }
    world.settle();
    // Three fills the left side: the others are offered the right.
    let [one, two, three] = &programs;
    let size = arrow(&mut world, three, KEY_LEFT, 10);
    fills(&mut world, ((0, 0), (512, 768)), size, 10);
    let offered = rows(&["Two", "One"]);
    let corner = in_the_middle(&offered, ((512, 0), (512, 768)));
    world.shows_panel(480, &offered, Some(0), corner);
    // A key of the window manager's own does what it does, and the
    // offer is for the room left then: the right side still.
    let size = arrow(&mut world, three, KEY_UP, 11);
    fills(&mut world, ((0, 0), (512, 384)), size, 11);
    world.shows_panel(480, &offered, Some(0), corner);
    // The keys choose: One fills the right side, in front and with the
    // keyboard, and Two is offered the part that is left.
    world.tap(KEY_DOWN);
    world.settle();
    world.shows_panel(480, &offered, Some(1), corner);
    world.tap(KEY_ENTER);
    let (size, _) = world.follow(one, (320, 200), 12);
    world.settle();
    fills(&mut world, ((512, 0), (512, 768)), size, 12);
    let heard = world.heard(one, |heard| *heard == Heard::Keyboard(true));
    assert!(heard.contains(&Heard::Active(true)), "{heard:?}");
    let offered = rows(&["Two"]);
    let corner = in_the_middle(&offered, ((0, 384), (512, 384)));
    world.shows_panel(480, &offered, Some(0), corner);
    // The pointer chooses too.
    let row = Theme::basic().panel;
    world.click(corner.0 + 240, corner.1 + 1 + row.padding + row.row / 2);
    let (size, _) = world.follow(two, (320, 200), 13);
    world.settle();
    fills(&mut world, ((0, 384), (512, 384)), size, 13);
    // Nothing is left to offer, and the keys are the window's again.
    world.tap(KEY_A);
    let heard = world.heard(two, |heard| matches!(heard, Heard::Key { pressed: false, .. }));
    assert!(heard.contains(&Heard::Key { code: u32::from(KEY_A), pressed: true }), "{heard:?}");
}

#[test]
fn an_offer_goes_at_esc_at_any_other_key_and_at_a_press_beside_it() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, (320, 200), 7);
    world.show(&second, (320, 200), 8);
    // The list is in the middle of the side that is left, over the
    // desktop at these places.
    let (in_right, in_left) = ((900, 384), (120, 384));
    arrow(&mut world, &second, KEY_LEFT, 9);
    assert_ne!(world.colour(in_right.0, in_right.1), BACKGROUND);
    world.tap(KEY_ESC);
    world.settle();
    assert_eq!(world.colour(in_right.0, in_right.1), BACKGROUND);
    // Any other key puts it away too, and is no program's.
    arrow_alone(&mut world, KEY_RIGHT);
    assert_ne!(world.colour(in_left.0, in_left.1), BACKGROUND);
    world.tap(KEY_A);
    world.settle();
    assert_eq!(world.colour(in_left.0, in_left.1), BACKGROUND);
    let told = world.told(&second);
    let of_a =
        |heard: &Heard| matches!(heard, Heard::Key { code, .. } if *code == u32::from(KEY_A));
    assert!(!told.iter().any(of_a), "{told:?}");
    // And a press beside it.
    arrow_alone(&mut world, KEY_LEFT);
    assert_ne!(world.colour(in_right.0, in_right.1), BACKGROUND);
    world.click(900, 100);
    world.settle();
    assert_eq!(world.colour(in_right.0, in_right.1), BACKGROUND);
    // The window that was offered is where it was.
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left + 300, top + 150), pattern::colour(7, 300, 150));
}

/// Has `program` draw its window at each size it is asked for, with
/// the pattern of `seed`, until it is asked for no other. Returns the
/// last, if it was asked for any.
fn draws_on(world: &mut World, program: &Program, seed: u32) -> Option<(u32, u32)> {
    let mut last = None;
    loop {
        world.settle();
        let asked = world.told(program).into_iter().rev().find_map(|heard| match heard {
            Heard::Size { width, height, .. } => Some((width as u32, height as u32)),
            _ => None,
        });
        let Some(size) = asked else { return last };
        world.show(program, size, seed);
        last = Some(size);
    }
}

#[test]
fn the_edge_two_snapped_windows_share_resizes_both() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, (320, 200), 7);
    world.show(&second, (320, 200), 8);
    // The second on the left and, from the offer, the first on the
    // right.
    arrow(&mut world, &second, KEY_LEFT, 9);
    world.tap(KEY_ENTER);
    let (size, _) = world.follow(&first, (320, 200), 10);
    world.settle();
    fills(&mut world, ((512, 0), (512, 768)), size, 10);
    // The right edge of the left one, dragged a hundred to the right:
    // each program is asked for one size at a time, and has the last.
    world.point(510, 400);
    world.button(BTN_LEFT, true);
    world.point(560, 400);
    world.point(610, 400);
    let left = draws_on(&mut world, &second, 11).unwrap();
    let right = draws_on(&mut world, &first, 12).unwrap();
    world.button(BTN_LEFT, false);
    assert_eq!(draws_on(&mut world, &second, 11), None);
    assert_eq!(draws_on(&mut world, &first, 12), None);
    fills(&mut world, ((0, 0), (612, 768)), left, 11);
    fills(&mut world, ((612, 0), (412, 768)), right, 12);
    // The sides meet there for a window that is snapped after.
    let size = arrow(&mut world, &first, KEY_UP, 13);
    fills(&mut world, ((612, 0), (412, 384)), size, 13);
    // The lower edge of a part moves where that side's parts meet.
    world.point(612 + 200, 382);
    world.button(BTN_LEFT, true);
    world.point(612 + 200, 482);
    world.button(BTN_LEFT, false);
    let size = draws_on(&mut world, &first, 14).unwrap();
    fills(&mut world, ((612, 0), (412, 484)), size, 14);
    let size = arrow(&mut world, &first, KEY_DOWN, 15);
    fills(&mut world, ((612, 0), (412, 768)), size, 15);
    let size = arrow(&mut world, &first, KEY_DOWN, 16);
    fills(&mut world, ((612, 484), (412, 284)), size, 16);
    // An edge that lies along the screen's own resizes that window
    // alone, which is snapped no more: the sides meet where they did.
    world.point(1022, 600);
    world.button(BTN_LEFT, true);
    world.point(922, 600);
    world.button(BTN_LEFT, false);
    let size = draws_on(&mut world, &first, 17).unwrap();
    fills(&mut world, ((612, 484), (312, 284)), size, 17);
    assert_eq!(draws_on(&mut world, &second, 11), None);
}

#[test]
fn a_window_along_a_split_that_was_dragged_is_as_it_was_when_dragged_by_its_bar() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, (320, 200), 7);
    world.show(&second, (280, 180), 8);
    // The second on the left and, from the offer, the first on the
    // right. The edge they share is dragged a hundred to the right,
    // and both have drawn the last size when it is let go.
    arrow(&mut world, &second, KEY_LEFT, 9);
    world.tap(KEY_ENTER);
    world.follow(&first, (320, 200), 10);
    world.settle();
    world.point(510, 400);
    world.button(BTN_LEFT, true);
    world.point(610, 400);
    draws_on(&mut world, &second, 11).unwrap();
    draws_on(&mut world, &first, 12).unwrap();
    world.button(BTN_LEFT, false);
    world.settle();
    // Dragged by its bar the left one is asked for the size it had
    // before it was snapped, and for nothing after it has drawn that.
    world.point(300, 15);
    world.button(BTN_LEFT, true);
    world.point(420, 330);
    assert_eq!(world.follow(&second, (320, 200), 13), ((280, 180), false));
    assert_eq!(draws_on(&mut world, &second, 13), None);
    assert_eq!(world.colour(20, 700), BACKGROUND);
    world.button(BTN_LEFT, false);
}

/// Has the pointer rest where it is for longer than the picker of
/// layouts waits.
fn rest(world: &mut World) {
    for _ in 0..270 {
        world.turn();
    }
    world.settle();
}

/// The middle of the maximize button of a window whose frame's box is
/// `frame` wide and high with its corner at `corner`, and where the
/// corner of the picker is under it: in the middle of the button's
/// width, or as near to that as leaves it whole on a screen 1024 wide.
fn picker_under(corner: (i32, i32), frame: (i32, i32)) -> ((i32, i32), (i32, i32)) {
    let ((width, height), _) = inside(frame);
    let layout =
        Layout::of(&Theme::basic(), (width as i32, height as i32), Shape::Normal, Bar::Full, None);
    let button = layout.button(Button::Maximize);
    let (left, top) = (corner.0 + button.x, corner.1 + button.y);
    let (across, _) = window_frame::picker::place(&Theme::basic()).size;
    let under = (left + (button.w - across) / 2).min(1024 - across);
    ((left + button.w / 2, top + button.h / 2), (under, top + button.h))
}

/// Fails unless the first screen shows the picker with its corner at
/// `corner` and the target `hot` lit.
fn shows_picker(world: &mut World, corner: (i32, i32), hot: Option<usize>) {
    let theme = Theme::basic();
    let (width, height) = window_frame::picker::place(&theme).size;
    let all = Rect::new(0, 0, width, height);
    let mut drawn = vec![0; Canvas::bytes_of(&all)];
    window_frame::picker::draw(&theme, hot, &mut Canvas::over(all, &mut drawn));
    world.shows_drawn((all, &drawn), corner, &[], false);
}

#[test]
fn the_pointer_resting_on_the_maximize_button_is_shown_the_layouts_to_pick_from() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let (layout, at) = framed((320, 200));
    let (on, corner) = picker_under(at, (layout.frame.w, layout.frame.h));
    let targets = window_frame::picker::place(&Theme::basic()).targets;
    let target = Theme::basic().panel.target;
    let target = [target.red, target.green, target.blue];
    let in_first = ((corner.0 + 20) as u32, (corner.1 + 20) as u32);
    // The pointer comes to the button: nothing shows until it has been
    // there half a second.
    world.point(on.0, on.1);
    world.settle();
    assert_ne!(world.colour(in_first.0, in_first.1), target);
    rest(&mut world);
    shows_picker(&mut world, corner, None);
    // The target under the pointer is lit, and a click on it snaps the
    // window to what it stands for: the lower part of the right side.
    let lower = targets[4].rect;
    world.point(corner.0 + lower.x + 10, corner.1 + lower.y + 10);
    world.settle();
    shows_picker(&mut world, corner, Some(4));
    world.button(BTN_LEFT, true);
    world.button(BTN_LEFT, false);
    let (size, _) = world.follow(&program, (320, 200), 6);
    world.settle();
    fills(&mut world, ((512, 384), (512, 384)), size, 6);
    assert_eq!(world.colour(in_first.0, in_first.1), BACKGROUND);
    // It shows again for the button where it is now, and goes when the
    // pointer leaves it and the button.
    let (on, corner) = picker_under((512, 384), (512, 384));
    world.point(on.0, on.1);
    rest(&mut world);
    shows_picker(&mut world, corner, None);
    world.point(on.0, on.1 + 60);
    world.settle();
    shows_picker(&mut world, corner, None);
    world.point(600, 500);
    world.settle();
    assert_ne!(world.colour((corner.0 + 20) as u32, (corner.1 + 20) as u32), target);
    // A click on the button does what it does, with the picker or not.
    world.point(on.0, on.1);
    rest(&mut world);
    shows_picker(&mut world, corner, None);
    world.button(BTN_LEFT, true);
    world.button(BTN_LEFT, false);
    let bar = Theme::basic().bar as u32;
    assert_eq!(world.follow(&program, (320, 200), 7), ((1024, 768 - bar), true));
    world.settle();
    assert_eq!(world.colour(1023, 767), pattern::colour(7, 1023, 767 - bar));
}

#[test]
fn the_window_menu_restores_a_snapped_window_and_maximizes_it() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let size = arrow(&mut world, &program, KEY_LEFT, 6);
    fills(&mut world, ((0, 0), (512, 768)), size, 6);
    // The menu of a snapped window: it is restored to be moved or
    // resized, and can be maximized as it is.
    let line = |words, key, off| Entry::Line { words, key, mark: Mark::None, off };
    let menu = [
        line("Restore", "", false),
        line("Move", "", true),
        line("Size", "", true),
        line("Minimize", "", false),
        line("Maximize", "", false),
        Entry::Gap,
        line("Close", "Alt+F4", false),
    ];
    let bar = Theme::basic().bar;
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.settle();
    world.shows_panel(330, &menu, None, (0, bar));
    world.tap(KEY_DOWN);
    world.tap(KEY_ENTER);
    assert_eq!(world.follow(&program, (320, 200), 7), ((320, 200), false));
    world.settle();
    let (left, top) = middle((320, 200));
    assert_eq!(world.colour(left, top), pattern::colour(7, 0, 0));
    assert_eq!(world.colour(100, 600), BACKGROUND);
}
