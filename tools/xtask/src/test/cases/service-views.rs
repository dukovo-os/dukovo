// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a service holds of the root's own subvolumes (ADR 0473), on a
//! Dukovo root of Btrfs. viewtest (userspace/posix/viewtest.c) writes a
//! service file with files.root-subvolumes naming @snapshots and starts
//! it. In the view Spark builds of it, the root's disk is the only name
//! in /volumes and @snapshots the only one in it; a name it does not
//! hold cannot be made there, an absent one cannot be removed, and none
//! can be renamed to; and a snapshot of /system/settings goes into
//! @snapshots, as the snap service makes its own. service status says
//! what it holds, and service check refuses a file that names @users.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "service-views",
    about: "a service that holds the root's @snapshots alone (ADR 0473) sees only that name in the root's Btrfs at /volumes: no new name can be made there, an absent one cannot be removed, none can be renamed to, and a snapshot of /system/settings goes into it; service status says what it holds, and service check refuses a file that names @users",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --services --posix /system/tests/posix/viewtest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: service_views,
    disks: ROOT_DISKS,
    second: None,
};

fn service_views(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    // The root is on the disk, and Spark has recorded which it is.
    vm.expect_anywhere("spark: /system is Dukovo's root on ")?;
    vm.expect_ahead("spark: started /system/tests/posix/viewtest as process ")?;
    let code = program_line(vm, "(/system/tests/posix/viewtest) exited with code ")?;
    let log = vm.log();
    // The holder's lines are Spark's copies of its output.
    let failed: Vec<&str> = log
        .lines()
        .filter(|line| ["viewtest: FAILED", "inside: FAILED"].iter().any(|f| line.contains(f)))
        .map(str::trim)
        .collect();
    if !failed.is_empty() {
        return Err(format!("viewtest failed: {}", failed.join("; ")));
    }
    if code != "0x0" || !log.contains("viewtest: every check passed") {
        return Err(format!("viewtest exited with code {code}"));
    }
    for said in ["inside: every check passed", " holds @snapshots "] {
        if !log.contains(said) {
            return Err(format!("the holder did not say \"{said}\""));
        }
    }
    Ok(())
}
