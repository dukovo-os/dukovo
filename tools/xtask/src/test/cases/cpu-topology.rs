// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The last-level caches split a package (ADR 0797): QEMU gives an
//! emulated Intel Skylake server one package of two dies, each of four
//! cores with an L3 of its own, as CPUID's leaves 0x1F and 4 describe it.
//! The kernel logs 8 cores, 2 last-level caches and 1 package, and the
//! balancing self-test spreads 16 threads started on one CPU over all 8,
//! some of them pulled from one cache's CPUs to the other's.

use super::super::*;

pub const CASE: Case = Case {
    name: "cpu-topology",
    about: "an emulated Skylake server package of two dies with an L3 each (CPUID 0x1F and 4): the kernel counts 2 last-level caches in 1 package, and balancing spreads threads within and between them (ADR 0797)",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "qemu-exit selftest=sched-balance",
    cpus: 8,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    // Without RDRAND and RDSEED, as the harness expects of a model of the
    // test's own.
    cpu_model: Some("Skylake-Server,-rdrand,-rdseed"),
    qemu: TWO_DIES,
    script: two_caches,
    disks: &[],
    second: None,
};

/// One package of two dies of four cores, one hardware thread each; QEMU
/// gives each die its own L3.
const TWO_DIES: &[&str] = &["-smp", "8,sockets=1,dies=2,cores=4,threads=1"];

/// The boot report names the two caches, and the balancing self-test
/// passes, having pulled at least one thread across the package level,
/// which is the only way to the other cache's CPUs.
fn two_caches(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    let topology = "CPU topology: 8 hardware threads on 8 cores, 2 last-level caches, 1 \
                    package, 1 NUMA node";
    if !vm.log().contains(topology) {
        return Err(format!("the boot log lacks \"{topology}\""));
    }
    vm.expect("selftest sched-balance: ")?;
    selftest_passed(vm, "sched-balance")?;
    let log = vm.log();
    let across = log
        .split("within caches, ")
        .nth(1)
        .and_then(|rest| rest.split(" within packages").next())
        .and_then(|count| count.trim().parse::<u64>().ok())
        .ok_or("selftest sched-balance says nothing of threads pulled within packages")?;
    if across == 0 {
        return Err("no thread was pulled between the two last-level caches".into());
    }
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
