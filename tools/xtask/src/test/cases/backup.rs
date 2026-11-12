// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Backups (ADRs 0463 and 0464), on a Dukovo root of Btrfs with an exFAT
//! disk at */volumes/vdb*. Alice, the owner, keeps a folder of notes and
//! numbers, and as an administrator starts a service whose backup hooks
//! write a file in */shared*, turns sshd on and lets the machine's backup
//! key sign in as her, then adds three destinations: the exFAT disk, her
//! own account over SSH on the loopback, and a folder that is then removed,
//! as a disk unplugged. A round to the disk and the server sends each
//! place, */applications* among them, whole, with the hooks run around the
//! snapshots, and the machine's record; she changes her folder, and a
//! second round sends the changes. *backup show* lists two backups in each
//! place's chain, and only the newest round's snapshot is kept. The disk's
//! backups are read through and restored, each found as the snapshot it was
//! made from, and the server's are read through. With her note deleted, the
//! server's newest backup of */users* is restored and *snap restore* brings
//! the note back from it as herself; */shared*'s, restored from the disk's
//! folder with the recovery key alone, holds what the before hook wrote and
//! not what the after hook wrote. A round of the service passes the
//! unplugged one over; asked for it, it fails, which *backup list* shows
//! her. Without an administrator a run, the key and the disk's contents are
//! refused. A changed byte in a backup is refused by *backup verify*.
//! Removing the destinations removes their snapshots. The power button ends
//! it, and exfatprogs' `fsck.exfat` reads the disk clean.

use super::super::*;

/// Dukovo's root on a copy of the test image, so that /users, /shared,
/// /applications and /system/settings are its subvolumes, and an exFAT
/// disk to back up to, which Spark mounts at /volumes/vdb.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    exfat_disk(128 * 1024, 4096, "BACKUP", Bus::Virtio),
];

/// The test.
pub const CASE: Case = Case {
    name: "backup",
    about: "backups of /users, /shared, /system/settings and /applications, with the machine's record, to an exFAT disk and over SSH (ADRs 0463 and 0464): whole, then the changes, with the services' hooks run around the snapshots; read through and restored as they were; a note restored from the server's backup with snap restore, and /shared's from the disk with the recovery key alone, as the before hook left it; an unplugged disk passed over by the service; a run, the key and the contents refused without an administrator; a changed byte refused; fsck.exfat reads the disk clean",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark mounts the disk and starts the services,
    // backup and sshd among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: backup,
    disks: DISKS,
    second: None,
};

/// Alice's password, which elevate asks for.
const PASSWORD: &str = "queen of hearts";

/// A service whose backup hooks write a file in /shared: `flushed` before
/// the snapshots, and `resumed` after them.
const HOOKED: &[&str] = &[
    "service_version = 1",
    "description = \"Keeps a file in step with backups\"",
    "program = \"/system/commands/sleep\"",
    "arguments = [\"1000000\"]",
    "[permissions]",
    "files.read-write = [\"/shared/hooked\"]",
    "[backup]",
    "before = [\"/system/commands/sh\", \"-c\", \"echo flushed > /shared/hooked/state.txt\"]",
    "after = [\"/system/commands/sh\", \"-c\", \"echo resumed >> /shared/hooked/state.txt\"]",
];

/// What an administrator does first: the service, sshd with the machine's
/// key let in as alice, and the three destinations.
const SETUP: &[&str] = &[
    "mkdir -m 777 /shared/hooked",
    "cp /users/alice/hooked.toml /system/settings/spark/services/hooked.toml",
    "service hooked start",
    "service sshd enable",
    "backup ssh-key > /users/alice/.ssh/authorized_keys",
    "chown 1000 /users/alice/.ssh/authorized_keys",
    "backup add stick /volumes/vdb | grep -e Added -e AGE-SECRET-KEY-1 | cut -c 1-40",
    "backup add --every hand --verify never server ssh://alice@127.0.0.1/tmp/server | head -1",
    "mkdir /tmp/gone && backup add gone /tmp/gone > /dev/null && rm -r /tmp/gone && echo gone",
];

/// The second round, then what the disk holds.
const SECOND: &[&str] = &[
    "backup run stick server | grep -e changes -e done",
    "backup show stick",
    "echo \"kept $(ls /snapshots/users | grep -c backup-)\"",
];

/// Both destinations checked.
const CHECK: &[&str] = &["backup verify --restore stick", "backup verify server"];

/// /users restored from the server, and /shared from the disk's folder
/// with the recovery key alone.
const RESTORE: &[&str] = &[
    "backup restore --place /users server",
    "backup key stick > /tmp/recovery.key",
    "backup restore --key /tmp/recovery.key --place shared /volumes/vdb/dukovo-backup/*",
];

/// A round of the service, which passes the unplugged disk over, then a
/// run that asks for it.
const UNPLUGGED: &[&str] = &["backup auto; echo \"auto $?\"", "backup run gone; echo \"run $?\""];

/// A changed byte in the newest backup of /users on the disk, then the
/// destinations removed.
const TAMPER: &[&str] = &[
    "f=$(ls /volumes/vdb/dukovo-backup/*/users/*/*.age | tail -1); s=$(wc -c < $f)",
    "printf XY | dd of=$f bs=1 seek=$((s - 20)) conv=notrunc 2> /dev/null",
    "backup verify stick; echo \"verify $?\"",
    "for d in gone server stick; do backup remove $d > /dev/null; done",
    "echo \"left $(ls /snapshots/users | grep -c backup-)\"",
];

/// A shell command that writes `lines` to `file`, one a line.
pub(super) fn script(file: &str, lines: &[&str]) -> String {
    let quoted: Vec<String> = lines.iter().map(|line| format!("'{line}'")).collect();
    format!("printf '%s\\n' {} > {file}", quoted.join(" "))
}

/// Writes `lines` to alice's `name` and runs it as an administrator; her
/// pane typed the password at the first, so elevate asks for none after
/// it (elevate(8)).
pub(super) fn elevated(vm: &mut Vm, name: &str, lines: &[&str], first: bool) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!("clear; {}; elevate sh /users/alice/{name}; true", script(name, lines)),
    )?;
    if first {
        answer_prompt(vm, &format!("{name}:"), PASSWORD)?;
    }
    answer_prompt_within(vm, "$", "", 300)
}

fn backup(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("snap: /users: made /snapshots/users/")?;

    // Her folder, and the service's file.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; mkdir -p work .ssh; chmod 700 .ssh; echo 'first draft' > work/notes.txt; \
             seq 1 40000 > work/numbers.txt; ln -s notes.txt work/latest; {}; echo made",
            script("hooked.toml", HOOKED)
        ),
    )?;
    vm.expect_screen(&["made"])?;
    elevated(vm, "setup.sh", SETUP, true)?;
    vm.expect_screen(&[
        "sshd is running, and starts at each boot.",
        "Added stick: /volumes/vdb/dukovo-backup/",
        "    AGE-SECRET-KEY-1",
        "Added server: ssh://alice@127.0.0.1/tmp/server/dukovo-backup/",
        "gone",
    ])?;

    // The first round: every place whole, the hooks around the snapshots.
    answer_prompt(vm, "$", "clear; elevate backup run stick server; cat /shared/hooked/state.txt")?;
    answer_prompt_within(vm, "$", "", 300)?;
    vm.expect_screen(&[
        "backup: hook ok: hooked",
        "backup: stick: /users: ",
        " (whole)",
        "backup: stick: done, to /volumes/vdb/dukovo-backup/",
        "backup: server: /system/settings: ",
        "backup: server: done, to ssh://alice@127.0.0.1/tmp/server/dukovo-backup/",
        "flushed",
        "resumed",
    ])?;

    // Changes, and the second round as the changes from the first.
    answer_prompt(
        vm,
        "$",
        "clear; cd work; echo 'second draft' >> notes.txt; rm numbers.txt; echo fresh > \
         new.txt; cd; echo changed",
    )?;
    vm.expect_screen(&["changed"])?;
    elevated(vm, "second.sh", SECOND, false)?;
    vm.expect_screen(&[
        "backup: stick: /users: ",
        " (changes)",
        "backup: stick: done",
        "backup: server: done",
        "  applications:",
        "  settings:",
        "  shared:",
        "  users:",
        "kept 1",
    ])?;
    let screen = vm.screen()?;
    let changes = screen.iter().filter(|line| line.ends_with("(changes)")).count();
    if changes != 8 {
        return Err(format!("{changes} backups of changes in the second round, not 8"));
    }
    // Each place's chain: the whole backup and the changes, in one line.
    let chains =
        screen.iter().filter(|line| line.starts_with("    ") && line.contains(", ")).count();
    if chains != 4 {
        return Err(format!("{chains} chains of two backups on the disk, not 4"));
    }

    elevated(vm, "check.sh", CHECK, false)?;
    vm.expect_screen(&[
        "backup: stick: read 8 backups and 2 records through, every chain whole",
        "backup: stick: /applications: restored the backup of ",
        "backup: stick: /users: restored the backup of ",
        "backup: stick: /shared: restored the backup of ",
        "backup: stick: /system/settings: restored the backup of ",
        "files and folders as they were",
        "backup: server: read 8 backups and 2 records through, every chain whole",
    ])?;

    // Her note, deleted, comes back from the server's backup.
    answer_prompt(vm, "$", "clear; rm work/notes.txt; echo deleted")?;
    vm.expect_screen(&["deleted"])?;
    elevated(vm, "restore.sh", RESTORE, false)?;
    vm.expect_screen(&[
        "Restored the backup of users at ",
        " as /snapshots/users/restored-",
        "Restored the backup of shared at ",
        " as /snapshots/shared/restored-",
    ])?;
    answer_prompt(
        vm,
        "$",
        "clear; snap restore --from $(ls /snapshots/users | grep restored-) work/notes.txt; cat \
         work/notes.txt; state=$(echo /snapshots/shared/restored-*/hooked/state.txt); echo \
         \"restored $(grep -c flushed $state) $(grep -c resumed $state), live $(grep -c resumed \
         /shared/hooked/state.txt)\"",
    )?;
    vm.expect_screen(&["first draft", "second draft", "restored 1 0, live 1"])?;

    // The unplugged disk: passed over by the service's round, refused
    // when asked for; she sees it in the list.
    elevated(vm, "unplugged.sh", UNPLUGGED, false)?;
    vm.expect_screen(&[
        "auto 0",
        "backup: gone: /tmp/gone: not there; is the disk plugged in?; not backed up",
        "run 1",
    ])?;
    let screen = vm.screen()?;
    if screen
        .iter()
        .any(|line| line.starts_with("backup: stick") || line.starts_with("backup: server"))
    {
        return Err("the service's round backed up a destination that was not due".into());
    }
    answer_prompt(
        vm,
        "$",
        "clear; backup list | tr -s ' '; backup run stick; backup key stick; backup show stick; \
         true",
    )?;
    vm.expect_screen(&[
        "NAME TO EVERY LAST BACKUP RESULT CHECKED",
        "gone /tmp/gone day - failed: /tmp/gone: not there",
        "server ssh://alice@127.0.0.1/tmp/server hand ",
        "stick /volumes/vdb day ",
        "backup run needs an administrator's session (elevate)",
        "backup key needs an administrator's session (elevate)",
        "backup show needs an administrator's session (elevate)",
    ])?;

    // A changed byte is refused; removing the destinations removes their
    // snapshots.
    elevated(vm, "tamper.sh", TAMPER, false)?;
    vm.expect_screen(&[": its tag does not match", "verify 1", "left 0"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
