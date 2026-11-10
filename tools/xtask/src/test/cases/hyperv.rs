// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine runs as under Hyper-V (ADR 0423): KVM gives its CPU
//! Hyper-V's CPUID leaves, hypercalls, reference counter, frequency
//! registers and synthetic interrupt controller, and QEMU gives it a
//! VMBus with the dynamic memory service on it. The kernel takes the
//! TSC's rate from Hyper-V, starts its synthetic interrupt controller,
//! connects to the VMBus, asks for its channels and leaves the one it
//! does not use, and the boot goes on past the disks the host offered
//! (none here). QEMU has no synthetic disk or network card, which the
//! driver's unit tests and real Hyper-V and Azure machines test. Without
//! KVM, whose synthetic interrupt controller QEMU's VMBus needs, the
//! machine is a plain one, and the kernel finds no Hyper-V on it.

use super::super::*;

pub const CASE: Case = Case {
    name: "hyperv",
    about: "under Hyper-V's enlightenments (KVM), the kernel takes Hyper-V's clock rates, connects to the VMBus and sets up its channels",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "qemu-exit lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[HYPERV],
    script: hyperv,
    disks: &[],
    second: None,
};

/// What the log holds on a machine under Hyper-V, each somewhere.
const UNDER_HYPERV: [&str; 4] = [
    " MHz (Hyper-V)",
    "Hyper-V: version ",
    "Hyper-V: dynamic memory (",
    "Hyper-V: 1 channel(s) offered; 0 SCSI controller(s), 0 disk(s)",
];

/// The `hyperv` case.
fn hyperv(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let log = vm.log();
    if !vm.kvm {
        if log.contains("Hyper-V:") {
            return Err("the kernel found Hyper-V on a machine without it".into());
        }
        return vm.expect_exit(QEMU_EXIT_SUCCESS);
    }
    // The boot waits for the channels the host offered, so all of this
    // was logged before it completed.
    for needle in UNDER_HYPERV {
        if !log.contains(needle) {
            return Err(format!("no \"{needle}\" under Hyper-V"));
        }
    }
    let lines: Vec<&str> = log.lines().map(str::trim_end).collect();
    if !lines.iter().any(|line| line.contains("Hyper-V: VMBus ") && line.ends_with(" connected")) {
        return Err("the kernel did not connect to the VMBus".into());
    }
    if !lines
        .iter()
        .any(|line| line.contains("Hyper-V: dynamic memory (") && line.ends_with(") is not used"))
    {
        return Err("the dynamic memory service was not left alone".into());
    }
    if let Some(line) = lines.iter().find(|line| line.contains("Hyper-V: the host did not")) {
        return Err(format!("the host did not answer: {line}"));
    }
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
