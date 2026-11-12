// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update from an update file, with no network, under UEFI on an
//! installed system's disk (ADR 0466). A FAT disk holds the test update
//! server's bridge release as one update file, a tar archive of its
//! repository. The machine has no network card: the update server cannot
//! be read. `system-release check` reads the file from the root the
//! system trusts and checks every file in it, in place; `sysupdate stage`
//! builds slot B from it, and the restart tries it and the health check
//! proves it.

use super::super::*;
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "update-offline",
    about: "an update from an update file on a disk, with no network (ADR 0466): the file is checked in place against the root the system trusts, staged into slot B, started at the restart and proved",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_offline,
    disks: &[update_file_disk(512 * 1024, "UPDATEFILE")],
    second: None,
};

/// Check, stage, start, prove.
fn update_offline(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let bridge = test_server_version("bridge");
    let file = format!("/volumes/vda/{}", test_update_file());
    started(vm, 'a')?;
    vm.expect_anywhere("spark: mounted the FAT filesystem of vda at /volumes/vda")?;
    run(
        vm,
        "echo 'stable = 10' > /system/settings/sysupdate/health.toml && echo \"set: $((6 * 7))\"",
        &["set: 42"],
    )?;
    run(vm, "sysupdate check; echo \"check $?\"", &["nothing was changed", "check 1"])?;
    // The release's line, which names the commit, runs past the screen's
    // width: the exit status says it was signed by the trusted root.
    run(vm, &format!("system-release check {file}; echo \"check $?\""), &[
        &format!("system-release: release {bridge} ("),
        "files match the signed list",
        "check 0",
    ])?;
    run(vm, &format!("sysupdate stage {file}"), &[
        "checking the release against the root this system trusts",
        &format!("sysupdate: staged Dukovo {bridge} in slot B: it starts at the next restart"),
    ])?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!("sysupdate: slot B (Dukovo {bridge}) proved itself: it is now the default"))?;
    run(vm, "cat /system/resources/test-update/notes.txt", &["bridge"])?;
    power_off(vm)
}
