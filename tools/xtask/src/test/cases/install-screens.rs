// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick's *Install Dukovo* entry, as the live menu starts
//! it, with *install* on the command line: consoled runs
//! *dukovo-install*'s screens on tty1 (ADR 0310). Live console leaves
//! them for root's shell on tty1, where *dukovo-install* shows them
//! again. Each page is answered by keys as a person would: a computer
//! name, a time zone found by typing the start of its name, the blank
//! virtio disk, the Server profile with File sharing ticked (ADR 0317),
//! and alice with a password typed twice; the review takes
//! the disk's name to confirm, and the progress runs to the Done page.
//! L shows the install log, and the Done page saves the answers as a
//! setup file on the stick, which the shell on tty2 finds there. The
//! host finds the disk sound, as *install-uefi* does, and the second
//! boot is from that disk alone: alice signs in with the password typed
//! in the screens and finds the settings they made, and File sharing
//! added, its services started at that first boot.

use super::super::*;
use super::install_uefi::{INSTALL_DISKS, wait_for_screen};

pub const CASE: Case = Case {
    name: "install-screens",
    about: "The installer's screens on tty1 install Dukovo onto a blank disk under UEFI, every page answered by keys: Live console and back, a time zone typed by name, the Server profile with File sharing, the disk confirmed by its name, the progress to Done, the log, and a setup file saved on the stick; the disk boots alone, where alice signs in with the password typed in the screens",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As the Install Dukovo entry gives it: the test image's menu starts
    // Try Dukovo, whose `live console-sign-in=off` come before this.
    cmdline: "install",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    // One network card, so that the network page's fields are in a known
    // order.
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: screens,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: super::install_uefi::nothing, script: installed }),
};

/// alice's password, typed into the screens.
const PASSWORD: &str = "queen of hearts";

/// The computer's name typed into the screens.
const HOSTNAME: &str = "screens";

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// The first boot, from the stick: the screens on tty1, answered by keys.
fn screens(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
    vm.expect_anywhere("consoled: install: the installer's screens on tty1")?;
    vm.expect_anywhere("consoled: started /system/commands/dukovo-install on tty1 as process ")?;
    let welcome = ["Step 1 of 6", "How do you want to install?", "Step by step"];
    vm.expect_screen(&[&format!("Install Dukovo {VERSION}"), welcome[0], welcome[1]])?;

    // F1 shows the page's help, and Esc closes it.
    vm.press(&["f1"])?;
    vm.expect_screen(&["Live console leaves the installer for a shell", "Esc closes"])?;
    vm.press(&["esc"])?;
    vm.expect_screen_without("Esc closes")?;

    // Live console, the button three before the first field, leaves the
    // screens for root's shell on tty1, which starts them again.
    vm.press(&["up", "up", "up", "ret"])?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("dukovo-install\n")?;
    vm.expect_screen(&welcome)?;

    // Network: the card, DHCP and fixed, then the computer's name and
    // its time zone, found by the start of its name.
    vm.press(&["ret"])?;
    vm.expect_screen(&["Step 2 of 6", "Network cards found:", "Automatic (DHCP)"])?;
    vm.press(&["down", "down", "down"])?;
    vm.press(&["backspace"; 6])?;
    vm.type_text(HOSTNAME)?;
    vm.press(&["down"])?;
    vm.type_text("america/denver")?;
    vm.expect_screen(&["America/Denver"])?;

    // Disk: the stick cannot be chosen; the blank disk is.
    vm.press(&["ret"])?;
    vm.expect_screen(&["Step 3 of 6", "the install stick, can't be chosen", "vda", "Erase the disk"])?;
    vm.press(&["spc", "ret"])?;

    // Software: the Server profile, with the web console, and File
    // sharing ticked; Tab goes on to Next past the web console and Back.
    vm.expect_screen(&[
        "Step 4 of 6",
        "What is this machine for?",
        "Server",
        "[ ] File sharing",
        "turns on smbd and nfsd",
        "[x] Turn on Dukovo Admin, the web console",
    ])?;
    vm.press(&["down", "down", "down", "spc"])?;
    vm.expect_screen(&["[x] File sharing"])?;
    vm.press(&["tab", "tab", "tab", "ret"])?;

    // You: the name makes the user name.
    vm.expect_screen(&["Step 5 of 6", "The administrator account."])?;
    vm.type_text("Alice Liddell")?;
    vm.press(&["tab", "tab"])?;
    vm.type_text(PASSWORD)?;
    vm.press(&["tab"])?;
    vm.type_text(PASSWORD)?;
    vm.press(&["ret"])?;

    // Review: everything the install does, and the disk's name to
    // confirm it.
    vm.expect_screen(&[
        "Step 6 of 6",
        "Erase     vda",
        "alice (Alice Liddell), administrator",
        "America/Denver",
        "web console on (from release 0.16)",
        "Type vda to confirm",
    ])?;
    vm.expect_screen(&[HOSTNAME])?;
    vm.type_text("vda")?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["Installing Dukovo"])?;
    if let Err(err) = wait_for_screen(vm, "Remove the install stick, then restart.", INSTALL_SECONDS) {
        // The shell on tty2 shows the log's last lines whole, as the
        // screens' log view cuts them at its edge.
        vm.press(&["alt-f2"])?;
        vm.expect_screen(&["# "])?;
        vm.type_text("clear; tail -n 12 /tmp/install.log\n")?;
        let _ = vm.expect_screen(&["problem: "]);
        return Err(format!("{err}; the log: {}", vm.screen()?.join(" | ")));
    }
    vm.expect_screen(&["Dukovo is installed", "Your answers were saved"])?;

    // L shows the install log, which ends with the install.
    vm.press(&["l"])?;
    vm.expect_screen(&[&format!("==> Dukovo {VERSION} is installed on vda.")])?;
    vm.press(&["esc"])?;

    // The Done page saves the answers on the stick, without the
    // passwords' hashes, as they were not kept.
    vm.press(&["tab", "tab", "ret"])?;
    vm.expect_screen(&["Saved as install.json on the stick, without the passwords' hashes"])?;

    // The shell on tty2 finds the setup file on the stick's EFI system
    // partition, with the computer's name the screens took.
    vm.press(&["alt-f2"])?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty2)"), "# "])?;
    vm.type_text(&format!(
        "mkdir /tmp/s && mount -t fat -o ro sda2 /tmp/s && grep -c '\"{HOSTNAME}\"' /tmp/s/install.json; \
         umount /tmp/s; echo \"setup: $((10 + $?))\"\n"
    ))?;
    vm.expect_screen(&["setup: 10"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, from the installed disk alone: alice signs in with
/// the password typed in the screens and finds the settings they made.
fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: /system is Dukovo's root on ")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION}")])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;

    // Each answer has a label the command line does not hold whole.
    answer_prompt(vm, "$", "echo \"host\"\": $(cat /system/settings/hostname)\"")?;
    vm.expect_screen(&[&format!("host: {HOSTNAME}")])?;
    answer_prompt(vm, "$", "echo \"zone\"\": $(readlink /system/settings/localtime)\"")?;
    vm.expect_screen(&["zone: /system/resources/zoneinfo/America/Denver"])?;
    answer_prompt(vm, "$", "echo \"admins\"\": $(grep '^admins:' /system/settings/group)\"")?;
    vm.expect_screen(&["admins: admins:x:10:alice"])?;
    answer_prompt(
        vm,
        "$",
        "grep -q '\"name\": \"Alice Liddell\"' /system/settings/install/answers.json && \
         ! grep -q argon2 /system/settings/install/answers.json; echo \"saved: $((10 + $?))\"",
    )?;
    vm.expect_screen(&["saved: 10"])?;

    // File sharing was added: Spark started its services at this first
    // boot, and the web console's choice waits in the answers.
    vm.expect_anywhere("spark: started smbd (/system/commands/smbd) as process ")?;
    vm.expect_anywhere("spark: started nfsd (/system/commands/nfsd) as process ")?;
    answer_prompt(vm, "$", "clear; feature list | tail -1")?;
    vm.expect_screen(&["file-sharing  added      File sharing"])?;
    answer_prompt(
        vm,
        "$",
        "echo \"chose\"\": $(grep -c -e '\"profile\": \"server\"' -e '\"web_console\": true' \
         /system/settings/install/answers.json)\"",
    )?;
    vm.expect_screen(&["chose: 2"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
