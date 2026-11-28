// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `window-test screens`: the program's part of the boot test of the
//! desktop on two screens (ADRs 0725 to 0728).
//!
//! The host plugs a second monitor in, pulls it out and plugs it in
//! again, and works the window with the keys alone: the pointer rests
//! in the middle of the first screen, where the desktop puts it. As in
//! [`crate::windows`], the program says what each screen shows after
//! each thing the host does, and the host compares the screens with
//! that, pixel for pixel.
//!
//! | Step | The host has just |
//! |---|---|
//! | `shown` | started the desktop, on one screen |
//! | `plugged` | plugged a monitor into the second |
//! | `snapped` | pressed Super+Left |
//! | `desk` | pressed Super+Ctrl+D: the first screen shows a new desktop |
//! | `first` | pressed Super+Ctrl+Left |
//! | `carried` | pressed Super+Shift+Right |
//! | `full` | nothing; the program asked for its window to be full screen |
//! | `laid` | nothing; the window is as it was, and the settings have the second screen left of the first |
//! | `unplugged` | pulled the second monitor out |
//! | `home` | plugged it in again |
//!
//! The program checks what the host cannot see: where the window
//! server says each screen is on the desktop, that the window is
//! snapped, and that the screen showed the full-screen window from the
//! memory the program drew it in. Last the host presses Alt+F4, and the
//! program ends.

use window_test::{Client, Heard, Outcome, Screen};

use crate::PATIENCE;
use crate::windows::{Desk, Pane, settle_on};

/// The size of the window's own pixels when it is first shown.
const WINDOW: (u32, u32) = (320, 240);

/// How many frames of the full-screen window the program draws, at
/// most, for one to be shown from its own memory. One should do; the
/// rest allow for frames windowd has cause to draw itself, as it may
/// at any time (ADR 0728).
const OWN_WITHIN: u32 = 8;

/// The settings that put the screen `second` at the desktop's corner
/// and `first` to the right of it (windowd.json(5)).
fn laid_left(first: &Screen, second: &Screen) -> String {
    let (left, right, width) = (&second.name, &first.name, second.width);
    format!(
        "{{\"layouts\": [{{\"screens\": {{\"{left}\": [0, 0], \"{right}\": [{width}, 0]}}}}]}}\n"
    )
}

/// The screens the window server has told `client` of, by their names,
/// if it has said of each all it says of a screen.
fn told(client: &Client) -> Option<Vec<Screen>> {
    let mut screens = client.screens();
    screens.sort_by(|one, other| one.name.cmp(&other.name));
    screens.iter().all(|screen| !screen.name.is_empty() && screen.width > 0).then_some(screens)
}

/// Waits until the window server has told the window in front of as
/// many screens as `corners`, by their names, each with its corner at
/// that place of the desktop. They are then the desk's screens.
fn await_screens(desk: &mut Desk, corners: &[(i32, i32)]) -> Outcome {
    let mut last = Vec::new();
    let came = desk.listen_until("the screens", |desk| {
        let Some(screens) = told(&desk.front().client) else { return false };
        last = screens;
        last.iter().map(|screen| (screen.x, screen.y)).eq(corners.iter().copied())
    });
    came.map_err(|why| format!("{why}: the screens are to be at {corners:?} and are {last:?}"))?;
    desk.screens = last;
    Ok(())
}

/// Waits until the window in front is snapped and drawn as large as a
/// frame of `frame` asks, as the host's last doing should have it, on
/// the screen at place `on` with the frame's corner at `corner` of it,
/// and draws it once more so that the frame the next step is compared
/// with has it.
fn fills(desk: &mut Desk, on: usize, corner: (i32, i32), frame: (u32, u32)) -> Outcome {
    let framed = |pane: &Pane| {
        let layout = pane.layout();
        (layout.frame.w as u32, layout.frame.h as u32)
    };
    let came = desk.listen_until("the size of the window's part of the screen", |desk| {
        let front = desk.front();
        front.snapped && !front.full && framed(front) == frame
    });
    let front = desk.front();
    came.map_err(|why| format!("{why}: its frame is {:?}, not {frame:?}", framed(front)))?;
    (front.on, front.corner) = (on, corner);
    front.draw()
}

/// Has the window in front be full screen on the screen it is on,
/// which is `screen`, and waits until that screen has shown a frame of
/// it from the memory the program drew it in (ADR 0728).
fn fill_screen(desk: &mut Desk, screen: &Screen) -> Outcome {
    let size = (screen.width, screen.height);
    desk.front().client.full(true, None)?;
    desk.listen_until("the size of the screen", |desk| {
        let front = desk.front();
        front.full && front.size == size
    })?;
    let front = desk.front();
    let shown = |heard: &Heard| match heard {
        Heard::Shown { from_own, .. } => Some(*from_own),
        Heard::NotShown => Some(false),
        _ => None,
    };
    for _ in 0..OWN_WITHIN {
        front.client.ask_shown();
        front.client.show(front.size, front.seed)?;
        if front.client.wait(PATIENCE, shown)? {
            return Ok(());
        }
    }
    Err(format!("none of {OWN_WITHIN} frames of the full-screen window showed from its own memory"))
}

/// Goes through the steps.
pub fn run() -> Outcome {
    let screens = Client::of_environment()?.screens();
    let [first] = &screens[..] else {
        return Err(format!("the steps start on one screen, and the desktop has {screens:?}"));
    };
    let first = first.clone();
    let mut desk = Desk::on(vec![first.clone()]);
    desk.open(0, WINDOW)?;
    desk.step("shown")?;

    // A screen that comes is beside the last, and the window stays.
    await_screens(&mut desk, &[(0, 0), (first.width as i32, 0)])?;
    let second = desk.screens[1].clone();
    desk.step("plugged")?;

    // Super+Left: the left side of the screen the window is on.
    let side = |screen: &Screen| (screen.width / 2, screen.height);
    fills(&mut desk, 0, (0, 0), side(&first))?;
    desk.step("snapped")?;

    // The host makes a desktop after the key. It is the first
    // screen's, where the pointer is, and bare: the window is on the
    // desktop before it, and has no keys to hear.
    desk.listen("the keyboard's going", |heard| *heard == Heard::Keyboard(false))?;
    desk.front().away = true;
    desk.at_step("desk")?;
    desk.listen("the keyboard's coming back", |heard| *heard == Heard::Keyboard(true))?;
    let front = desk.front();
    front.away = false;
    front.await_active(true)?;
    desk.step("first")?;

    // Super+Shift+Right: the same side of the second screen.
    fills(&mut desk, 1, (0, 0), side(&second))?;
    desk.step("carried")?;

    fill_screen(&mut desk, &second)?;
    desk.step("full")?;

    // As it was before, and the second screen is on the left from now
    // on whenever both are there (windowd.json(5)).
    desk.front().client.full(false, None)?;
    fills(&mut desk, 1, (0, 0), side(&second))?;
    settle_on(&laid_left(&first, &second))?;
    let laid = [(second.width as i32, 0), (0, 0)];
    await_screens(&mut desk, &laid)?;
    desk.step("laid")?;

    // Its screen gone, the window is on the one that is left, which is
    // alone at the desktop's corner.
    await_screens(&mut desk, &[(0, 0)])?;
    desk.dark = vec![second.name.clone()];
    fills(&mut desk, 0, (0, 0), side(&first))?;
    desk.step("unplugged")?;

    // Its screen back, it is where it was, and the screens are laid
    // out as the settings say for the two.
    await_screens(&mut desk, &laid)?;
    desk.dark.clear();
    fills(&mut desk, 1, (0, 0), side(&second))?;
    desk.step("home")?;

    desk.listen("Alt+F4's asking", |heard| *heard == Heard::Close)
}
