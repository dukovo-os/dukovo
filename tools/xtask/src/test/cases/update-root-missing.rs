// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update whose root cannot be found falls back by itself after its
//! three tries (ADRs 0452 and 0536). The test update staged into slot B
//! starts with `root=UUID=` and a UUID no disk has among its flags. GRUB
//! puts a slot's flags after its own `root=`, and Spark takes the last
//! one, so that each start of slot B finds no Dukovo root: Spark says so
//! and restarts the machine 30 seconds later. GRUB takes a try before each
//! start, and shows its menu after the first; with none left, it starts
//! slot A, saying the update failed, and Spark puts the settings from
//! before the update back (ADR 0454).

use super::super::*;
use super::update_panic::tries;
use super::update_stage::{UPDATE_DISKS, power_off, restart, run, stage, started};

pub const CASE: Case = Case {
    name: "update-root-missing",
    about: "an update whose start finds no disk with the Dukovo root its flags name, Spark saying so and restarting the machine each time, uses up its three tries, and slot A starts by itself after them with the settings from before it put back (ADRs 0452 and 0454)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_root_missing,
    disks: UPDATE_DISKS,
    second: None,
};

/// The setting changed after the stage, which the fallback takes back.
const CHANGED: &str = "/system/settings/update-test";

/// Three starts without a root, then slot A.
fn update_root_missing(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("noroot");
    started(vm, 'a')?;
    stage(vm, "noroot")?;
    run(
        vm,
        &format!("echo changed > {CHANGED} && echo \"changed: $((6 * 8))\""),
        &["changed: 48"],
    )?;
    restart(vm)?;
    tries(vm, &version, "spark: no disk holds the Dukovo root the boot menu names")?;
    vm.expect("spark: the update in slot b fell back: putting the settings from before it back")?;
    vm.expect("spark: the settings from before the update are back")?;
    started(vm, 'a')?;
    vm.expect(&format!(
        "sysupdate: the update in slot B (Dukovo {version}) did not prove itself; slot A (Dukovo \
         {VERSION}) runs, with the settings from before the update"
    ))?;
    run(
        vm,
        &format!(
            "[ -e {CHANGED} ] || echo \"taken back: $((7 * 7))\"; cat \
             /volumes/*/@snapshots/update/settings-failed/update-test"
        ),
        &["taken back: 49", "changed"],
    )?;
    run(
        vm,
        "sysupdate status",
        &[&format!("Running:  slot A, Dukovo {VERSION}"), "Next start: slot A"],
    )?;
    power_off(vm)
}
