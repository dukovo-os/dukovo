// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The arguments of `clone` and `clone3`, and which native operation they
//! ask for.
//!
//! Both calls are one call underneath: Linux builds the same record from
//! either and checks it in `copy_process`. [`CloneArgs`] is that record,
//! made by [`CloneArgs::from_clone`] from the registers of the raw `clone`
//! or by [`CloneArgs::read`] from the `struct clone_args` of `clone3`, and
//! [`classify`] applies Linux's consistency rules, then Dukovo's own, and
//! says whether the new task is a thread of the caller's process or a
//! process of its own.
//!
//! Dukovo's threads always share what a thread shares in Linux: the
//! address space, the signal handlers, the descriptor table and the file
//! system view (`CLONE_VM`, `CLONE_SIGHAND`, `CLONE_FILES`, `CLONE_FS`,
//! with `CLONE_THREAD`). Its processes share nothing with their parent,
//! and a `CLONE_VFORK` child's parent waits for it to `exec` or end, with
//! its own copy of the memory. A flag combination between the two, such as
//! processes that share memory or descriptors, is refused with EINVAL.

use crate::errno::{E2BIG, EINVAL, EPERM};

/// Share the address space.
pub const CLONE_VM: u64 = 0x100;
/// Share the root and working directory and the umask.
pub const CLONE_FS: u64 = 0x200;
/// Share the descriptor table.
pub const CLONE_FILES: u64 = 0x400;
/// Share the signal handlers.
pub const CLONE_SIGHAND: u64 = 0x800;
/// Store a descriptor for the new process at `pidfd`.
pub const CLONE_PIDFD: u64 = 0x1000;
/// Let a tracer that traces the caller trace the new task too.
pub const CLONE_PTRACE: u64 = 0x2000;
/// The caller waits until the new task execs or ends.
pub const CLONE_VFORK: u64 = 0x4000;
/// The new task's parent is the caller's parent.
pub const CLONE_PARENT: u64 = 0x8000;
/// The new task is a thread of the caller's process.
pub const CLONE_THREAD: u64 = 0x1_0000;
/// The new task gets its own mount namespace.
pub const CLONE_NEWNS: u64 = 0x2_0000;
/// Share the System V semaphore adjustments.
pub const CLONE_SYSVSEM: u64 = 0x4_0000;
/// Set the new task's thread pointer to `tls`.
pub const CLONE_SETTLS: u64 = 0x8_0000;
/// Store the new task's thread ID at `parent_tid`, in the caller's memory.
pub const CLONE_PARENT_SETTID: u64 = 0x10_0000;
/// Clear the word at `child_tid` and wake it when the new task ends.
pub const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
/// Ignored by Linux since 2.6.
pub const CLONE_DETACHED: u64 = 0x40_0000;
/// A tracer cannot force `CLONE_PTRACE` on the new task.
pub const CLONE_UNTRACED: u64 = 0x80_0000;
/// Store the new task's thread ID at `child_tid`, in the new task's memory.
pub const CLONE_CHILD_SETTID: u64 = 0x100_0000;
/// The new task gets its own cgroup namespace.
pub const CLONE_NEWCGROUP: u64 = 0x200_0000;
/// The new task gets its own UTS namespace.
pub const CLONE_NEWUTS: u64 = 0x400_0000;
/// The new task gets its own IPC namespace.
pub const CLONE_NEWIPC: u64 = 0x800_0000;
/// The new task gets its own user namespace.
pub const CLONE_NEWUSER: u64 = 0x1000_0000;
/// The new task gets its own process ID namespace.
pub const CLONE_NEWPID: u64 = 0x2000_0000;
/// The new task gets its own network namespace.
pub const CLONE_NEWNET: u64 = 0x4000_0000;
/// The new task shares the caller's I/O context.
pub const CLONE_IO: u64 = 0x8000_0000;
/// `clone3` only: the new process's handlers all go back to the default.
pub const CLONE_CLEAR_SIGHAND: u64 = 0x1_0000_0000;
/// `clone3` only: put the new process in the cgroup at `cgroup`.
pub const CLONE_INTO_CGROUP: u64 = 0x2_0000_0000;
/// The new task gets its own time namespace. It is `clone3` only: the
/// value lies in the byte of `clone`'s flags that holds the exit signal.
pub const CLONE_NEWTIME: u64 = 0x80;

/// The bits of `clone`'s flags word that are the exit signal.
const EXIT_SIGNAL_MASK: u64 = 0xff;

/// Every flag a `clone` or `clone3` request may have: `clone`'s 32 bits
/// but the byte of the exit signal, with `CLONE_NEWTIME`, which lies in that
/// byte and so only `clone3` can set, and the two flags above 32 bits.
/// A word with a bit outside it is not a request.
const KNOWN_FLAGS: u64 = 0xffff_ff00 | CLONE_NEWTIME | CLONE_CLEAR_SIGHAND | CLONE_INTO_CGROUP;

/// The namespaces a new task may ask for; Dukovo has none of them yet.
const NAMESPACE_FLAGS: u64 = CLONE_NEWNS
    | CLONE_NEWCGROUP
    | CLONE_NEWUTS
    | CLONE_NEWIPC
    | CLONE_NEWUSER
    | CLONE_NEWPID
    | CLONE_NEWNET
    | CLONE_NEWTIME;

/// The sharing flags a process may not have: the kernel shares this state
/// among the threads of one process only, and `classify` requires all three
/// of a thread.
const SHARED_STATE: u64 = CLONE_FS | CLONE_FILES | CLONE_SIGHAND;

/// The signals Linux numbers, which an exit signal may be.
const SIGNAL_MAX: u64 = 64;

/// The size of `struct clone_args` in Linux 5.3, its first version, and the
/// smallest a caller may pass.
pub const CLONE_ARGS_SIZE_VER0: usize = 64;
/// The size of `struct clone_args` with `cgroup`: the one this module
/// knows. A caller's larger structure is accepted if what it adds is zero.
pub const CLONE_ARGS_SIZE_VER2: usize = 88;
/// The largest `size` Linux takes for `clone3`: a page.
pub const CLONE_ARGS_SIZE_MAX: usize = 4096;

/// What a `clone` or `clone3` call asks for, in the fields of
/// `struct clone_args`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloneArgs {
    /// The `CLONE_` flags, without the exit signal.
    pub flags: u64,
    /// Where `CLONE_PIDFD` stores the descriptor, in the caller's memory.
    pub pidfd: u64,
    /// Where `CLONE_CHILD_SETTID` and `CLONE_CHILD_CLEARTID` keep the
    /// thread ID, in the new task's memory.
    pub child_tid: u64,
    /// Where `CLONE_PARENT_SETTID` stores it, in the caller's memory.
    pub parent_tid: u64,
    /// The signal the parent gets when the new process ends; 0 for none.
    pub exit_signal: u64,
    /// The new task's stack: for `clone`, the stack pointer it starts with
    /// (0 to go on with the caller's); for `clone3`, the lowest address of
    /// the stack, which is `stack_size` bytes long.
    pub stack: u64,
    /// The length of the stack `clone3` names, and 0 for `clone`.
    pub stack_size: u64,
    /// The thread pointer `CLONE_SETTLS` sets.
    pub tls: u64,
}

/// What a valid request makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A new thread in the caller's process.
    Thread,
    /// A new process that shares nothing with the caller. With `vfork`,
    /// the caller waits until it execs or ends.
    Process {
        /// Whether `CLONE_VFORK` is set.
        vfork: bool,
    },
}

impl CloneArgs {
    /// The request of the raw `clone` call.
    ///
    /// The x86-64 call takes its arguments in the order `flags`, `stack`,
    /// `parent_tid`, `child_tid`, `tls`, which differs from other
    /// architectures'. Only the low 32 bits of `flags` count, and the low
    /// byte of them is the exit signal. `CLONE_PIDFD` uses `parent_tid` as
    /// its descriptor's address.
    pub fn from_clone(flags: u64, stack: u64, parent_tid: u64, child_tid: u64, tls: u64) -> Self {
        let flags = flags & u64::from(u32::MAX);
        CloneArgs {
            flags: flags & !EXIT_SIGNAL_MASK,
            pidfd: parent_tid,
            child_tid,
            parent_tid,
            exit_signal: flags & EXIT_SIGNAL_MASK,
            stack,
            stack_size: 0,
            tls,
        }
    }

    /// The request in the `struct clone_args` of `clone3`, from the `size`
    /// bytes the caller gave.
    ///
    /// A caller may pass a structure of any size from
    /// [`CLONE_ARGS_SIZE_VER0`] to [`CLONE_ARGS_SIZE_MAX`]. What the
    /// structure has past this module's [`CLONE_ARGS_SIZE_VER2`] bytes must
    /// be zero, as Linux's `copy_struct_from_user` requires, so that a
    /// newer program that asks for something this kernel lacks gets an
    /// error and not a quiet omission.
    ///
    /// # Errors
    ///
    /// - E2BIG for a `bytes` longer than a page, or with a nonzero byte
    ///   past [`CLONE_ARGS_SIZE_VER2`].
    /// - EINVAL for one shorter than [`CLONE_ARGS_SIZE_VER0`]; for flags
    ///   `clone3` does not take (an exit signal or `CLONE_DETACHED` in
    ///   them, or a bit no flag has); for `CLONE_SIGHAND` with
    ///   `CLONE_CLEAR_SIGHAND`; for an exit signal with `CLONE_THREAD` or
    ///   `CLONE_PARENT`; for a stack with no size, a size with no stack, or
    ///   a stack whose top overflows; and for `set_tid` or `set_tid_size`,
    ///   which pick the new task's ID and are not supported.
    pub fn read(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() > CLONE_ARGS_SIZE_MAX {
            return Err(E2BIG);
        }
        if bytes.len() < CLONE_ARGS_SIZE_VER0 {
            return Err(EINVAL);
        }
        if bytes.iter().skip(CLONE_ARGS_SIZE_VER2).any(|&byte| byte != 0) {
            return Err(E2BIG);
        }
        // What a short structure lacks reads as zero.
        let word = |at: usize| {
            let mut field = [0u8; 8];
            if let Some(source) = bytes.get(at..at + 8) {
                field.copy_from_slice(source);
            }
            u64::from_le_bytes(field)
        };
        let args = CloneArgs {
            flags: word(0),
            pidfd: word(8),
            child_tid: word(16),
            parent_tid: word(24),
            exit_signal: word(32),
            stack: word(40),
            stack_size: word(48),
            tls: word(56),
        };
        let (set_tid, set_tid_size) = (word(64), word(72));
        if set_tid != 0 || set_tid_size != 0 {
            return Err(EINVAL);
        }
        args.check_clone3_form()?;
        Ok(args)
    }

    /// Linux's `clone3_args_valid`: the rules that only a `struct clone_args`
    /// can break, because the raw call has no field to put the offence in.
    fn check_clone3_form(&self) -> Result<(), i32> {
        let flags = self.flags;
        // `CLONE_DETACHED` and the exit signal's byte are left free in
        // `clone3` so that they can be given a meaning later: a program that
        // sets them is refused, not ignored.
        if flags & !KNOWN_FLAGS != 0 || flags & CLONE_DETACHED != 0 {
            return Err(EINVAL);
        }
        if flags & (CLONE_THREAD | CLONE_PARENT) != 0 && self.exit_signal != 0 {
            return Err(EINVAL);
        }
        if flags & (CLONE_SIGHAND | CLONE_CLEAR_SIGHAND) == (CLONE_SIGHAND | CLONE_CLEAR_SIGHAND) {
            return Err(EINVAL);
        }
        // A stack is an address and a length together, and its top is an
        // address too.
        let has_stack = self.stack != 0;
        let has_size = self.stack_size != 0;
        if has_stack != has_size || self.stack.checked_add(self.stack_size).is_none() {
            return Err(EINVAL);
        }
        Ok(())
    }

    /// The stack pointer the new task starts with: `stack` for `clone`, and
    /// the top of the stack `clone3` names. 0 means the caller's own stack
    /// pointer, as for a `fork`.
    pub fn child_stack(&self) -> u64 {
        self.stack.saturating_add(self.stack_size)
    }
}

/// Which native operation `args` ask for, or the errno of the refusal.
///
/// Linux's own rules come first, as `copy_process` applies them, so that a
/// request Linux refuses gets Linux's errno: EINVAL for `CLONE_NEWNS` or
/// `CLONE_NEWUSER` with `CLONE_FS`, for `CLONE_THREAD` without
/// `CLONE_SIGHAND`, for `CLONE_SIGHAND` without `CLONE_VM`, for
/// `CLONE_THREAD` with `CLONE_NEWUSER` or `CLONE_NEWPID`, and for
/// `CLONE_CLEAR_SIGHAND` with `CLONE_SIGHAND`.
///
/// Then Dukovo's:
///
/// - Any `CLONE_NEW*` flag: EPERM. Namespaces come in a later change, and
///   until then no caller holds what they need.
/// - `CLONE_PIDFD`, `CLONE_PARENT` and `CLONE_INTO_CGROUP`: EINVAL for now.
/// - A thread (`CLONE_THREAD`, which Linux makes imply `CLONE_SIGHAND` and
///   `CLONE_VM`) must also have `CLONE_FS` and `CLONE_FILES`: EINVAL
///   without them, since a Dukovo thread always shares both. A thread with
///   `CLONE_VFORK` is EINVAL, as a thread has no `exec` of its own to wait
///   for.
/// - A process (no `CLONE_THREAD`) with `CLONE_VM` and no `CLONE_VFORK`, or
///   with `CLONE_FS`, `CLONE_FILES` or `CLONE_SIGHAND`: EINVAL, as
///   processes that share memory or state are not supported. `CLONE_VM`
///   with `CLONE_VFORK` is the `posix_spawn` and `vfork` of C libraries:
///   a process whose parent waits, with a copy of the memory and not the
///   memory itself.
///
/// An exit signal that is not a signal (above 64) is EINVAL for both
/// calls, though Linux ignores it in the raw `clone`: it cannot mean
/// anything, and the caller need not check it.
///
/// `CLONE_PTRACE`, `CLONE_UNTRACED`, `CLONE_IO`, `CLONE_DETACHED` and
/// `CLONE_SYSVSEM` are accepted and change nothing: a tracer cannot be
/// forced on the new task here, I/O contexts are not kept, Linux itself
/// ignores `CLONE_DETACHED`, and no System V semaphore adjustments are
/// shared. `CLONE_CLEAR_SIGHAND` is accepted for a process; the caller
/// resets the new process's handlers to the default when it is set.
///
/// # Errors
///
/// EINVAL or EPERM, as above.
pub fn classify(args: &CloneArgs) -> Result<Kind, i32> {
    let flags = args.flags;
    let has = |flag: u64| flags & flag != 0;
    if flags & !KNOWN_FLAGS != 0 || args.exit_signal > SIGNAL_MAX {
        return Err(EINVAL);
    }
    let in_new_root = has(CLONE_FS) && (has(CLONE_NEWNS) || has(CLONE_NEWUSER));
    let thread_without_handlers = has(CLONE_THREAD) && !has(CLONE_SIGHAND);
    let handlers_without_memory = has(CLONE_SIGHAND) && !has(CLONE_VM);
    let thread_in_new_ids = has(CLONE_THREAD) && (has(CLONE_NEWUSER) || has(CLONE_NEWPID));
    let handlers_cleared_and_shared = has(CLONE_SIGHAND) && has(CLONE_CLEAR_SIGHAND);
    if in_new_root
        || thread_without_handlers
        || handlers_without_memory
        || thread_in_new_ids
        || handlers_cleared_and_shared
    {
        return Err(EINVAL);
    }
    if flags & NAMESPACE_FLAGS != 0 {
        return Err(EPERM);
    }
    if has(CLONE_PIDFD) || has(CLONE_PARENT) || has(CLONE_INTO_CGROUP) {
        return Err(EINVAL);
    }
    if has(CLONE_THREAD) { classify_thread(flags) } else { classify_process(flags) }
}

/// A request with `CLONE_THREAD`, which already has `CLONE_SIGHAND` and
/// `CLONE_VM`.
fn classify_thread(flags: u64) -> Result<Kind, i32> {
    if flags & SHARED_STATE != SHARED_STATE || flags & CLONE_VFORK != 0 {
        return Err(EINVAL);
    }
    Ok(Kind::Thread)
}

/// A request with no `CLONE_THREAD`.
fn classify_process(flags: u64) -> Result<Kind, i32> {
    let vfork = flags & CLONE_VFORK != 0;
    let shares_memory = flags & CLONE_VM != 0 && !vfork;
    if shares_memory || flags & SHARED_STATE != 0 {
        return Err(EINVAL);
    }
    Ok(Kind::Process { vfork })
}
