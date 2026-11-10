// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A quiet boot (ADR 0426): with `quiet`, as an installed system's menu
//! entry gives it, the kernel draws the startup screen in place of its
//! log, Dukovo's name and a bar on a slate background, and the bar fills
//! as the boot goes on. The log is kept: the serial port gets all of it,
//! and Esc shows it on the screen. Spark runs one program here, sleep(1),
//! so that no console service takes the screen while it is looked at.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "quiet-boot",
    about: "a quiet boot draws the startup screen in place of the log, its bar filled as Spark starts its programs, while the serial port gets the log; Esc shows the log on the screen (ADR 0426)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep quiet -- --posix /system/commands/sleep 600",
    cpus: 2,
    memory: "512M",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: quiet_boot,
    disks: &[],
    second: None,
};

/// The startup screen shows until Esc, then the log does.
fn quiet_boot(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_ahead("spark: started /system/commands/sleep as process ")?;

    // The startup screen: its background in the corner, a bar filled past
    // the half Spark's start gives it, and none of the log.
    let image = vm.screenshot()?;
    let background = rgb(radix_fbcon::SPLASH_BACKGROUND);
    if image.pixel(0, 0) != background {
        return Err(format!(
            "the corner of the screen is {:02x?}, not the startup screen's background",
            image.pixel(0, 0)
        ));
    }
    let filled = bar_filled(&image).ok_or("the startup screen shows no bar")?;
    if filled <= 500 {
        return Err(format!(
            "the startup screen's bar is filled {filled} of 1000, not past Spark's start"
        ));
    }
    let text = vm.screen()?;
    if text.iter().any(|line| line.contains("Boot complete.")) {
        return Err("the log shows on the screen behind the startup screen".into());
    }

    // Esc shows the log, all of it kept from before.
    vm.press(&["esc"])?;
    vm.expect_screen(&["Boot complete.", "spark: started /system/commands/sleep as process "])?;
    let image = vm.screenshot()?;
    if image.pixel(0, 0) == background {
        return Err("the startup screen's background is still in the corner after Esc".into());
    }
    Ok(())
}

/// A colour as the screenshot gives it.
fn rgb((r, g, b): (u8, u8, u8)) -> [u8; 3] {
    [r, g, b]
}

/// How much of the bar is filled, of 1000: the row with the most pixels
/// of the bar's colour, measured from its first to its last pixel of the
/// bar's or the track's colour.
fn bar_filled(image: &screen::Image) -> Option<usize> {
    let bar = rgb(radix_fbcon::SPLASH_BAR);
    let background = rgb(radix_fbcon::SPLASH_BACKGROUND);
    let mut best: Option<(usize, usize)> = None;
    for y in 0..image.height {
        let pixels: Vec<[u8; 3]> = (0..image.width).map(|x| image.pixel(x, y)).collect();
        let filled = pixels.iter().filter(|&&pixel| pixel == bar).count();
        if filled == 0 || best.is_some_and(|(most, _)| most >= filled) {
            continue;
        }
        // The bar and its track are what is not background on its row.
        let first = pixels.iter().position(|&pixel| pixel != background)?;
        let last = pixels.iter().rposition(|&pixel| pixel != background)?;
        best = Some((filled, last + 1 - first));
    }
    best.map(|(filled, whole)| filled * 1000 / whole)
}
