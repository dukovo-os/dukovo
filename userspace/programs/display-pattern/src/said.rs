// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lines `display-test` says, and the host reads.
//!
//! Each starts with [`START`]. A screen is named by its card and its
//! place on the card, as `virtio0/1`. The lines that say what a screen
//! shows build on each other: the host keeps a picture for each screen
//! and changes it as the lines say, so a line says only what is new.
//!
//! ```text
//! display-test: shows virtio0/0 1024x768 seed 3
//! display-test: patch virtio0/0 200x100 at 300,200 seed 4
//! display-test: pointer firmware0/0 32x32 at 196,144 seed 5
//! display-test: dark virtio0/1
//! display-test: step shown.
//! display-test: passed
//! display-test: FAILED: the card has 3 screens, not 4
//! ```
//!
//! - `shows`: the screen is of this size and shows all of the picture
//!   of the seed ([`crate::pattern`]).
//! - `patch`: the rectangle shows the picture of the seed, each pixel
//!   that picture's pixel at the same place of the screen.
//! - `pointer`: the rectangle shows the picture of the seed from that
//!   picture's top left corner, as a pointer moved there does.
//! - `dark`: the screen shows nothing.
//! - `step`: the program has done what the lines before say and waits
//!   for a key. The host looks at the screens, does what the step is
//!   for (plugs a monitor in, say) and presses one. The name ends with
//!   a period, so that no step's line starts another's.
//! - `passed` and `FAILED`: how the program ends.

use core::fmt;

/// How every line starts.
pub const START: &str = "display-test: ";

/// How the line of a failure starts.
pub const FAILED: &str = "display-test: FAILED";

/// A rectangle of a screen, in pixels from its top left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Its left column.
    pub x: u32,
    /// Its top row.
    pub y: u32,
    /// Its width.
    pub width: u32,
    /// Its height.
    pub height: u32,
}

/// One line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// The screen is `width` by `height` and shows the picture of
    /// `seed`.
    Shows {
        /// The screen.
        screen: String,
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
        /// The picture.
        seed: u32,
    },
    /// `rect` of the screen shows the picture of `seed`, pixel for
    /// pixel in place.
    Patch {
        /// The screen.
        screen: String,
        /// The rectangle.
        rect: Rect,
        /// The picture.
        seed: u32,
    },
    /// `rect` of the screen shows the picture of `seed` from its top
    /// left corner.
    Pointer {
        /// The screen.
        screen: String,
        /// The rectangle.
        rect: Rect,
        /// The picture.
        seed: u32,
    },
    /// The screen shows nothing.
    Dark {
        /// The screen.
        screen: String,
    },
    /// The program waits for a key at the step of this name.
    Step(String),
    /// Every check passed.
    Passed,
    /// A check failed, and why.
    Failed(String),
}

/// The name of the screen at place `index` of the card named `card`.
pub fn screen(card: &str, index: u32) -> String {
    format!("{card}/{index}")
}

impl fmt::Display for Said {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(START)?;
        let placed = |f: &mut fmt::Formatter<'_>, word: &str, screen: &str, rect: &Rect, seed| {
            let Rect { x, y, width, height } = rect;
            write!(f, "{word} {screen} {width}x{height} at {x},{y} seed {seed}")
        };
        match self {
            Said::Shows { screen, width, height, seed } => {
                write!(f, "shows {screen} {width}x{height} seed {seed}")
            }
            Said::Patch { screen, rect, seed } => placed(f, "patch", screen, rect, seed),
            Said::Pointer { screen, rect, seed } => placed(f, "pointer", screen, rect, seed),
            Said::Dark { screen } => write!(f, "dark {screen}"),
            Said::Step(name) => write!(f, "step {name}."),
            Said::Passed => f.write_str("passed"),
            Said::Failed(why) => write!(f, "FAILED: {why}"),
        }
    }
}

/// The two numbers of `text`, which `between` parts.
fn pair(text: &str, between: char) -> Option<(u32, u32)> {
    let (first, second) = text.split_once(between)?;
    Some((first.parse().ok()?, second.parse().ok()?))
}

impl Said {
    /// Reads a line of a log: what follows [`START`] in it, if that is
    /// a whole line of the program's. Anything else is `None`.
    pub fn read(line: &str) -> Option<Said> {
        let said = line.split_once(START)?.1.trim_end();
        if let Some(why) = said.strip_prefix("FAILED: ") {
            return Some(Said::Failed(why.to_string()));
        }
        let words: Vec<&str> = said.split(' ').collect();
        let screen = |at: usize| words.get(at).map(|name| (*name).to_string());
        let seed = |at: usize| words.get(at)?.parse().ok().filter(|_| words[at - 1] == "seed");
        match *words.first()? {
            "shows" if words.len() == 5 => {
                let (width, height) = pair(words[2], 'x')?;
                Some(Said::Shows { screen: screen(1)?, width, height, seed: seed(4)? })
            }
            word @ ("patch" | "pointer") if words.len() == 7 && words[3] == "at" => {
                let ((width, height), (x, y)) = (pair(words[2], 'x')?, pair(words[4], ',')?);
                let (screen, rect, seed) = (screen(1)?, Rect { x, y, width, height }, seed(6)?);
                Some(if word == "patch" {
                    Said::Patch { screen, rect, seed }
                } else {
                    Said::Pointer { screen, rect, seed }
                })
            }
            "dark" if words.len() == 2 => Some(Said::Dark { screen: screen(1)? }),
            "step" if words.len() == 2 => Some(Said::Step(words[1].strip_suffix('.')?.to_string())),
            "passed" if words.len() == 1 => Some(Said::Passed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_reads_back_as_it_was_said() {
        let rect = Rect { x: 300, y: 200, width: 200, height: 100 };
        let lines = [
            Said::Shows { screen: screen("virtio0", 2), width: 1024, height: 768, seed: 3 },
            Said::Patch { screen: screen("virtio0", 0), rect, seed: 4 },
            Said::Pointer { screen: screen("firmware0", 0), rect, seed: 5 },
            Said::Dark { screen: screen("virtio0", 1) },
            Said::Step("shown".into()),
            Said::Passed,
            Said::Failed("the card has 3 screens, not 4".into()),
        ];
        for line in lines {
            // As the kernel's log carries it: after the log's own words.
            let logged = format!("[   12.345678] {line}\r");
            assert_eq!(Said::read(&logged), Some(line), "{logged}");
        }
        assert_eq!(
            Said::Patch { screen: "virtio0/0".into(), rect, seed: 4 }.to_string(),
            "display-test: patch virtio0/0 200x100 at 300,200 seed 4"
        );
        assert_eq!(Said::Step("shown".into()).to_string(), "display-test: step shown.");
        assert!(Said::Failed("why".into()).to_string().starts_with(FAILED));
    }

    #[test]
    fn what_is_no_whole_line_of_the_programs_is_nothing() {
        for line in [
            "spark: started /system/tests/posix/display-test as process 7",
            "display-test: shows virtio0/0 1024x768 seed",
            "display-test: shows virtio0/0 1024x768 sown 3",
            "display-test: shows virtio0/0 1024 by 768 seed 3",
            "display-test: patch virtio0/0 200x100 to 300,200 seed 4",
            "display-test: step shown",
            "display-test: passed by",
            "display-test: ",
        ] {
            assert_eq!(Said::read(line), None, "{line}");
        }
    }
}
