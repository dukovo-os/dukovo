// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The desktop's windows worked as a person works them (ADR 0721), on
//! a machine with a virtio graphics card and a tablet, which points at
//! a pixel exactly where a mouse's motion depends on its speed. Spark
//! starts window-test, which runs the desktop with itself in it
//! (`window-test windows`), says what the screen shows at each step and
//! waits. The host compares the screen with that, pixel for pixel, the
//! frames with it, and does the next thing:
//!
//! - drags the window by its title bar, and by its bottom right corner,
//!   the program drawing each size on the way;
//! - puts the pointer on the close button, which lights;
//! - clicks the maximize button, and double clicks the title bar to
//!   have the window back as it was;
//! - clicks the minimize button, which leaves the desktop bare, and
//!   presses Alt+Tab for the window;
//! - presses Alt+Tab again once the program has shown a second window
//!   in front, which puts the first in front and makes the second's
//!   frame the one of a window not worked in.
//!
//! The program then takes its second window away, writes the keys'
//! repeat into windowd.json and sends the desktop SIGUSR1, and is told
//! the new repeat (ADR 0722). Last the host clicks the close button:
//! the program ends, the desktop with it, and the consoles show.

use display_pattern::said::{DRAGGED_BY, RESIZED_BY};
use window_frame::{Button, Edge, Part};

use super::super::*;
use super::input_follow::{abs, button};
use super::screens::Framed;

/// The id of the machine's graphics card, which the events of its
/// tablet are sent by, and the size of its screen.
const CARD: &str = "gpu0";
const SCREEN: (i32, i32) = (1024, 768);

/// The most a tablet says along an axis.
const ABS_MOST: f64 = 32767.0;

/// The first window's title, and where the pointer rests on no frame:
/// near the screen's bottom left corner.
const TITLE: &str = "window-test";
const REST: (i32, i32) = (8, 759);

/// The test.
pub const CASE: Case = Case {
    name: "desktop-windows",
    about: "the desktop's windows worked with a pointer and the keys on a virtio graphics card (ADR 0721), the screen compared with what the program says at every step, frames and all, pixel for pixel: a window dragged by its title bar and resized by its corner, the close button lit under the pointer, the window maximized by its button and restored by a double click on the bar, minimized and brought back with Alt+Tab, a second window in front and Alt+Tab between the two; the desktop reads a changed windowd.json at SIGUSR1 and tells the program the keys' new repeat (ADR 0722); a click on the close button ends the program, and the desktop with it",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off -- --services --posix /system/tests/posix/window-test in-desktop windows",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-vga",
        "none",
        "-device",
        "virtio-gpu-pci,id=gpu0,xres=1024,yres=768",
        "-device",
        "virtio-tablet-pci,display=gpu0",
    ],
    script: desktop_windows,
    disks: &[],
    second: None,
};

/// What a tablet says along an axis `size` pixels long to point at the
/// middle of the pixel `at`: the desktop has the tablet's whole range
/// reach from the first pixel's place to the last's (windowd(1)).
fn pointing(at: i32, size: i32) -> i32 {
    ((f64::from(at) + 0.5) / f64::from(size - 1) * ABS_MOST).round().clamp(0.0, ABS_MOST) as i32
}

/// Points at the pixel `at` of the screen.
fn point(vm: &mut Vm, at: (i32, i32)) -> Result<(), String> {
    vm.input_events(CARD, &[abs("x", pointing(at.0, SCREEN.0)), abs("y", pointing(at.1, SCREEN.1))])
}

/// The left button goes down, or up.
fn hold(vm: &mut Vm, down: bool) -> Result<(), String> {
    vm.input_events(CARD, &[button("left", down)])
}

/// One click of the left button at `at`.
fn click(vm: &mut Vm, at: (i32, i32)) -> Result<(), String> {
    point(vm, at)?;
    hold(vm, true)?;
    hold(vm, false)
}

/// Presses the left button at `from`, moves the pointer by `by` in two
/// goes, as a hand does not jump, and lets the button go there.
fn drag(vm: &mut Vm, from: (i32, i32), by: (i32, i32)) -> Result<(), String> {
    point(vm, from)?;
    hold(vm, true)?;
    point(vm, (from.0 + by.0 / 2, from.1 + by.1 / 2))?;
    point(vm, (from.0 + by.0, from.1 + by.1))?;
    hold(vm, false)
}

/// Where in the frame of `window` to point for `part`, which the place
/// is checked to be.
fn place_of(window: &Framed, part: Part) -> Result<(i32, i32), String> {
    let layout = &window.layout;
    let (x, y) = match part {
        Part::Title => (layout.title.x + layout.title.w / 2, layout.title.y + layout.title.h / 2),
        Part::Button(button) => {
            let rect = layout.button(button);
            (rect.x + rect.w / 2, rect.y + rect.h / 2)
        }
        Part::Edge(Edge::BottomRight) => (layout.frame.right() - 2, layout.frame.bottom() - 2),
        other => return Err(format!("the test points at no {other:?}")),
    };
    let there = layout.part_at(x, y);
    if there != part {
        return Err(format!("({x}, {y}) of the frame is {there:?}, not {part:?}"));
    }
    Ok(window.at(x, y))
}

fn desktop_windows(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let mut watch = screens::Watch::of_desktop();
    let place = |watch: &screens::Watch, part| place_of(&watch.window(TITLE)?, part);

    watch.step(vm, "shown")?;
    drag(vm, place(&watch, Part::Title)?, DRAGGED_BY)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "dragged")?;
    drag(vm, place(&watch, Part::Edge(Edge::BottomRight))?, RESIZED_BY)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "resized")?;
    point(vm, place(&watch, Part::Button(Button::Close))?)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "pointed")?;
    click(vm, place(&watch, Part::Button(Button::Maximize))?)?;
    point(vm, REST)?;
    screens::Watch::go_on(vm)?;

    // Two clicks at one place, one straight after the other.
    watch.step(vm, "maximized")?;
    let bar = place(&watch, Part::Title)?;
    click(vm, bar)?;
    click(vm, bar)?;
    point(vm, REST)?;
    screens::Watch::go_on(vm)?;

    // A minimized window hears no key: the program goes on to wait for
    // the keyboard's going before the click that takes it.
    watch.step(vm, "restored")?;
    screens::Watch::go_on(vm)?;
    click(vm, place(&watch, Part::Button(Button::Minimize))?)?;

    watch.step(vm, "minimized")?;
    vm.press(&["alt-tab"])?;

    // The pointer is still where it clicked, on the minimize button.
    watch.step(vm, "back")?;
    point(vm, REST)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "second")?;
    vm.press(&["alt-tab"])?;
    screens::Watch::go_on(vm)?;

    watch.steps(vm, &["switched"])?;

    watch.step(vm, "alone")?;
    screens::Watch::go_on(vm)?;
    click(vm, place(&watch, Part::Button(Button::Close))?)?;
    screens::desktop_passed(vm)?;
    vm.expect_screen(&[&super::desktop_window::console_line(1), "# "])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tablet_points_at_the_middle_of_a_pixel() {
        // The desktop puts the pointer at the tablet's share of the way
        // from 0 to the last pixel's place.
        let placed = |said: i32, size: i32| f64::from(said) / ABS_MOST * f64::from(size - 1);
        for (at, size) in [(0, 1024), (511, 1024), (1022, 1024), (0, 768), (759, 768)] {
            let lands = placed(pointing(at, size), size);
            let off = lands - (f64::from(at) + 0.5);
            assert!(off.abs() < 0.05, "{at} of {size} lands at {lands}");
        }
        // The middle of the last pixel is past the tablet's range.
        assert_eq!(pointing(1023, 1024), ABS_MOST as i32);
    }
}
