// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *ls*'s Dukovo marks (ADR 0442), on a Dukovo root of Btrfs. Alice signs
//! in, as the owner, and installs a bundle of her own, Notes, and runs it
//! once, which makes its data and temporary folders. *ls -l --app* gives
//! each of its files Notes and sealed, its data folder Notes and data, its
//! /tmp folder Notes and temp, and her own file a dash, under the title
//! App with *--header*. Unsealed by an administrator, its files say not
//! sealed, by its ID. *--subvolumes=always* marks */users* a subvolume
//! and */system* a read-only one, and not her home; at the console *ls -l*
//! marks them unasked, and into a pipe it does not. Without the flags,
//! the listing is GNU's. The power button ends it.

use super::super::*;

/// Dukovo's root on a copy of the test image, so that /users and
/// /system are its subvolumes.
const DISKS: &[Disk] =
    &[Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) }];

/// The test.
pub const CASE: Case = Case {
    name: "ls-apps",
    about: "ls -l --app names the app each file belongs to (ADR 0442): sealed for an installed bundle's files, data for its app-data, temp for its /tmp folder, - for a file of no app, not sealed once an administrator unseals it; --subvolumes marks /users a subvolume and /system a read-only one, unasked at the console and not into a pipe",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: ls_apps,
    disks: DISKS,
    second: None,
};

/// The bundle's command: a script that says it ran.
const NOTES: &str = "#!/bin/sh\\necho notes ran\\n";

fn ls_apps(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; bundle new org.example.notes --name Notes > /dev/null; printf '{NOTES}' > \
             notes/bin/notes; chmod 755 notes/bin/notes; bundle pack notes -o notes.dkv > \
             /dev/null; bundle install --yes notes.dkv | tail -1; rm -r notes notes.dkv; notes"
        ),
    )?;
    vm.expect_screen(&["installed org.example.notes 0.1.0, for you", "notes ran"])?;

    // Each of its files is the sealed app's, by its manifest's name. `tr`
    // squeezes the columns' spaces, so the lines read the same whatever
    // the sizes and times.
    answer_prompt(
        vm,
        "$",
        "clear; ls -l --app --header applications/org.example.notes | tr -s ' '",
    )?;
    vm.expect_screen(&[
        "Permissions Links Owner Group Size Modified App Name",
        "Notes sealed bin",
        "Notes sealed manifest.json",
    ])?;
    // Its data and its /tmp folder, and a file of no app beside them.
    answer_prompt(
        vm,
        "$",
        "clear; echo mine > mine.txt; ls -ld --app mine.txt app-data/org.example.notes \
         /tmp/.app-org.example.notes | tr -s ' '",
    )?;
    vm.expect_screen(&[
        " - mine.txt",
        "Notes data app-data/org.example.notes",
        "Notes temp /tmp/.app-org.example.notes",
    ])?;
    // A listing with nothing of an app has no column.
    answer_prompt(vm, "$", "clear; ls -l --app --header . | head -2 | tail -1 | tr -s ' '")?;
    vm.expect_screen(&["Permissions Links Owner Group Size Modified Name"])?;

    // Unsealed by an administrator, it is named by its ID.
    answer_prompt(
        vm,
        "$",
        "clear; sudo chmod 755 applications/org.example.notes; ls -l --app \
         applications/org.example.notes | tr -s ' '",
    )?;
    answer_prompt(vm, "chmod 755 applications/org.example.notes:", "queen of hearts")?;
    vm.expect_screen(&["org.example.notes not sealed manifest.json"])?;

    // The subvolumes, asked for; her home is a folder of /users'.
    answer_prompt(
        vm,
        "$",
        "clear; ls -ld --subvolumes=always /users /system /users/alice | tr -s ' '",
    )?;
    vm.expect_screen(&["/system [subvolume, read-only]", "/users [subvolume]"])?;
    answer_prompt(
        vm,
        "$",
        "clear; echo \"home marks: $(ls -ld --subvolumes=always /users/alice | grep -c subvolume)\"",
    )?;
    vm.expect_screen(&["home marks: 0"])?;
    // At the console unasked; into a pipe, GNU's text.
    answer_prompt(
        vm,
        "$",
        "clear; ls -ld /users; echo \"piped marks: $(ls -ld /users /system | grep -c subvolume)\"",
    )?;
    vm.expect_screen(&["/users [subvolume]", "piped marks: 0"])?;
    // Without the flags, no column.
    answer_prompt(
        vm,
        "$",
        "clear; echo \"plain: $(ls -l applications/org.example.notes | grep -c sealed)\"",
    )?;
    vm.expect_screen(&["plain: 0"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
