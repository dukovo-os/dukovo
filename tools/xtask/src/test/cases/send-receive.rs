// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Send and receive (ADR 0462), on a Dukovo root of Btrfs with a second
//! Btrfs disk at */volumes/vdb*. Alice, the owner, keeps a folder of a
//! note with a second name and an access list, numbers, a symbolic link
//! and a named pipe, and gives a subfolder to user 1001. As an
//! administrator she snapshots */users* and pipes *subvolume send* into
//! *subvolume receive* on the other disk. She changes the folder: the
//! note grows, the numbers move, the link points anew, the pipe goes and
//! a folder comes. A second snapshot goes as its changes from the first,
//! through a file, which *--dump* shows as a snapshot of the first with a
//! rename. The copy lists as the snapshot does, owners, modes, sizes,
//! times and links alike; its files are the same, the access list came
//! along, and it is read-only and received from the snapshot's UUID. The
//! same stream into a folder without its parent is refused and leaves
//! nothing. Without an administrator a send is refused, as is one of a
//! subvolume that can change, and one to the terminal. The power button
//! ends it, and Linux's `btrfs check` reads the other disk clean.

use super::super::*;

/// Dukovo's root on a copy of the test image, so that /users and
/// /snapshots are its subvolumes, and a Btrfs disk to receive into, which
/// Spark mounts at /volumes/vdb.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    btrfs_disk(Bus::Virtio, 512, BTRFS_NEW, btrfs_checked),
];

/// The test.
pub const CASE: Case = Case {
    name: "send-receive",
    about: "a snapshot of /users sent whole and then as its changes from the first is received on another Btrfs disk (ADR 0462): the copy lists as the snapshot does, with the same files, links, owners, times and access list, read-only and marked received; an incremental stream without its parent is refused and leaves nothing; a send without an administrator, of a subvolume that can change, or to the terminal is refused; Linux's btrfs check reads the disk clean",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark mounts the second disk.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: send_receive,
    disks: DISKS,
    second: None,
};

/// Alice's password, which elevate asks for.
const PASSWORD: &str = "queen of hearts";

/// The first snapshot, sent whole down a pipe.
const FIRST: &[&str] = &[
    "chown -R 1001:1001 /users/alice/work/deep",
    "snap create --name one /users",
    "subvolume send /snapshots/users/one | subvolume receive /volumes/vdb",
];

/// The second snapshot, sent as its changes through a file, then the copy
/// compared with it.
const SECOND: &[&str] = &[
    "snap create --name two /users",
    "subvolume send -p /snapshots/users/one /snapshots/users/two > /tmp/two.stream",
    "subvolume receive -f /tmp/two.stream /volumes/vdb",
    "subvolume receive --dump -f /tmp/two.stream | grep -e ^snapshot -e numbers.txt | tr -s \" \" | cut -d \" \" -f 1-2",
    "cd /snapshots/users/two && ls -lAR --time-style=+%s | grep -v ^total > /tmp/sent.txt",
    "cd /volumes/vdb/two && ls -lAR --time-style=+%s | grep -v ^total > /tmp/received.txt",
    "echo \"listed $(wc -l < /tmp/sent.txt)\"; diff /tmp/sent.txt /tmp/received.txt && echo \"the listings match\"",
    "diff -r /snapshots/users/two/alice/work /volumes/vdb/two/alice/work && echo \"the files match\"",
    "getfacl -n /volumes/vdb/two/alice/work/notes.txt | grep user:1001",
    "subvolume show /snapshots/users/two | grep -v -e Parent -e Received | grep UUID",
    "subvolume show /volumes/vdb/two | grep -e Received -e Flags",
    "touch /volumes/vdb/two/alice/new.txt",
    "mkdir /volumes/vdb/elsewhere; subvolume receive -f /tmp/two.stream /volumes/vdb/elsewhere",
    "echo \"left $(ls -A /volumes/vdb/elsewhere | wc -l)\"",
];

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

fn send_receive(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("snap: /users: made /snapshots/users/")?;

    // Her folder, then the first snapshot, sent whole down a pipe.
    answer_prompt(
        vm,
        "$",
        "clear; cd; mkdir -p work/deep; echo 'first draft' > work/notes.txt; seq 1 40000 > \
         work/numbers.txt; ln work/notes.txt work/also-notes.txt; ln -s notes.txt work/latest; \
         setfacl -m u:1001:r work/notes.txt; mkfifo work/pipe; echo made",
    )?;
    vm.expect_screen(&["made"])?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; {}; elevate sh /users/alice/first.sh", script("first.sh", FIRST)),
    )?;
    answer_prompt(vm, "first.sh:", PASSWORD)?;
    vm.expect_screen(&["/snapshots/users/one", "At subvol /snapshots/users/one", "At subvol one"])?;

    // Changes, and the second snapshot as its changes from the first.
    answer_prompt(
        vm,
        "$",
        "clear; cd work; echo 'second draft' >> notes.txt; mv numbers.txt counted.txt; \
         rm latest pipe; ln -s counted.txt latest; mkdir new; echo fresh > new/file.txt; cd; \
         echo changed",
    )?;
    vm.expect_screen(&["changed"])?;
    // Her pane typed the password a moment ago, so elevate asks for none
    // (elevate(8)).
    answer_prompt(
        vm,
        "$",
        &format!("clear; {}; elevate sh /users/alice/second.sh; true", script("second.sh", SECOND)),
    )?;
    answer_prompt_within(vm, "$", "", 300)?;
    vm.expect_screen(&[
        "At subvol /snapshots/users/two",
        "At snapshot two",
        "snapshot ./two",
        "rename ./two/alice/work/numbers.txt",
        "the listings match",
        "the files match",
        "user:1001:r--",
        "Flags:           readonly",
        "two/alice/new.txt': Read-only file system",
        "no subvolume in /volumes/vdb/elsewhere is the stream's parent; receive its parent first",
        "left 0",
    ])?;
    let screen = vm.screen()?;
    if !screen.iter().any(|line| line.starts_with("listed ") && line != "listed 0") {
        return Err("the snapshot's listing is empty".into());
    }
    let sent = uuid_after(&screen, "UUID:").ok_or("no UUID for the snapshot")?;
    let received = uuid_after(&screen, "Received UUID:").ok_or("no received UUID")?;
    if sent != received {
        return Err(format!("the copy was received from {received}, not the snapshot's {sent}"));
    }

    // Refused without an administrator, of what can change, and to the
    // terminal.
    answer_prompt(
        vm,
        "$",
        "clear; subvolume send /snapshots/users/two > /dev/null; subvolume send /users > \
         /dev/null; subvolume send /snapshots/users/two; true",
    )?;
    vm.expect_screen(&[
        "/snapshots/users/two: Operation not permitted",
        "subvolume /users is not read-only; send a read-only snapshot of it",
        "not writing a send stream to a terminal: redirect it, or name a file with -f",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
