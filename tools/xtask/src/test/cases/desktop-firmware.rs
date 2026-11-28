// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The desktop's first window on the screen the firmware set up
//! (ADR 0712), which every PC has: the machine has no other graphics
//! card. The steps are `desktop-window`'s. This card has no pointer
//! plane of its own, so the kernel draws the pointer into what the
//! screen shows (ADR 0705), and a picture of the screen has it:
//! window-test gives the pointer a picture of its own over its window,
//! and says where that shows, before the mouse moves and after
//! (ADR 0715).

use super::super::*;
use super::desktop_window::first_window;

/// The id of the machine's graphics card, which the events of its mouse
/// are sent by.
const CARD: &str = "video0";

/// The test.
pub const CASE: Case = Case {
    name: "desktop-firmware",
    about: "the desktop's first window on the firmware's screen alone (ADR 0712), as `desktop-window`, with the pointer in the picture: a window's own pointer shows over it where the mouse put it, drawn by the kernel on a card with no pointer plane, pixel for pixel",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off -- --services --posix /system/tests/posix/window-test in-desktop steps pointer-drawn",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-vga", "none", "-device", "VGA,id=video0", "-device", "usb-mouse,bus=xhci.0,port=3"],
    script: desktop_firmware,
    disks: &[],
    second: None,
};

fn desktop_firmware(vm: &mut Vm, case: &Case) -> Result<(), String> {
    first_window(vm, case, CARD)
}
