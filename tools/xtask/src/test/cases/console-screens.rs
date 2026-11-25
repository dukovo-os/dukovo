// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The consoles on every screen (consoled(8)), on a machine with the
//! firmware's card and a virtio one of two screens, the second with no
//! monitor at first. The consoles show on the firmware's screen and on
//! the virtio card's first, the same text on one grid that fits both,
//! and what is typed shows on both. The host plugs a monitor into the
//! virtio card's second screen: consoled lays the consoles out again
//! and they show on all three. It pulls the monitor out, and they show
//! on the two that are left.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "console-screens",
    about: "the consoles show on every screen, the firmware's and a virtio card's: what is typed shows on both, on a third when the host plugs a monitor in, and on two again when it pulls it out (ADR 0700)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-device",
        "virtio-gpu-pci,id=gpu0,max_outputs=2,xres=1024,yres=768",
        "-vnc",
        screens::SCREEN_VIEW[0],
        "-vnc",
        screens::SCREEN_VIEW[1],
    ],
    script: console_screens,
    disks: &[],
    second: None,
};

/// Types a command that prints `word`, and waits for the word on the
/// first `screens` screens the consoles show on: the firmware's, then
/// the virtio card's in order.
fn typed_shows_on(vm: &mut Vm, word: &str, screens: usize) -> Result<(), String> {
    // The word is put together by the shell, so that the command typed
    // does not show it.
    let (head, tail) = word.split_at(word.len() / 2);
    vm.type_text(&format!("echo {head}\"\"{tail}\n"))?;
    vm.expect_screen(&[word])?;
    for number in 2..=screens {
        vm.expect_console_on(number, Some(number - 2), &[word])?;
    }
    Ok(())
}

/// Waits until consoled's last account of where the consoles are is of
/// `screens` screens: a line for the first, and one for each other.
fn laid_out_on(vm: &mut Vm, screens: usize) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let log = vm.log();
        let ended = log.rfind('\n').map_or("", |end| &log[..end]);
        let last = ended.rfind("consoled: console of ").map_or("", |at| &ended[at..]);
        let others = (2..=4).filter(|n| last.contains(&format!("consoled: screen {n}: console of ")));
        let named = others.count() + usize::from(!last.is_empty());
        if named == screens {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(format!("consoled has the consoles on {named} screens, not {screens}"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn console_screens(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    laid_out_on(vm, 2)?;
    let login = format!("{RELEASE} {VERSION} (tty1)");
    vm.expect_screen(&[&login, "# "])?;
    vm.expect_console_on(2, Some(0), &[&login, "# "])?;
    typed_shows_on(vm, "on-two-screens", 2)?;
    // A third monitor: the consoles are laid out again, on three.
    vm.plug_monitor(case, 1, (800, 600))?;
    laid_out_on(vm, 3)?;
    typed_shows_on(vm, "on-three-screens", 3)?;
    // And gone again: the consoles are on the two that are left.
    vm.unplug_monitor(case, 1)?;
    laid_out_on(vm, 2)?;
    typed_shows_on(vm, "on-two-again", 2)?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
