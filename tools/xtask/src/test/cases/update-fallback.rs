// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A broken update falls back by itself, with the settings from before
//! it (ADRs 0453 and 0454). The test update staged into slot B has its
//! system log service's program a link to nothing, so Spark cannot start
//! it. A setting is changed after the stage. On the try, the health
//! check finds the service restarted three times, takes the update's
//! tries away and restarts the machine; GRUB, told the last start was not
//! seen through, shows its menu and starts slot A, saying the update
//! failed. Spark keeps the failed attempt's settings and puts back those
//! snapshotted at the stage, before any service starts: the changed
//! setting is gone from */system/settings*, and kept in
//! *@snapshots/update/settings-failed*.

use super::super::*;
use super::update_stage::{UPDATE_DISKS, power_off, restart, run, stage, started};

pub const CASE: Case = Case {
    name: "update-fallback",
    about: "an update whose system log service cannot start fails its health check on its first try and falls back to slot A by itself, with the settings from before it put back and the failed attempt's kept (ADRs 0453 and 0454)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_fallback,
    disks: UPDATE_DISKS,
    second: None,
};

/// The setting changed after the stage, which the fallback takes back.
const CHANGED: &str = "/system/settings/update-test";

/// Stage the broken update, change a setting, and see both undone.
fn update_fallback(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("broken");
    started(vm, 'a')?;
    stage(vm, "broken")?;
    run(
        vm,
        &format!("echo changed > {CHANGED} && echo \"changed: $((6 * 8))\""),
        &["changed: 48"],
    )?;

    // Without its log service, the update may not get as far as tty1.
    restart(vm)?;
    vm.expect("spark: started slot b, @system.b")?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) failed its health check: logd has restarted "
    ))?;
    vm.expect(&format!(
        "sysupdate: the update gives up its tries; restarting into slot A (Dukovo {VERSION})"
    ))?;
    vm.expect("spark: asked to restart the machine")?;

    // The menu shows, as the try was not seen through, and starts slot A.
    vm.expect(&format!("Booting `Dukovo Server {VERSION} (the update to {version} failed)'"))?;
    vm.expect("spark: the update in slot b fell back: putting the settings from before it back")?;
    vm.expect("spark: the settings from before the update are back")?;
    started(vm, 'a')?;
    vm.expect(&format!(
        "sysupdate: the update in slot B (Dukovo {version}) did not prove itself; slot A (Dukovo \
         {VERSION}) runs, with the settings from before the update"
    ))?;
    // The start after the failed one says so on the screen's bottom line
    // until a key is pressed (ADR 0501); Shift takes it off.
    vm.expect_screen(&["sysupdate: Start 2 failed its health check: logd has restarted "])?;
    vm.press(&["shift"])?;
    vm.expect_screen_without("changes --failed says what changed")?;
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
        &[
            &format!("Running:  slot A, Dukovo {VERSION}"),
            &format!("Slot A:   Dukovo {VERSION}, the default"),
            "Next start: slot A",
            "Last health check: passed",
            &format!(
                "Last update: {VERSION} to {version} in slot B; a fallback put the settings from \
                 before it back"
            ),
        ],
    )?;
    power_off(vm)
}
