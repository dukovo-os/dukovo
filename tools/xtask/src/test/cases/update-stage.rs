// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update staged, tried, proved and rolled back on an installed
//! system's disk, under UEFI (ADRs 0451 to 0454). *sysupdate stage*
//! builds a signed test update, this build under a newer version, into
//! slot B from a FAT disk that holds only the files it changes, while
//! slot A runs. At the restart GRUB takes one of the update's three
//! tries and starts slot B; the health check finds the essential
//! services running for 10 seconds and makes it the default, with slot
//! A the previous version, and *sysupdate verify* reads it back against
//! the signed list. A setting changed there, the rollback *sysupdate
//! rollback --settings-from-before* then makes slot A the default again,
//! and it starts at the next restart with the settings from before the
//! update, the changed ones kept aside (ADR 0454).

use super::super::*;
use super::crash_dump::answer_within;

pub const CASE: Case = Case {
    name: "update-stage",
    about: "an update is staged into slot B while slot A runs, starts at the restart as a try, proves itself to the health check and becomes the default, and reads back as its signed list says; a rollback with the settings from before the update then starts slot A again, a setting changed since kept aside (ADRs 0451 to 0454)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_stage,
    disks: UPDATE_DISKS,
    second: None,
};

/// The update tests' disk: the test updates, on a virtio disk Spark
/// mounts at [`UPDATES`].
pub const UPDATE_DISKS: &[Disk] = &[update_releases_disk(64 * 1024, "UPDATES")];

/// Where Spark mounts the test updates' disk.
pub const UPDATES: &str = "/volumes/vda";

/// How long a command typed at tty1 may take, in seconds: a stage reads
/// the whole slot twice.
const COMMAND_SECONDS: u64 = 300;

/// Waits for an installed system that started `slot` (`a` or `b`) to show
/// tty1.
pub fn started(vm: &mut Vm, slot: char) -> Result<(), String> {
    vm.expect(&format!("spark: started slot {slot}, @system.{slot}"))?;
    vm.expect("; tty1 is shown")?;
    Ok(())
}

/// Types `command` at tty1's prompt, on a cleared screen, once the last
/// command has ended. After a kernel panic, as update-panic's tries end,
/// alertd's notification of it may show on the bottom line, below the
/// prompt: it is taken off as crash-dump takes it off.
pub fn type_command(vm: &mut Vm, command: &str) -> Result<(), String> {
    answer_within(vm, "#", &format!("clear; {command}"), COMMAND_SECONDS)
}

/// Types `command`, waits for it to end, and checks that the screen then
/// shows each of `shown`.
pub fn run(vm: &mut Vm, command: &str, shown: &[&str]) -> Result<(), String> {
    type_command(vm, command)?;
    answer_within(vm, "#", "", COMMAND_SECONDS)?;
    vm.expect_screen(shown).map_err(|err| format!("`{command}`: {err}"))
}

/// Stages the test update `name` from the updates' disk, with the health
/// check's wait for a try made 10 seconds, as a test can wait for.
pub fn stage(vm: &mut Vm, name: &str) -> Result<(), String> {
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {UPDATES}"))?;
    run(
        vm,
        "printf 'stable = 10\\n' > /system/settings/sysupdate/health.toml && echo \"short: $((6 * 7))\"",
        &["short: 42"],
    )?;
    let version = test_update_version(name);
    run(
        vm,
        &format!("sysupdate stage {UPDATES}/{name}"),
        &[&format!("sysupdate: staged Dukovo {version} in slot B: it starts at the next restart")],
    )
}

/// Restarts the machine from tty1.
pub fn restart(vm: &mut Vm) -> Result<(), String> {
    type_command(vm, "reboot")?;
    vm.expect("spark: asked to restart the machine")
}

/// Powers the machine off from tty1.
pub fn power_off(vm: &mut Vm) -> Result<(), String> {
    type_command(vm, "poweroff")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Stage, try, prove, and roll back.
fn update_stage(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("good");
    started(vm, 'a')?;
    stage(vm, "good")?;
    run(
        vm,
        "sysupdate status",
        &[
            &format!("Running:  slot A, Dukovo {VERSION}"),
            &format!("Slot B:   Dukovo {version}, an update being tried: 3 tries left"),
            "Next start: slot B",
        ],
    )?;

    // The try: no menu, as the last start was seen through.
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) is an update being tried: it proves itself when \
         its essential services run for 10 seconds"
    ))?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) proved itself: it is now the default, and slot A \
         (Dukovo {VERSION}) the previous version"
    ))?;
    run(
        vm,
        "sysupdate status",
        &[
            &format!("Running:  slot B, Dukovo {version}"),
            &format!("Slot A:   Dukovo {VERSION}, the previous version"),
            &format!("Slot B:   Dukovo {version}, the default"),
            "Next start: slot B",
            "Last health check: passed",
        ],
    )?;
    // The slot read back against the signed list (ADR 0451).
    run(vm, &format!("sysupdate verify {UPDATES}/good"), &[&format!(
        "sysupdate: slot B (Dukovo {version}) is the release its signed list names"
    )])?;

    // Back to the version before, with the settings from before the
    // update: one changed since is kept aside.
    run(
        vm,
        "echo changed > /system/settings/update-test && sysupdate rollback \
         --settings-from-before",
        &[
            &format!(
                "sysupdate: slot A (Dukovo {VERSION}) starts from the next restart on; restart to \
                 go back to it"
            ),
            "sysupdate: the settings from before the update come back at that start; today's are",
        ],
    )?;
    restart(vm)?;
    vm.expect(
        "spark: the rollback asked for the settings from before the update in slot b: putting \
         them back",
    )?;
    started(vm, 'a')?;
    run(
        vm,
        "[ -e /system/settings/update-test ] || echo \"taken back: $((8 * 8))\"; cat \
         /volumes/*/@snapshots/update/settings-failed/update-test",
        &["taken back: 64", "changed"],
    )?;
    run(
        vm,
        "sysupdate status",
        &[
            &format!("Running:  slot A, Dukovo {VERSION}"),
            &format!("Slot A:   Dukovo {VERSION}, the default"),
            &format!("Slot B:   Dukovo {version}, the previous version"),
            "Next start: slot A",
        ],
    )?;
    power_off(vm)
}
