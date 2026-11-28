// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's `memcpy` (ADR 0775) and its copy for write-combining
//! memory (ADR 0718): every size from 0 to 1,100 bytes, past each way
//! the x86-64 kernel's `memcpy` copies by, which change at 16, 32 and
//! 512 bytes, from and to each of the 8 places in a quadword. Each copy
//! must match its source and write nothing around it.

use super::super::*;

pub const CASE: Case = Case {
    name: "copy",
    about: "both kernel copies copy every size up to 1,100 bytes at every alignment, and nothing more",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "qemu-exit selftest=copy",
    cpus: 1,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: copy,
    disks: &[],
    second: None,
};

/// The `copy` case.
fn copy(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    selftest_passed(vm, "copy")?;
    if !vm.log().contains("selftest copy: passed; 140928 copies of 0 to 1100 bytes") {
        return Err("selftest copy passed, but its report lacks the count".into());
    }
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
