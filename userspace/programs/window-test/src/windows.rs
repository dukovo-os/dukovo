// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `window-test windows`: the program's part of the boot test of window
//! management (ADR 0721).
//!
//! The host works the windows as a person does, with a pointer and the
//! keys: it drags a window by its title bar and by its corner, presses
//! the frame's buttons, double clicks the bar and goes through the
//! windows with Alt+Tab. The program cannot see any of that. It says
//! what the screen shows after each thing the host does, reckoned from
//! what the host is to do and from the sizes the window server asks
//! for, and the host compares the screen with that, pixel for pixel.
//!
//! | Step | The host has just |
//! |---|---|
//! | `shown` | started the desktop |
//! | `dragged` | dragged the window by its title bar, by [`DRAGGED_BY`] |
//! | `resized` | dragged its bottom right corner, by [`RESIZED_BY`] |
//! | `pointed` | put the pointer on the close button |
//! | `maximized` | clicked the maximize button |
//! | `restored` | double clicked the title bar |
//! | `minimized` | clicked the minimize button |
//! | `back` | pressed Alt+Tab |
//! | `second` | taken the pointer off the window; the program showed a second window |
//! | `switched` | pressed Alt+Tab |
//! | `alone` | nothing; the program took its second window away and changed the settings |
//!
//! At a step the program waits for the space bar, which whichever of
//! its windows has the keys hears. A minimized window has none, so the
//! steps around it wait for the keyboard to go and to come back. Last
//! the host clicks the close button, and the program ends.
//!
//! Before `alone` the program writes the keys' repeat into the system's
//! windowd.json and sends the desktop SIGUSR1, as windowd.json(5) says
//! to: the window server tells it the new repeat.

use std::process::Command;
use std::time::{Duration, Instant};

use display_pattern::said::{DRAGGED_BY, FrameButton, RESIZED_BY, Rect, Said};
use window_frame::{Bar, Layout, Shape, Theme};
use window_test::{Client, DESKTOP, Heard, Outcome, Screen};

use crate::{KEY_SPACE, PATIENCE, STEP_PATIENCE, ensure, say};

/// The sizes of the two windows' own pixels, their titles and the seeds
/// of their pictures.
const FIRST: (u32, u32) = (320, 240);
const SECOND: (u32, u32) = (240, 160);
const TITLES: [&str; 2] = ["window-test", "window-test two"];
const SEEDS: [u32; 2] = [7, 9];

/// How far down and to the right of the screen's middle the second
/// window of a desktop shows (windowd(1)).
const PLACE_STEP: i32 = 32;

/// How long one of the program's windows is listened to before the
/// other.
const TURN: Duration = Duration::from_millis(10);

/// The system's settings of the desktop, what the program writes there,
/// and the keys' repeat the window server then tells: after so many
/// milliseconds, so many a second.
const SETTINGS: &str = "/system/settings/windowd.json";
const SETTINGS_TEXT: &str = "{\"keyboard\": {\"repeat_delay_ms\": 400, \"repeat_rate\": 12}}\n";
const REPEAT: (i32, i32) = (400, 12);

/// The command that has the desktop read its settings again.
const RELOAD: [&str; 3] = ["/system/commands/pkill", "-USR1", "desktop"];

/// One of the program's windows, as the program reckons its screen
/// shows it.
pub(crate) struct Pane {
    pub(crate) client: Client,
    title: &'static str,
    pub(crate) seed: u32,
    /// The size of its own pixels, as it last drew them.
    pub(crate) size: (u32, u32),
    /// That size when the window last filled nothing of its screen.
    normal: (u32, u32),
    /// Which of the desk's screens it is on.
    pub(crate) on: usize,
    /// Where its frame's corner is on that screen when it is neither
    /// maximized nor full screen.
    pub(crate) corner: (i32, i32),
    maximized: bool,
    /// Whether the window server last said it is snapped.
    pub(crate) snapped: bool,
    /// Whether the window server last asked for it to be full screen.
    pub(crate) full: bool,
    /// Whether it is off the screen: minimized, or on a desktop that
    /// does not show.
    pub(crate) away: bool,
    /// Whether the window server last said it is the window the person
    /// works in.
    active: bool,
    /// The button of its frame the host put the pointer on.
    hot: Option<FrameButton>,
}

impl Pane {
    /// The frame's layout around the window as it is drawn now.
    pub(crate) fn layout(&self) -> Layout {
        let shape = match (self.full, self.maximized) {
            (true, _) => Shape::Full,
            (false, true) => Shape::Maximized,
            (false, false) => Shape::Normal,
        };
        let size = (self.size.0 as i32, self.size.1 as i32);
        Layout::of(&Theme::basic(), size, shape, Bar::Full, None)
    }

    /// Where the window's own pixels are on its screen, whose corner
    /// the frame of a window that fills it is at.
    fn rect(&self) -> Outcome<Rect> {
        let layout = self.layout();
        let corner = if self.maximized || self.full { (0, 0) } else { self.corner };
        let (x, y) = (corner.0 + layout.surface.x, corner.1 + layout.surface.y);
        ensure(x >= 0 && y >= 0, || format!("{} is off the screen at {x},{y}", self.title))?;
        Ok(Rect { x: x as u32, y: y as u32, width: self.size.0, height: self.size.1 })
    }

    /// Draws the window at its size, and waits for the frame that shows
    /// it.
    pub(crate) fn draw(&mut self) -> Outcome {
        self.client.show(self.size, self.seed)?;
        self.client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(()))
    }

    /// Waits until the window server has said that the window is, or is
    /// no longer, the one the person works in.
    pub(crate) fn await_active(&mut self, active: bool) -> Outcome {
        if self.active != active {
            let said = Heard::Active(active);
            self.client.wait(PATIENCE, |heard| (*heard == said).then_some(()))?;
            self.active = active;
        }
        Ok(())
    }

    /// Takes in one thing the window server said: a size it asks for is
    /// drawn at once, as a program does while its window is resized.
    fn take(&mut self, heard: &Heard) -> Outcome {
        match *heard {
            Heard::Size { width, height, maximized } => {
                // A side of 0 is the program's to choose: the size the
                // window had before it filled anything.
                let side = |asked: i32, own: u32| if asked > 0 { asked as u32 } else { own };
                self.size = (side(width, self.normal.0), side(height, self.normal.1));
                self.maximized = maximized;
                if !(maximized || self.snapped || self.full) {
                    self.normal = self.size;
                }
                self.client.show(self.size, self.seed)
            }
            // The size to draw the window at comes next.
            Heard::Snapped(snapped) => {
                self.snapped = snapped;
                Ok(())
            }
            Heard::Full(full) => {
                self.full = full;
                Ok(())
            }
            Heard::Active(active) => {
                self.active = active;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// The desktop's screens and the program's windows on them, from the
/// one at the back to the one in front.
pub(crate) struct Desk {
    /// The screens that show the desktop.
    pub(crate) screens: Vec<Screen>,
    /// The names of the screens that show nothing: their monitors are
    /// gone.
    pub(crate) dark: Vec<String>,
    panes: Vec<Pane>,
}

impl Desk {
    /// A desktop on `screens`, with no window yet.
    pub(crate) fn on(screens: Vec<Screen>) -> Desk {
        Desk { screens, dark: Vec::new(), panes: Vec::new() }
    }

    /// Shows a window of `size` as the `nth` the desktop has shown, on
    /// the first screen, and waits until it is the one the person works
    /// in and has the keys.
    pub(crate) fn open(&mut self, nth: usize, size: (u32, u32)) -> Outcome {
        let mut client = Client::of_environment()?;
        client.open(window_test::Open::default())?;
        client.title(TITLES[nth])?;
        client.show(size, SEEDS[nth])?;
        client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(()))?;
        client.wait(PATIENCE, |heard| (*heard == Heard::Active(true)).then_some(()))?;
        client.wait(PATIENCE, |heard| (*heard == Heard::Keyboard(true)).then_some(()))?;
        let mut pane = Pane {
            client,
            title: TITLES[nth],
            seed: SEEDS[nth],
            size,
            normal: size,
            on: 0,
            corner: (0, 0),
            maximized: false,
            snapped: false,
            full: false,
            away: false,
            active: true,
            hot: None,
        };
        // A new window's frame goes in the middle of the screen, a step
        // down and to the right for each window before it.
        let layout = pane.layout();
        let step = nth as i32 * PLACE_STEP;
        let middle = |room: u32, taken: i32| ((room as i32 - taken) / 2 + step).max(0);
        let screen = &self.screens[0];
        pane.corner = (middle(screen.width, layout.frame.w), middle(screen.height, layout.frame.h));
        self.panes.push(pane);
        Ok(())
    }

    /// The window in front.
    pub(crate) fn front(&mut self) -> &mut Pane {
        let last = self.panes.len() - 1;
        &mut self.panes[last]
    }

    /// Says what each screen shows: a full-screen window has no frame.
    fn say_screens(&self) -> Outcome {
        for (index, Screen { name, width, height, .. }) in self.screens.iter().enumerate() {
            let (width, height) = (*width, *height);
            say(&Said::Fill { screen: name.clone(), width, height, colour: DESKTOP });
            for pane in self.panes.iter().filter(|pane| pane.on == index && !pane.away) {
                let rect = pane.rect()?;
                if !pane.full {
                    say(&Said::Frame {
                        screen: name.clone(),
                        rect,
                        maximized: pane.maximized,
                        active: pane.active,
                        hot: pane.hot,
                        title: pane.title.to_owned(),
                    });
                }
                say(&Said::Pointer { screen: name.clone(), rect, seed: pane.seed });
            }
        }
        for name in &self.dark {
            say(&Said::Dark { screen: name.clone() });
        }
        Ok(())
    }

    /// Listens to every window, each taking in what it hears, until
    /// `wanted` takes one thing a window heard.
    pub(crate) fn listen(&mut self, what: &str, mut wanted: impl FnMut(&Heard) -> bool) -> Outcome {
        let until = Instant::now() + STEP_PATIENCE;
        loop {
            for pane in &mut self.panes {
                while let Some(heard) = pane.client.heard_within(TURN)? {
                    pane.take(&heard)?;
                    if wanted(&heard) {
                        return Ok(());
                    }
                }
            }
            ensure(Instant::now() < until, || format!("{what} did not come in {STEP_PATIENCE:?}"))?;
        }
    }

    /// Listens to every window for a moment, each taking in what it
    /// hears, and waits so, for up to as long as a step may take, until
    /// `holds` says of the desk that it does. `what` names what is
    /// waited for.
    pub(crate) fn listen_until(
        &mut self,
        what: &str,
        mut holds: impl FnMut(&mut Desk) -> bool,
    ) -> Outcome {
        let until = Instant::now() + STEP_PATIENCE;
        while !holds(self) {
            ensure(Instant::now() < until, || format!("{what} did not come in {STEP_PATIENCE:?}"))?;
            for pane in &mut self.panes {
                while let Some(heard) = pane.client.heard_within(TURN)? {
                    pane.take(&heard)?;
                }
            }
        }
        Ok(())
    }

    /// Says what the screens show and that the program is at the step
    /// `name`.
    pub(crate) fn at_step(&self, name: &str) -> Outcome {
        self.say_screens()?;
        say(&Said::Step(name.to_owned()));
        Ok(())
    }

    /// As [`Desk::at_step`], and waits for the space bar to go down and
    /// up in whichever window has the keys.
    pub(crate) fn step(&mut self, name: &str) -> Outcome {
        self.at_step(name)?;
        for pressed in [true, false] {
            let key = Heard::Key { code: KEY_SPACE, pressed };
            self.listen("the space bar", |heard| *heard == key)?;
        }
        Ok(())
    }

    /// Checks that the window in front is `size` and maximized or not,
    /// as the host's last doing should have left it, and draws it once
    /// more so that the frame the next step is compared with has it.
    fn settled(&mut self, size: (u32, u32), maximized: bool) -> Outcome {
        let front = self.front();
        ensure(front.maximized == maximized, || {
            let was = if front.maximized { "maximized" } else { "not maximized" };
            format!("{} is {was}", front.title)
        })?;
        ensure(maximized || front.size == size, || {
            format!("{} is {:?}, not {size:?}", front.title, front.size)
        })?;
        front.hot = None;
        front.draw()
    }
}

/// Writes `text` as the system's windowd.json and has the desktop read
/// it.
pub(crate) fn settle_on(text: &str) -> Outcome {
    std::fs::write(SETTINGS, text).map_err(|err| format!("{SETTINGS}: {err}"))?;
    let [command, arguments @ ..] = RELOAD;
    let ended = Command::new(command).args(arguments).status();
    let ended = ended.map_err(|err| format!("{command}: {err}"))?;
    ensure(ended.success(), || format!("{}: {ended}", RELOAD.join(" ")))
}

/// Has the desktop read a changed windowd.json, and waits until the
/// window server tells `client` the keys' new repeat.
fn reload_settings(client: &mut Client) -> Outcome {
    let before = client.repeat();
    ensure(before != Some(REPEAT), || format!("the keys repeat at {REPEAT:?} from the first"))?;
    settle_on(SETTINGS_TEXT)?;
    let until = Instant::now() + PATIENCE;
    while client.repeat() != Some(REPEAT) {
        ensure(Instant::now() < until, || {
            format!("the keys' repeat stayed {:?}, not {REPEAT:?}, after SIGUSR1", client.repeat())
        })?;
        client.listen(TURN)?;
    }
    Ok(())
}

/// Goes through the steps.
pub fn run() -> Outcome {
    let screens = Client::of_environment()?.screens();
    let [screen] = &screens[..] else {
        return Err(format!("the steps are for one screen, and the desktop has {screens:?}"));
    };
    let mut desk = Desk::on(vec![screen.clone()]);
    desk.open(0, FIRST)?;
    desk.step("shown")?;

    // The host dragged the window by its bar: the program is told
    // nothing of where a window is.
    let moved = desk.front().corner;
    desk.front().corner = (moved.0 + DRAGGED_BY.0, moved.1 + DRAGGED_BY.1);
    desk.step("dragged")?;

    // It dragged the corner: the window server asked for each size on
    // the way.
    let resized =
        (FIRST.0.saturating_add_signed(RESIZED_BY.0), FIRST.1.saturating_add_signed(RESIZED_BY.1));
    desk.settled(resized, false)?;
    desk.step("resized")?;

    desk.front().hot = Some(FrameButton::Close);
    desk.step("pointed")?;

    desk.settled(resized, true)?;
    desk.step("maximized")?;

    desk.settled(resized, false)?;
    desk.step("restored")?;

    // The host clicks the minimize button after the key: a window that
    // is put away has no keys to hear, and loses them.
    desk.listen("the keyboard's going", |heard| *heard == Heard::Keyboard(false))?;
    desk.front().away = true;
    desk.at_step("minimized")?;
    desk.listen("the keyboard's coming back", |heard| *heard == Heard::Keyboard(true))?;
    let front = desk.front();
    front.away = false;
    // The pointer is where it clicked, and the button is under it again.
    front.hot = Some(FrameButton::Minimize);
    front.await_active(true)?;
    desk.step("back")?;

    // The second window shows in front, and the first is no longer the
    // one worked in.
    desk.front().hot = None;
    desk.open(1, SECOND)?;
    desk.panes[0].await_active(false)?;
    desk.step("second")?;

    // Alt+Tab: the first is in front again, and heard the space bar.
    desk.panes[1].await_active(false)?;
    desk.panes[0].await_active(true)?;
    desk.panes.swap(0, 1);
    desk.step("switched")?;

    // The second window's program goes, and the desktop takes new
    // settings.
    desk.panes.remove(0);
    reload_settings(&mut desk.front().client)?;
    desk.step("alone")?;

    desk.listen("the close button's asking", |heard| *heard == Heard::Close)
}
