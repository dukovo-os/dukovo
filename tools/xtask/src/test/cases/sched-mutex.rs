// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kernel mutexes under contention (ADR 0368): three fair threads per CPU
//! and a FIFO thread take one mutex in turn, 242,000 times on 4 CPUs, its
//! holders now and then yielding the CPU with it held. Waiters spin while
//! the holder runs and sleep once it does not; a holder lets the mutex go
//! to its fair waiters, waking one to try again, and hands it straight to
//! the FIFO thread. Every increment under the mutex must count, every
//! thread must get through, both ways of letting go must happen, and no
//! thread may wait long, with the lock checker on.

use super::super::*;

pub const CASE: Case = Case {
    name: "sched-mutex",
    about: "a mutex under contention: no lost update, no lost wake-up, nobody starves",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "qemu-exit lockdep selftest=sched-mutex",
    cpus: 4,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: sched_mutex,
    disks: &[],
    second: None,
};

/// The `sched-mutex` case.
fn sched_mutex(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("selftest sched-mutex: ")?;
    selftest_passed(vm, "sched-mutex")?;
    if !vm.log().contains(" threads took one mutex 242000 times; ") {
        return Err("selftest sched-mutex passed, but its report lacks the count".into());
    }
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
