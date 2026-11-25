// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A kernel panic while a program has the screens, on a machine whose
//! only card is a virtio one, which has no framebuffer the kernel could
//! write to (ADR 0704). display-test
//! (userspace/programs/display-test) shows a picture, which the host
//! compares with what the program says, and asks for a test panic. The
//! panic report takes the screen from the program: the host reads it
//! from a picture of the card's screen, with the reason, the recent
//! kernel log and the word that a key restarts the machine, which one
//! then does.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "display-panic",
    about: "a panic while display-test has the screen of a virtio graphics card, the machine's only one: the report is read from the card's screen (ADR 0704)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "-- --posix /system/tests/posix/display-test panic",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-vga", "none", "-device", "virtio-gpu-pci,id=gpu0,xres=1024,yres=768"],
    script: screens::panic_behind_a_program,
    disks: &[],
    second: None,
};
