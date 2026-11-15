// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update staged onto a root with no room (ADRs 0451 and 0536). A
//! file written from `/devices/zero` into `/system/state`, as root at
//! the console, takes the root's free space. *sysupdate stage* adds up
//! the files the update changes and a margin for Btrfs's metadata before
//! it writes anything, and refuses with "the update needs N MiB on the
//! Dukovo root, and M MiB are free"; *sysupdate status* is as it was, and
//! no slot was built. With the file removed, the stage goes through, the
//! update starts at the restart, and proves itself to the health check.

use super::super::*;
use super::crash_dump::answer_within;
use super::update_stage::{
    UPDATE_DISKS, UPDATES, power_off, restart, run, stage, started, type_command,
};

pub const CASE: Case = Case {
    name: "update-disk-full",
    about: "an update staged onto a root whose free space is used up is refused before anything is written, saying what it needs and what is free, and the slot state is as it was; with the space given back, the stage succeeds and the update starts and proves itself (ADRs 0451 and 0536)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_disk_full,
    disks: UPDATE_DISKS,
    second: None,
};

/// The file that takes the room.
const FILLER: &str = "/system/state/filler";

/// More than the test image's root can hold, so that the write ends with
/// no space left.
const FILL_BYTES: u64 = 2_000_000_000;

/// How long the filler's write may take, in seconds.
const FILL_SECONDS: u64 = 600;

/// Fill the root, be refused, make room, and stage.
fn update_disk_full(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("good");
    started(vm, 'a')?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {UPDATES}"))?;
    run(vm, "sysupdate status > /tmp/before && echo \"short: $((6 * 7))\"", &["short: 42"])?;

    // The write ends when the root has no room left for it.
    type_command(
        vm,
        &format!("head -c {FILL_BYTES} /devices/zero > {FILLER}; sync; df -h /system/state"),
    )?;
    answer_within(vm, "#", "", FILL_SECONDS)?;

    run(
        vm,
        &format!("sysupdate stage {UPDATES}/good; echo \"stage $?\""),
        &["the update needs ", " MiB on the Dukovo root, and ", " MiB are free", "stage 1"],
    )?;
    run(
        vm,
        "sysupdate status > /tmp/after; cmp /tmp/before /tmp/after && echo \"unchanged: $((8 * 8))\"",
        &["unchanged: 64"],
    )?;

    run(vm, &format!("rm {FILLER}; sync; echo \"room: $((9 * 9))\""), &["room: 81"])?;
    stage(vm, "good")?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) proved itself: it is now the default, and slot A \
         (Dukovo {VERSION}) the previous version"
    ))?;
    run(
        vm,
        "sysupdate status",
        &[
            &format!("Running:  slot B, Dukovo {version}"),
            &format!("Slot B:   Dukovo {version}, the default"),
            "Last health check: passed",
        ],
    )?;
    power_off(vm)
}
