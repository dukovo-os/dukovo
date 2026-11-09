// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The consoles for those who need bigger text, or cannot hold two keys
//! or press one briefly (ADR 0325), with the screen reader listening
//! (ADR 0324): Ctrl+Alt+Plus and Ctrl+Alt+Minus step the text size, which
//! the shell is told as the terminal's size and accessibility.toml(5)
//! keeps; a program names what has its focus with `OSC 7700`, which the
//! reader says, and its drawing is not read until it leaves the alternate
//! screen; Shift pressed five times turns sticky keys on, and Shift then
//! holds for the next key; with slow keys, keys tapped type nothing and
//! keys held type.

use super::super::*;

pub const CASE: Case = Case {
    name: "console-access",
    about: "Ctrl+Alt+Plus and Minus step the console's text size and the shell sees its new size; a program names its focus to the screen reader; Shift five times turns sticky keys on; slow keys type only keys held",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "speech reader-log console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // The screen reader needs an output to speak on; `none` plays it to
    // nothing.
    qemu: &["-audiodev", "none,id=snd0", "-device", "virtio-sound-pci,audiodev=snd0"],
    script: console_access,
    disks: &[],
    second: None,
};

/// The `console-access` case.
fn console_access(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("consoled: the screen reader is listening")?;
    vm.expect_screen(&["# "])?;

    // The text size: QEMU's screen is 1280 by 800, 160 by 50 cells of the
    // 8x16 font; larger is the 12x24 font and largest the 16x32 one.
    vm.press(&["ctrl-alt-equal"])?;
    vm.expect("consoled: text size larger")?;
    vm.expect("consoled: console of 106x33 cells with font 12x24 at (4, 4)")?;
    vm.expect("readerd: says: Text larger")?;
    vm.type_text("clear; stty size; grep text-size /system/settings/console/accessibility.toml\n")?;
    vm.expect_screen(&["33 106", "text-size = \"larger\""])?;
    vm.press(&["ctrl-alt-equal"])?;
    vm.expect("consoled: console of 80x25 cells with font 16x32 at (0, 0)")?;
    vm.press(&["ctrl-alt-equal"])?;
    vm.expect("readerd: says: Text is at its largest")?;
    vm.type_text("clear; stty size\n")?;
    vm.expect_screen(&["25 80"])?;
    vm.press(&["ctrl-alt-minus", "ctrl-alt-minus"])?;
    vm.expect("consoled: text size normal")?;
    vm.expect("consoled: console of 160x50 cells with font 8x16 at (0, 0)")?;
    vm.type_text("clear; stty size\n")?;
    vm.expect_screen(&["50 160"])?;

    // A program names its focus: said at once, and what it draws is not
    // read until it leaves the alternate screen, as it does as it ends.
    vm.type_text("clear; printf '\\033[?1049h\\033]7700;Disk, %s of 2\\033\\\\' $((1 + 1))\n")?;
    vm.expect("readerd: says: Disk, 2 of 2")?;
    vm.type_text("echo drawn-$((6 * 7))\n")?;
    vm.expect_screen(&["drawn-42"])?;
    vm.type_text("printf '\\033[?1049l'; echo read-$((6 * 7))\n")?;
    vm.expect("readerd: says: read-42")?;
    if vm.log().contains("readerd: says: drawn-42") {
        return Err("the reader read what the program drew while it named its focus".into());
    }

    // Sticky keys: Shift five times, then Shift alone holds for one key.
    vm.press(&["shift", "shift", "shift", "shift", "shift"])?;
    vm.expect("consoled: sticky keys on")?;
    vm.expect_screen(&["Sticky keys on: Shift, Ctrl and Alt hold for the next key."])?;
    // The note goes with the next key, which it takes.
    vm.press(&["esc"])?;
    vm.expect_screen_without("Sticky keys on")?;
    vm.type_text("clear; echo x")?;
    vm.press(&["shift"])?;
    vm.expect("readerd: says: shift")?;
    vm.type_text("ay\n")?;
    vm.expect_screen(&["xAy"])?;
    vm.type_text("grep sticky /system/settings/console/accessibility.toml\n")?;
    vm.expect_screen(&["sticky-keys = true"])?;
    vm.press(&["shift", "shift", "shift", "shift", "shift"])?;
    vm.expect("consoled: sticky keys off")?;
    vm.press(&["esc"])?;

    // Slow keys, from the settings: a key tapped types nothing, one held
    // for the delay types. The file is written whole, as the default one
    // already sets the key, and a key set twice is not TOML.
    vm.type_text(
        "clear; echo slow-keys = true > /system/settings/console/accessibility.toml; service consoled reload\n",
    )?;
    vm.expect("consoled: slow keys on: a key counts once held 300 ms")?;
    vm.expect("readerd: says: Slow keys on")?;
    vm.type_text("echo tapped\n")?;
    held(vm, "echo held\n")?;
    vm.expect_screen(&["held"])?;
    if vm.screen()?.iter().any(|line| line.contains("tapped")) {
        return Err("with slow keys, keys tapped typed".into());
    }
    held(vm, "poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Types `text` a key at a time, each held 400 ms: longer than slow keys'
/// 300, shorter than the half second before a key repeats.
fn held(vm: &mut Vm, text: &str) -> Result<(), String> {
    for c in text.chars() {
        vm.monitor(&format!("sendkey {} 400", key_combo(c)?))?;
        std::thread::sleep(Duration::from_millis(550));
    }
    Ok(())
}
