// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The person's settings laid on a desktop (windowd.json(5), ADR 0722).

use window_frame::{Bar, Layout, Shape, Theme};
use window_test::{BarSaid, Heard, Mode, Open};

use super::{Program, World, framed, look};
use crate::settings::Settings;

/// The size the test window draws itself, and the size it is asked for
/// when it fills the screen under a title bar.
const SIZE: (u32, u32) = (320, 200);
const FILLS: (u32, u32) = (1024, 732);

/// The settings of a file of `text`, which has nothing wrong in it.
fn settings(text: &str) -> Settings {
    let mut settings = Settings::default();
    let wrong = settings.lay("windowd.json", text);
    assert!(wrong.is_empty(), "{wrong:?}");
    settings
}

impl World {
    /// A program whose window asks for a part of its title bar, and
    /// what it is told of the bar.
    fn asks_for_the_bar(&mut self) -> (Program, Option<BarSaid>) {
        let program = self.program();
        let said = self.ask(&program, |client| {
            client.open(Open { shares_bar: true, ..Open::default() }).unwrap();
            client.bar().unwrap()
        });
        (program, said)
    }

    /// Lets `ms` milliseconds pass with nothing done in them.
    fn rest(&mut self, ms: u64) {
        self.kernel.borrow_mut().pass(ms * 1_000_000);
        self.hear_layer();
    }
}

#[test]
fn the_pointer_moves_as_many_times_as_far_as_the_person_has_it() {
    let mut world = World::one_screen();
    let before = world.showed();
    // A slow hand: a pixel for a count as built, from the middle of
    // the screen.
    world.desktop.set(&settings(r#"{"pointer": {"speed": 1}}"#));
    let now = world.now();
    world.desktop.moved(now, 4, -2);
    assert_eq!(world.showed_since(before).last().unwrap().pointer_at, Some((520, 380)));
    world.desktop.set(&settings(r#"{"pointer": {"speed": -1}}"#));
    world.desktop.moved(now + 20_000_000, -4, 2);
    assert_eq!(world.showed_since(before).last().unwrap().pointer_at, Some((518, 381)));
    // Laid again without the key, the speed is as built.
    world.desktop.set(&Settings::default());
    world.desktop.moved(now + 40_000_000, 4, -2);
    assert_eq!(world.showed_since(before).last().unwrap().pointer_at, Some((522, 379)));
}

#[test]
fn two_clicks_are_a_double_click_within_the_time_the_person_has_it() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, SIZE, 7);
    let (layout, (left, top)) = framed(SIZE);
    world.desktop.set(&settings(r#"{"pointer": {"double_click_ms": 100}}"#));
    // Two clicks on the title bar further apart than that are two
    // clicks: the window is as it was.
    world.click(left + 100, top + 15);
    world.rest(150);
    world.click(left + 100, top + 15);
    world.settle();
    let told = world.told(&program);
    assert!(!told.iter().any(|heard| matches!(heard, Heard::Size { .. })), "{told:?}");
    world.shows_frame(&layout, (left, top), &look(true));
    // Two within it maximize the window.
    world.rest(150);
    world.click(left + 100, top + 15);
    world.rest(90);
    world.click(left + 100, top + 15);
    assert_eq!(world.follow(&program, SIZE, 8), (FILLS, true));
    world.settle();
    let fills = Layout::of(&Theme::basic(), (1024, 732), Shape::Maximized, Bar::Full, None);
    world.shows_frame(&fills, (0, 0), &look(true));
}

#[test]
fn every_program_is_told_how_the_person_has_a_held_key_repeat() {
    let mut world = World::one_screen();
    let first = world.program();
    world.show(&first, SIZE, 7);
    assert_eq!(world.ask(&first, |client| client.repeat()), Some((250, 30)));
    let text = r#"{"keyboard": {"repeat_delay_ms": 400, "repeat_rate": 12}}"#;
    world.desktop.set(&settings(text));
    // The program that was there is told, and one that comes after.
    world.told(&first);
    assert_eq!(world.ask(&first, |client| client.repeat()), Some((400, 12)));
    let second = world.program();
    assert_eq!(world.ask(&second, |client| client.repeat()), Some((400, 12)));
}

#[test]
fn the_person_s_settings_say_which_applications_share_their_title_bars() {
    const SHARED: BarSaid = BarSaid { mode: Mode::Shared, height: 36, left: 30, right: 226 };
    const FULL: BarSaid = BarSaid { mode: Mode::Full, ..SHARED };
    let mut world = World::one_screen();
    // Every bar full: a program that asks is told the bar is windowd's.
    world.desktop.set(&settings(r#"{"title_bars": {"apps": "full"}}"#));
    let (first, said) = world.asks_for_the_bar();
    assert_eq!(said, Some(FULL));
    world.show(&first, SIZE, 7);
    let (layout, corner) = framed(SIZE);
    // The application named as one that shares is told so when the
    // settings are laid, and has the bar once it has drawn for it.
    let text = r#"{"title_bars": {"apps": "full", "shared": ["org.dukovo.window-test"]}}"#;
    world.desktop.set(&settings(text));
    assert_eq!(world.ask(&first, |client| client.bar().unwrap()), Some(SHARED));
    world.settle();
    world.shows_frame(&layout, corner, &look(true));
    let higher = (SIZE.0, SIZE.1 + 36);
    world.show(&first, higher, 8);
    let shared = Layout::of(&Theme::basic(), (320, 236), Shape::Normal, Bar::Shared, None);
    world.shows_frame(&shared, corner, &look(true));
    // Named as one with the full bar where programs decide, it is told
    // that, and so is a window of it that comes after.
    let text = r#"{"title_bars": {"full": ["org.dukovo.window-test"]}}"#;
    world.desktop.set(&settings(text));
    assert_eq!(world.ask(&first, |client| client.bar().unwrap()), Some(FULL));
    world.show(&first, SIZE, 9);
    world.shows_frame(&layout, corner, &look(true));
    let (_second, said) = world.asks_for_the_bar();
    assert_eq!(said, Some(FULL));
    // Nothing was chosen from a window's menu: there is nothing to
    // keep in the person's file.
    assert!(world.desktop.chosen().is_empty());
}
