// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The CPUs' idle states (ADR 0795) on QEMU, whose firmware describes no
//! state deeper than C1 and whose CPUs would not be let use one. The
//! kernel finds both CPUs' processor objects in the namespace, tells them
//! what it can do and reads their `_CST`, and the log says C1 is all
//! there is. Each CPU's /kernel/devices/system/cpu/cpuN/cpuidle has that
//! one state, C1 entered with HLT, which it has entered and stayed in, and
//! cpuidle/current_driver is *none*, as on Linux without a driver.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "cpu-idle",
    about: "the CPUs' idle states on QEMU (ADR 0795): the kernel finds both processor objects and the log says they wait in C1 only; each CPU's cpuidle directory has one state, C1 with HLT, entered and stayed in, and cpuidle/current_driver is none",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: cpu_idle,
    disks: &[],
    second: None,
};

fn cpu_idle(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect(
        "Idle: C1 only; the firmware describes no deeper state the CPUs can use (2 processor \
         objects)",
    )?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // One line for each CPU, from its state's files: its name, how it is
    // entered, its exit latency, whether it was used and stayed in, and
    // whether there is a second state.
    let check = "d=/kernel/devices/system/cpu; for c in 0 1; do s=$d/cpu$c/cpuidle/state0; \
                 u=no; [ \"$(cat $s/usage)\" -gt 0 ] && u=used; t=no; [ \"$(cat $s/time)\" -gt 0 ] \
                 && t=stayed; n=one; [ -e $d/cpu$c/cpuidle/state1 ] && n=more; echo \"cpu$c \
                 $(cat $s/name) $(cat $s/desc) $(cat $s/latency) $u $t $n\"; done; echo \"driver \
                 $(cat $d/cpuidle/current_driver)\"";
    answer_prompt(vm, "#", &format!("clear; {check}"))?;
    vm.expect_screen(&[
        "cpu0 C1 ACPI HLT 0 used stayed one",
        "cpu1 C1 ACPI HLT 0 used stayed one",
        "driver none",
    ])?;
    Ok(())
}
