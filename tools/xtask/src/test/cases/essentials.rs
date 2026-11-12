// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A start with only the essentials (ADR 0501), as the boot menu's
//! Troubleshooting starts one with the kernel's *essentials*. The first
//! boot is service-bundle's: Alice installs beacon, a test bundle whose
//! service starts at each boot, for everyone. The second boot has
//! *essentials*: Spark says that only the system's own services start,
//! and beacon, which an app added, stays off until Alice starts it by
//! hand, when it runs as its account again.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "essentials",
    about: "a start with only the essentials (ADR 0501), as the boot menu's Troubleshooting starts one: the system's own services start, and a bundle's service, added by an app, stays off until it is started by hand",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: super::service_bundle::install,
    disks: super::service_bundle::DISKS,
    second: Some(SecondBoot {
        cmdline: "lockdep essentials",
        before: nothing_between,
        script: essentials,
    }),
};

/// The second boot, with only the essentials.
fn essentials(vm: &mut Vm, case: &Case) -> Result<(), String> {
    vm.expect(
        "spark: essentials: only the system's own services start; those an app or an \
         administrator added stay off until they are started",
    )?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", "queen of hearts")?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    if vm.log().contains("beacon: beacon 1.0.0 runs as beacon, start 3") {
        return Err(format!("{}: beacon started, though only the essentials start", case.name));
    }
    answer_prompt(vm, "$", "clear; elevate service beacon start; echo \"start $?\"")?;
    answer_prompt(vm, "service beacon start:", "queen of hearts")?;
    vm.expect_anywhere("beacon: beacon 1.0.0 runs as beacon, start 3")?;
    vm.expect_screen(&["start 0"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
