// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A kernel panic while a program has the screens, on a machine with the
//! firmware's card and a virtio one (ADR 0705). display-test
//! (userspace/programs/display-test) shows a picture on each screen,
//! which the host compares with what the program says, and asks for a
//! test panic. The panic report takes the firmware's screen, where the
//! kernel's log is, from the program: the host reads it there, with the
//! reason, the recent kernel log and the word that a key restarts the
//! machine, which one then does.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "display-panic-vga",
    about: "a panic while display-test has the screens of the firmware's card and a virtio one: the report is read from the firmware's screen, where the kernel's log is (ADR 0705)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "-- --posix /system/tests/posix/display-test panic",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-device", "virtio-gpu-pci,id=gpu0,xres=1024,yres=768"],
    script: screens::panic_behind_a_program,
    disks: &[],
    second: None,
};
