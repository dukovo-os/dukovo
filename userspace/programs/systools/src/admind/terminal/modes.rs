// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The private modes a pane's program has set that change how its screen
//! must be drawn, followed in its output as it passes: `CSI ? Pm h` sets
//! them and `CSI ? Pm l` resets them. A page that attaches to a pane that
//! has run a while gets them first, so that the replay of the ring draws
//! on the right screen, with the cursor shown or hidden, and with the
//! mouse and paste the program asked for.
//!
//! This is a scanner, not a parser: it looks for those sequences alone,
//! across the pieces the output comes in, and lets every other byte pass.
//! A sequence longer than any it follows is let go.

/// The modes followed, and whether each is set before any program says:
/// the cursor keys (1), wrap at the margin (7, set), the cursor shown (25,
/// set), the other screen (47, 1047, 1049), the mouse (1000, 1002, 1006),
/// and bracketed paste (2004). The screens come first, as the others are
/// set on the screen in use.
const FOLLOWED: [(u16, bool); 10] = [
    (1049, false),
    (1047, false),
    (47, false),
    (1, false),
    (7, true),
    (25, true),
    (1000, false),
    (1002, false),
    (1006, false),
    (2004, false),
];

/// The most bytes of parameters a sequence may have before it is let go.
const PARAMETERS_MAX: usize = 32;

/// Where the scanner is in a sequence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Outside any.
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// After ESC [.
    Csi,
    /// After ESC [ ?, in the parameters.
    Private,
}

/// The modes of a pane, as its output has set them so far.
#[derive(Clone, Debug)]
pub struct Modes {
    set: [bool; FOLLOWED.len()],
    state: State,
    parameters: Vec<u8>,
}

impl Default for Modes {
    fn default() -> Modes {
        Modes { set: FOLLOWED.map(|(_, set)| set), state: State::Ground, parameters: Vec::new() }
    }
}

impl Modes {
    /// Follows the modes `output` sets and resets.
    pub fn scan(&mut self, output: &[u8]) {
        for &byte in output {
            self.state = match (self.state, byte) {
                // ESC starts a sequence anywhere, as a terminal reads it.
                (_, 0x1b) => State::Escape,
                (State::Escape, b'[') => State::Csi,
                (State::Csi, b'?') => {
                    self.parameters.clear();
                    State::Private
                }
                (State::Private, b'0'..=b'9' | b';') if self.parameters.len() < PARAMETERS_MAX => {
                    self.parameters.push(byte);
                    State::Private
                }
                (State::Private, b'h') => {
                    self.apply(true);
                    State::Ground
                }
                (State::Private, b'l') => {
                    self.apply(false);
                    State::Ground
                }
                _ => State::Ground,
            };
        }
    }

    /// Sets or resets each followed mode the parameters name.
    fn apply(&mut self, on: bool) {
        let text = String::from_utf8_lossy(&self.parameters);
        for number in text.split(';').filter_map(|part| part.parse::<u16>().ok()) {
            if let Some(at) = FOLLOWED.iter().position(|&(mode, _)| mode == number) {
                self.set[at] = on;
            }
        }
    }

    /// The sequences that put a new screen in the modes the pane is in,
    /// for those that differ from where a screen starts: none for a pane
    /// whose program changed nothing.
    pub fn sequences(&self) -> String {
        use std::fmt::Write;
        let mut sequences = String::new();
        for (&(mode, start), set) in FOLLOWED.iter().zip(self.set) {
            if set != start {
                let _ = write!(sequences, "\x1b[?{mode}{}", if set { 'h' } else { 'l' });
            }
        }
        sequences
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pane_needs_no_sequence() {
        assert_eq!(Modes::default().sequences(), "");
    }

    #[test]
    fn modes_set_and_reset_are_followed_and_the_screen_comes_first() {
        let mut modes = Modes::default();
        modes.scan(b"\x1b[?2004h\x1b[?25l\x1b[?1049h hello \x1b[?1000;1006h\x1b[?7l");
        assert_eq!(
            modes.sequences(),
            "\x1b[?1049h\x1b[?7l\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[?2004h"
        );
        modes.scan(b"\x1b[?1049l\x1b[?25h\x1b[?7h\x1b[?1000;1006;2004l");
        assert_eq!(modes.sequences(), "");
    }

    #[test]
    fn a_sequence_split_between_pieces_is_followed() {
        let sequence = b"\x1b[?1;1049h";
        for at in 0..sequence.len() {
            let mut modes = Modes::default();
            modes.scan(&sequence[..at]);
            modes.scan(&sequence[at..]);
            assert_eq!(modes.sequences(), "\x1b[?1049h\x1b[?1h", "split at {at}");
        }
    }

    #[test]
    fn other_sequences_and_modes_pass_and_a_long_one_is_let_go() {
        let mut modes = Modes::default();
        // Not private, another final byte, a mode not followed, a number
        // too big, and an ESC that starts another sequence.
        modes.scan(b"\x1b[1049h\x1b[?1049r\x1b[?12h\x1b[?99999h\x1b[?10\x1b[?2004h");
        assert_eq!(modes.sequences(), "\x1b[?2004h");
        let mut long = b"\x1b[?".to_vec();
        long.extend(std::iter::repeat_n(b';', PARAMETERS_MAX));
        long.extend_from_slice(b"25l");
        modes.scan(&long);
        assert_eq!(modes.sequences(), "\x1b[?2004h", "let go");
    }
}
