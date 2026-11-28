// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The keyboard's layout for the programs, and the keys windowd keeps
//! for itself.
//!
//! The programs are given the layout the consoles use
//! (_/system/settings/keyboard_, keymap(1)), as the layout of that name
//! in the XKB data: `us-intl` is XKB's layout `us` in its variant
//! `intl`. A layout of a person's own (_/system/settings/keymaps_) has
//! no XKB twin, and the programs get `us`.
//!
//! Some chords never reach a program. Ctrl+Alt+F1 to F12 shows the
//! console of that number, as it does at a console (consoled(8)).
//! Ctrl+Alt+Backspace ends the desktop. The window manager's own are
//! Alt+Tab and Alt+Shift+Tab for the list of windows, Alt+F4 to close
//! a window, Alt+Space for its menu, and the Super key with the up or
//! the down arrow to maximize, restore or minimize it; Super pressed
//! and let go alone shows the list.

use input::abi::{
    KEY_BACKSPACE, KEY_CAPSLOCK, KEY_DOWN, KEY_F1, KEY_F4, KEY_F10, KEY_F11, KEY_F12, KEY_LEFT,
    KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_NUMLOCK, KEY_RIGHT, KEY_RIGHTALT,
    KEY_RIGHTCTRL, KEY_RIGHTMETA, KEY_RIGHTSHIFT, KEY_SCROLLLOCK, KEY_SPACE, KEY_TAB, KEY_UP,
};
use smithay::input::keyboard::{LedState, ModifiersState};

/// The XKB layout and variant of the consoles' layout `name`.
pub fn xkb_names(name: &str) -> (&str, &str) {
    if !keymap::LAYOUTS.iter().any(|(built_in, _)| *built_in == name) {
        return (keymap::DEFAULT, "");
    }
    name.split_once('-').unwrap_or((name, ""))
}

/// What a chord asks of windowd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    /// The console of this number is to show.
    Console(u32),
    /// The desktop is to end.
    End,
}

/// What pressing the key of Linux code `code` asks of windowd while the
/// modifiers are `held`, if it is a chord.
pub fn chord(held: &ModifiersState, code: u16) -> Option<Chord> {
    if !held.ctrl || !held.alt || held.shift || held.logo {
        return None;
    }
    match code {
        KEY_F1..=KEY_F10 => Some(Chord::Console(u32::from(code - KEY_F1) + 1)),
        KEY_F11 => Some(Chord::Console(11)),
        KEY_F12 => Some(Chord::Console(12)),
        KEY_BACKSPACE => Some(Chord::End),
        _ => None,
    }
}

/// What a key asks of the window manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// The list of windows, at the window used before this one, and
    /// from there the next.
    Next,
    /// The list of windows, at the one used longest ago, and from there
    /// the one before.
    Previous,
    /// The list of windows, to stay until a window is chosen.
    List,
    /// The window the person works in is to close.
    Close,
    /// That window's menu.
    Menu,
    /// That window is to be maximized.
    Up,
    /// That window is to be restored if it is maximized, and minimized
    /// if it is not.
    Down,
    /// That window is to go to the next screen from left to right, or
    /// to the one before for `back`.
    ToScreen {
        /// Whether to the screen before.
        back: bool,
    },
}

/// What pressing the key of Linux code `code` asks of the window
/// manager while the modifiers are `held`, if anything.
pub fn ask(held: &ModifiersState, code: u16) -> Option<Ask> {
    let alt_alone = held.alt && !held.ctrl && !held.logo;
    let super_alone = held.logo && !held.ctrl && !held.alt && !held.shift;
    let super_shift = held.logo && held.shift && !held.ctrl && !held.alt;
    match code {
        KEY_TAB if alt_alone && held.shift => Some(Ask::Previous),
        KEY_TAB if alt_alone => Some(Ask::Next),
        KEY_F4 if alt_alone && !held.shift => Some(Ask::Close),
        KEY_SPACE if alt_alone && !held.shift => Some(Ask::Menu),
        KEY_UP if super_alone => Some(Ask::Up),
        KEY_DOWN if super_alone => Some(Ask::Down),
        KEY_LEFT if super_shift => Some(Ask::ToScreen { back: true }),
        KEY_RIGHT if super_shift => Some(Ask::ToScreen { back: false }),
        _ => None,
    }
}

/// Whether the key of Linux code `code` is one that changes what other
/// keys mean. Such a key always reaches the program that has the
/// keyboard, so that the program never holds one down that the person
/// has let go.
pub fn modifier(code: u16) -> bool {
    matches!(
        code,
        KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_CAPSLOCK
            | KEY_NUMLOCK
            | KEY_SCROLLLOCK
    ) || is_super(code)
}

/// Whether the key of Linux code `code` is a Super key.
pub fn is_super(code: u16) -> bool {
    matches!(code, KEY_LEFTMETA | KEY_RIGHTMETA)
}

/// The lock lights `lights` asks for, as INPUT_LIGHTS takes them
/// (radix-input(2)).
pub fn light_bits(lights: LedState) -> u32 {
    use input::abi::{LED_CAPS_LOCK, LED_NUM_LOCK, LED_SCROLL_LOCK};
    let lit = |on: Option<bool>, bit: u32| if on == Some(true) { bit } else { 0 };
    lit(lights.num, LED_NUM_LOCK)
        | lit(lights.caps, LED_CAPS_LOCK)
        | lit(lights.scroll, LED_SCROLL_LOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_layout_is_its_xkb_layout_and_variant() {
        assert_eq!(xkb_names("us"), ("us", ""));
        assert_eq!(xkb_names("us-intl"), ("us", "intl"));
        assert_eq!(xkb_names("us-altgr-intl"), ("us", "altgr-intl"));
        assert_eq!(xkb_names("ch-fr"), ("ch", "fr"));
        assert_eq!(xkb_names("latam"), ("latam", ""));
        // A person's own layout.
        assert_eq!(xkb_names("mine"), ("us", ""));
    }

    #[test]
    fn only_ctrl_and_alt_alone_make_a_chord() {
        let held =
            |ctrl, alt, shift| ModifiersState { ctrl, alt, shift, ..ModifiersState::default() };
        assert_eq!(chord(&held(true, true, false), KEY_F1), Some(Chord::Console(1)));
        assert_eq!(chord(&held(true, true, false), KEY_F10), Some(Chord::Console(10)));
        assert_eq!(chord(&held(true, true, false), KEY_F11), Some(Chord::Console(11)));
        assert_eq!(chord(&held(true, true, false), KEY_F12), Some(Chord::Console(12)));
        assert_eq!(chord(&held(true, true, false), KEY_BACKSPACE), Some(Chord::End));
        assert_eq!(chord(&held(true, true, false), input::abi::KEY_A), None);
        assert_eq!(chord(&held(true, false, false), KEY_F1), None);
        assert_eq!(chord(&held(false, true, false), KEY_F1), None);
        assert_eq!(chord(&held(true, true, true), KEY_F1), None);
    }

    #[test]
    fn the_window_managers_keys_are_alt_and_super_alone() {
        let held = |alt, logo, shift| ModifiersState { alt, logo, shift, ..Default::default() };
        assert_eq!(ask(&held(true, false, false), KEY_TAB), Some(Ask::Next));
        assert_eq!(ask(&held(true, false, true), KEY_TAB), Some(Ask::Previous));
        assert_eq!(ask(&held(true, false, false), KEY_F4), Some(Ask::Close));
        assert_eq!(ask(&held(true, false, false), KEY_SPACE), Some(Ask::Menu));
        assert_eq!(ask(&held(false, true, false), KEY_UP), Some(Ask::Up));
        assert_eq!(ask(&held(false, true, false), KEY_DOWN), Some(Ask::Down));
        let to_screen = |back| Some(Ask::ToScreen { back });
        assert_eq!(ask(&held(false, true, true), KEY_LEFT), to_screen(true));
        assert_eq!(ask(&held(false, true, true), KEY_RIGHT), to_screen(false));
        assert_eq!(ask(&held(false, true, false), KEY_LEFT), None);
        // Not with another modifier, and not the key alone.
        assert_eq!(ask(&held(false, false, false), KEY_TAB), None);
        assert_eq!(ask(&held(true, true, false), KEY_TAB), None);
        assert_eq!(ask(&held(true, false, true), KEY_F4), None);
        assert_eq!(ask(&held(false, true, true), KEY_UP), None);
        assert_eq!(ask(&held(true, false, false), KEY_UP), None);
        let ctrl = ModifiersState { ctrl: true, alt: true, ..Default::default() };
        assert_eq!(ask(&ctrl, KEY_TAB), None);
    }

    #[test]
    fn the_keys_that_change_others_are_modifiers() {
        for code in [KEY_LEFTSHIFT, KEY_RIGHTCTRL, KEY_LEFTALT, KEY_RIGHTMETA, KEY_CAPSLOCK] {
            assert!(modifier(code), "{code}");
        }
        assert!(!modifier(KEY_TAB) && !modifier(input::abi::KEY_A));
        assert!(is_super(KEY_LEFTMETA) && !is_super(KEY_LEFTALT));
    }

    #[test]
    fn the_lights_are_the_bits_the_devices_take() {
        let lights = LedState { num: Some(true), caps: Some(false), scroll: None };
        assert_eq!(light_bits(lights), input::abi::LED_NUM_LOCK);
        let all = LedState { num: Some(true), caps: Some(true), scroll: Some(true) };
        assert_eq!(light_bits(all), 7);
    }
}
