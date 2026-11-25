// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's virtio graphics card (ADR 0704) with four screens, on a machine
//! with no other card, driven by display-test
//! (userspace/programs/display-test) through the display layer
//! (radix-display(2)). The host takes a picture of each screen at every
//! step and compares it, pixel for pixel, with the test patterns the
//! program says it shows. The first screen shows a picture the card
//! keeps, sent with PUT, whole and then in part. The host plugs monitors
//! into the second and third screens, as a viewer of each asks for a
//! size: one change shows a picture on all three, from memory files the
//! program draws in, and a second gives all three new pictures and sets
//! no mode. A change that names a rectangle shows that rectangle alone.
//! Another mode of a screen is refused unless the change may blank it,
//! and is then one mode set of its pipe. The host resizes the third
//! screen's window, and the screen is shown in the new size; it pulls
//! the second's monitor out, and that screen shows nothing. A pointer is
//! the host's to draw, and is in no screen's picture.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "display-virtio",
    about: "a virtio graphics card of four screens, each compared with what display-test draws, pixel for pixel: a kept picture sent with PUT, three screens shown by one change, a rectangle alone, another mode, monitors plugged in, resized and pulled out by the host, a pointer (ADR 0704)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --posix /system/tests/posix/display-test virtio",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-vga",
        "none",
        "-device",
        "virtio-gpu-pci,id=gpu0,max_outputs=4,xres=1024,yres=768",
        "-vnc",
        screens::SCREEN_VIEW[0],
        "-vnc",
        screens::SCREEN_VIEW[1],
        "-vnc",
        screens::SCREEN_VIEW[2],
        "-vnc",
        screens::SCREEN_VIEW[3],
    ],
    script: display_virtio,
    disks: &[],
    second: None,
};

fn display_virtio(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    let card = screens::found(vm, &["Display: virtio0 is a virtio graphics card at "])?;
    if !card[0].contains(": 4 screens, 1 with a monitor, ") {
        return Err(format!("the kernel found the card as: {}", card[0]));
    }
    vm.expect("Boot complete.")?;
    let mut watch = screens::Watch::default();
    watch.steps(vm, &["kept"])?;
    watch.step(vm, "put")?;
    vm.plug_monitor(case, 1, (800, 600))?;
    vm.plug_monitor(case, 2, (640, 480))?;
    screens::Watch::go_on(vm)?;
    watch.steps(vm, &["three", "again", "patch"])?;
    watch.step(vm, "mode")?;
    vm.plug_monitor(case, 2, (1024, 600))?;
    screens::Watch::go_on(vm)?;
    watch.step(vm, "resized")?;
    vm.unplug_monitor(case, 1)?;
    screens::Watch::go_on(vm)?;
    watch.steps(vm, &["unplugged", "pointer"])?;
    screens::passed(vm)
}
