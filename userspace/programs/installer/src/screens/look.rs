// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How the screens look: the approved design's sixteen colours, and the
//! styles the screens draw with (ADR 0402).
//!
//! The screens draw with the terminal's palette indices 0 to 15, and set
//! those indices to the design's colours while they show (xterm's OSC 4,
//! which consoled(8) reads too), giving the terminal's own back when they
//! end (OSC 104). The background is index 0, which consoled also paints
//! the screen's margins in, so the screens' slate fills the screen edge to
//! edge. A terminal that cannot change its palette shows the design in
//! its own sixteen, which keep their places: the focus yellow, the fields
//! blue.
//!
//! The console draws bold text in the bright one of the first eight
//! colours, as Linux's does, and has no bold font, so the styles here
//! make nothing bold that is drawn in colour 0: the focus's dark text
//! would come out grey.

use std::fmt::Write as _;

use ratatui::style::{Color, Modifier, Style};

/// The design's sixteen colours, as red, green and blue, for palette
/// indices 0 to 15.
pub const PALETTE: [(u8, u8, u8); 16] = [
    (0x15, 0x18, 0x1c), // 0 slate: the background
    (0xe0, 0x6c, 0x6c), // 1 red
    (0x86, 0xc2, 0x8a), // 2 green
    (0xc8, 0x87, 0x3a), // 3 tan: what has the focus, the progress
    (0x44, 0x59, 0x6b), // 4 steel: fields, the key bar
    (0xb4, 0x8e, 0xad), // 5 mauve
    (0x6f, 0xa8, 0xb8), // 6 teal
    (0xd9, 0xd4, 0xca), // 7 cream: text
    (0x7c, 0x88, 0x96), // 8 grey: notes, what is not chosen
    (0xff, 0x8f, 0x87), // 9 coral: problems
    (0xa8, 0xe6, 0xa1), // 10 mint: what is done
    (0xe9, 0xc4, 0x8b), // 11 sand: titles, what is said
    (0x8f, 0xa6, 0xba), // 12 blue grey: borders, hints in fields
    (0xd7, 0xb4, 0xf0), // 13 lilac
    (0xa9, 0xd7, 0xe6), // 14 sky
    (0xf6, 0xf1, 0xe8), // 15 white: values, what is chosen
];

/// The background, the slate the screen is painted in.
pub const SLATE: Color = Color::Indexed(0);
/// What has the focus, and the progress bar.
pub const TAN: Color = Color::Indexed(3);
/// The fields' background, and the key bar's.
pub const STEEL: Color = Color::Indexed(4);
/// Text.
pub const CREAM: Color = Color::Indexed(7);
/// Notes, and what is not chosen or cannot be.
pub const GREY: Color = Color::Indexed(8);
/// Problems.
pub const CORAL: Color = Color::Indexed(9);
/// What is done.
pub const MINT: Color = Color::Indexed(10);
/// Titles, warnings, and what the screens say.
pub const SAND: Color = Color::Indexed(11);
/// Borders, and the hint in an empty field.
pub const BLUE_GREY: Color = Color::Indexed(12);
/// A field's value, what is chosen, and the key bar's text.
pub const WHITE: Color = Color::Indexed(15);

/// What has the focus: dark on tan, not bold (see the module's notes).
#[must_use]
pub fn focus() -> Style {
    Style::new().fg(SLATE).bg(TAN)
}

/// A field to type in or choose in: white on steel.
#[must_use]
pub fn field() -> Style {
    Style::new().fg(WHITE).bg(STEEL)
}

/// A title: sand and bold.
#[must_use]
pub fn title() -> Style {
    Style::new().fg(SAND).add_modifier(Modifier::BOLD)
}

/// The key bar at the bottom of the screen: white on steel.
#[must_use]
pub fn key_bar() -> Style {
    Style::new().fg(WHITE).bg(STEEL)
}

/// What the terminal is sent when the screens start: the design's
/// colours, each index set by its own OSC 4, ended by ST.
#[must_use]
pub fn set_palette() -> String {
    let mut codes = String::new();
    for (index, (r, g, b)) in PALETTE.iter().enumerate() {
        let _ = write!(codes, "\x1b]4;{index};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\");
    }
    codes
}

/// What the terminal is sent when the screens end: OSC 104, which gives
/// every index its colour back.
pub const RESET_PALETTE: &str = "\x1b]104\x1b\\";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_each_of_the_sixteen() {
        let codes = set_palette();
        assert_eq!(codes.matches("\x1b]4;").count(), 16);
        assert!(codes.starts_with("\x1b]4;0;rgb:15/18/1c\x1b\\"), "{codes:?}");
        assert!(codes.ends_with("\x1b]4;15;rgb:f6/f1/e8\x1b\\"), "{codes:?}");
    }
}
