// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Linux system call interface as an x86-64 Linux program sees it:
//! its call numbers, its error numbers, its flags and the records it
//! reads and writes, with their translation to and from Radix's, for the
//! kernel's Linux table (ADR 0610). Nothing here touches memory it was
//! not given: every record is built in, or read from, a byte slice.
//!
//! A static x86-64 Linux program makes its calls with the `syscall`
//! instruction, and the kernel's Linux table answers each by calling a
//! native method the process could call itself. This crate holds what that
//! table needs and the kernel does not: the values and layouts of the Linux
//! interface, and the rules that turn them into the native interface's.
//! It has no locks, no allocation and no `unsafe` code, and no input makes
//! it panic, so its host tests and property tests (`./ob test unit`) check
//! it all. When a Linux program gets a wrong answer, the value or record of
//! this crate that the call used is the first suspect: [`nr::name`] names a
//! call from its number, and the module that owns the value has the test
//! that pins it. The boot case `linux-programs` covers the table as a
//! whole.
//!
//! - [`nr`]: the x86-64 call numbers, and their names for log lines.
//! - [`errno`]: Linux's error numbers, the kernel-internal restart codes
//!   that never reach a program, and [`errno::from_status`], which gives a
//!   native error the errno the C library's Radix layer gives it.
//! - [`flags`]: the `open` and `AT_` flags, and [`flags::open`], which says
//!   what native open they ask for, as the C library's layer does.
//! - [`records`]: `struct stat` and `struct statx`, `linux_dirent64`, the
//!   kernel's `struct sigaction`, `struct utsname`, `timespec` and
//!   `timeval`, `struct rusage` and `struct sysinfo`, each written to or
//!   read from an array of the record's size.
//! - [`clone`]: the arguments of `clone` and `clone3`, Linux's rules for
//!   them and Dukovo's, and whether they make a thread or a process.
//! - [`futex`]: the command and flags in a futex call's `op`.
//! - [`signals`]: Linux's signal flags and `sigprocmask` operations, which
//!   equal the native ones, so that the table passes them through.
//! - [`sockets`]: the socket addresses, `msghdr`, control messages, `MSG_`
//!   flags and options of Linux's socket calls, as Dukovo's C library takes
//!   them.
//! - [`poll`]: the events of `poll` and `ppoll`, and `select`'s sets and
//!   times.
//! - [`system`]: the records and rules of the calls for limits, times,
//!   resource usage, scheduling, `prctl`, `waitid` and `siginfo_t`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod clone;
pub mod errno;
pub mod flags;
pub mod futex;
pub mod nr;
pub mod poll;
pub mod records;
pub mod signals;
pub mod sockets;
pub mod system;

#[cfg(test)]
mod tests;
