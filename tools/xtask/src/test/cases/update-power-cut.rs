// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A power cut in the middle of a stage leaves the running slot as it was
//! (ADR 0451). *sysupdate stage* builds the test update into
//! *@system.new* and, told by `--test-stop-after build`, stops there, as
//! a power cut before the slot is sealed would find it; the filesystems
//! are synced and the machine is reset. Slot A starts again, as the slot
//! state was never written, and the health check removes the half-built
//! *@system.new*. A stage then runs to its end.

use super::super::*;
use super::update_stage::{UPDATE_DISKS, UPDATES, power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "update-power-cut",
    about: "the power is cut while a stage builds the update: slot A starts again, unchanged, the half-built slot is removed at that start, and the update then stages (ADR 0451)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_power_cut,
    disks: UPDATE_DISKS,
    second: None,
};

/// Stop a stage after the build, cut the power, and start again.
fn update_power_cut(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("good");
    started(vm, 'a')?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {UPDATES}"))?;
    // In the background, as it waits there for the power cut.
    run(
        vm,
        &format!(
            "sysupdate stage --test-stop-after build {UPDATES}/good > /system/live/stage.log & \
             while ! grep -q 'stopped after building' /system/live/stage.log; do sleep 1; done; \
             sync; echo \"built: $((8 * 8))\""
        ),
        &["built: 64"],
    )?;
    vm.monitor("system_reset")?;

    // Nothing was sealed and the slot state never named slot B. The
    // health check may remove the half-built slot before tty1 shows; on
    // a failure, what sysupdate said tells why.
    started(vm, 'a')?;
    vm.expect_anywhere(
        "sysupdate: removed @system.new, a slot left half built by a stage that did not finish",
    )
    .map_err(|err| {
        let log = vm.log();
        let said: Vec<&str> = log.lines().filter(|line| line.contains("sysupdate")).collect();
        format!("{err}; sysupdate said: {}", said[said.len().saturating_sub(12)..].join(" | "))
    })?;
    run(
        vm,
        "sysupdate status",
        &[&format!("Running:  slot A, Dukovo {VERSION}"), "Slot B:   empty", "Next start: slot A"],
    )?;
    run(
        vm,
        &format!("sysupdate stage {UPDATES}/good"),
        &[&format!("sysupdate: staged Dukovo {version} in slot B: it starts at the next restart")],
    )?;
    restart(vm)?;
    started(vm, 'b')?;
    power_off(vm)
}
