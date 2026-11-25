// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The vDSO (ADR 0616): a small shared object the kernel maps into every
//! program it runs with `exec`, right above the clock page
//! (`radix_hal::clock::ClockPage`), and names in the auxiliary vector's
//! AT_SYSINFO_EHDR. Its functions read the monotonic clock and the time
//! of day from the page and the CPU's counter, and the CPU's number with
//! RDPID or RDTSCP, without a system call: Linux's `__vdso_clock_gettime`,
//! `__vdso_gettimeofday`, `__vdso_time`, `__vdso_clock_getres` and
//! `__vdso_getcpu` (version LINUX_2.6) for Linux programs, and
//! `__dukovo_clock_read` for Dukovo's C library.
//!
//! The C is in `src/vdso.c`; `build.rs` compiles, links and checks it.
//! The host tests below map the image as the kernel does, below a clock
//! page the HAL's writer filled, and call each function.

#![cfg_attr(not(test), no_std)]

/// The image, mapped as it is: empty where there is no vDSO.
pub static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vdso.so"));

#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
