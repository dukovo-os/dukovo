// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A virtio graphics card that is the machine's VGA card too (QEMU's
//! virtio-vga), under BIOS. The boot loader leaves the kernel a
//! framebuffer of that card, which the kernel's log starts on as the
//! firmware's card (ADR 0705). When the virtio driver takes the card,
//! the framebuffer shows nothing any more: the firmware's card is taken
//! away, the screen is the virtio card's, and the log goes on there
//! (ADR 0704). The consoles then show on it, and *hardware screens*
//! names the one card that is left, with consoled in front.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "display-vga-virtio",
    about: "a virtio graphics card that is the VGA card too: the firmware's framebuffer gives way to the virtio card when its driver takes it, the consoles show on it, and hardware screens names it alone (ADRs 0704 and 0705)",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-vga", "none", "-device", "virtio-vga,id=gpu0,xres=1024,yres=768"],
    script: display_vga_virtio,
    disks: &[],
    second: None,
};

fn display_vga_virtio(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    screens::found(
        vm,
        &[
            "Display: firmware0 is the firmware's framebuffer, ",
            "Display: virtio0 is a virtio graphics card at ",
            "Display: firmware0 is gone: its screen is virtio0's",
        ],
    )?;
    vm.expect("Boot complete.")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("hardware screens\n")?;
    vm.expect_screen(&[
        "In front: consoled (process ",
        "virtio0: Virtio graphics card",
        "Shows:  1024x768 at ",
    ])?;
    // The firmware's card is gone from the layer, not only from use.
    if vm.screen()?.iter().any(|line| line.contains("firmware0")) {
        return Err("hardware screens still names the firmware's card".into());
    }
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
