// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Page-table isolation (ADR 0794) under PCIDs with INVPCID, on the host's
//! own processor under KVM with `pti=on`: as cpu-pti-pcid, but a program's
//! pages that are unmapped leave its view's TLB one by one, with INVPCID,
//! rather than the whole view at its next exit. On a host with PCIDs and
//! no INVPCID the log says the view is flushed whole; without KVM, or on a
//! host without PCIDs, that the kernel isolates itself without them.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-pti-invpcid",
    about: "page-table isolation under PCIDs with INVPCID, with pti=on under KVM (ADR 0794): the system starts to its console, Meltdown's file reads Mitigation: PTI or Not affected, a hundred programs run, and the log says a program's pages are flushed with INVPCID where the host has it",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off pti=on",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: invpcid,
    disks: &[],
    second: None,
};

fn invpcid(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let how = match (vm.kvm && host_cpu_has("pcid"), host_cpu_has("invpcid")) {
        (true, true) => "both views under PCIDs, a program's pages flushed with INVPCID",
        (true, false) => "both views under PCIDs, a program's view flushed whole on exit",
        (false, _) => super::cpu_pti_pcid::WITHOUT_PCIDS,
    };
    super::cpu_pti_pcid::programs(vm, how)
}
