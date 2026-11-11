// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update whose kernel panics falls back by itself after its three
//! tries (ADR 0452). The test update staged into slot B starts its kernel
//! with `selftest=page-fault`, which panics early in each start; the
//! menu's `panic=10` restarts the machine 10 seconds later. GRUB takes a
//! try before each start, and shows its menu after the first, as that
//! start was not seen through. With none left, it starts slot A, saying
//! the update failed, and slot A's health check acknowledges the
//! fallback.

use super::super::*;
use super::update_stage::{UPDATE_DISKS, power_off, restart, run, stage, started};

pub const CASE: Case = Case {
    name: "update-panic",
    about: "an update whose kernel panics at every start uses up its three tries, GRUB counting each before the start, and slot A starts by itself after them (ADR 0452)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: update_panic,
    disks: UPDATE_DISKS,
    second: None,
};

/// Three panics, then slot A.
fn update_panic(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("panic");
    started(vm, 'a')?;
    stage(vm, "panic")?;
    restart(vm)?;
    tries(vm, &version, "Radix panic on CPU ")?;
    vm.expect("spark: the update in slot b fell back: putting the settings from before it back")?;
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

/// Follows the update's three tries, each a start of slot B that does not
/// come up, as the kernel's log line `failure` says, and the fallback's
/// start of slot A after them, which GRUB's menu says: the first try from
/// a hidden menu, as the start before it was seen through, and the others
/// from the menu shown.
pub fn tries(vm: &mut Vm, version: &str, failure: &str) -> Result<(), String> {
    for (n, word) in ["first", "second", "last"].iter().enumerate() {
        if n > 0 {
            vm.expect(&format!("Booting `Dukovo Server {version} (update, {word} try)'"))?;
        }
        vm.expect(" slot=b ")?;
        vm.expect(failure).map_err(|err| format!("the {word} try: {err}"))?;
    }
    vm.expect(&format!("Booting `Dukovo Server {VERSION} (the update to {version} failed)'"))?;
    Ok(())
}
