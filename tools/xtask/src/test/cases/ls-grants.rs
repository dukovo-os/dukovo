// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *ls --grants* and reach(1) (ADR 0443), on a Dukovo root of Btrfs.
//! Alice signs in, as the owner, makes bob's account through elevate,
//! and installs Notes, which asks for her documents folder. Her plan in
//! her documents is hers alone, and no app of hers reaches it until she
//! allows Notes her documents at the trusted prompt; then Notes reaches
//! it, by that folder. Named in its access list, with leave to pass her
//! home, bob may read it, and reach says the same as *ls --grants*. Her
//! downloads Notes does not reach, its own data it does, and a file of
//! her own /tmp only she reaches, whatever its mode. Every app reads
//! /system/commands, which nothing can change; a file that is not there
//! is an error, and status 1. The power button ends it.

use super::super::*;

/// Dukovo's root on a copy of the test image, so that her files are on
/// Btrfs, which keeps access lists, and /system is read-only.
const DISKS: &[Disk] =
    &[Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) }];

/// The test.
pub const CASE: Case = Case {
    name: "ls-grants",
    about: "ls --grants and reach say who can reach a file (ADR 0443): alice's plan is hers alone; Notes reaches it once she allows it her documents at the trusted prompt; bob reaches it once named in its access list; Notes reaches its own data but not her downloads; a file of her own /tmp only she reaches; every app reads /system/commands, which nothing changes; a file not there is status 1",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled, authd and
    // consoled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: ls_grants,
    disks: DISKS,
    second: None,
};

/// The bundle's command: a script that says it ran.
const NOTES: &str = "#!/bin/sh\\necho notes ran\\n";

/// Its permissions, as its manifest writes them: her documents.
const PERMISSIONS: &str =
    r#"{"files.folder": {"need": "required", "folders": {"documents": "read-write"}}}"#;

fn ls_grants(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere("asks for passwords on the consoles")?;

    // Bob, an account of no group of hers.
    answer_prompt(vm, "$", "clear; sudo user add bob")?;
    answer_prompt(vm, "user add bob:", "queen of hearts")?;
    answer_prompt(vm, "New password:", "white rabbit")?;
    answer_prompt(vm, "The same again:", "white rabbit")?;
    vm.expect_screen(&["Made bob, number 1001, home /users/bob"])?;

    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; bundle new org.example.notes --name Notes > /dev/null; printf '{NOTES}' > \
             notes/bin/notes; chmod 755 notes/bin/notes; sed -i 's/\"permissions\": \
             {{}}/\"permissions\": {PERMISSIONS}/' notes/manifest.json; bundle pack notes -o \
             notes.dkv > /dev/null; bundle install --yes notes.dkv | grep -e yours -e installed; \
             rm -r notes notes.dkv"
        ),
    )?;
    vm.expect_screen(&[
        "Use these folders of yours: documents (read-write) (on first use)",
        "installed org.example.notes 0.1.0, for you",
    ])?;

    // Her plan: hers alone, and no app's until she allows one. `tr`
    // squeezes the columns' spaces, so the lines read the same whatever
    // the widths.
    answer_prompt(
        vm,
        "$",
        "clear; echo plan > documents/plan.txt; chmod 600 documents/plan.txt; ls --grants \
         documents/plan.txt | tr -s ' '",
    )?;
    vm.expect_screen(&[
        "documents/plan.txt (/users/alice/documents/plan.txt)",
        "alice read and write owner",
        "Administrators reach every file with elevate: alice.",
        "None of them reaches it.",
    ])?;

    // She allows Notes her documents at the trusted prompt.
    answer_prompt(vm, "$", "clear; notes; echo \"status $?\"")?;
    vm.expect_screen(&["[permission] Notes (org.example", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_anywhere(
        "bundled: alice allowed org.example.notes files.folder (documents=read-write), from now \
         on, on pts/",
    )?;
    vm.expect_screen(&["notes ran", "status 0"])?;

    // Named in its access list, and let through her home, bob may read
    // it; Notes reaches it by her documents folder.
    answer_prompt(
        vm,
        "$",
        "clear; setfacl -m u:bob:x . && setfacl -m u:bob:r documents/plan.txt && ls --grants \
         documents/plan.txt | tr -s ' '",
    )?;
    vm.expect_screen(&[
        "alice read and write owner",
        "bob read its access list",
        "Notes read and write your documents folder, allowed ",
    ])?;
    // reach says the same as ls --grants.
    answer_prompt(
        vm,
        "$",
        "clear; ls --grants documents/plan.txt > a.txt; reach documents/plan.txt > b.txt; cmp \
         a.txt b.txt && echo same; rm a.txt b.txt",
    )?;
    vm.expect_screen(&["same"])?;

    // Not her downloads, which she did not allow it; its own data.
    answer_prompt(
        vm,
        "$",
        "clear; echo x > downloads/d.txt; reach downloads/d.txt app-data/org.example.notes | tr \
         -s ' '",
    )?;
    vm.expect_screen(&["None of them reaches it.", "Notes read and write its own data"])?;

    // Her own /tmp is no one else's, whatever the mode says.
    answer_prompt(
        vm,
        "$",
        "clear; touch /tmp/t; chmod 666 /tmp/t; reach /tmp/t | tr -s ' '; echo \"everyone: \
         $(reach /tmp/t | grep -c everyone)\"",
    )?;
    vm.expect_screen(&[
        "alice read and write owner",
        "It is in alice's own /tmp, which no one else's sessions hold.",
        "everyone: 0",
    ])?;

    // The system's commands: everyone and every app may read them, and
    // nothing can change them.
    answer_prompt(vm, "$", "clear; reach /system/commands | tr -s ' '")?;
    vm.expect_screen(&[
        "everyone read",
        "every app read /system/commands, which every app sees",
        "Nothing can change it: it is on a filesystem or subvolume kept read-only.",
    ])?;

    // A file that is not there.
    answer_prompt(vm, "$", "clear; ls --grants nothing; echo \"status $?\"")?;
    vm.expect_screen(&["reach: nothing: No such file or directory", "status 1"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
