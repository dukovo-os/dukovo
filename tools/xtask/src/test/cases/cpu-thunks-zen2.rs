// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The thunks the kernel's indirect branches and returns go through
//! (ADR 0793), rewritten for an emulated AMD Zen 2 processor with one
//! thread a core, under QEMU's emulation without KVM. Every indirect
//! branch goes through the retpoline and every return through the
//! untrained return, whose untraining runs on every entry from a program;
//! with one thread a core SRSO cannot reach the kernel, so its safe return
//! is not needed. The boot goes on to its end, and Spectre v2, Retbleed
//! and SRSO are not among the flaws the log says are left open.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-thunks-zen2",
    about: "the kernel's indirect branches and returns rewritten for an emulated Zen 2 (ADR 0793): through the retpoline and the untrained return, untrained on entry, and the boot completes",
    firmware: Firmware::Bios,
    media: Media::Cdrom,
    cmdline: "qemu-exit",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: Some("qemu64,vendor=AuthenticAMD,family=23,model=49,stepping=0"),
    qemu: &[],
    script: zen_2,
    disks: &[],
    second: None,
};

fn zen_2(vm: &mut Vm, case: &Case) -> Result<(), String> {
    super::cpu_thunks::paths(
        vm,
        case,
        "through the retpoline",
        "through the untrained return, untrained on entry",
        &["spectre_v2", "retbleed", "spec_rstack_overflow"],
    )
}
