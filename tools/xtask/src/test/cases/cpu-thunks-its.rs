// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The thunks the kernel's indirect branches and returns go through
//! (ADR 0793), rewritten for an emulated Intel Ice Lake processor, which
//! has indirect target selection, under QEMU's emulation without KVM. As
//! the emulation has no IBRS, indirect branches go through the retpoline;
//! every return is placed so that it ends in the upper half of its cache
//! line, or left to the return thunk that does, and the boot goes on to
//! its end. Spectre v2 and ITS are not among the flaws the log says are
//! left open.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-thunks-its",
    about: "the kernel's indirect branches and returns rewritten for an emulated Ice Lake (ADR 0793): through the retpoline, and returns aligned against ITS, and the boot completes",
    firmware: Firmware::Bios,
    media: Media::Cdrom,
    cmdline: "qemu-exit",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: Some("qemu64,vendor=GenuineIntel,family=6,model=106,stepping=6"),
    qemu: &[],
    script: ice_lake,
    disks: &[],
    second: None,
};

fn ice_lake(vm: &mut Vm, case: &Case) -> Result<(), String> {
    super::cpu_thunks::paths(
        vm,
        case,
        "through the retpoline",
        "aligned against ITS",
        &["spectre_v2", "indirect_target_selection"],
    )
}
