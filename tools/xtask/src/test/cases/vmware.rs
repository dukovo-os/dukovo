// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firmware says VMware made the machine (ADR 0424), and QEMU answers
//! VMware's backdoor port as its `vmport` does: the kernel looks for the
//! backdoor and, if it answers, starts the toolbox, which finds no guest
//! RPC behind it, says so once, and leaves the boot alone. QEMU has no
//! guest RPC, so the toolbox's talk with the host, its answers to Shut
//! Down Guest and Restart Guest and its guest information are tested by
//! its unit tests and on VMware's own hypervisors (lab test
//! hardware/vmware).

use super::super::*;

pub const CASE: Case = Case {
    name: "vmware",
    about: "the firmware names VMware: the kernel finds VMware's backdoor as QEMU answers it, and the toolbox, finding no guest RPC behind it, says so once and leaves the boot alone",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "qemu-exit",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-smbios", "type=1,manufacturer=VMware,, Inc.,product=VMware Virtual Platform"],
    script: vmware,
    disks: &[],
    second: None,
};

/// What the log says, in order: the firmware's maker, the backdoor's
/// version as QEMU answers it, and the toolbox finding no guest RPC.
const SAID: [&str; 3] = [
    "DMI: VMware, Inc. VMware Virtual Platform",
    "VMware: backdoor version ",
    "VMware: the host opened no channel: the host gives no guest RPC; trying again each 15 s",
];

/// The `vmware` case.
fn vmware(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let log = vm.log();
    // In order, each after the one before.
    let mut from = 0;
    for needle in SAID {
        let at = log[from..].find(needle).ok_or_else(|| format!("no \"{needle}\" in order"))?;
        from += at + needle.len();
    }
    let lines: Vec<&str> = log.lines().filter(|line| line.contains("VMware: ")).collect();
    // Once: the toolbox tries again without saying so again.
    if lines.iter().filter(|line| line.contains("gives no guest RPC")).count() != 1 {
        return Err(format!("the toolbox said more than once that there is no RPC: {lines:?}"));
    }
    if let Some(line) = lines.iter().find(|line| line.contains("failed") || line.contains("could not")) {
        return Err(format!("the toolbox failed: {line}"));
    }
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
