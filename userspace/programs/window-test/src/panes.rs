// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `window-test panes`: three windows that stay, for a person who
//! tries the desktop by hand.
//!
//! The steps of the other modes are a test's: each waits for one thing
//! and says what the screen shows. These windows wait for nothing. Each
//! is drawn at whatever size the window server asks for, as a program's
//! is while its window is resized, snapped or shown full screen, and
//! each has a name the window server knows it again by
//! (`dukovo_session_v1`). The window with the keys takes four of them:
//!
//! | Key | The window |
//! |---|---|
//! | F | asks to show full screen, or no longer |
//! | L | holds the pointer where it is, or lets it go |
//! | N | is taken away and asked for anew, as by a program that starts again |
//! | Q | closes, as by the close button of its frame |
//!
//! The program says `shown` once its windows show, and passes when the
//! last of them has closed.

use std::time::Duration;

use display_pattern::said::Said;
use window_test::{Client, Heard, Open, Outcome};

use crate::say;

/// The Linux codes of the keys a window takes.
const KEY_Q: u32 = 16;
const KEY_F: u32 = 33;
const KEY_L: u32 = 38;
const KEY_N: u32 = 49;

/// How long one window is listened to before the next.
const TURN: Duration = Duration::from_millis(10);

/// How long a window that is asked for anew is away, for a person to
/// see that it was.
const AWAY: Duration = Duration::from_millis(600);

/// What one of the windows is.
struct About {
    /// The name the window server knows it again by, and its title.
    name: &'static str,
    title: &'static str,
    /// The size of its own pixels where the window server leaves that
    /// to the program, and the seed of its picture.
    size: (u32, u32),
    seed: u32,
}

/// The windows, from the first shown to the last.
const WINDOWS: [About; 3] = [
    About { name: "one", title: "One", size: (360, 260), seed: 7 },
    About { name: "two", title: "Two", size: (320, 240), seed: 9 },
    About { name: "three", title: "Three", size: (280, 200), seed: 12 },
];

/// One window and its connection to the window server.
struct Pane {
    client: Client,
    about: &'static About,
    /// The size of its own pixels when the window was last neither
    /// maximized, snapped nor full screen.
    normal: (u32, u32),
    /// Whether the window server last had it drawn full screen, and
    /// whether snapped.
    full: bool,
    snapped: bool,
    /// Whether it asked for the pointer to stay where it is.
    holds: bool,
}

impl Pane {
    /// Connects to the window server and shows the window.
    fn open(about: &'static About) -> Outcome<Pane> {
        let client = Client::of_environment()?;
        let mut pane =
            Pane { client, about, normal: about.size, full: false, snapped: false, holds: false };
        pane.ask()?;
        Ok(pane)
    }

    /// Asks for the window by its name, and draws it at the size the
    /// window server answers with: the one of a window it knows again,
    /// or the program's own.
    fn ask(&mut self) -> Outcome {
        self.client.open(Open { name: Some(self.about.name), ..Open::default() })?;
        self.client.title(self.about.title)?;
        // Everything the answer said is taken in here, and its size
        // is not drawn once more when the window is next listened to.
        for heard in self.client.take_heard() {
            self.note(&heard);
        }
        let (width, height, maximized) = self.client.asked();
        self.draw(width, height, maximized)
    }

    /// Notes what the window server said of how the window is shown.
    fn note(&mut self, heard: &Heard) {
        match *heard {
            Heard::Full(full) => self.full = full,
            Heard::Snapped(snapped) => self.snapped = snapped,
            _ => {}
        }
    }

    /// Draws the window `width` by `height`, where a side of 0 is the
    /// program's to choose: the side the window had when it was last
    /// neither maximized, snapped nor full screen.
    fn draw(&mut self, width: i32, height: i32, maximized: bool) -> Outcome {
        let side = |asked: i32, own: u32| if asked > 0 { asked as u32 } else { own };
        let size = (side(width, self.normal.0), side(height, self.normal.1));
        if !maximized && !self.snapped && !self.full {
            self.normal = size;
        }
        self.client.show(size, self.about.seed)
    }

    /// Does what the key of `code` has the window do. Returns whether
    /// the window stays.
    fn press(&mut self, code: u32) -> Outcome<bool> {
        match code {
            KEY_F => self.client.full(!self.full, None)?,
            KEY_L if self.holds => {
                self.client.let_go(None)?;
                self.holds = false;
            }
            KEY_L => {
                self.client.lock(true)?;
                self.holds = true;
            }
            KEY_N => {
                self.client.close_window()?;
                (self.full, self.snapped, self.holds) = (false, false, false);
                std::thread::sleep(AWAY);
                self.ask()?;
            }
            KEY_Q => return Ok(false),
            _ => {}
        }
        Ok(true)
    }

    /// Takes in everything the window server has said to the window.
    /// Returns whether the window stays.
    fn serve(&mut self) -> Outcome<bool> {
        while let Some(heard) = self.client.heard_within(TURN)? {
            match heard {
                Heard::Size { width, height, maximized } => self.draw(width, height, maximized)?,
                Heard::Key { code, pressed: true } => {
                    if !self.press(code)? {
                        return Ok(false);
                    }
                }
                Heard::Close => return Ok(false),
                other => self.note(&other),
            }
        }
        Ok(true)
    }
}

/// Shows the windows and serves them until the last has closed.
pub fn run() -> Outcome {
    let mut panes = Vec::new();
    for about in &WINDOWS {
        panes.push(Pane::open(about)?);
    }
    say(&Said::Step("shown".to_owned()));
    while !panes.is_empty() {
        let mut stay = Vec::with_capacity(panes.len());
        for mut pane in panes {
            if pane.serve()? {
                stay.push(pane);
            }
        }
        panes = stay;
    }
    Ok(())
}
