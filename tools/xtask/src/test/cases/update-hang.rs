// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update whose start hangs falls back by itself after its three
//! tries (ADR 0452). The test update staged into slot B starts its kernel
//! with `test-hang`, which has Spark stop responding once it has started
//! QEMU's i6300esb watchdog, at every start; the watchdog restarts the
//! machine 10 seconds later. GRUB takes a try before each start, and
//! shows its menu after the first; with none left, it starts slot A,
//! saying the update failed, and slot A's health check acknowledges the
//! fallback.

use super::super::*;
use super::update_panic::tries;
use super::update_stage::{UPDATE_DISKS, power_off, restart, run, stage, started};

pub const CASE: Case = Case {
    name: "update-hang",
    about: "an update whose start hangs, Spark not responding, uses up its three tries as the watchdog restarts the machine each time, and slot A starts by itself after them (ADR 0452)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off -- --watchdog 10",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-device", "i6300esb"],
    script: update_hang,
    disks: UPDATE_DISKS,
    second: None,
};

/// Three hangs, then slot A.
fn update_hang(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("hang");
    started(vm, 'a')?;
    stage(vm, "hang")?;
    restart(vm)?;
    tries(vm, &version, "spark: not responding, as the kernel's test-hang asks, at every start")?;
    vm.expect("spark: the update in slot b fell back: putting the settings from before it back")?;
    vm.expect_ahead("spark: the watchdog restarted the machine: the last boot stopped responding")?;
    started(vm, 'a')?;
    vm.expect(&format!(
        "sysupdate: the update in slot B (Dukovo {version}) did not prove itself; slot A (Dukovo \
         {VERSION}) runs, with the settings from before the update"
    ))?;
    run(
        vm,
        "sysupdate status",
        &[&format!("Running:  slot A, Dukovo {VERSION}"), "Next start: slot A"],
    )?;
    power_off(vm)
}
