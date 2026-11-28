// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the pointer and the keys do to the windows themselves: the
//! frame's parts, the window menu, the list of windows, and what a
//! program may ask for its own window.

use display_pattern::pattern;
use input::abi::{
    BTN_LEFT, BTN_RIGHT, KEY_A, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_F4, KEY_LEFT, KEY_LEFTALT,
    KEY_LEFTCTRL, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_RIGHT, KEY_SPACE, KEY_TAB, KEY_UP,
};
use window_frame::panel::{Entry, Mark};
use window_frame::{Bar, Button, Layout, Look, Shape, Theme};
use window_test::{Client, Heard, Open, ResizeEdge};

use super::{PATIENCE, World, framed, look, middle};
use crate::screens::BACKGROUND;

/// The size of the windows' own pixels when they first show.
const SIZE: (u32, u32) = (320, 200);

/// The size of the own pixels of a maximized window on a screen of
/// 1024 by 768: all of it but the title bar.
const FILLS: (u32, u32) = (1024, 732);

/// How wide the window menu and the list of windows are.
const MENU_WIDTH: i32 = 330;
const LIST_WIDTH: i32 = 480;

/// The frame of a window whose own pixels are `size`.
fn frame_of((width, height): (u32, u32), shape: Shape) -> Layout {
    Layout::of(&Theme::basic(), (width as i32, height as i32), shape, Bar::Full, None)
}

/// The middle of `button` in the frame of `layout`.
fn on(layout: &Layout, button: Button) -> (i32, i32) {
    let rect = layout.button(button);
    (rect.x + rect.w / 2, rect.y + rect.h / 2)
}

/// The window menu's lines for a window that is maximized or not.
fn menu(maximized: bool) -> [Entry<'static>; 7] {
    let line = |words, key, off| Entry::Line { words, key, mark: Mark::None, off };
    [
        line("Restore", "", !maximized),
        line("Move", "", maximized),
        line("Size", "", maximized),
        line("Minimize", "", false),
        line("Maximize", "", maximized),
        Entry::Gap,
        line("Close", "Alt+F4", false),
    ]
}

/// The list's lines for windows of `titles`, and where the list's
/// corner is on a screen of 1024 by 768.
fn list<'a>(titles: &[&'a str]) -> (Vec<Entry<'a>>, (i32, i32)) {
    let line = |words: &&'a str| Entry::Line { words, key: "", mark: Mark::Icon, off: false };
    let entries: Vec<Entry<'a>> = titles.iter().map(line).collect();
    let (width, height) = window_frame::panel::place(&Theme::basic(), LIST_WIDTH, &entries).size;
    (entries, ((1024 - width) / 2, (768 - height) / 2))
}

/// The middle of the row `row` of a panel `width` wide whose corner is
/// at `corner`, where no row before it is a gap.
fn row_of(width: i32, corner: (i32, i32), row: i32) -> (i32, i32) {
    let panel = Theme::basic().panel;
    (corner.0 + width / 2, corner.1 + 1 + panel.padding + row * panel.row + panel.row / 2)
}

/// Whether `heard` is the left button's going down.
fn pressed(heard: &Heard) -> Option<()> {
    (*heard == Heard::Button { code: u32::from(BTN_LEFT), pressed: true }).then_some(())
}

/// Whether `heard` is a key's going down or up that is not `but`.
fn key_but(heard: &Heard, but: &[u16]) -> bool {
    matches!(heard, Heard::Key { code, .. } if !but.iter().any(|but| u32::from(*but) == *code))
}

impl World {
    /// The seed of the window in front, of windows that showed one
    /// after another with the seeds 7, 8 and 9.
    fn front(&mut self) -> u32 {
        let (left, top) = middle(SIZE);
        // A place all three have their own pixels at.
        let shows = self.colour(left + 70, top + 70);
        let of = |(seed, at): &(u32, u32)| pattern::colour(*seed, *at, *at) == shows;
        [(7, 70), (8, 38), (9, 6)]
            .iter()
            .find(|window| of(window))
            .expect("no window is in front")
            .0
    }
}

#[test]
fn a_title_bar_moves_its_window_and_a_double_click_on_it_maximizes_and_restores() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    let (layout, (left, top)) = framed(SIZE);
    // A press on the title bar and a hand that trembles: nothing moves.
    world.point(left + 100, top + 15);
    world.button(BTN_LEFT, true);
    world.point(left + 102, top + 16);
    world.settle();
    world.shows_frame(&layout, (left, top), &look(true));
    // Further, and the window follows the pointer.
    world.point(left + 150, top + 45);
    world.point(left + 160, top + 55);
    world.button(BTN_LEFT, false);
    world.settle();
    let moved = (left + 60, top + 40);
    world.shows_frame(&layout, moved, &look(true));
    let own = ((moved.0 + layout.surface.x) as u32, (moved.1 + layout.surface.y) as u32);
    assert_eq!(world.colour(own.0, own.1), pattern::colour(7, 0, 0));
    assert_eq!(world.colour(left as u32 + 2, top as u32 + 2), BACKGROUND);
    // The program heard the pointer leave for the frame, and nothing
    // of the button.
    let told = world.told(&program);
    assert!(told.contains(&Heard::Leave), "{told:?}");
    assert!(!told.iter().any(|heard| matches!(heard, Heard::Button { .. })), "{told:?}");

    // A double click on the bar asks the program to fill the screen,
    // and the frame is as it was until the program has drawn so.
    for _ in 0..2 {
        world.click(moved.0 + 100, moved.1 + 15);
    }
    world.settle();
    world.shows_frame(&layout, moved, &look(true));
    assert_eq!(world.follow(&program, SIZE, 8), (FILLS, true));
    world.settle();
    let fills = frame_of(FILLS, Shape::Maximized);
    world.shows_frame(&fills, (0, 0), &look(true));
    assert_eq!(world.colour(0, 36), pattern::colour(8, 0, 0));
    assert_eq!(world.colour(1023, 767), pattern::colour(8, 1023, 731));
    // Another puts it back where it was, at the size it had.
    for _ in 0..2 {
        world.click(400, 10);
    }
    assert_eq!(world.follow(&program, SIZE, 9), (SIZE, false));
    world.settle();
    world.shows_frame(&layout, moved, &look(true));
    assert_eq!(world.colour(own.0, own.1), pattern::colour(9, 0, 0));

    // A maximized window dragged by its bar is restored under the
    // pointer, which keeps the same share of the bar's width.
    for _ in 0..2 {
        world.click(moved.0 + 100, moved.1 + 15);
    }
    assert_eq!(world.follow(&program, SIZE, 8), (FILLS, true));
    world.point(512, 10);
    world.button(BTN_LEFT, true);
    world.point(512, 100);
    assert_eq!(world.follow(&program, SIZE, 9), (SIZE, false));
    world.settle();
    world.shows_frame(&layout, (512 - layout.frame.w / 2, 90), &look(true));
    world.point(522, 110);
    world.button(BTN_LEFT, false);
    world.settle();
    world.shows_frame(&layout, (522 - layout.frame.w / 2, 100), &look(true));
}

#[test]
fn an_edge_resizes_its_window_one_size_at_a_time_and_the_other_sides_stay() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    world.settle();
    let (layout, (left, top)) = framed(SIZE);
    // Over an edge the pointer shows which way the edge goes.
    let before = world.showed();
    let edge = left + layout.frame.w - 2;
    world.point(edge, top + 120);
    world.settle();
    assert!(world.showed_since(before).iter().any(|showed| showed.pointer));
    world.button(BTN_LEFT, true);
    for by in [40, 50, 60] {
        world.point(edge + by, top + 120);
    }
    // The program is asked for the first size, and the frame stays as
    // it is until the program has drawn it.
    world.settle();
    world.shows_frame(&layout, (left, top), &look(true));
    assert_eq!(world.follow(&program, SIZE, 8), ((360, 200), false));
    // Then for the last: the one between was never asked for.
    assert_eq!(world.follow(&program, SIZE, 9), ((380, 200), false));
    world.button(BTN_LEFT, false);
    world.settle();
    world.shows_frame(&frame_of((380, 200), Shape::Normal), (left, top), &look(true));
    let (own_left, own_top) = middle(SIZE);
    assert_eq!(world.colour(own_left + 379, own_top), pattern::colour(9, 379, 0));

    // A corner moves two sides, and the corner across from it stays.
    world.point(left, top);
    world.button(BTN_LEFT, true);
    world.point(left - 10, top - 20);
    assert_eq!(world.follow(&program, SIZE, 10), ((390, 220), false));
    world.button(BTN_LEFT, false);
    world.settle();
    world.shows_frame(&frame_of((390, 220), Shape::Normal), (left - 10, top - 20), &look(true));

    // No window is narrower than its title bar's parts take.
    let least = Layout::least(&Theme::basic(), Bar::Full).0 as u32;
    world.point(left - 8, top + 100);
    world.button(BTN_LEFT, true);
    world.point(1023, top + 100);
    assert_eq!(world.follow(&program, SIZE, 11), ((least, 220), false));
    world.button(BTN_LEFT, false);
    world.settle();
    let narrow = frame_of((least, 220), Shape::Normal);
    let right = left - 10 + frame_of((390, 220), Shape::Normal).frame.w;
    world.shows_frame(&narrow, (right - narrow.frame.w, top - 20), &look(true));
}

#[test]
fn a_window_moved_before_its_program_drew_a_resize_s_last_size_stays_where_it_was_put() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    world.settle();
    let (layout, (left, top)) = framed(SIZE);
    // The right edge is dragged and let go, and the program is slow to
    // draw the size it was asked for.
    let edge = left + layout.frame.w - 2;
    world.point(edge, top + 120);
    world.button(BTN_LEFT, true);
    world.point(edge + 40, top + 120);
    world.button(BTN_LEFT, false);
    // Meanwhile the title bar moves the window.
    world.point(left + 100, top + 15);
    world.button(BTN_LEFT, true);
    world.point(left + 150, top + 40);
    world.point(left + 200, top + 65);
    world.button(BTN_LEFT, false);
    world.settle();
    let moved = (left + 100, top + 50);
    world.shows_frame(&layout, moved, &look(true));
    // The size shows where the window is now, not where it was resized.
    assert_eq!(world.follow(&program, SIZE, 8), ((360, 200), false));
    world.settle();
    world.shows_frame(&frame_of((360, 200), Shape::Normal), moved, &look(true));
}

#[test]
fn the_buttons_show_the_pointer_and_minimize_maximize_and_close() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    let (layout, (left, top)) = framed(SIZE);
    // A button is brighter under the pointer, and darker while held.
    let close = on(&layout, Button::Close);
    world.point(left + close.0, top + close.1);
    world.settle();
    let hot = Look { hot: Some(Button::Close), ..look(true) };
    world.shows_frame(&layout, (left, top), &hot);
    world.button(BTN_LEFT, true);
    world.settle();
    world.shows_frame(&layout, (left, top), &Look { pressed: Some(Button::Close), ..hot });
    // Let go beside the button, the press does nothing.
    world.point(left + 100, top + 15);
    world.settle();
    world.shows_frame(&layout, (left, top), &look(true));
    world.button(BTN_LEFT, false);
    assert!(!world.told(&program).contains(&Heard::Close));
    // On it, the program is asked to close the window.
    world.click(left + close.0, top + close.1);
    assert!(world.told(&program).contains(&Heard::Close));

    let maximize = on(&layout, Button::Maximize);
    world.click(left + maximize.0, top + maximize.1);
    assert_eq!(world.follow(&program, SIZE, 8), (FILLS, true));
    world.settle();
    let fills = frame_of(FILLS, Shape::Maximized);
    world.shows_frame(&fills, (0, 0), &look(true));
    let restore = on(&fills, Button::Maximize);
    world.click(restore.0, restore.1);
    assert_eq!(world.follow(&program, SIZE, 9), (SIZE, false));
    world.settle();
    world.shows_frame(&layout, (left, top), &look(true));

    // A minimized window is off the screen and has the keyboard no
    // more. The list of windows still has it.
    let minimize = on(&layout, Button::Minimize);
    world.click(left + minimize.0, top + minimize.1);
    let heard = world.heard(&program, |heard| *heard == Heard::Keyboard(false));
    assert!(heard.contains(&Heard::Active(false)), "{heard:?}");
    world.settle();
    for (x, y) in [(left, top), (left + 100, top + 15), (left + 160, top + 120)] {
        assert_eq!(world.colour(x as u32, y as u32), BACKGROUND, "({x}, {y})");
    }
    world.chord(KEY_LEFTALT, KEY_TAB);
    let heard = world.heard(&program, |heard| *heard == Heard::Keyboard(true));
    assert!(heard.contains(&Heard::Active(true)), "{heard:?}");
    world.settle();
    let hot = Look { hot: Some(Button::Minimize), ..look(true) };
    world.shows_frame(&layout, (left, top), &hot);
}

#[test]
fn alt_tab_goes_through_the_windows_by_last_use_and_shows_one_that_was_minimized() {
    let mut world = World::one_screen();
    let programs = [world.program(), world.program(), world.program()];
    for (program, (seed, title)) in programs.iter().zip([(7, "One"), (8, "Two"), (9, "Three")]) {
        world.show(program, SIZE, seed);
        world.ask(program, move |client| client.title(title)).unwrap();
    }
    world.settle();
    assert_eq!(world.front(), 9);
    // The list shows with the window used before this one chosen, and
    // Tab goes on from there, Shift+Tab back.
    let (entries, corner) = list(&["Three", "Two", "One"]);
    world.key(KEY_LEFTALT, true);
    world.tap(KEY_TAB);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(1), corner);
    world.tap(KEY_TAB);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(2), corner);
    world.chord(KEY_LEFTSHIFT, KEY_TAB);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(1), corner);
    world.tap(KEY_TAB);
    // Letting Alt go chooses: the window is in front with the keyboard.
    world.key(KEY_LEFTALT, false);
    world.settle();
    assert_eq!(world.front(), 7);
    let heard = world.heard(&programs[0], |heard| *heard == Heard::Keyboard(true));
    assert!(heard.contains(&Heard::Active(true)), "{heard:?}");
    // The window that had the keyboard heard Alt and Shift go down and
    // up, and nothing of Tab.
    let heard = world.heard(&programs[2], |heard| *heard == Heard::Keyboard(false));
    assert!(!heard.iter().any(|heard| key_but(heard, &[KEY_LEFTALT, KEY_LEFTSHIFT])), "{heard:?}");
    let alt_up = Heard::Key { code: u32::from(KEY_LEFTALT), pressed: false };
    assert!(heard.contains(&alt_up), "{heard:?}");

    // Super and the down arrow minimize the window, and the one then
    // in front has the keyboard. Alt+Tab shows it again.
    world.chord(KEY_LEFTMETA, KEY_DOWN);
    world.settle();
    assert_eq!(world.front(), 9);
    world.heard(&programs[2], |heard| *heard == Heard::Keyboard(true));
    let (entries, corner) = list(&["Three", "One", "Two"]);
    world.key(KEY_LEFTALT, true);
    world.tap(KEY_TAB);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(1), corner);
    world.key(KEY_LEFTALT, false);
    world.settle();
    assert_eq!(world.front(), 7);
    world.heard(&programs[0], |heard| *heard == Heard::Keyboard(true));
    // Alt+Shift+Tab begins at the window used longest ago.
    world.key(KEY_LEFTALT, true);
    world.chord(KEY_LEFTSHIFT, KEY_TAB);
    world.key(KEY_LEFTALT, false);
    world.settle();
    assert_eq!(world.front(), 8);
}

#[test]
fn the_super_key_alone_shows_the_list_for_the_keys_or_the_pointer_to_choose_from() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    for (program, (seed, title)) in [&first, &second].into_iter().zip([(7, "One"), (8, "Two")]) {
        world.show(program, SIZE, seed);
        world.ask(program, move |client| client.title(title)).unwrap();
    }
    // The list stays when Super is let go, with the window the person
    // works in chosen. The keys are the list's.
    let (entries, corner) = list(&["Two", "One"]);
    world.tap(KEY_LEFTMETA);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(0), corner);
    world.tap(KEY_A);
    world.tap(KEY_DOWN);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(1), corner);
    world.tap(KEY_ENTER);
    world.settle();
    assert_eq!(world.front(), 7);
    let heard = world.heard(&second, |heard| *heard == Heard::Keyboard(false));
    assert!(!heard.iter().any(|heard| key_but(heard, &[KEY_LEFTMETA])), "{heard:?}");

    // Esc puts the list away and changes nothing.
    world.tap(KEY_LEFTMETA);
    world.tap(KEY_DOWN);
    world.tap(KEY_ESC);
    world.settle();
    assert_eq!(world.front(), 7);
    assert_eq!(world.colour(corner.0 as u32 + 2, corner.1 as u32), BACKGROUND);

    // The pointer chooses the line it is on, and a click takes it.
    let (entries, corner) = list(&["One", "Two"]);
    world.tap(KEY_LEFTMETA);
    let (x, y) = row_of(LIST_WIDTH, corner, 1);
    world.point(x, y);
    world.settle();
    world.shows_panel(LIST_WIDTH, &entries, Some(1), corner);
    world.click(x, y);
    world.settle();
    assert_eq!(world.front(), 8);
    world.heard(&second, |heard| *heard == Heard::Keyboard(true));
    // A press beside the list puts it away.
    world.tap(KEY_LEFTMETA);
    world.click(10, 10);
    world.settle();
    assert_eq!(world.front(), 8);
    assert_eq!(world.colour(corner.0 as u32 + 2, corner.1 as u32), BACKGROUND);
}

#[test]
fn the_window_menu_does_what_the_buttons_do_and_has_the_keys_move_and_resize() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    let (layout, (left, top)) = framed(SIZE);
    // Alt+Space shows it under the title bar. The arrows go over the
    // lines that are off, and round.
    let under_bar = (left, top + layout.bar_rows());
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(false), None, under_bar);
    world.tap(KEY_DOWN);
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(false), Some(1), under_bar);
    world.tap(KEY_UP);
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(false), Some(6), under_bar);
    for _ in 0..4 {
        world.tap(KEY_DOWN);
    }
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(false), Some(4), under_bar);
    world.tap(KEY_ENTER);
    assert_eq!(world.follow(&program, SIZE, 8), (FILLS, true));

    // The right button on the title bar shows it at the pointer, and
    // the lines a maximized window has no use for are off.
    world.point(300, 10);
    world.button(BTN_RIGHT, true);
    world.button(BTN_RIGHT, false);
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(true), None, (300, 10));
    let (x, y) = row_of(MENU_WIDTH, (300, 10), 0);
    world.click(x, y);
    assert_eq!(world.follow(&program, SIZE, 9), (SIZE, false));
    world.settle();
    world.shows_frame(&layout, (left, top), &look(true));

    // Move: the arrows move the window, by a pixel with Ctrl, until
    // Enter.
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.tap(KEY_DOWN);
    world.tap(KEY_ENTER);
    world.tap(KEY_RIGHT);
    world.tap(KEY_RIGHT);
    world.chord(KEY_LEFTCTRL, KEY_DOWN);
    world.tap(KEY_ENTER);
    world.settle();
    let moved = (left + 32, top + 1);
    world.shows_frame(&layout, moved, &look(true));
    // Esc puts the window back where it was.
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.tap(KEY_DOWN);
    world.tap(KEY_ENTER);
    for _ in 0..3 {
        world.tap(KEY_LEFT);
    }
    world.settle();
    world.shows_frame(&layout, (moved.0 - 48, moved.1), &look(true));
    world.tap(KEY_ESC);
    world.settle();
    world.shows_frame(&layout, moved, &look(true));

    // Size: the arrows move the right and the bottom side.
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.tap(KEY_DOWN);
    world.tap(KEY_DOWN);
    world.tap(KEY_ENTER);
    world.tap(KEY_RIGHT);
    assert_eq!(world.follow(&program, SIZE, 10), ((336, 200), false));
    world.tap(KEY_DOWN);
    assert_eq!(world.follow(&program, SIZE, 11), ((336, 216), false));
    world.tap(KEY_ENTER);
    world.settle();
    world.shows_frame(&frame_of((336, 216), Shape::Normal), moved, &look(true));

    // None of those keys reached the program, and Close and Alt+F4 ask
    // it to close the window.
    let told = world.told(&program);
    assert!(!told.iter().any(|heard| key_but(heard, &[KEY_LEFTALT, KEY_LEFTCTRL])), "{told:?}");
    assert!(!told.contains(&Heard::Close));
    world.chord(KEY_LEFTALT, KEY_SPACE);
    world.tap(KEY_UP);
    world.tap(KEY_ENTER);
    assert!(world.told(&program).contains(&Heard::Close));
    world.chord(KEY_LEFTALT, KEY_F4);
    let told = world.told(&program);
    assert!(told.contains(&Heard::Close), "{told:?}");
    assert!(!told.iter().any(|heard| key_but(heard, &[KEY_LEFTALT])), "{told:?}");
}

#[test]
fn a_program_has_the_pointer_move_and_resize_its_window_with_the_button_it_was_told_of() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    let (layout, (left, top)) = framed(SIZE);
    let own = (left + layout.surface.x, top + layout.surface.y);
    // A press on the window's own pixels, as on a title bar the
    // program draws itself.
    world.point(own.0 + 10, own.1 + 10);
    world.button(BTN_LEFT, true);
    world
        .ask(&program, |client| {
            client.wait(PATIENCE, pressed).unwrap();
            client.take_heard();
            client.ask_move()
        })
        .unwrap();
    world.point(own.0 + 40, own.1 + 30);
    world.settle();
    world.shows_frame(&layout, (left + 30, top + 20), &look(true));
    // The pointer is the window manager's until the button is let go:
    // the program hears it leave, and come back then.
    world.button(BTN_LEFT, false);
    let told = world.told(&program);
    let came = told.iter().position(|heard| matches!(heard, Heard::Enter(..)));
    let left_at = told.iter().position(|heard| *heard == Heard::Leave);
    assert!(left_at.is_some() && left_at < came, "{told:?}");
    assert!(!told.iter().any(|heard| matches!(heard, Heard::Button { .. })), "{told:?}");

    // The same for an edge.
    world.button(BTN_LEFT, true);
    world
        .ask(&program, |client| {
            client.wait(PATIENCE, pressed).unwrap();
            client.ask_resize(ResizeEdge::BottomRight)
        })
        .unwrap();
    world.point(own.0 + 56, own.1 + 38);
    assert_eq!(world.follow(&program, SIZE, 8), ((336, 208), false));
    world.button(BTN_LEFT, false);
    world.settle();
    let larger = frame_of((336, 208), Shape::Normal);
    world.shows_frame(&larger, (left + 30, top + 20), &look(true));

    // With the serial of a button that is up again, nothing moves.
    world.ask(&program, Client::ask_move).unwrap();
    world.point(own.0 + 100, own.1 + 100);
    world.settle();
    world.shows_frame(&larger, (left + 30, top + 20), &look(true));

    // The program may ask for the window's menu, at a place of the
    // window.
    world.ask(&program, |client| client.ask_menu(10, 10)).unwrap();
    world.settle();
    world.shows_panel(MENU_WIDTH, &menu(false), None, (own.0 + 40, own.1 + 30));
    world.tap(KEY_ESC);
    world.settle();
    assert_eq!(world.colour(own.0 as u32 + 42, own.1 as u32 + 30), pattern::colour(8, 12, 10));
}

#[test]
fn a_popup_has_the_keys_and_the_pointer_until_a_press_elsewhere_or_a_key_of_windowd_s() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, SIZE, 7);
    world.show(&second, SIZE, 8);
    let (left, top) = middle(SIZE);
    // A press on the first, whose program opens a menu of its own while
    // the button is down.
    world.point(left as i32 + 5, top as i32 + 5);
    world.button(BTN_LEFT, true);
    world
        .ask(&first, |client| {
            client.wait(PATIENCE, pressed).unwrap();
            client.popup((100, 60), (300, 180), 21, true)
        })
        .unwrap();
    world.button(BTN_LEFT, false);
    world.settle();
    // It shows where the program put it, over the window's frame and
    // beside the window.
    for (x, y) in [(0, 0), (99, 59), (50, 30)] {
        let shows = world.colour(left + 300 + x, top + 180 + y);
        assert_eq!(shows, pattern::colour(21, x, y), "({x}, {y})");
    }
    // The keys are its program's, and the pointer over it.
    world.tap(KEY_A);
    world.point(left as i32 + 350, top as i32 + 210);
    let told = world.told(&first);
    assert!(told.contains(&Heard::Key { code: u32::from(KEY_A), pressed: true }), "{told:?}");
    assert_eq!(told.last(), Some(&Heard::Enter(50.5, 30.5)), "{told:?}");

    // A press on the other window: the popup is done, and the press is
    // the other window's, with the keyboard.
    world.click(left as i32 + 345, top as i32 + 100);
    world.heard(&first, |heard| *heard == Heard::PopupDone);
    world.ask(&first, Client::end_popup).unwrap();
    let heard = world.heard(&second, |heard| pressed(heard).is_some());
    assert!(heard.contains(&Heard::Keyboard(true)), "{heard:?}");
    world.settle();
    assert_eq!(world.colour(left + 350, top + 210), pattern::colour(8, 318, 178));

    // A key of windowd's ends a popup as well.
    world.button(BTN_LEFT, true);
    world
        .ask(&second, |client| {
            client.wait(PATIENCE, pressed).unwrap();
            client.popup((80, 40), (10, 10), 22, true)
        })
        .unwrap();
    world.button(BTN_LEFT, false);
    world.settle();
    assert_eq!(world.colour(left + 42, top + 42), pattern::colour(22, 0, 0));
    world.chord(KEY_LEFTALT, KEY_TAB);
    world.heard(&second, |heard| *heard == Heard::PopupDone);
    world.ask(&second, Client::end_popup).unwrap();
    world.heard(&first, |heard| *heard == Heard::Keyboard(true));
    world.settle();
    assert_eq!(world.colour(left + 42, top + 42), pattern::colour(7, 42, 42));
}

#[test]
fn a_program_asks_for_its_window_to_be_maximized_restored_and_minimized() {
    let mut world = World::one_screen();
    let program = world.program();
    // Before the window shows: it shows maximized.
    world.ask(&program, |client| client.open(Open { maximized: true, ..Open::default() })).unwrap();
    assert_eq!(world.follow(&program, SIZE, 7), (FILLS, true));
    world.settle();
    let fills = frame_of(FILLS, Shape::Maximized);
    world.shows_frame(&fills, (0, 0), &look(true));
    assert_eq!(world.colour(0, 36), pattern::colour(7, 0, 0));
    // Restored, it is of the size its program chooses, in the middle.
    world.ask(&program, |client| client.maximize(false)).unwrap();
    assert_eq!(world.follow(&program, SIZE, 8), (SIZE, false));
    world.settle();
    let (layout, corner) = framed(SIZE);
    world.shows_frame(&layout, corner, &look(true));
    world.ask(&program, |client| client.maximize(true)).unwrap();
    assert_eq!(world.follow(&program, SIZE, 9), (FILLS, true));
    world.settle();
    world.shows_frame(&fills, (0, 0), &look(true));
    world.ask(&program, Client::minimize).unwrap();
    world.heard(&program, |heard| *heard == Heard::Keyboard(false));
    world.settle();
    assert_eq!(world.colour(512, 10), BACKGROUND);
    assert_eq!(world.colour(512, 384), BACKGROUND);
}

#[test]
fn a_frame_shows_its_window_s_title_and_whether_the_person_works_in_it() {
    let mut world = World::one_screen();
    let (first, second) = (world.program(), world.program());
    world.show(&first, SIZE, 7);
    world.show(&second, SIZE, 8);
    let (layout, (left, top)) = framed(SIZE);
    // The second, a step down and to the right, is moved clear of the
    // first.
    world.point(left + 132, top + 47);
    world.button(BTN_LEFT, true);
    world.point(left + 132, top + 267);
    world.button(BTN_LEFT, false);
    world.settle();
    world.shows_frame(&layout, (left, top), &look(false));
    world.shows_frame(&layout, (left + 32, top + 252), &look(true));
    world.ask(&first, |client| client.title("A longer name")).unwrap();
    world.settle();
    let named = |active| Look { title: "A longer name", ..look(active) };
    world.shows_frame(&layout, (left, top), &named(false));
    // A press on a frame gives its window the keyboard.
    world.click(left + 100, top + 15);
    world.heard(&first, |heard| *heard == Heard::Keyboard(true));
    world.settle();
    world.shows_frame(&layout, (left, top), &named(true));
    world.shows_frame(&layout, (left + 32, top + 252), &look(false));
}
