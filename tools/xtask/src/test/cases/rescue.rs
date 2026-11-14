// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rescue repairs a machine whose slots are both broken, and keeps a
//! user's files and apps (ADRs 0459 and 0460). The install stick installs
//! Dukovo onto a blank disk under UEFI, as *install-uefi* does, and puts
//! Rescue on its EFI system partition. From that disk alone, alice, the
//! administrator, keeps a file in her home and installs an app for
//! herself, then adds a health check that never ends, so that no start is
//! seen through. Three starts in a row are cut short by power cuts, and
//! the menu starts Rescue by itself: networking stays off, alice signs
//! in with her password, and Rescue says the last three starts were not
//! seen through (ADR 0501), and the menu's "Find the likely cause" finds
//! none (ADR 0474). Check the network turns networking on and shows what
//! netcheck finds, until Enter is pressed (ADR 0489). From Rescue's shell
//! the health check goes and the only slot, *@system.a*, is removed; at the restart no slot is
//! left, and Rescue starts by itself again. It reinstalls Dukovo from its
//! own copy, every file checked, and the system that then starts has
//! alice's file and app as they were. On the host, after each boot, the
//! disk holds the install, with an EFI system partition `fsck.fat` finds
//! sound and a root `btrfs check` finds sound.

use super::super::*;
use super::install_uefi::{INSTALL_DISKS, PASSWORD, install, wait_for_screen};

pub const CASE: Case = Case {
    name: "rescue",
    about: "Rescue repairs a machine whose slots are both broken (ADRs 0459, 0460): after an install from the stick, three starts cut short start Rescue by itself, with networking off and an administrator's password; Check the network turns networking on and runs netcheck (ADR 0489); with the only slot removed it starts by itself again, reinstalls Dukovo from its own copy, and alice's file and app are there in the system that then starts",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: install,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: nothing, script: rescued }),
};

/// Where the installed root is in the second boot: the installed disk is
/// a SATA disk then, and the guided layout's root its fourth partition.
const ROOT: &str = "sda4";

/// How long a reinstall may take, in seconds: it copies and reads back
/// the whole system.
const REINSTALL_SECONDS: u64 = 300;

/// How long Rescue may take to turn networking on, wait for an address
/// and run netcheck, in seconds.
const NETWORK_CHECK_SECONDS: u64 = 90;

/// A health check that never ends, so that no start is seen through.
const NEVER_HEALTHY: &str =
    "[[check]]\\nname = \"wait\"\\ncommand = [\"sleep\", \"600\"]\\nrequired = true\\ntimeout = 600\\n";

/// Nothing to do on the host between the boots: the disk check has run.
fn nothing(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// Waits for the installed system's sign-in on tty1, and signs alice in.
fn alice_signs_in(vm: &mut Vm) -> Result<(), String> {
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION}")])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")
}

/// Waits for Rescue, started by the menu, and signs alice in to it as
/// the administrator. Gives where in the log Rescue started.
fn in_rescue(vm: &mut Vm) -> Result<usize, String> {
    vm.expect("spark: this is Rescue (ADR 0460): dukovo-rescue(8) runs on the consoles")?;
    let at = vm.log().len();
    vm.expect("spark: Rescue starts only the consoles, the log, the firewall and the reader")?;
    vm.expect_screen(&[&format!("Dukovo Rescue {VERSION}")])?;
    vm.expect_screen(&[&format!(", on {ROOT}.")])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_screen(&["Rescue: alice is an administrator of this computer."])?;
    vm.expect_screen(&["What would you like to do?"])?;
    Ok(at)
}

/// Chooses `choice` in Rescue's menu.
fn choose(vm: &mut Vm, choice: &str) -> Result<(), String> {
    answer_prompt(vm, "Choose 0 to 12:", choice)
}

/// The second boot, from the installed disk alone.
fn rescued(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect(&format!("spark: /system is Dukovo's root on {ROOT}"))?;
    vm.expect("sysupdate: this start of slot A (Dukovo ")?;
    alice_signs_in(vm)?;

    // A file and an app of alice's own, then a health check that never
    // ends, written through elevate.
    answer_prompt(
        vm,
        "$",
        "clear; cd; echo 'kept by Rescue' > keep.txt; bundle new org.example.notes notes > \
         /dev/null; echo echo notes > notes/bin/notes; chmod 755 notes/bin/notes; bundle pack \
         notes -o notes.dkv > /dev/null; bundle install --yes notes.dkv | tail -1",
    )?;
    vm.expect_screen(&["installed org.example.notes 0.1.0, for you"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; printf '{NEVER_HEALTHY}' > health.toml; elevate cp /users/alice/health.toml \
             /system/settings/sysupdate/health.toml"
        ),
    )?;
    answer_prompt(vm, "health.toml:", PASSWORD)?;
    answer_prompt(vm, "$", "clear; sync; echo \"kept: $((6 * 7))\"")?;
    vm.expect_screen(&["kept: 42"])?;

    // Three starts in a row cut short before the health check sees them
    // through: Rescue starts by itself at the fourth.
    // Each reaches its services, as Spark marks it (ADR 0503), but not
    // its health check, which starts last.
    vm.monitor("system_reset")?;
    for _ in 0..3 {
        vm.expect("spark: started slot a, @system.a")?;
        vm.expect("spark: started logd (")?;
        vm.monitor("system_reset")?;
    }
    let at = in_rescue(vm)?;
    // Why: the starts the menu counted (ADR 0501), and how far they got.
    vm.expect_screen(&[
        "The last 3 starts were not seen through.",
        "At least one of them reached its services",
    ])?;
    if vm.log()[at..].contains("spark: started netd") {
        return Err("Rescue started networking before anyone turned it on".into());
    }
    // Its likely cause (ADR 0474): no start failed, so no signature the
    // release ships matches what Rescue can see.
    choose(vm, "12")?;
    vm.expect_screen(&["No known cause matches what Rescue can see."])?;

    // Check the network turns networking on and shows what netcheck
    // finds, until a key is pressed (ADR 0489).
    choose(vm, "7")?;
    wait_for_screen(vm, "Press Enter to go back to the choices.", NETWORK_CHECK_SECONDS)?;
    vm.expect_screen(&["netcheck: "])?;
    answer_prompt(vm, "Press Enter to go back to the choices.", "")?;
    vm.expect_screen(&["Back in Rescue."])?;

    // Both slots broken: the health check goes, and the only slot.
    choose(vm, "8")?;
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; rm /volumes/{ROOT}/@settings/sysupdate/health.toml && subvolume delete \
             /volumes/{ROOT}/@system.a && echo \"gone: $((40 + 2))\""
        ),
    )?;
    vm.expect_screen(&["gone: 42"])?;
    answer_prompt(vm, "#", "exit")?;
    vm.expect_screen(&["Back in Rescue."])?;
    choose(vm, "10")?;

    // No slot to start: Rescue starts by itself, and reinstalls.
    in_rescue(vm)?;
    choose(vm, "1")?;
    answer_prompt(vm, "type the folder or file:", "")?;
    vm.expect_screen(&[&format!("Reinstall Dukovo {VERSION} into slot A?")])?;
    answer_prompt(vm, "Type yes to reinstall:", "yes")?;
    wait_for_screen(vm, "is back in slot A", REINSTALL_SECONDS)?;
    vm.expect_screen(&[&format!("Dukovo {VERSION} is back in slot A, every file checked")])?;
    choose(vm, "10")?;

    // The system starts as usual, with alice's file and app.
    vm.expect(&format!("spark: /system is Dukovo's root on {ROOT}"))?;
    vm.expect("spark: started slot a, @system.a")?;
    vm.expect("sysupdate: this start of slot A (Dukovo ")?;
    alice_signs_in(vm)?;
    answer_prompt(vm, "$", "clear; cat keep.txt; notes; echo \"apps: $((5 * 5))\"")?;
    vm.expect_screen(&["kept by Rescue", "notes", "apps: 25"])?;
    super::install_uefi::power_off(vm)
}
