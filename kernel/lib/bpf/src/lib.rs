// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Classic BPF, McCanne and Jacobson's packet filter machine, for the two
//! kinds of program the kernel runs on it: the packet filters libpcap
//! compiles from what tcpdump(8) is asked for, which a packet capture
//! runs on each frame (ADR 0346), and the seccomp filters a Linux program
//! installs with `seccomp(2)` or `prctl(PR_SET_SECCOMP)` to say which of
//! its own system calls it may make (ADR 0623).
//!
//! - [`machine`]: the instructions, the checks every program passes
//!   ([`check`], Linux's `bpf_check_classic`) and the interpreter
//!   ([`run`]), on any [`Input`]: a frame (radix-net's `Packet`), or a
//!   call.
//! - [`filter`]: a seccomp filter, checked further as Linux's
//!   `seccomp_check_filter` checks it ([`Program::check`]) and run on the
//!   call's [`SeccompData`].
//! - [`calls`]: the calls a filter always allows ([`Calls`],
//!   [`Program::always_allows`]), Linux's cache of 5.11, so that the Linux
//!   table (`kernel/radix/src/user/linux/seccomp.rs`) skips the filters
//!   for those.
//! - [`seccomp`]: `struct seccomp_data`, the actions a filter returns, and
//!   which of two is the stricter.
//!
//! It has no locks, no allocation and no `unsafe` code, and no input makes
//! it panic: a checked program jumps only forward, so a run ends within
//! as many steps as it has instructions. `./ob test unit` runs its tests,
//! which hold seccomp filters libseccomp writes; `./ob test kani` proves
//! the checks, the interpreter and the cache on every program of a few
//! instructions; the fuzz target `bpf-filter` runs any bytes. When a
//! Linux program's filter gives another answer than Linux's, run the
//! filter here on the call's `SeccompData` first: the test that pins the
//! instruction it took is in `tests.rs`. The boot case `linux-programs`
//! (`seccomp.c`) and the `linux-suite` CI job's seccomp differential
//! cover the whole.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod calls;
pub mod filter;
pub mod machine;
pub mod seccomp;

pub use calls::Calls;
pub use filter::Program;
pub use machine::{INSTRUCTION, Input, Instruction, Invalid, MAX_INSTRUCTIONS, MEMORY, check, run};
pub use seccomp::SeccompData;

#[cfg(kani)]
mod proofs;
#[cfg(test)]
mod tests;
