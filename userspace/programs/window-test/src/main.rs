// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! window-test: the program of the desktop's boot tests and lab test.
//!
//! `desktop` starts it (desktop(1)). It shows one window and says in the
//! log, in `display-test`'s lines ([`display_pattern::said`]), what
//! every screen then shows: the desktop's colour, the window's frame
//! and picture in the middle of the first screen, and the pointer's
//! picture where
//! the machine's card draws the pointer into what a test can see of the
//! screen (`pointer-drawn`). At each step it waits for the space bar,
//! which reaches it through the kernel, inputd and windowd as any key
//! does. Its last steps are after it has had Spark restart consoled,
//! which lent the desktop the screens: the desktop goes on. `alone` goes
//! through what needs nobody at the keyboard and leaves consoled be, for
//! a machine in the lab: it also asks for its window to be maximized,
//! which with its frame then fills the screen, and has it back.
//! `windows` is the boot test of window management ([`windows`]): the
//! host works two windows with the pointer and the keys, and the
//! program says what the screen shows after each. `screens` is the boot
//! test of the desktop on two screens ([`screens`]). `cost` does the
//! work `ob bench --desktop` measures ([`cost`]). `panes` shows three
//! windows that stay, for a person who tries the desktop by hand
//! ([`panes`]).
//!
//! `in-desktop` starts the desktop itself, with the program in it, once
//! consoled is there to lend the screens. A boot test's command starts
//! with the services, where a person's starts at a console that
//! consoled already shows.

#![forbid(unsafe_code)]

mod cost;
mod panes;
mod screens;
mod tally;
mod windows;

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use display_pattern::said::{Rect, Said, WINDOW_START};
use window_frame::{Bar, Layout, Shape, Theme};
use window_test::{Client, DESKTOP, Heard, Outcome, Screen};

/// The width and height of the window's own pixels.
const WINDOW: (u32, u32) = (320, 240);

/// The window's title.
const TITLE: &str = "window-test";

/// The side of the pointer's picture.
const POINTER: u32 = 16;

/// The Linux code of the space bar.
const KEY_SPACE: u32 = 57;

/// The Linux code of the left button.
const BTN_LEFT: u32 = 0x110;

/// How long the window server may take to answer.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long a step waits for its key: the host looks at every screen
/// first, and waits for what it looks for.
const STEP_PATIENCE: Duration = Duration::from_secs(180);

/// Says `line` in the log.
fn say(line: &Said) {
    println!("{}", line.said_by(WINDOW_START));
}

/// Fails with `why` unless `holds`.
fn ensure(holds: bool, why: impl FnOnce() -> String) -> Outcome {
    if holds { Ok(()) } else { Err(why()) }
}

/// The window on the desktop, as the program works it out from what
/// the window server told it.
struct Shown {
    client: Client,
    screens: Vec<Screen>,
    /// Which of the screens the window is on.
    on: usize,
    /// The top left corner of the window's own pixels on that screen.
    at: (u32, u32),
    /// The seed of the window's picture.
    seed: u32,
    /// Where in the window the pointer came onto it.
    entered: (f64, f64),
    /// Where the pointer is in the window, if its picture is said.
    pointer: Option<(f64, f64)>,
}

impl Shown {
    /// Connects, checks what the window server offers, and shows the
    /// window: the first window is put with its frame in the middle of
    /// the screen the pointer is on, which is the first, and the
    /// pointer starts in that screen's middle (windowd(1)).
    fn open(pointer_drawn: bool) -> Outcome<Shown> {
        let mut client = Client::of_environment()?;
        let screens = client.screens();
        let on = screens.iter().position(|screen| (screen.x, screen.y) == (0, 0));
        let on = on.ok_or_else(|| format!("no screen is at the desktop's corner: {screens:?}"))?;
        let (width, height) = (screens[on].width, screens[on].height);
        let own = (WINDOW.0 as i32, WINDOW.1 as i32);
        let layout = Layout::of(&Theme::basic(), own, Shape::Normal, Bar::Full, None);
        let frame = (layout.frame.w as u32, layout.frame.h as u32);
        ensure(width >= frame.0 && height >= frame.1, || {
            format!("the first screen is {width}x{height}, too small for the window")
        })?;
        let at = (
            (width - frame.0) / 2 + layout.surface.x as u32,
            (height - frame.1) / 2 + layout.surface.y as u32,
        );
        ensure(client.has() == (true, true), || {
            format!("the seat's keyboard and pointer: {:?}", client.has())
        })?;
        ensure(client.layout_bytes().is_some_and(|bytes| bytes > 0), || {
            "the keyboard came with no layout".to_owned()
        })?;
        ensure(client.repeat().is_some_and(|(delay, rate)| delay > 0 && rate > 0), || {
            format!("the keyboard's repeat: {:?}", client.repeat())
        })?;
        let seed = 7;
        client.show(WINDOW, seed)?;
        client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(()))?;
        client.wait(PATIENCE, |heard| (*heard == Heard::Active(true)).then_some(()))?;
        client.wait(PATIENCE, |heard| (*heard == Heard::Keyboard(true)).then_some(()))?;
        let entered = client.wait(PATIENCE, |heard| match heard {
            Heard::Enter(x, y) => Some((*x, *y)),
            _ => None,
        })?;
        let middle = (f64::from(width / 2 - at.0), f64::from(height / 2 - at.1));
        ensure(entered == middle, || {
            format!("the pointer came onto the window at {entered:?}, not at {middle:?}")
        })?;
        if pointer_drawn {
            client.point_with(POINTER, 9)?;
        }
        let pointer = pointer_drawn.then_some(entered);
        Ok(Shown { client, screens, on, at, seed, entered, pointer })
    }

    /// Draws the window anew with the picture of `seed`, and waits for
    /// the frame that shows it.
    fn draw(&mut self, seed: u32) -> Outcome {
        self.client.show(WINDOW, seed)?;
        self.client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(()))?;
        self.seed = seed;
        Ok(())
    }

    /// Says what every screen shows.
    fn say_screens(&self) {
        for (place, screen) in self.screens.iter().enumerate() {
            let Screen { name, width, height, .. } = screen.clone();
            say(&Said::Fill { screen: name.clone(), width, height, colour: DESKTOP });
            if place != self.on {
                continue;
            }
            let (x, y) = self.at;
            let window = Rect { x, y, width: WINDOW.0, height: WINDOW.1 };
            say(&Said::Frame {
                screen: name.clone(),
                rect: window,
                maximized: false,
                active: true,
                hot: None,
                title: TITLE.to_owned(),
            });
            say(&Said::Pointer { screen: name.clone(), rect: window, seed: self.seed });
            if let Some((across, down)) = self.pointer {
                // The pointer's picture is put at the whole pixel its
                // place is in.
                let rect = Rect {
                    x: x + across as u32,
                    y: y + down as u32,
                    width: POINTER,
                    height: POINTER,
                };
                say(&Said::Pointer { screen: name, rect, seed: 9 });
            }
        }
    }

    /// Says what the screens show and that the program is at the step
    /// `name`, and waits for the space bar to go down and up.
    fn step(&mut self, name: &str) -> Outcome {
        self.say_screens();
        say(&Said::Step(name.to_owned()));
        for pressed in [true, false] {
            let key = Heard::Key { code: KEY_SPACE, pressed };
            self.client.wait(STEP_PATIENCE, |heard| (*heard == key).then_some(()))?;
        }
        Ok(())
    }

    /// Takes in what the mouse did during the step before: it moved
    /// right and down, and its left button went down and up, in that
    /// order.
    fn mouse_moved(&mut self) -> Outcome {
        let heard = self.client.take_heard();
        let mut places = heard.iter().filter_map(|heard| match heard {
            Heard::Motion(x, y) => Some((*x, *y)),
            _ => None,
        });
        let from = self.entered;
        let to =
            places.next_back().ok_or_else(|| format!("the pointer did not move: {heard:?}"))?;
        let within = to.0 < f64::from(WINDOW.0 - POINTER) && to.1 < f64::from(WINDOW.1 - POINTER);
        ensure(to.0 > from.0 && to.1 > from.1 && within, || {
            format!("the pointer moved from {from:?} to {to:?}, not right and down in the window")
        })?;
        let buttons: Vec<&Heard> =
            heard.iter().filter(|heard| matches!(heard, Heard::Button { .. })).collect();
        let click = [
            &Heard::Button { code: BTN_LEFT, pressed: true },
            &Heard::Button { code: BTN_LEFT, pressed: false },
        ];
        ensure(buttons == click, || format!("the buttons said {buttons:?}, not one left click"))?;
        let last_motion = heard.iter().rposition(|heard| matches!(heard, Heard::Motion(..)));
        let first_button = heard.iter().position(|heard| matches!(heard, Heard::Button { .. }));
        ensure(last_motion < first_button, || {
            format!("the click came before the move: {heard:?}")
        })?;
        if self.pointer.is_some() {
            self.pointer = Some(to);
        }
        Ok(())
    }

    /// A second program shows a window and cuts its pixels' file short:
    /// the window server lets that program go and draws on, and the
    /// window that stays has the keyboard.
    fn outlive_cut_pixels(&mut self) -> Outcome {
        self.client.take_heard();
        Client::of_environment()?.cut_short(WINDOW)?;
        self.draw(7)?;
        let heard = self.client.take_heard();
        let keyboard = heard.iter().rev().find_map(|heard| match heard {
            Heard::Keyboard(has) => Some(*has),
            _ => None,
        });
        ensure(keyboard != Some(false), || {
            format!("the window that stays did not get the keyboard back: {heard:?}")
        })
    }
}

impl Shown {
    /// Asks for the window to be maximized, as a program may, draws it
    /// at the size the window server then asks for, which with its
    /// frame is the screen's, and has it back as it was.
    fn maximize_and_back(&mut self) -> Outcome {
        let size = |maximized: bool| {
            move |heard: &Heard| match *heard {
                Heard::Size { width, height, maximized: asked } if asked == maximized => {
                    Some((width, height))
                }
                _ => None,
            }
        };
        self.client.take_heard();
        self.client.maximize(true)?;
        let (width, height) = self.client.wait(PATIENCE, size(true))?;
        let layout =
            Layout::of(&Theme::basic(), (width, height), Shape::Maximized, Bar::Full, None);
        let screen = &self.screens[self.on];
        let fills = (layout.frame.w, layout.frame.h) == (screen.width as i32, screen.height as i32);
        ensure(width > 0 && height > 0 && fills, || {
            format!(
                "maximized, the window is asked to be {width}x{height}, which with its frame \
                 is not the screen's {}x{}",
                screen.width, screen.height
            )
        })?;
        self.client.show((width as u32, height as u32), self.seed)?;
        self.client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(()))?;
        self.client.maximize(false)?;
        let (width, height) = self.client.wait(PATIENCE, size(false))?;
        // A side of 0 is the program's to choose.
        let own = (WINDOW.0 as i32, WINDOW.1 as i32);
        ensure([0, own.0].contains(&width) && [0, own.1].contains(&height), || {
            format!("restored, the window is asked to be {width}x{height}, not {own:?}")
        })?;
        self.draw(self.seed)
    }
}

/// The command that has Spark restart consoled (service(8)).
const RESTART_CONSOLED: [&str; 3] = ["/system/commands/service", "consoled", "restart"];

/// Has Spark restart consoled. The command returns once Spark has taken
/// the request, and the host waits for the new consoled in the log.
fn restart_consoled() -> Outcome {
    let [service, arguments @ ..] = RESTART_CONSOLED;
    let ended = Command::new(service).args(arguments).status();
    let ended = ended.map_err(|err| format!("{service}: {err}"))?;
    ensure(ended.success(), || format!("{}: {ended}", RESTART_CONSOLED.join(" ")))
}

/// The steps a boot test goes through with the program.
fn steps(pointer_drawn: bool) -> Outcome {
    let mut shown = Shown::open(pointer_drawn)?;
    shown.step("shown")?;
    shown.draw(8)?;
    shown.step("redrawn")?;
    // The host moves the mouse and clicks before it presses the key.
    shown.client.take_heard();
    shown.step("mouse")?;
    shown.mouse_moved()?;
    shown.step("moved")?;
    shown.outlive_cut_pixels()?;
    shown.step("outlived")?;
    // The host switches consoles and comes back at the step before, while
    // the consoled that lent the screens is there to switch them.
    restart_consoled()?;
    shown.step("restarted")?;
    shown.draw(8)?;
    shown.step("stayed")
}

/// What needs nobody at the keyboard.
fn alone() -> Outcome {
    let mut shown = Shown::open(false)?;
    shown.draw(8)?;
    shown.outlive_cut_pixels()?;
    shown.maximize_and_back()?;
    shown.say_screens();
    Ok(())
}

/// The command that shows the desktop (desktop(1)).
const DESKTOP_COMMAND: &str = "/system/commands/desktop";

/// How long consoled may take to start.
const LENDER_PATIENCE: Duration = Duration::from_secs(60);

/// Runs the desktop with this program in it, which goes through `inside`,
/// once consoled listens for a program that asks for the screens: it
/// does from when it has them to lend (consoled(8)). The program in the
/// desktop says whether it passed, and the desktop ends as it did.
fn in_desktop(inside: &[&str]) -> Outcome {
    let until = Instant::now() + LENDER_PATIENCE;
    while !Path::new(display::loan::SOCKET).exists() {
        ensure(Instant::now() < until, || {
            format!("consoled did not listen at {} in {LENDER_PATIENCE:?}", display::loan::SOCKET)
        })?;
        std::thread::sleep(Duration::from_millis(100));
    }
    let itself = std::env::current_exe().map_err(|err| format!("the program's own path: {err}"))?;
    let ended = Command::new(DESKTOP_COMMAND).arg(itself).args(inside).status();
    let ended = ended.map_err(|err| format!("{DESKTOP_COMMAND}: {err}"))?;
    ensure(ended.success(), || format!("{DESKTOP_COMMAND}: {ended}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let outcome = match words[..] {
        ["steps"] => steps(false),
        ["steps", "pointer-drawn"] => steps(true),
        ["alone"] => alone(),
        ["cost"] => cost::run(),
        ["windows"] => windows::run(),
        ["screens"] => screens::run(),
        ["panes"] => panes::run(),
        // The program in the desktop has said whether it passed.
        ["in-desktop", ref inside @ ..] => match in_desktop(inside) {
            Ok(()) => return ExitCode::SUCCESS,
            failed => failed,
        },
        _ => {
            Err("usage: window-test [in-desktop] steps [pointer-drawn] | windows | screens | panes | alone | cost"
                .into())
        }
    };
    match outcome {
        Ok(()) => {
            say(&Said::Passed);
            ExitCode::SUCCESS
        }
        Err(why) => {
            say(&Said::Failed(why));
            ExitCode::FAILURE
        }
    }
}
