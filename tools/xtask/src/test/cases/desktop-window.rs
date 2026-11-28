// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The desktop's first window (ADR 0712), on a machine whose one screen
//! is a virtio graphics card's. Spark starts window-test
//! (userspace/programs/window-test), which waits for consoled and runs
//! the desktop (desktop(1)) with itself in it. The desktop borrows the
//! screens from consoled, and the window-test in it shows one window and
//! says what the screen then shows: the desktop's colour and the
//! window's picture in the middle. The host compares the screen with
//! that, pixel for pixel, at every step, and presses the space bar,
//! which the window reads as any program reads a key. The window draws
//! a second picture. The mouse moves and clicks, and the window hears
//! where and in which order. A second program cuts the file of its
//! pixels short: it is let go, and the desktop and the first window go
//! on (ADR 0717). Ctrl+Alt+F2 shows tty2 and Ctrl+Alt+F1 the desktop
//! again, as it was (ADR 0713). Spark restarts consoled: the desktop
//! goes on with the screens and the keys, and the new consoled stands
//! under it (ADR 0716). The window's program ends, so the desktop ends
//! as it did, and the consoles show.
//!
//! The pointer is on the card's pointer plane, which a picture of this
//! machine's screen leaves out; `desktop-firmware` sees it.

use super::super::*;
use super::input_follow::{button, rel};

/// The id of the machine's graphics card, which the events of its mouse
/// are sent by.
const CARD: &str = "gpu0";

/// The test.
pub const CASE: Case = Case {
    name: "desktop-window",
    about: "the desktop's first window on a virtio graphics card (ADR 0712): `desktop` borrows the screens and runs a program whose window the host compares with what it says, pixel for pixel; keys, the mouse and its buttons reach the window; a program that cuts its pixels short is let go and the desktop draws on; Ctrl+Alt+F2 shows a console and Ctrl+Alt+F1 the desktop again; consoled is restarted under the desktop, which goes on; the desktop ends as its command did, and the consoles show",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off -- --services --posix /system/tests/posix/window-test in-desktop steps",
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
        "usb-mouse,bus=xhci.0,port=3",
    ],
    script: desktop_window,
    disks: &[],
    second: None,
};

fn desktop_window(vm: &mut Vm, case: &Case) -> Result<(), String> {
    first_window(vm, case, CARD)
}

/// The sign-in line of console `number`, which shows there with or
/// without a shell.
fn console_line(number: u32) -> String {
    format!("{RELEASE} {VERSION} (tty{number})")
}

/// The script of the desktop's tests, on a machine whose mouse the
/// events sent by the card `card` reach.
pub(super) fn first_window(vm: &mut Vm, case: &Case, card: &str) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let mut watch = screens::Watch::of_desktop();
    watch.steps(vm, &["shown", "redrawn"])?;
    // Right and down, by less than the pointer's fastest takes it out
    // of the window (ADR 0715), and one click of the left button.
    watch.step(vm, "mouse")?;
    vm.input_events(card, &[rel("x", 40), rel("y", 30)])?;
    for down in [true, false] {
        vm.input_events(card, &[button("left", down)])?;
    }
    screens::Watch::go_on(vm)?;
    watch.steps(vm, &["moved"])?;
    // To a console and back: the desktop shows as it was.
    watch.step(vm, "outlived")?;
    vm.press(&["ctrl-alt-f2"])?;
    vm.expect_screen(&[&console_line(2)])?;
    vm.press(&["ctrl-alt-f1"])?;
    watch.look(vm).map_err(|why| format!("back from tty2: {why}"))?;
    // window-test has Spark restart consoled, and its step comes before
    // or after the new consoled's lines.
    let from = vm.cursor;
    screens::Watch::go_on(vm)?;
    watch.arrive(vm, "restarted")?;
    let arrived = vm.cursor;
    vm.cursor = from;
    vm.expect("spark: starting consoled again in ")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect(" consoles; a program has the screens")?;
    vm.cursor = vm.cursor.max(arrived);
    watch.look(vm).map_err(|why| format!("with consoled started again: {why}"))?;
    screens::Watch::go_on(vm)?;
    watch.steps(vm, &["stayed"])?;
    screens::desktop_passed(vm)?;
    vm.expect_screen(&[&console_line(1), "# "])
}
