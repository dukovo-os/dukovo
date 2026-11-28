// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The thunks the kernel's indirect branches and returns go through
//! (ADR 0793), rewritten for an emulated AMD Zen 3 processor: QEMU's
//! emulation without KVM, so that the processor is the same whatever the
//! host's. The boot CPU sends every indirect branch through the retpoline
//! and every return through the safe return at the second mapping of its
//! code, whose untraining runs on every entry from a program, and the
//! boot goes on to its end with them. Spectre v2 and SRSO are not among
//! the flaws the log says are left open.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-thunks",
    about: "the kernel's indirect branches and returns rewritten for an emulated Zen 3 (ADR 0793): through the retpoline and the safe return at aliased addresses, untrained on entry, and the boot completes",
    firmware: Firmware::Bios,
    media: Media::Cdrom,
    cmdline: "qemu-exit",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: Some("qemu64,vendor=AuthenticAMD,family=25,model=1,stepping=1"),
    qemu: &[],
    script: zen_3,
    disks: &[],
    second: None,
};

fn zen_3(vm: &mut Vm, case: &Case) -> Result<(), String> {
    paths(
        vm,
        case,
        "through the retpoline",
        "through the safe return at aliased addresses, untrained on entry",
        &["spectre_v2", "spec_rstack_overflow"],
    )
}

/// Boots to the end and checks the log's line on how the kernel's
/// indirect branches and returns run, and that none of `closed` is left
/// open. The other cpu-thunks tests use it too.
pub(super) fn paths(
    vm: &mut Vm,
    case: &Case,
    indirect: &str,
    returns: &str,
    closed: &[&str],
) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let log = vm.log();
    let line = |starts: &str| {
        log.lines()
            .filter_map(|line| line.split("CPU flaws: ").nth(1))
            .find(|rest| rest.contains(starts))
            .map(str::to_string)
    };
    let Some(sites) = line(" indirect branches ") else {
        return Err("the log has no line on the indirect branches and returns".into());
    };
    let (branches, rest) = sites.split_once(" indirect branches ").unwrap_or_default();
    let (how_branches, rest) = rest.split_once(", ").unwrap_or_default();
    let (returned, how_returns) = rest.split_once(" returns ").unwrap_or_default();
    let counted = |n: &str| n.parse::<u32>().is_ok_and(|n| n > 1000);
    if !counted(branches) || !counted(returned) {
        return Err(format!("the kernel found too few sites: {sites}"));
    }
    if how_branches != indirect || how_returns != returns {
        return Err(format!("indirect branches {indirect}, returns {returns} expected: {sites}"));
    }
    let Some(applying) = line(" apply; ") else {
        return Err("the log has no line on the flaws that apply".into());
    };
    let open = applying.split("; open: ").nth(1).unwrap_or_default();
    if let Some(flaw) = closed.iter().find(|&&flaw| open.split(", ").any(|o| o == flaw)) {
        return Err(format!("{flaw} is left open: {applying}"));
    }
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
