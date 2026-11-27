// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The words of `seccomp(2)` and of `prctl`'s seccomp options
//! (linux/seccomp.h, linux/prctl.h): the operations, the flags a filter
//! is installed with, the modes, `struct sock_fprog`, and the `si_code`
//! of the SIGSYS a filter's TRAP sends. The kernel's Linux table keeps
//! the filters (`kernel/radix/src/user/linux/seccomp.rs`, ADR 0623);
//! radix-bpf checks and runs them.

use crate::errno::EINVAL;

/// `seccomp`'s operation: strict mode.
pub const SECCOMP_SET_MODE_STRICT: u32 = 0;
/// `seccomp`'s operation: add a filter.
pub const SECCOMP_SET_MODE_FILTER: u32 = 1;
/// `seccomp`'s operation: whether the kernel knows an action.
pub const SECCOMP_GET_ACTION_AVAIL: u32 = 2;
/// `seccomp`'s operation: the sizes of the user-notification records.
pub const SECCOMP_GET_NOTIF_SIZES: u32 = 3;

/// The mode of a thread no filter or strict mode limits.
pub const SECCOMP_MODE_DISABLED: u8 = 0;
/// Strict mode: `read`, `write`, `exit` and `rt_sigreturn` only.
pub const SECCOMP_MODE_STRICT: u8 = 1;
/// Filter mode: each call is judged by the thread's filters.
pub const SECCOMP_MODE_FILTER: u8 = 2;

/// The calls strict mode allows, by x86-64's numbers: `read`, `write`,
/// `exit` and `rt_sigreturn`. `exit_group` is not one of them.
pub const STRICT_CALLS: [u64; 4] = [0, 1, 60, 15];

/// `prctl`'s `PR_GET_SECCOMP`.
pub const PR_GET_SECCOMP: u32 = 21;
/// `prctl`'s `PR_SET_SECCOMP`.
pub const PR_SET_SECCOMP: u32 = 22;
/// `prctl`'s `PR_SET_NO_NEW_PRIVS`.
pub const PR_SET_NO_NEW_PRIVS: u32 = 38;
/// `prctl`'s `PR_GET_NO_NEW_PRIVS`.
pub const PR_GET_NO_NEW_PRIVS: u32 = 39;

/// The `si_code` of the SIGSYS a filter's TRAP sends.
pub const SYS_SECCOMP: i32 = 1;

/// The most instructions along a thread's chain of filters, as Linux
/// counts them: those of each filter's conversion to eBPF, and 4 more for
/// each filter before the newest. Linux's `MAX_INSNS_PER_PATH`.
pub const MAX_INSTRUCTIONS_PER_PATH: u32 = 32_768;
/// What each filter adds to the length of a chain besides its
/// instructions.
pub const FILTER_PENALTY: u32 = 4;

/// The size of `struct sock_fprog`: a `u16` count, padding, and the
/// address of the instructions.
pub const SOCK_FPROG_SIZE: usize = 16;

/// A filter's program as `seccomp` and `prctl` are given it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SockFprog {
    /// How many instructions it has.
    pub len: u16,
    /// Where they are, 8 bytes each.
    pub filter: u64,
}

impl SockFprog {
    /// The record in `bytes`.
    pub fn read(bytes: &[u8; SOCK_FPROG_SIZE]) -> SockFprog {
        let mut filter = [0u8; 8];
        filter.copy_from_slice(&bytes[8..16]);
        SockFprog {
            len: u16::from_le_bytes([bytes[0], bytes[1]]),
            filter: u64::from_le_bytes(filter),
        }
    }
}

/// SECCOMP_FILTER_FLAG_TSYNC: every thread of the process gets the chain.
pub const FLAG_TSYNC: u32 = 1 << 0;
/// SECCOMP_FILTER_FLAG_LOG: the calls the filter refuses are logged.
pub const FLAG_LOG: u32 = 1 << 1;
/// SECCOMP_FILTER_FLAG_SPEC_ALLOW: leave speculation mitigations as they
/// are. Dukovo's are the same for every thread, so it changes nothing.
pub const FLAG_SPEC_ALLOW: u32 = 1 << 2;
/// SECCOMP_FILTER_FLAG_NEW_LISTENER: a listener for USER_NOTIF.
pub const FLAG_NEW_LISTENER: u32 = 1 << 3;
/// SECCOMP_FILTER_FLAG_TSYNC_ESRCH: a failed TSYNC answers ESRCH.
pub const FLAG_TSYNC_ESRCH: u32 = 1 << 4;
/// SECCOMP_FILTER_FLAG_WAIT_KILLABLE_RECV: a listener's wait.
pub const FLAG_WAIT_KILLABLE_RECV: u32 = 1 << 5;

/// The flags a filter may be installed with here. A listener for
/// USER_NOTIF is not one Dukovo makes, so NEW_LISTENER and
/// WAIT_KILLABLE_RECV are refused as a kernel without them refuses them,
/// which is how libseccomp and container runtimes find out.
const FLAGS_TAKEN: u32 = FLAG_TSYNC | FLAG_LOG | FLAG_SPEC_ALLOW | FLAG_TSYNC_ESRCH;

/// How a filter is to be installed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FilterFlags {
    /// Every thread of the process gets the chain.
    pub tsync: bool,
    /// The filter's refusals are logged.
    pub log: bool,
    /// A failed TSYNC answers ESRCH, not the thread's number.
    pub tsync_esrch: bool,
}

/// The flags `SECCOMP_SET_MODE_FILTER` was given, or the errno for them.
///
/// # Errors
///
/// EINVAL for a flag this kernel does not take.
pub fn filter_flags(flags: u32) -> Result<FilterFlags, i32> {
    if flags & !FLAGS_TAKEN != 0 {
        return Err(EINVAL);
    }
    Ok(FilterFlags {
        tsync: flags & FLAG_TSYNC != 0,
        log: flags & FLAG_LOG != 0,
        tsync_esrch: flags & FLAG_TSYNC_ESRCH != 0,
    })
}
