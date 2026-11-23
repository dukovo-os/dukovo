// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As the *watchdog* test, with the watchdog of a BMC (ADR 0661): QEMU's
//! ipmi-bmc-sim on an isa-ipmi-kcs interface, where the other tests have
//! the chipset's. The kernel names the BMC's watchdog the machine's,
//! Spark starts it and, asked to stop responding, leaves it unpetted
//! until the BMC restarts the machine with its hard reset; the next boot
//! says the watchdog restarted it, which the BMC's expiration flags tell,
//! and Spark pets the watchdog for twice its timeout.

use super::super::*;

pub const CASE: Case = Case {
    name: "ipmi-watchdog",
    about: "as watchdog, with the BMC's watchdog through QEMU's ipmi-bmc-sim on a KCS interface in place of a chipset's",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "-- --watchdog 12 --test-hang",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-device", IPMI_BMC, "-device", "isa-ipmi-kcs,bmc=bmc0"],
    script: ipmi_watchdog,
    disks: &[],
    second: None,
};

fn ipmi_watchdog(vm: &mut Vm, case: &Case) -> Result<(), String> {
    watchdog_restarts(vm, case, "the BMC's watchdog (IPMI), with timeouts from 1 to 6553 s")
}
