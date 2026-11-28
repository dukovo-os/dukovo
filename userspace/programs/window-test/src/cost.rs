// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `window-test cost`: what a desktop's everyday work costs, for `ob
//! bench` (ADR 0724). The same program runs in Dukovo's desktop and in
//! a Linux compositor, each in a virtual machine of its own, and `ob
//! bench` sends the keys and the mouse's motions through QEMU.
//!
//! | Row | What is measured |
//! |---|---|
//! | `desktop_key` | from the time a key was stamped with by the kernel's driver to this program having read it from the window server: the middle one of the keys |
//! | `desktop_key_p99` | the same, for the key that 99 in 100 were faster than |
//! | `desktop_idle` | what the machine's processors spend in a second with the window showing and nothing moving |
//! | `desktop_frame` | what they spend on one frame of the window drawn anew, this program's share with it |
//! | `desktop_move` | the same for one motion of the mouse that drags the window |
//! | `desktop_resize` | the same for one size of the window while the mouse resizes it, drawn anew at that size |
//!
//! The key rows are timed here and printed as `BENCH` lines. The time
//! a key was stamped with is the one the window server tells
//! (`zwp_input_timestamps_v1`, ADR 0723) where it tells any; where it
//! does not, on Linux, the program reads it from the keyboard's event
//! device, which stamps the same event when the kernel's driver hands
//! it on.
//!
//! The other rows are measured from outside, by `ob bench`, as the
//! time the host's processors ran the virtual machine: no count kept
//! inside shows all of what a frame costs, and both systems are
//! measured by the same clock. The program does the work and says in
//! the log when:
//!
//! 1. `cost: ready for a key`, where `ob bench` first watches the
//!    desktop with nothing moving, and after the key [`FRAMES`] frames,
//!    each drawn when the one before showed; then `cost: drew the
//!    frames`.
//! 2. `cost: ready for <count> keys`, and the two `BENCH` lines after
//!    that many keys went down and up.
//! 3. `cost: ready for the pointer`, until the pointer is well inside
//!    the window; then `cost: ready for a press`. With the button down
//!    the program asks for the window to be moved by the pointer and
//!    says `cost: moving`. When the button is let go it says so again
//!    for a second press, asks for the window to be resized by its
//!    corner and says `cost: resizing`, and draws the window at each
//!    size it is asked for until the button is let go; then `cost: drew
//!    <count> sizes`.
//! 4. `cost: done`.

use std::time::Duration;

use sysapi::time;
use window_test::{Client, Heard, Outcome, ResizeEdge};

/// The window's own pixels.
const WINDOW: (u32, u32) = (640, 480);

/// How many frames the frame row is measured over.
const FRAMES: u32 = 600;

/// How many keys the key rows are measured over, each down and up.
const KEYS: usize = 200;

/// How far from its left and top edges the pointer is well inside the
/// window: a press there is on no frame.
const INSIDE: f64 = 48.0;

/// How long the window server may take to answer.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long `ob bench` may take over its part of a step.
const STEP_PATIENCE: Duration = Duration::from_secs(300);

fn say(what: &str) {
    println!("window-test: cost: {what}");
}

/// Waits for the frame that shows what the program last drew.
fn frame(client: &mut Client) -> Outcome {
    client.wait(PATIENCE, |heard| matches!(heard, Heard::Frame(_)).then_some(()))
}

/// Where the times of the keys come from.
enum Stamps {
    /// The window server tells them before each key.
    Told,
    /// The keyboards' event devices.
    #[cfg(target_os = "linux")]
    Devices(Vec<std::fs::File>),
}

impl Stamps {
    fn of(client: &mut Client) -> Outcome<Stamps> {
        if client.tells_times() {
            client.ask_times()?;
            return Ok(Stamps::Told);
        }
        Stamps::devices()
    }

    #[cfg(target_os = "linux")]
    fn devices() -> Outcome<Stamps> {
        use std::os::unix::fs::OpenOptionsExt;
        use sysapi::linux::evdev;
        let folder = "/dev/input";
        let listed = std::fs::read_dir(folder).map_err(|err| format!("{folder}: {err}"))?;
        let mut devices = Vec::new();
        for entry in listed.flatten() {
            if !entry.file_name().to_string_lossy().starts_with("event") {
                continue;
            }
            let mut how = std::fs::OpenOptions::new();
            let Ok(device) = how.read(true).custom_flags(libc::O_NONBLOCK).open(entry.path())
            else {
                continue;
            };
            if evdev::stamp_with_monotonic(&device).is_ok() {
                devices.push(device);
            }
        }
        if devices.is_empty() {
            return Err(format!("the window server tells no times, and {folder} has no device"));
        }
        Ok(Stamps::Devices(devices))
    }

    #[cfg(not(target_os = "linux"))]
    fn devices() -> Outcome<Stamps> {
        Err("the window server tells no times of input events".into())
    }

    /// Lets go of the keys that came before the ones to be timed: an
    /// event device keeps every event until it is read.
    fn forget(&mut self) {
        match self {
            Stamps::Told => {}
            #[cfg(target_os = "linux")]
            Stamps::Devices(devices) => {
                use std::io::Read;
                let mut bytes = [0u8; 1024];
                for device in devices {
                    while device.read(&mut bytes).is_ok_and(|read| read > 0) {}
                }
            }
        }
    }

    /// When the key of `code` that the program just read went down or
    /// up, given the time the window server told before it, if any.
    fn of_key(&mut self, told: Option<u64>, code: u32, pressed: bool) -> Outcome<u64> {
        match self {
            Stamps::Told => told.ok_or_else(|| "a key came with no time before it".into()),
            #[cfg(target_os = "linux")]
            Stamps::Devices(devices) => devices
                .iter_mut()
                .find_map(|device| key_of(device, code, pressed))
                .ok_or_else(|| format!("no event device had the key {code}")),
        }
    }
}

/// The time of the key of `code` going down or up among the events
/// `device` has to read. The window server read the same events before
/// it sent the key, so they are there.
#[cfg(target_os = "linux")]
fn key_of(device: &mut std::fs::File, code: u32, pressed: bool) -> Option<u64> {
    use std::io::Read;
    use sysapi::linux::evdev::{EVENT_LEN, Event};
    const EV_KEY: u16 = 1;
    let mut bytes = [0u8; EVENT_LEN];
    while device.read_exact(&mut bytes).is_ok() {
        let event = Event::of(&bytes);
        if event.kind == EV_KEY
            && u32::from(event.code) == code
            && event.value == i32::from(pressed)
        {
            return Some(event.time_ns);
        }
    }
    None
}

/// The value of `sorted` that `part` in 100 of them are at or below.
fn at_part(sorted: &[u64], part: usize) -> u64 {
    sorted[(sorted.len() - 1) * part / 100]
}

/// The next key the program is told of, with the time the window
/// server told before it, if it told one.
fn key(client: &mut Client) -> Outcome<(Option<u64>, u32, bool)> {
    let mut told = None;
    loop {
        match client.heard(STEP_PATIENCE)? {
            Heard::At(at) => told = Some(at),
            Heard::Key { code, pressed } => return Ok((told, code, pressed)),
            _ => told = None,
        }
    }
}

/// The frame row's work: [`FRAMES`] frames after a key.
fn frames(client: &mut Client) -> Outcome {
    say("ready for a key");
    while !key(client)?.2 {}
    for round in 0..FRAMES {
        client.repaint(0x40 + (round % 0x80) as u8)?;
        frame(client)?;
    }
    say("drew the frames");
    // The key was let go meanwhile.
    while key(client)?.2 {}
    Ok(())
}

/// The key rows.
fn keys(client: &mut Client, stamps: &mut Stamps) -> Outcome {
    stamps.forget();
    say(&format!("ready for {KEYS} keys"));
    let mut took = Vec::with_capacity(KEYS * 2);
    while took.len() < KEYS * 2 {
        let (told, code, pressed) = key(client)?;
        let now = time::monotonic_ns();
        took.push(now.saturating_sub(stamps.of_key(told, code, pressed)?));
    }
    took.sort_unstable();
    let keys = took.len();
    println!("BENCH name=desktop_key ns={} iterations={keys}", at_part(&took, 50));
    println!("BENCH name=desktop_key_p99 ns={} iterations={keys}", at_part(&took, 99));
    Ok(())
}

/// Waits until the pointer is well inside the window.
fn pointer_inside(client: &mut Client) -> Outcome {
    say("ready for the pointer");
    let inside = |heard: &Heard| match heard {
        Heard::Enter(x, y) | Heard::Motion(x, y) => (*x >= INSIDE && *y >= INSIDE).then_some(()),
        _ => None,
    };
    client.wait(STEP_PATIENCE, inside)
}

/// Waits for the left button to go down, or to be let go.
fn button(client: &mut Client, down: bool) -> Outcome {
    let wanted = |heard: &Heard| match heard {
        Heard::Button { pressed, .. } => (*pressed == down).then_some(()),
        _ => None,
    };
    client.wait(STEP_PATIENCE, wanted)
}

/// The move row's work: the window server moves the window by the
/// pointer from a press to the button's letting go, of which the
/// program hears when the pointer comes onto the window again.
fn moved(client: &mut Client) -> Outcome {
    say("ready for a press");
    button(client, true)?;
    client.take_heard();
    client.ask_move()?;
    say("moving");
    client.wait(STEP_PATIENCE, |heard| matches!(heard, Heard::Enter(..)).then_some(()))
}

/// The resize row's work: the window drawn at each size the window
/// server asks for, until the pointer comes onto the window again.
fn resized(client: &mut Client) -> Outcome {
    say("ready for a press");
    button(client, true)?;
    client.take_heard();
    client.ask_resize(ResizeEdge::BottomRight)?;
    say("resizing");
    let (mut drawn, mut shade) = (0u32, 0x40u8);
    loop {
        match client.heard(STEP_PATIENCE)? {
            Heard::Size { width, height, .. } => {
                let side = |asked: i32, own: u32| if asked > 0 { asked as u32 } else { own };
                shade = shade.wrapping_add(1) | 0x40;
                client.show_plain((side(width, WINDOW.0), side(height, WINDOW.1)), shade)?;
                drawn += 1;
            }
            Heard::Enter(..) if drawn > 0 => break,
            _ => {}
        }
    }
    say(&format!("drew {drawn} sizes"));
    Ok(())
}

/// Goes through the rows' work in the desktop the environment names.
pub fn run() -> Outcome {
    let mut client = Client::of_environment()?;
    client.show_plain(WINDOW, 0x40)?;
    frame(&mut client)?;
    // A compositor that finds its keyboard after it started tells of it
    // only now, and gives the window the keys then.
    client.await_seat(PATIENCE)?;
    client.wait(PATIENCE, |heard| (*heard == Heard::Keyboard(true)).then_some(()))?;
    let mut stamps = Stamps::of(&mut client)?;
    let (width, height) = WINDOW;
    say(&format!("a window of {width} by {height} shows"));
    frames(&mut client)?;
    keys(&mut client, &mut stamps)?;
    pointer_inside(&mut client)?;
    moved(&mut client)?;
    resized(&mut client)?;
    say("done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_and_the_99th_of_100() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!((at_part(&sorted, 50), at_part(&sorted, 99)), (50, 99));
        assert_eq!(at_part(&[7], 99), 7);
    }
}
