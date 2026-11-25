// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens passed from program to program (ADR 0702), on a machine
//! with the firmware's card and a virtio one, both showing the consoles.
//! display-test (userspace/programs/display-test) borrows the screens
//! from consoled at its socket, as a desktop does, and shows its own
//! pictures, which the host compares with what the program says, pixel
//! for pixel, at every step. It lends the screens to a second holder,
//! whose pictures show, and gets them back: the layer puts its pictures
//! back by itself. The second holder then ends with the screens, and
//! its pictures stay until the first makes a change. No hand-over sets a
//! mode, by the kernel's own count. Given back to consoled, the screens
//! show the consoles. A child that has borrowed the screens is killed
//! and the test borrows them at once: its first frame is drawn over the
//! child's last, and stays when consoled's five seconds for the child
//! are over. A second child is killed and nobody borrows: its pictures
//! stay, and after five seconds the consoles show again.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "display-handover",
    about: "the screens borrowed from consoled, lent on, given back (the layer puts the pictures back, and sets no mode) and left by a holder that ends; a borrower killed and replaced at once shows no console between, and one killed for good leaves its picture for five seconds before the consoles show (ADR 0702)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off -- --services --posix /system/tests/posix/display-test handover",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-device", "virtio-gpu-pci,id=gpu0,xres=1024,yres=768"],
    script: display_handover,
    disks: &[],
    second: None,
};

/// Waits for the program's step `name`, at which it has given the
/// screens back, and for the consoles on both screens.
fn consoles_show(vm: &mut Vm, watch: &mut screens::Watch, name: &str) -> Result<(), String> {
    watch.arrive(vm, name)?;
    let login = format!("{RELEASE} {VERSION} (tty1)");
    vm.expect_screen(&[&login]).map_err(|why| format!("at step {name}: {why}"))?;
    vm.expect_console_on(2, Some(0), &[&login]).map_err(|why| format!("at step {name}: {why}"))?;
    screens::Watch::go_on(vm)
}

fn display_handover(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let mut watch = screens::Watch::default();
    watch.steps(vm, &["borrowed", "lent", "returned", "ended"])?;
    consoles_show(vm, &mut watch, "consoles")?;
    // A borrower killed, and another at once: its pictures stay when
    // consoled's five seconds for the dead one are over.
    watch.steps(vm, &["child"])?;
    watch.step(vm, "again")?;
    std::thread::sleep(Duration::from_secs(7));
    watch.look(vm).map_err(|why| format!("seven seconds after a new borrower's frame: {why}"))?;
    screens::Watch::go_on(vm)?;
    consoles_show(vm, &mut watch, "consoles-again")?;
    // A borrower killed for good: its pictures stay a while, and then
    // the consoles show.
    watch.steps(vm, &["child-again"])?;
    watch.arrive(vm, "killed")?;
    let killed = Instant::now();
    watch.look(vm).map_err(|why| format!("right after the borrower was killed: {why}"))?;
    // A second look two seconds on, unless the first took so long that
    // consoled's five seconds may be over.
    if killed.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_secs(2).saturating_sub(killed.elapsed()));
        watch.look(vm).map_err(|why| format!("two seconds after the borrower was killed: {why}"))?;
    }
    let login = format!("{RELEASE} {VERSION} (tty1)");
    vm.expect_screen(&[&login])?;
    vm.expect_console_on(2, Some(0), &[&login])?;
    screens::Watch::go_on(vm)?;
    screens::passed(vm)
}
