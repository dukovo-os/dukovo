// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A whole machine restored from its backups (ADR 0464), on a Dukovo root
//! of Btrfs with an exFAT disk at */volumes/vdb*. Alice, the owner, keeps a
//! note and a secret in her keyring, installs Jot for herself and gives
//! it her keyring after the trusted prompt's Y, and makes Hello, which she
//! installs for everyone as an administrator. She names the machine
//! *atlas*, adds the disk as a destination, keeps its recovery key on it,
//! and backs the machine up: each place, */applications* among them, and
//! the machine's record. Then the machine is changed past knowing: Hello
//! and Jot removed, her note deleted, a file left in */shared*, and the
//! machine named *other*. A restore with another destination's key is
//! refused, as is the record with a changed tag. With the recovery key,
//! *backup restore --machine* stages the machine as it was, and the power
//! button ends the first boot. At the second, Spark puts the restored
//! places in place before any service starts: the machine is *atlas*
//! again, her note is back and the left file gone, kept with what it
//! replaced in */snapshots/machine-restore*, Hello runs for her, and Jot
//! reads her secret without asking again. The power button ends it.

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
    name: "machine-restore",
    about: "a whole machine restored from its backups (ADR 0464): backed up with its apps and record to an exFAT disk, then changed; restores with another key or a changed tag refused; staged with the recovery key and put in place by Spark at the next start: its name, a note, the apps for everyone and for her, and her app's grant back without asking, what it replaced kept",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark mounts the disk and starts the services,
    // bundled, authd and consoled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: before_the_restore,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: "lockdep", before: nothing_between, script: restored }),
};

/// Alice's password, which elevate asks for.
const PASSWORD: &str = "queen of hearts";

/// Jot's command, run in its sandbox: reads alice's secret, which it
/// reaches only with keyring.shared.
const JOT: &str = "#!/bin/sh\\nkeyring get bottle; echo \"bottle $?\"\\n";

/// Hello's command.
const HELLO: &str = "#!/bin/sh\\necho hello from everyone\\n";

/// Jot's permissions, as its manifest writes them.
const PERMISSIONS: &str = r#"{"keyring.shared": {"need": "optional"}}"#;

/// What an administrator does first: the machine's name, Hello for
/// everyone, the disk as a destination with its recovery key kept on it,
/// another destination whose key is not the disk's, and a backup.
const SETUP: &[&str] = &[
    "hostname atlas",
    "bundle install --for-everyone --yes /users/alice/hello.dkv | tail -1",
    "backup add stick /volumes/vdb | head -1",
    "backup key stick > /volumes/vdb/recovery.key",
    "mkdir /tmp/other && backup add other /tmp/other > /dev/null",
    "backup key other > /tmp/other.key",
    "backup run stick | grep -e applications -e done",
];

/// The machine changed past knowing.
const CHANGE: &[&str] = &[
    "bundle remove --for-everyone --purge org.example.hello > /dev/null; echo removed $?",
    "rm -r /users/alice/work",
    "echo left > /shared/left.txt",
    "hostname other",
];

/// The restores refused, then the one staged.
const RESTORE: &[&str] = &[
    "d=/volumes/vdb/dukovo-backup/atlas; k=/volumes/vdb/recovery.key",
    "backup restore --machine --key /tmp/other.key $d; echo \"other key $?\"",
    "t=$(ls $d/machine/*.tag); cp $t /tmp/tag; sed -i y/0123456789abcdef/123456789abcdef0/ $t",
    "backup restore --machine --key $k $d; echo \"changed tag $?\"; cp /tmp/tag $t",
    "backup restore --machine --key $k $d | grep -e staged -e Staged -e Restart",
];

/// A shell command that writes `lines` to `file`, one a line.
fn script(file: &str, lines: &[&str]) -> String {
    let quoted: Vec<String> = lines.iter().map(|line| format!("'{line}'")).collect();
    format!("printf '%s\\n' {} > {file}", quoted.join(" "))
}

/// Writes `lines` to alice's `name` and runs it as an administrator; her
/// pane typed the password at the first, so elevate asks for none after
/// it (elevate(8)).
fn elevated(vm: &mut Vm, name: &str, lines: &[&str], first: bool) -> Result<(), String> {
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

fn before_the_restore(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere("authd: keyring of alice: open")?;

    // Her note and secret, Jot for herself, and Hello to install for
    // everyone.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; mkdir work; echo 'first draft' > work/notes.txt; echo drink me | keyring \
             set bottle; bundle new org.example.jot --name Jot > /dev/null; printf '{JOT}' > \
             jot/bin/jot; chmod 755 jot/bin/jot; sed -i 's/\"permissions\": {{}}/\"permissions\": \
             {PERMISSIONS}/' jot/manifest.json; bundle pack jot -o jot.dkv > /dev/null; bundle \
             install --yes jot.dkv | tail -1"
        ),
    )?;
    vm.expect_screen(&["installed org.example.jot 0.1.0, for you"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; bundle new org.example.hello --name Hello > /dev/null; printf '{HELLO}' > \
             hello/bin/hello; chmod 755 hello/bin/hello; bundle pack hello -o hello.dkv > \
             /dev/null; rm -r jot jot.dkv hello; echo made"
        ),
    )?;
    vm.expect_screen(&["made"])?;
    // Jot is given her keyring after the prompt's Y.
    answer_prompt(vm, "$", "clear; bundle grant org.example.jot keyring.shared; echo \"granted $?\"")?;
    vm.expect_screen(&["Give Jot (org.example", "Y: yes"])?;
    vm.type_text("y")?;
    vm.expect_screen(&["gave org.example.jot keyring.shared", "granted 0"])?;
    answer_prompt(vm, "$", "clear; jot")?;
    vm.expect_screen(&["drink me", "bottle 0"])?;

    // Named atlas, Hello for everyone, and the backup, /applications and
    // the record with the rest.
    elevated(vm, "setup.sh", SETUP, true)?;
    vm.expect_screen(&[
        "installed org.example.hello 0.1.0, for everyone",
        "Added stick: /volumes/vdb/dukovo-backup/atlas",
        "backup: stick: /applications: ",
        " (whole)",
        "backup: stick: done, to /volumes/vdb/dukovo-backup/atlas",
    ])?;
    answer_prompt(
        vm,
        "$",
        "clear; hello; ls /volumes/vdb/dukovo-backup/atlas/machine | sed 's/^[0-9-]*/T/'",
    )?;
    vm.expect_screen(&["hello from everyone", "T.age", "T.tag"])?;

    // Changed past knowing: Hello and Jot gone, her note, a file left in
    // /shared, another name.
    answer_prompt(vm, "$", "clear; bundle remove --purge org.example.jot > /dev/null; echo \"jot $?\"")?;
    vm.expect_screen(&["jot 0"])?;
    elevated(vm, "change.sh", CHANGE, false)?;
    vm.expect_screen(&["removed 0"])?;
    answer_prompt(vm, "$", "clear; hostname; ls /applications | grep -c example; ls work; true")?;
    vm.expect_screen(&["other", "0", "No such file or directory"])?;

    // Another key, or a changed tag, is refused; the recovery key stages.
    elevated(vm, "restore.sh", RESTORE, false)?;
    vm.expect_screen(&[
        "is not the destination's own key, the recovery key",
        "other key 1",
        "its tag does not match",
        "changed tag 1",
        "/applications: staged its backup of ",
        "/users: staged its backup of ",
        "Staged atlas as it was at ",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

fn restored(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("spark: restoring the machine atlas from its backup of ")?;
    vm.expect_anywhere("spark: the machine is as it was at ")?;
    vm.expect_anywhere("bundled: 2 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_anywhere("authd: keyring of alice: open")?;
    answer_prompt(
        vm,
        "$",
        "clear; hostname; cat work/notes.txt; ls /shared/left.txt; cat \
         /snapshots/machine-restore/before-*/shared/left.txt; ls \
         /snapshots/machine-restore | grep -c staged; true",
    )?;
    vm.expect_screen(&["atlas", "first draft", "No such file or directory", "left", "0"])?;
    // Hello for everyone, and Jot with her keyring, asking nothing.
    answer_prompt(vm, "$", "clear; hello; jot; bundle list | grep -c example")?;
    vm.expect_screen(&["hello from everyone", "drink me", "bottle 0", "2"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
