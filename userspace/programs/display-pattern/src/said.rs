// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lines `display-test` says, and the host reads. `window-test`
//! says the same lines of the desktop's screens (windowd(1)).
//!
//! Each starts with [`START`], or with [`WINDOW_START`] when
//! `window-test` says it. A screen is named by its card and its
//! place on the card, as `virtio0/1`. The lines that say what a screen
//! shows build on each other: the host keeps a picture for each screen
//! and changes it as the lines say, so a line says only what is new.
//!
//! ```text
//! display-test: shows virtio0/0 1024x768 seed 3
//! display-test: fill virtio0/0 1024x768 colour 12161e
//! display-test: patch virtio0/0 200x100 at 300,200 seed 4
//! display-test: pointer firmware0/0 32x32 at 196,144 seed 5
//! window-test: frame virtio0/0 320x240 at 352,279 normal active hot none title A window
//! display-test: dark virtio0/1
//! display-test: step shown.
//! display-test: passed
//! display-test: FAILED: the card has 3 screens, not 4
//! ```
//!
//! - `shows`: the screen is of this size and shows all of the picture
//!   of the seed ([`crate::pattern`]).
//! - `fill`: the screen is of this size and shows this colour all over,
//!   which is written as its red, green and blue in hexadecimal.
//! - `patch`: the rectangle shows the picture of the seed, each pixel
//!   that picture's pixel at the same place of the screen.
//! - `pointer`: the rectangle shows the picture of the seed from that
//!   picture's top left corner, as a pointer moved there does, and a
//!   window that shows there.
//! - `frame`: the rectangle is a window's own pixels, and around it is
//!   the frame windowd draws (windowd(1)): of a window that is `normal`
//!   or `maximized`, `active` (the one the person works in) or
//!   `inactive`, with the button the pointer is on (`minimize`,
//!   `maximize`, `close` or `none`) and the title, which is the rest of
//!   the line. What the rectangle itself shows is another line's to
//!   say.
//! - `dark`: the screen shows nothing.
//! - `step`: the program has done what the lines before say and waits
//!   for a key. The host looks at the screens, does what the step is
//!   for (plugs a monitor in, say) and presses one. The name ends with
//!   a period, so that no step's line starts another's.
//! - `passed` and `FAILED`: how the program ends.

use core::fmt;

/// How every line of `display-test` starts.
pub const START: &str = "display-test: ";

/// How every line of `window-test` starts.
pub const WINDOW_START: &str = "window-test: ";

/// How the line of a failure starts when the program whose lines start
/// with `start` says it.
pub fn failure(start: &str) -> String {
    format!("{start}FAILED")
}

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

/// A button of a window's frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameButton {
    /// The one that minimizes the window.
    Minimize,
    /// The one that maximizes it, or restores it.
    Maximize,
    /// The one that closes it.
    Close,
}

impl FrameButton {
    /// Every button, with the word a line has for it.
    const WORDS: [(FrameButton, &str); 3] = [
        (FrameButton::Minimize, "minimize"),
        (FrameButton::Maximize, "maximize"),
        (FrameButton::Close, "close"),
    ];
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
    /// The screen is `width` by `height` and shows `colour` all over.
    Fill {
        /// The screen.
        screen: String,
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
        /// The colour: red, green and blue.
        colour: [u8; 3],
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
    /// `rect` of the screen is a window's own pixels, with the
    /// window's frame around it.
    Frame {
        /// The screen.
        screen: String,
        /// The window's own pixels.
        rect: Rect,
        /// Whether the window is maximized.
        maximized: bool,
        /// Whether it is the window the person works in.
        active: bool,
        /// The button the pointer is on.
        hot: Option<FrameButton>,
        /// The window's title.
        title: String,
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

/// A line as `display-test` says it.
impl fmt::Display for Said {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{START}{}", self.words())
    }
}

impl Said {
    /// The line as the program whose lines start with `start` says it.
    pub fn said_by(&self, start: &str) -> String {
        format!("{start}{}", self.words())
    }

    /// The line without its start.
    fn words(&self) -> String {
        let placed = |word: &str, screen: &str, rect: &Rect, seed: &u32| {
            let Rect { x, y, width, height } = rect;
            format!("{word} {screen} {width}x{height} at {x},{y} seed {seed}")
        };
        match self {
            Said::Shows { screen, width, height, seed } => {
                format!("shows {screen} {width}x{height} seed {seed}")
            }
            Said::Fill { screen, width, height, colour: [red, green, blue] } => {
                format!("fill {screen} {width}x{height} colour {red:02x}{green:02x}{blue:02x}")
            }
            Said::Patch { screen, rect, seed } => placed("patch", screen, rect, seed),
            Said::Pointer { screen, rect, seed } => placed("pointer", screen, rect, seed),
            Said::Frame { screen, rect, maximized, active, hot, title } => {
                let Rect { x, y, width, height } = rect;
                let shape = if *maximized { "maximized" } else { "normal" };
                let active = if *active { "active" } else { "inactive" };
                let word = |button: &FrameButton| {
                    FrameButton::WORDS
                        .iter()
                        .find(|(other, _)| other == button)
                        .map(|(_, word)| *word)
                };
                let hot = hot.as_ref().and_then(word).unwrap_or("none");
                format!(
                    "frame {screen} {width}x{height} at {x},{y} {shape} {active} hot {hot} title {title}"
                )
            }
            Said::Dark { screen } => format!("dark {screen}"),
            Said::Step(name) => format!("step {name}."),
            Said::Passed => "passed".to_owned(),
            Said::Failed(why) => format!("FAILED: {why}"),
        }
    }
}

/// The two numbers of `text`, which `between` parts.
fn pair(text: &str, between: char) -> Option<(u32, u32)> {
    let (first, second) = text.split_once(between)?;
    Some((first.parse().ok()?, second.parse().ok()?))
}

impl Said {
    /// Reads a line of a log: what follows [`START`] or [`WINDOW_START`]
    /// in it, if that is a whole line of the program's. Anything else is
    /// `None`.
    pub fn read(line: &str) -> Option<Said> {
        let started = line.split_once(START).or_else(|| line.split_once(WINDOW_START));
        let said = started?.1.trim_end();
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
            "fill" if words.len() == 5 && words[3] == "colour" => {
                let (width, height) = pair(words[2], 'x')?;
                let colour =
                    u32::from_str_radix(words[4], 16).ok().filter(|_| words[4].len() == 6)?;
                let [_, red, green, blue] = colour.to_be_bytes();
                Some(Said::Fill { screen: screen(1)?, width, height, colour: [red, green, blue] })
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
            "frame"
                if words.len() >= 10
                    && words[3] == "at"
                    && words[7] == "hot"
                    && words[9] == "title" =>
            {
                let ((width, height), (x, y)) = (pair(words[2], 'x')?, pair(words[4], ',')?);
                let either = |word: &str, yes: &str, no: &str| match word {
                    word if word == yes => Some(true),
                    word if word == no => Some(false),
                    _ => None,
                };
                let hot = match words[8] {
                    "none" => None,
                    word => Some(FrameButton::WORDS.iter().find(|(_, other)| *other == word)?.0),
                };
                // The title is the rest of the line, with its spaces.
                let title = said.split_once(" title ").map_or("", |(_, title)| title);
                Some(Said::Frame {
                    screen: screen(1)?,
                    rect: Rect { x, y, width, height },
                    maximized: either(words[5], "maximized", "normal")?,
                    active: either(words[6], "active", "inactive")?,
                    hot,
                    title: title.to_owned(),
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
            Said::Fill {
                screen: screen("virtio0", 1),
                width: 800,
                height: 600,
                colour: [0x12, 0x16, 0x1e],
            },
            Said::Patch { screen: screen("virtio0", 0), rect, seed: 4 },
            Said::Pointer { screen: screen("firmware0", 0), rect, seed: 5 },
            Said::Dark { screen: screen("virtio0", 1) },
            Said::Frame {
                screen: screen("virtio0", 0),
                rect,
                maximized: false,
                active: true,
                hot: None,
                title: "A window  of two words".into(),
            },
            Said::Frame {
                screen: screen("virtio0", 0),
                rect,
                maximized: true,
                active: false,
                hot: Some(FrameButton::Close),
                title: String::new(),
            },
            Said::Step("shown".into()),
            Said::Passed,
            Said::Failed("the card has 3 screens, not 4".into()),
        ];
        for line in lines {
            // As the kernel's log carries it: after the log's own words.
            let logged = format!("[   12.345678] {line}\r");
            assert_eq!(Said::read(&logged), Some(line.clone()), "{logged}");
            let by_window_test = line.said_by(WINDOW_START);
            assert!(by_window_test.starts_with("window-test: "), "{by_window_test}");
            assert_eq!(Said::read(&by_window_test), Some(line), "{by_window_test}");
        }
        assert_eq!(
            Said::Fill { screen: "firmware0/0".into(), width: 4, height: 3, colour: [1, 2, 255] }
                .to_string(),
            "display-test: fill firmware0/0 4x3 colour 0102ff"
        );
        assert_eq!(
            Said::Patch { screen: "virtio0/0".into(), rect, seed: 4 }.to_string(),
            "display-test: patch virtio0/0 200x100 at 300,200 seed 4"
        );
        assert_eq!(Said::Step("shown".into()).to_string(), "display-test: step shown.");
        let frame = Said::Frame {
            screen: "virtio0/0".into(),
            rect,
            maximized: false,
            active: true,
            hot: Some(FrameButton::Maximize),
            title: "A window".into(),
        };
        assert_eq!(
            frame.said_by(WINDOW_START),
            "window-test: frame virtio0/0 200x100 at 300,200 normal active hot maximize title A window"
        );
        assert!(Said::Failed("why".into()).to_string().starts_with(&failure(START)));
        assert!(
            Said::Failed("why".into()).said_by(WINDOW_START).starts_with(&failure(WINDOW_START))
        );
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
            "window-test: frame virtio0/0 200x100 at 300,200 normal active hot none",
            "window-test: frame virtio0/0 200x100 at 300,200 normal lively hot none title A",
            "window-test: frame virtio0/0 200x100 at 300,200 normal active hot all title A",
            "window-test: frame virtio0/0 200x100 at 300,200 normal active hot none named A",
            "display-test: passed by",
            "display-test: ",
        ] {
            assert_eq!(Said::read(line), None, "{line}");
        }
    }
}
