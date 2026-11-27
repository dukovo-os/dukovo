// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Page-table isolation (ADR 0794) under PCIDs without INVPCID, on the
//! host's own processor under KVM: `pti=on` isolates the kernel whether
//! the processor has Meltdown or not, and `noinvpcid` keeps INVPCID out.
//! QEMU's emulation has no PCIDs (QEMU 8.2 leaves them out), so only KVM
//! runs both views under PCIDs of their own, keeping their TLB entries
//! across the switch, and a program's view whose pages were unmapped is
//! flushed whole on its next exit; without KVM, or on a host without
//! PCIDs, the kernel isolates itself without them. Either way the system
//! starts to its console on two CPUs, Meltdown's file reads *Mitigation:
//! PTI* or, on a processor without it, *Not affected*, and the shell
//! starts a hundred programs (twenty under emulation, which is slower),
//! each a new address space made, switched to and torn down.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-pti-pcid",
    about: "page-table isolation under PCIDs without INVPCID, with pti=on noinvpcid under KVM (ADR 0794): the system starts to its console, Meltdown's file reads Mitigation: PTI or Not affected, a hundred programs run, and the log says both views run under PCIDs where the host has them",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off pti=on noinvpcid",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: pcids,
    disks: &[],
    second: None,
};

/// The log's word on isolation where the guest has no PCIDs.
pub(super) const WITHOUT_PCIDS: &str = "without PCIDs";

fn pcids(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let how = if vm.kvm && host_cpu_has("pcid") {
        "both views under PCIDs, a program's view flushed whole on exit"
    } else {
        WITHOUT_PCIDS
    };
    programs(vm, how)
}

/// Starts to the console, checks Meltdown's file, runs a hundred programs
/// (twenty without KVM) and checks the log's word on isolation, `how`.
/// cpu-pti-invpcid uses it too.
pub(super) fn programs(vm: &mut Vm, how: &str) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    answer_prompt(
        vm,
        "#",
        "clear; m=$(cat /kernel/devices/system/cpu/vulnerabilities/meltdown); case \"$m\" in \
         \"Mitigation: PTI\"|\"Not affected\") echo \"meltdown fine\";; *) echo \"meltdown $m\";; \
         esac",
    )?;
    vm.expect_screen(&["meltdown fine"])?;
    // Each `ls` is a process of its own: an address space made, its pages
    // mapped and unmapped, and torn down, on whichever CPU runs it.
    let count = if vm.kvm { 100 } else { 20 };
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; n=0; i=0; while [ $i -lt {count} ]; do ls /system/commands > /dev/null && \
             n=$((n+1)); i=$((i+1)); done; echo \"ran $n\""
        ),
    )?;
    vm.expect_screen(&[&format!("ran {count}")])?;
    super::cpu_pti::isolated(vm, how)
}
