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
//! Three chords never reach a program. Ctrl+Alt+F1 to F12 shows the
//! console of that number, as it does at a console (consoled(8)).
//! Ctrl+Alt+Backspace ends the desktop.

use input::abi::{KEY_BACKSPACE, KEY_F1, KEY_F10, KEY_F11, KEY_F12};
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
    fn the_lights_are_the_bits_the_devices_take() {
        let lights = LedState { num: Some(true), caps: Some(false), scroll: None };
        assert_eq!(light_bits(lights), input::abi::LED_NUM_LOCK);
        let all = LedState { num: Some(true), caps: Some(true), scroll: Some(true) };
        assert_eq!(light_bits(all), 7);
    }
}
