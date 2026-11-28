// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The desktop on two screens of a virtio graphics card (ADRs 0725 to
//! 0728), worked with the keys while monitors come and go. Spark
//! starts window-test, which runs the desktop with itself in it
//! (`window-test screens`), says what each screen shows at each step
//! and waits. The host compares both screens with that, pixel for
//! pixel, and does the next thing:
//!
//! - plugs a monitor into the second screen: the desktop is on it,
//!   beside the first;
//! - presses Super+Left, which snaps the window to the left side of
//!   its screen;
//! - presses Super+Ctrl+D for a second desktop, which is bare, and
//!   Super+Ctrl+Left for the first again;
//! - presses Super+Shift+Right, which carries the window to the same
//!   side of the second screen.
//!
//! The program then asks for its window to be full screen: it fills
//! the second screen with no frame, and the program checks that the
//! screen showed it from the memory the program drew it in. It has the
//! window back, and writes a layout into windowd.json that puts the
//! second screen to the left of the first, which the window server
//! tells it of when the desktop has read it. The host pulls the second
//! monitor out, and the window is on the first screen, on the same
//! side; it plugs it in again, and the window is back where it was,
//! the screens laid out as the settings say. Last the host presses
//! Alt+F4: the program ends, the desktop with it, and the consoles
//! show.

use super::super::*;

/// The size of the monitor the host plugs into the second screen.
const SECOND: (u16, u16) = (800, 600);

/// The test.
pub const CASE: Case = Case {
    name: "desktop-screens",
    about: "the desktop on two screens of a virtio graphics card, each compared with what the program says at every step, pixel for pixel: a monitor plugged in, the window snapped to a side with Super+Left (ADR 0727), a second desktop and back (ADR 0726), the window carried to the second screen with Super+Shift+Right, full screen there and shown from the program's own memory (ADR 0728), a layout from windowd.json, the monitor pulled out and plugged in again with the window following (ADR 0725); Alt+F4 ends the program, and the desktop with it",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off -- --services --posix /system/tests/posix/window-test in-desktop screens",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-vga",
        "none",
        "-device",
        "virtio-gpu-pci,id=gpu0,max_outputs=2,xres=1024,yres=768",
        "-vnc",
        screens::SCREEN_VIEW[0],
        "-vnc",
        screens::SCREEN_VIEW[1],
    ],
    script: desktop_screens,
    disks: &[],
    second: None,
};

fn desktop_screens(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let mut watch = screens::Watch::of_desktop();

    watch.step(vm, "shown")?;
    vm.plug_monitor(case, 1, SECOND)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "plugged")?;
    vm.press(&["meta_l-left"])?;
    screens::Watch::go_on(vm)?;

    // A window on a desktop that does not show hears no key: the
    // program goes on to wait for the keyboard's going before the keys
    // that take it.
    watch.step(vm, "snapped")?;
    screens::Watch::go_on(vm)?;
    vm.press(&["meta_l-ctrl-d"])?;

    watch.step(vm, "desk")?;
    vm.press(&["meta_l-ctrl-left"])?;

    watch.step(vm, "first")?;
    vm.press(&["meta_l-shift-right"])?;
    screens::Watch::go_on(vm)?;

    watch.steps(vm, &["carried", "full"])?;

    watch.step(vm, "laid")?;
    vm.unplug_monitor(case, 1)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "unplugged")?;
    vm.plug_monitor(case, 1, SECOND)?;
    screens::Watch::go_on(vm)?;

    watch.step(vm, "home")?;
    screens::Watch::go_on(vm)?;
    vm.press(&["alt-f4"])?;
    screens::desktop_passed(vm)?;
    vm.expect_screen(&[&super::desktop_window::console_line(1), "# "])
}
