// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's signal numbers, `sigaction` flags and `sigprocmask` operations,
//! which Radix's are the same as.
//!
//! Radix numbers its signals as Linux does on x86-64 (`radix_abi::signal`:
//! SIGHUP is 1, SIGKILL 9, SIGSYS 31, SIGRTMIN 32 and SIGRTMAX 64), keeps a
//! set of them as a 64-bit word with bit `n - 1` for signal `n`, and takes
//! `sigaction`'s flags and the alternate stack's flags with the bits
//! Linux gives them. The Linux table therefore passes signal numbers, sets
//! and flags through to the native methods unchanged, and no translation
//! function exists. This module holds Linux's values under their Linux
//! names so that a call can be written in them, and the host tests assert
//! that each equals the native one: a change to either side fails there,
//! where it is cheap to find, and not in a program's signal handler.
//!
//! Two differences remain, and the table handles both. Radix's `MaskHow`
//! has a fourth value, `Query` (3), which `rt_sigprocmask` must refuse with
//! EINVAL: Linux queries with a null set, not a fourth operation. And Radix
//! accepts only the flag bits it defines (`ActionFlags::ALL`), so the table
//! drops the others from `sa_flags` before passing it on.

/// The size of the signal set that `rt_sigaction`, `rt_sigprocmask` and the
/// other calls take, in bytes. Any other `sigsetsize` is EINVAL.
pub const SIGSET_SIZE: u64 = 8;

/// A signal handler value that means the default action.
pub const SIG_DFL: u64 = 0;
/// A signal handler value that means ignore the signal.
pub const SIG_IGN: u64 = 1;

/// `sa_flags`: no `SIGCHLD` when a child stops or continues.
pub const SA_NOCLDSTOP: u64 = 1;
/// `sa_flags`: children that end are not left to be waited for.
pub const SA_NOCLDWAIT: u64 = 2;
/// `sa_flags`: the handler takes three arguments, the second a `siginfo_t`.
pub const SA_SIGINFO: u64 = 4;
/// `sa_flags`: `sa_restorer` is the function that returns from the handler.
/// The C library always sets it on x86-64.
pub const SA_RESTORER: u64 = 0x0400_0000;
/// `sa_flags`: run the handler on the alternate stack.
pub const SA_ONSTACK: u64 = 0x0800_0000;
/// `sa_flags`: a call the handler interrupted runs again afterwards.
pub const SA_RESTART: u64 = 0x1000_0000;
/// `sa_flags`: the signal is not blocked while its handler runs.
pub const SA_NODEFER: u64 = 0x4000_0000;
/// `sa_flags`: the handler goes back to the default when it is called.
pub const SA_RESETHAND: u64 = 0x8000_0000;

/// `rt_sigprocmask`: add the set to the mask.
pub const SIG_BLOCK: u32 = 0;
/// `rt_sigprocmask`: remove the set from the mask.
pub const SIG_UNBLOCK: u32 = 1;
/// `rt_sigprocmask`: replace the mask with the set.
pub const SIG_SETMASK: u32 = 2;

/// `sigaltstack`: the thread is running on the alternate stack.
pub const SS_ONSTACK: u32 = 1;
/// `sigaltstack`: the alternate stack is off.
pub const SS_DISABLE: u32 = 2;
