// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pointer held to a window, and a mouse's own motion
//! (pointer-constraints and relative-pointer): ADR 0728.

use input::abi::{KEY_LEFTALT, KEY_TAB};
use window_test::{Client, Heard};

use super::{Program, World, middle};

/// One count of a mouse, or one pixel, as a mouse's motion is told: in
/// 256ths.
const ONE: i32 = 256;

/// The size of the windows' own pixels.
const SIZE: (u32, u32) = (320, 200);

/// What a program that asked hears of a mouse that moved (`dx`, `dy`)
/// counts slowly: the pointer goes a pixel a count.
fn mouse(dx: i32, dy: i32) -> Heard {
    Heard::Mouse { by: (dx * ONE, dy * ONE), counts: (dx * ONE, dy * ONE) }
}

/// The one motion of the pointer `told` has, after the one of a mouse:
/// where the pointer came to on the window.
fn came_to(told: &[Heard]) -> (f64, f64) {
    let [Heard::Mouse { .. }, Heard::Motion(x, y)] = told[..] else { panic!("{told:?}") };
    (x, y)
}

/// Shows the window of `program` in the middle of the screen, where the
/// pointer is, has the program ask for the mice's own motion, and
/// returns where the pointer is on the window.
fn shows_under_the_pointer(world: &mut World, program: &Program) -> (f64, f64) {
    world.show(program, SIZE, 7);
    world.heard(program, |heard| matches!(heard, Heard::Enter(..)));
    world.ask(program, Client::hear_mouse).unwrap();
    world.settle();
    let (left, top) = middle(SIZE);
    (f64::from(512 - left), f64::from(384 - top))
}

/// A mouse moves (`dx`, `dy`) counts, after a rest.
fn moves(world: &mut World, dx: i32, dy: i32) {
    world.settle();
    let now = world.now();
    world.desktop.moved(now, dx, dy);
}

/// Has `program` ask for something of the pointer with `asks`, and
/// waits until it hears `answer`.
fn holds(
    world: &mut World,
    program: &Program,
    asks: impl FnOnce(&mut Client) -> Result<(), String> + Send + 'static,
    answer: Heard,
) {
    world.ask(program, asks).unwrap();
    world.heard(program, move |heard| *heard == answer);
}

#[test]
fn a_mouse_s_own_motion_is_heard_with_the_pointer_s() {
    let mut world = World::one_screen();
    let program = world.program();
    let (x, y) = shows_under_the_pointer(&mut world, &program);
    moves(&mut world, 4, -2);
    assert_eq!(world.told(&program), [mouse(4, -2), Heard::Motion(x + 4.0, y - 2.0)]);
    // A fast hand moves the pointer further than the mouse went, and
    // the program hears both.
    let now = world.now();
    world.desktop.moved(now, 1, 0);
    world.desktop.moved(now + 1_000_000, 12, -9);
    let told = world.told(&program);
    let [_, _, Heard::Mouse { by, counts }, Heard::Motion(to_x, to_y)] = told[..] else {
        panic!("{told:?}");
    };
    assert_eq!(counts, (12 * ONE, -9 * ONE));
    assert!(by.0 > counts.0 && by.1 < counts.1, "{by:?}");
    let went = (to_x - (x + 5.0), to_y - (y - 2.0));
    assert_eq!((went.0 * 256.0, went.1 * 256.0), (f64::from(by.0), f64::from(by.1)));
    // A device that points at places is no mouse.
    world.point(500, 380);
    assert!(matches!(world.told(&program)[..], [Heard::Motion(..)]));
}

#[test]
fn a_locked_pointer_stays_where_it_is_and_the_mouse_is_heard_alone() {
    let mut world = World::one_screen();
    let program = world.program();
    shows_under_the_pointer(&mut world, &program);
    holds(&mut world, &program, |client| client.lock(false), Heard::Locked(true));
    let before = world.showed();
    moves(&mut world, 3, 5);
    assert_eq!(world.told(&program), [mouse(3, 5)]);
    world.point(10, 10);
    assert_eq!(world.told(&program), []);
    assert_eq!(world.desktop.pointer_at(), (512.0, 384.0));
    assert!(world.showed_since(before).iter().all(|showed| !showed.moved));
    // A window holds the pointer one way at a time.
    let second = world.ask(&program, Client::lock_again);
    assert!(second.is_err(), "{second:?}");
}

#[test]
fn a_lock_that_ends_leaves_the_pointer_where_its_program_says() {
    let mut world = World::one_screen();
    let program = world.program();
    shows_under_the_pointer(&mut world, &program);
    holds(&mut world, &program, |client| client.lock(false), Heard::Locked(true));
    world.ask(&program, |client| client.let_go(Some((20.0, 30.0)))).unwrap();
    assert_eq!(world.told(&program), [Heard::Motion(20.0, 30.0)]);
    let (left, top) = middle(SIZE);
    assert_eq!(world.desktop.pointer_at(), (f64::from(left) + 20.0, f64::from(top) + 30.0));
    moves(&mut world, 1, 1);
    assert_eq!(world.told(&program), [mouse(1, 1), Heard::Motion(21.0, 31.0)]);
    // A place that is not on the window is not gone to.
    holds(&mut world, &program, |client| client.lock(false), Heard::Locked(true));
    world.ask(&program, |client| client.let_go(Some((2000.0, 30.0)))).unwrap();
    assert_eq!(world.told(&program), []);
    // Nor is the pointer moved for a program that named no place.
    holds(&mut world, &program, |client| client.lock(false), Heard::Locked(true));
    world.ask(&program, |client| client.let_go(None)).unwrap();
    assert_eq!(world.told(&program), []);
}

#[test]
fn a_confined_pointer_stays_on_its_window_or_on_the_part_its_program_named() {
    let mut world = World::one_screen();
    let program = world.program();
    let (_, y) = shows_under_the_pointer(&mut world, &program);
    holds(&mut world, &program, |client| client.confine(false, None), Heard::Confined(true));
    // Against the window's right edge, then its bottom, and along it.
    moves(&mut world, 3000, 1);
    let (to_x, to_y) = came_to(&world.told(&program));
    assert!(to_x == 319.0 && to_y > y && to_y < y + 10.0, "({to_x}, {to_y})");
    moves(&mut world, -2, 3000);
    let (to_x, to_y) = came_to(&world.told(&program));
    assert!(to_x < 319.0 && to_x > 300.0 && to_y == 199.0, "({to_x}, {to_y})");
    // A device that points at a place off the window brings it to the
    // nearest place on it.
    world.point(5, 5);
    assert_eq!(world.told(&program), [Heard::Motion(0.0, 0.0)]);
    world.ask(&program, |client| client.let_go(None)).unwrap();

    // A part of the window: it holds once the pointer is in it.
    let part = Some((100, 50, 60, 40));
    world.ask(&program, move |client| client.confine(false, part)).unwrap();
    assert_eq!(world.told(&program), []);
    let (left, top) = middle(SIZE);
    world.point(left as i32 + 120, top as i32 + 60);
    let confined = Heard::Confined(true);
    let heard = world.heard(&program, move |heard| *heard == confined);
    assert!(heard.contains(&Heard::Motion(120.5, 60.5)), "{heard:?}");
    // Up to the part's right edge and on down; up to its top and on to
    // the left.
    moves(&mut world, 300, 2);
    let (to_x, to_y) = came_to(&world.told(&program));
    assert!((159.0..160.0).contains(&to_x) && to_y > 60.5 && to_y < 90.0, "({to_x}, {to_y})");
    moves(&mut world, -2, -300);
    let (to_x, to_y) = came_to(&world.told(&program));
    assert!((140.0..159.0).contains(&to_x) && (50.0..50.1).contains(&to_y), "({to_x}, {to_y})");
}

/// Shows a window whose program has it hold the pointer, locked, over
/// its top left corner, and then a second window, which is the one the
/// person works in from then on and leaves that corner to see: the
/// pointer stays over the first.
fn locked_then_left(world: &mut World, lasting: bool) -> (Program, Program) {
    let (first, second) = (world.program(), world.program());
    world.show(&first, SIZE, 7);
    let (left, top) = middle(SIZE);
    world.point(left as i32 + 5, top as i32 + 5);
    holds(world, &first, move |client| client.lock(lasting), Heard::Locked(true));
    world.show(&second, SIZE, 8);
    world.heard(&first, |heard| *heard == Heard::Locked(false));
    (first, second)
}

#[test]
fn a_lock_asked_for_once_is_gone_when_the_person_works_in_another_window() {
    let mut world = World::one_screen();
    let (first, second) = locked_then_left(&mut world, false);
    // The pointer is the person's again, and stays so when the window
    // is theirs again.
    moves(&mut world, 2, 1);
    assert_eq!(world.told(&first), [Heard::Motion(7.5, 6.5)]);
    drop(second);
    let heard = world.heard(&first, |heard| *heard == Heard::Keyboard(true));
    assert!(heard.contains(&Heard::Active(true)), "{heard:?}");
    moves(&mut world, 2, 1);
    assert_eq!(world.told(&first), [Heard::Motion(9.5, 7.5)]);
}

#[test]
fn a_lock_asked_for_to_last_holds_again_when_the_person_comes_back() {
    let mut world = World::one_screen();
    let (first, second) = locked_then_left(&mut world, true);
    drop(second);
    let heard = world.heard(&first, |heard| *heard == Heard::Locked(true));
    assert!(heard.contains(&Heard::Active(true)), "{heard:?}");
    let at = world.desktop.pointer_at();
    moves(&mut world, 9, 9);
    assert_eq!(world.desktop.pointer_at(), at);
}

#[test]
fn the_list_of_windows_takes_the_pointer_from_a_window_that_holds_it() {
    let mut world = World::one_screen();
    let program = world.program();
    shows_under_the_pointer(&mut world, &program);
    holds(&mut world, &program, |client| client.lock(true), Heard::Locked(true));
    // While the list shows, the pointer is the window manager's.
    world.key(KEY_LEFTALT, true);
    world.tap(KEY_TAB);
    world.heard(&program, |heard| *heard == Heard::Locked(false));
    moves(&mut world, 6, 0);
    assert_eq!(world.desktop.pointer_at(), (518.0, 384.0));
    world.key(KEY_LEFTALT, false);
    world.heard(&program, |heard| *heard == Heard::Locked(true));
}
