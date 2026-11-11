// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Snapshots (ADR 0461), on a Dukovo root of Btrfs. The snap service
//! makes the day's snapshots of */users*, */shared* and
//! */system/settings* as it starts, which */snapshots* shows read-only and
//! *ls -l* marks. Alice, the owner, keeps a note, and as an administrator
//! makes bob's private folder and a snapshot named *before-delete*. She
//! deletes the note and *snap restore* brings it back, as herself; it
//! will not write over it, and *--to* restores it beside it. A write into
//! the snapshot fails, as her and as root through */volumes*; bob's
//! folder in it stays unreadable to her; *subvolume show* gives the
//! snapshot */users*' UUID as its parent. An app she installs sees no
//! */snapshots* and no */volumes*. Then, with the clock moved through 40
//! days and a round of *snap auto* on each, exactly the retention set is
//! left: the seven latest days, the Sundays of the two weeks before them,
//! and the named one. The power button ends it.

use super::super::*;

/// Dukovo's root on a copy of the test image, so that /users and
/// /snapshots are its subvolumes.
const DISKS: &[Disk] =
    &[Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) }];

/// The test.
pub const CASE: Case = Case {
    name: "snapshots",
    about: "the snap service makes the day's snapshots, which /snapshots shows read-only (ADR 0461); a deleted file comes back with snap restore without an administrator, a write into a snapshot fails, another's folder in it stays unreadable, an app sees no snapshot, and 40 days of snap auto leave exactly the retention set",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, snap among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: snapshots,
    disks: DISKS,
    second: None,
};

/// Alice's password, which elevate asks for.
const PASSWORD: &str = "queen of hearts";

/// What an administrator does first: bob's private folder, then a
/// snapshot of /users with a name, which retention keeps.
const FIRST: &[&str] = &[
    "mkdir -m 700 /users/bob",
    "echo secret > /users/bob/diary.txt",
    "chown -R 1001:1001 /users/bob",
    "snap create --name before-delete /users",
];

/// Forty days of the service's rounds, with only /users to snapshot:
/// 2030-01-01 to 2030-02-09, a Saturday, at noon. Then a write into the
/// snapshot through the top of the disk, which is read-only too.
const DAYS: &[&str] = &[
    "service timed stop > /dev/null",
    "service snap stop > /dev/null",
    "cp /users/alice/snap.toml /system/settings/snap.toml",
    "for day in $(seq -w 1 31); do timedatectl set-time \"2030-01-$day 12:00:00\"; snap auto >> /tmp/auto.log; done",
    "for day in $(seq 1 9); do timedatectl set-time \"2030-02-0$day 12:00:00\"; snap auto >> /tmp/auto.log; done",
    "echo \"made $(grep -c made /tmp/auto.log), removed $(grep -c removed /tmp/auto.log)\"",
    "for top in /volumes/*/@snapshots; do touch $top/users/before-delete/alice/new.txt; done",
];

/// The app: a script that looks for the snapshots.
const PEEK: &str = "#!/bin/sh\\nls /snapshots /volumes 2>&1\\necho peeked\\n";

/// A shell command that writes `lines` to `file`, one a line.
fn script(file: &str, lines: &[&str]) -> String {
    let quoted: Vec<String> = lines.iter().map(|line| format!("'{line}'")).collect();
    format!("printf '%s\\n' {} > {file}", quoted.join(" "))
}

/// The UUID after `field` on the screen, as `subvolume show` writes it.
fn uuid_after(screen: &[String], field: &str) -> Option<String> {
    screen.iter().find_map(|line| {
        let rest = line.trim_start().strip_prefix(field)?;
        Some(rest.split_whitespace().next()?.to_string())
    })
}

fn snapshots(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    // The service's first round, at boot.
    vm.expect_anywhere("snap: /users: made /snapshots/users/")?;
    vm.expect_anywhere("snap: /system/settings: made /snapshots/settings/")?;
    answer_prompt(
        vm,
        "$",
        "clear; ls -ld --subvolumes=always /snapshots /snapshots/users/* | tr -s ' '",
    )?;
    vm.expect_screen(&["/snapshots [subvolume, read-only]", "[snapshot, read-only]"])?;

    // A note, bob's folder and a named snapshot.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; echo 'meeting at noon' > notes.txt; {}; elevate sh /users/alice/first.sh",
            script("first.sh", FIRST)
        ),
    )?;
    answer_prompt(vm, "first.sh:", PASSWORD)?;
    vm.expect_screen(&["/snapshots/users/before-delete"])?;

    // Deleted, it comes back without an administrator; it is not written
    // over, and comes back beside itself too.
    answer_prompt(vm, "$", "clear; rm notes.txt; snap restore notes.txt; cat notes.txt")?;
    vm.expect_screen(&[
        "Restored /users/alice/notes.txt from /snapshots/users/before-delete/alice/notes.txt",
        "meeting at noon",
    ])?;
    answer_prompt(
        vm,
        "$",
        "clear; snap restore notes.txt; echo \"status $?\"; snap restore --to old.txt notes.txt; \
         cat old.txt",
    )?;
    vm.expect_screen(&[
        "/users/alice/notes.txt: is there; restore it beside it with --to",
        "status 1",
        "Restored /users/alice/old.txt",
        "meeting at noon",
    ])?;

    // Nothing in it can be changed, and bob's folder is still his.
    answer_prompt(
        vm,
        "$",
        "clear; touch /snapshots/users/before-delete/alice/new.txt; \
         cat /snapshots/users/before-delete/bob/diary.txt; true",
    )?;
    vm.expect_screen(&["Read-only file system", "Permission denied"])?;
    // It is a snapshot of /users.
    answer_prompt(
        vm,
        "$",
        "clear; subvolume show /users | grep -v -e Parent -e Received | grep UUID; \
         subvolume show /snapshots/users/before-delete",
    )?;
    vm.expect_screen(&["Name:            before-delete", "Flags:           readonly"])?;
    let screen = vm.screen()?;
    let users = uuid_after(&screen, "UUID:").ok_or("no UUID for /users")?;
    let parent = uuid_after(&screen, "Parent UUID:").ok_or("no parent UUID")?;
    if users != parent {
        return Err(format!("the snapshot's parent is {parent}, not /users' {users}"));
    }

    // An app sees no snapshot.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; bundle new org.example.peek --name Peek > /dev/null; printf '{PEEK}' > \
             peek/bin/peek; chmod 755 peek/bin/peek; bundle pack peek -o peek.dkv > /dev/null; \
             bundle install --yes peek.dkv | tail -1; rm -r peek peek.dkv; peek"
        ),
    )?;
    vm.expect_screen(&[
        "installed org.example.peek 0.1.0, for you",
        "cannot access '/snapshots': No such file or directory",
        "cannot access '/volumes': No such file or directory",
        "peeked",
    ])?;

    // Forty days of rounds leave exactly the retention set.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; echo 'places = [\"/users\"]' > snap.toml; {}; elevate sh /users/alice/days.sh; true",
            script("days.sh", DAYS)
        ),
    )?;
    // Her pane typed the password minutes ago, so elevate asks for none
    // (elevate(8)). Enter at the prompt once the rounds are done, leaving
    // what they said on the screen.
    answer_prompt_within(vm, "$", "", 1500)?;
    vm.expect_screen(&["made 40, removed 32", "new.txt': Read-only file system"])?;
    answer_prompt(vm, "$", "clear; ls /snapshots/users | tr '\\n' ' '; echo")?;
    vm.expect_screen(&[
        "2030-01-20-1200 2030-01-27-1200 2030-02-03-1200 2030-02-04-1200 2030-02-05-1200 \
         2030-02-06-1200 2030-02-07-1200 2030-02-08-1200 2030-02-09-1200 before-delete",
    ])?;
    answer_prompt(vm, "$", "clear; snap list /users | tr -s ' '")?;
    vm.expect_screen(&[
        "NAME MADE KEPT",
        " named",
        "2030-01-20-1200 2030-01-20 12:00 weekly",
        "2030-02-04-1200 2030-02-04 12:00 daily",
        "2030-02-09-1200 2030-02-09 12:00 newest, daily, weekly",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
