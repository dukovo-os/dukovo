// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Page-table isolation (ADR 0794) on an emulated Intel Skylake, which has
//! Meltdown and no PCIDs: QEMU's emulation without KVM, so that the
//! processor is the same whatever the host's. The kernel isolates itself
//! on its own, the log says so, and Meltdown is not among the flaws left
//! open. The user self-test then runs every system call and makes eight
//! kinds of fault from a program on two CPUs, each an entry from and an
//! exit to a program's page tables, and passes as without isolation.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-pti",
    about: "page-table isolation on an emulated Skylake without PCIDs (ADR 0794): on by itself for Meltdown, and the user self-test's system calls and faults pass under it",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "qemu-exit lockdep selftest=user",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: Some(SKYLAKE),
    qemu: &[],
    script: without_pcids,
    disks: &[],
    second: None,
};

/// An Intel Skylake client part, as the microcode test emulates.
pub(super) const SKYLAKE: &str = "qemu64,vendor=GenuineIntel,family=6,model=94,stepping=3";

fn without_pcids(vm: &mut Vm, case: &Case) -> Result<(), String> {
    user(vm, case)?;
    isolated(vm, "without PCIDs")
}

/// Fails unless the log says the kernel isolates itself as `how` says,
/// names page-table isolation among its protections, and leaves Meltdown
/// closed. The other cpu-pti tests use it too.
pub(super) fn isolated(vm: &Vm, how: &str) -> Result<(), String> {
    let log = vm.log();
    let line =
        format!("Page-table isolation: programs run on page tables without the kernel, {how}");
    if !log.contains(&line) {
        return Err(format!("the log has no line \"{line}\""));
    }
    let said: Vec<&str> = log.lines().filter_map(|line| line.split("CPU flaws: ").nth(1)).collect();
    if !said.iter().any(|rest| rest.split(", ").any(|what| what == "page-table isolation")) {
        return Err("page-table isolation is not among the protections the log names".into());
    }
    let Some(applying) = said.iter().find(|rest| rest.contains(" apply; ")) else {
        return Err("the log has no line on the flaws that apply".into());
    };
    let open = applying.split("; open: ").nth(1).unwrap_or_default();
    if open.split(", ").any(|flaw| flaw == "meltdown") {
        return Err(format!("meltdown is left open: {applying}"));
    }
    Ok(())
}
