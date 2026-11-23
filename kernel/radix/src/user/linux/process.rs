// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's calls for processes and threads (ADR 0610, 0611). Each ends in
//! a native method on the process's own capability (SLOT_PROCESS), as a
//! Dukovo program would call it, or in the kernel's own thread start,
//! `fork` and `exec`, which those methods use.
//!
//! A thread's Linux number is its kernel number, and a process's first
//! thread has the process's number, as on Linux. `clone` makes a thread
//! that shares all a Linux thread shares, or a process that shares
//! nothing (`radix_linux::clone::classify`). `vfork` and CLONE_VFORK give
//! the child a copy of the memory, not the memory itself, and the parent
//! waits until the child has run `exec` or begun to end, as Linux's does.
//! `execve` opens the program as Dukovo's C library opens it, and builds
//! the native exec block in the kernel.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use radix_abi::UserBuffer;
use radix_abi::address_space::USER_TOP;
use radix_abi::endpoint::NO_SLOT;
use radix_abi::errors::Kernel;
use radix_abi::process::{self as abi, ChildWaitFlags, EXEC_MAX, EXEC_VERSION, ExecHeader};
use radix_abi::signal::SIGCHLD;
use radix_abi::startup::SLOT_PROCESS;
use radix_hal::{SyscallRegs, UserMode};
use radix_linux::clone::{self, CloneArgs, Kind};
use radix_linux::errno::{self, E2BIG, EACCES, EINVAL, EISDIR, ENAMETOOLONG, ENOMEM, EPERM, ESRCH};
use radix_linux::{flags, records};
use radix_mm::Kmalloc;
use radix_posix::exec::STRINGS_MAX;

use super::{
    Done, Error, Result, copy_in, copy_out, fail, files, kernel_files, kmalloc, paths, results,
    state,
};
use crate::Arch;
use crate::sched::{ThreadRef, wait};
use crate::user::methods::Answer;
use crate::user::posix::exec::{self, Strings};
use crate::user::posix::fork::fork_returning;
use crate::user::process::{ProcessObject, ProcessRef, Start};
use crate::user::{UserRef, error};

/// `getpid`: the process's number.
pub fn getpid(user: &UserRef) -> u64 {
    user.process.number()
}

/// `gettid`: the calling thread's number.
pub fn gettid(me: &ThreadRef) -> u64 {
    u64::from(state::tid(me))
}

/// `getppid`: the parent's number, 0 for a process with no POSIX parent.
pub fn getppid(me: &ThreadRef, user: &UserRef) -> Result<u64> {
    let words = results(me, user, SLOT_PROCESS, abi::IDS, abi::IdsArgs { pid: 0 }.encode())?;
    Ok(abi::IdsResults::decode(&words)?.parent)
}

/// The user and group the process acts for: Linux's real and effective
/// IDs, which are one here.
pub fn identity(me: &ThreadRef, user: &UserRef) -> Result<(u32, u32)> {
    let args = abi::IdentityArgs { groups: UserBuffer::default() };
    let words = results(me, user, SLOT_PROCESS, abi::IDENTITY, args.encode())?;
    let identity = abi::IdentityResults::decode(&words)?;
    Ok((identity.user, identity.group))
}

/// `clone`, whose x86-64 arguments are the flags, the stack, `parent_tid`,
/// `child_tid` and `tls`, in that order.
pub fn clone(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs) -> Result<u64> {
    let a = &regs.args;
    new_task(me, user, regs, &CloneArgs::from_clone(a[0], a[1], a[2], a[3], a[4]))
}

/// `clone3`, with the `struct clone_args` of `size` bytes at `address`.
pub fn clone3(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs) -> Result<u64> {
    let (address, size) = (regs.args[0], regs.args[1]);
    if size > clone::CLONE_ARGS_SIZE_MAX as u64 {
        return fail(E2BIG);
    }
    if size < clone::CLONE_ARGS_SIZE_VER0 as u64 {
        return fail(EINVAL);
    }
    let mut bytes =
        radix_mm::bytes::zeroed(size as usize, kmalloc()).map_err(|_| Error::Errno(ENOMEM))?;
    copy_in(address, &mut bytes)?;
    let args = CloneArgs::read(&bytes).map_err(Error::Errno)?;
    new_task(me, user, regs, &args)
}

/// `fork`, and `vfork`, as the `clone` calls they are.
pub fn fork(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs, vfork: bool) -> Result<u64> {
    let shared = if vfork { clone::CLONE_VM | clone::CLONE_VFORK } else { 0 };
    let flags = shared | u64::from(SIGCHLD);
    new_task(me, user, regs, &CloneArgs::from_clone(flags, 0, 0, 0, 0))
}

/// The words a new task's thread number goes to (0: none): its
/// CLONE_CHILD_SETTID and CLONE_CHILD_CLEARTID words, in its memory, and
/// its CLONE_PARENT_SETTID word and the one CLONE_PIDFD puts the descriptor
/// of a process file at, in the caller's.
struct Words {
    set: u64,
    clear: u64,
    parent: u64,
    /// Where CLONE_PIDFD puts the new process's descriptor, in the caller's
    /// memory.
    pidfd: u64,
}

/// A new thread or process, as `args` ask, which comes back from the call
/// with 0 in rax and every other register as the caller had it, on its
/// own stack if it was given one. Returns its number.
fn new_task(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs, args: &CloneArgs) -> Result<u64> {
    let kind = clone::classify(args).map_err(Error::Errno)?;
    let has = |flag: u64| args.flags & flag != 0;
    let tls = if has(clone::CLONE_SETTLS) {
        // As Linux's ARCH_SET_FS refuses it.
        if args.tls >= USER_TOP {
            return fail(EPERM);
        }
        Some(args.tls)
    } else {
        None
    };
    let word = |flag: u64, at: u64| if has(flag) { at } else { 0 };
    let words = Words {
        set: word(clone::CLONE_CHILD_SETTID, args.child_tid),
        clear: word(clone::CLONE_CHILD_CLEARTID, args.child_tid),
        parent: word(clone::CLONE_PARENT_SETTID, args.parent_tid),
        pidfd: word(clone::CLONE_PIDFD, args.pidfd),
    };
    let returns = SyscallRegs { number: 0, args: regs.args };
    let stack = Some(args.child_stack()).filter(|&stack| stack != 0);
    match kind {
        Kind::Thread => thread(user, &returns, stack, tls, &words),
        Kind::Process { vfork } => {
            let clear_handlers = has(clone::CLONE_CLEAR_SIGHAND);
            let child = process(me, user, &returns, stack, tls, &words, clear_handlers)?;
            if vfork {
                wait_for_exec(me, &child);
            }
            Ok(child.number())
        }
    }
}

/// A new thread of the caller's process, which starts with the caller's
/// registers but `returns`, its thread pointer unless it is given `tls`,
/// its signal mask, and its extended state, as Linux's does, and no
/// alternate signal stack.
fn thread(
    user: &UserRef,
    returns: &SyscallRegs,
    stack: Option<u64>,
    tls: Option<u64>,
    words: &Words,
) -> Result<u64> {
    // (AArch64's registers are a stub, `()`, until its user mode comes.)
    #[allow(clippy::let_unit_value)]
    let mut regs = Arch::user_registers().ok_or(error(Kernel::Gone))?;
    Arch::set_results(&mut regs, returns);
    if let Some(stack) = stack {
        Arch::set_stack_pointer(&mut regs, stack);
    }
    let pointer = tls.unwrap_or_else(|| Arch::thread_pointer(&regs));
    let mask = user.signals.lock().mask;
    let thread = ProcessObject::start(
        &user.process,
        Start::Registers(regs),
        NO_SLOT,
        None,
        None,
        mask,
        &mut |thread| {
            thread.copy_extended_state();
            thread.set_thread_pointer(pointer);
            if let Some(new) = thread.user() {
                new.linux.set_child_tid.store(words.set, Ordering::Relaxed);
                new.linux.clear_child_tid.store(words.clear, Ordering::Relaxed);
            }
            // In the caller's memory before the thread can run, and so
            // end: a join waiting for the word to be cleared sees it set.
            put_tid(words.parent, state::tid(thread));
            Ok(())
        },
    )?;
    Ok(u64::from(state::tid(&thread)))
}

/// A child process that is a copy of the caller's (the kernel's `fork`),
/// whose thread starts as [`thread`]'s does but with the caller's signal
/// handlers and alternate signal stack, which `clear_handlers`
/// (CLONE_CLEAR_SIGHAND) sets back to the default as `exec` does.
fn process(
    me: &ThreadRef,
    user: &UserRef,
    returns: &SyscallRegs,
    stack: Option<u64>,
    tls: Option<u64>,
    words: &Words,
    clear_handlers: bool,
) -> Result<ProcessRef> {
    let mut made = None;
    // What failed in making the process file for CLONE_PIDFD, which ends the
    // fork with the error Linux gives, and the slot of the one made, which a
    // fork that fails after it closes.
    let mut pidfd_failed = None;
    let mut pidfd_slot = None;
    let forked = fork_returning(user, returns, stack, &mut |child, thread| {
        if let Some(tls) = tls {
            thread.set_thread_pointer(tls);
        }
        if let Some(new) = thread.user() {
            new.linux.set_child_tid.store(words.set, Ordering::Relaxed);
            new.linux.clear_child_tid.store(words.clear, Ordering::Relaxed);
        }
        if clear_handlers {
            child.posix.lock().actions.reset_for_exec();
        }
        // Last of the preparations: Linux makes it before the child runs,
        // and fails the call if it cannot.
        if words.pidfd != 0 {
            match kernel_files::clone_pidfd(me, user, child.number(), words.pidfd) {
                Ok(slot) => pidfd_slot = Some(slot),
                Err(err) => {
                    pidfd_failed = Some(err);
                    return Err(error(Kernel::Gone));
                }
            }
        }
        made = Some(child.clone());
        Ok(())
    });
    if let Some(err) = pidfd_failed {
        return Err(err);
    }
    if let Err(err) = forked {
        // The child never ran: its process file goes with it.
        if let Some(slot) = pidfd_slot {
            kernel_files::clone_pidfd_failed(me, user, slot);
        }
        return Err(err.into());
    }
    let child = made.ok_or(error(Kernel::Gone))?;
    // In the caller's memory, which the child's is a copy of from before.
    put_tid(words.parent, child.number() as u32);
    Ok(child)
}

/// Writes thread number `tid` at `word` in the caller's memory, unless
/// `word` is 0. A word the caller cannot write is left alone, as Linux
/// leaves it.
fn put_tid(word: u64, tid: u32) {
    if word != 0 {
        let _ = copy_out(word, &tid.to_le_bytes());
    }
}

/// Waits until `child`, made by `vfork` or CLONE_VFORK, has run `exec` or
/// begun to end, as Linux's parent waits; or until `me` is to stop. A
/// signal does not end the wait, as it does not end Linux's.
fn wait_for_exec(me: &ThreadRef, child: &ProcessObject) {
    // `exec` and every thread's end wake the child's threads key.
    wait::wait_until(child.threads_key(), "vfork", || {
        child.linux.execed() || child.is_ending() || me.should_stop()
    });
}

/// `execve` and `execveat`: runs the program at the path at `address`
/// (from `dirfd`), with the arguments and environment the arrays at `argv`
/// and `envp` point to, as Dukovo's C library's `execve` does: opens it
/// to read, closed by the exec, and calls the process's `exec` with a
/// block the kernel builds. Returns only on a failure.
pub fn execve(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    argv: u64,
    envp: u64,
    at_flags: u64,
) -> Result<Done> {
    let known = u64::from(flags::AT_EMPTY_PATH | flags::AT_SYMLINK_NOFOLLOW);
    if at_flags & !known != 0 {
        return fail(EINVAL);
    }
    let path = paths::read(address)?;
    let block = block(user, &path, argv, envp)?;
    let (slot, opened) = if path.is_empty() && at_flags & u64::from(flags::AT_EMPTY_PATH) != 0 {
        // `fexecve`: the descriptor itself, which the exec leaves open
        // unless it closes on exec.
        (paths::slot_of(dirfd)?, false)
    } else {
        let no_follow = at_flags & u64::from(flags::AT_SYMLINK_NOFOLLOW) != 0;
        let open_flags = flags::O_CLOEXEC | if no_follow { flags::O_NOFOLLOW } else { 0 };
        (files::open_path(me, user, dirfd, &path, open_flags)?, true)
    };
    let ran =
        exec::exec_from(user, &user.process, SLOT_PROCESS, abi::EXEC, slot, Strings::Given(block));
    match ran {
        Ok(Answer::Exit(code)) => Ok(Done::Exit(code)),
        // Its own process's exec answers only so.
        Ok(_) => fail(errno::EIO),
        Err(status) => {
            if opened {
                files::delete(me, user, slot);
            }
            match errno::from_status(status) {
                // A directory is not a program a search may run.
                EISDIR => fail(EACCES),
                _ => Err(Error::Native(status)),
            }
        }
    }
}

/// The native exec block: the strings the NULL-ended arrays at `argv`
/// and `envp` point to (an array at 0 is empty, as Linux takes it), then
/// the working directory and `path`, which the program gets as AT_EXECFN.
fn block(user: &UserRef, path: &[u8], argv: u64, envp: u64) -> Result<Vec<u8, Kmalloc>> {
    let mut bytes = Vec::new_in(kmalloc());
    bytes.try_reserve(4096).map_err(|_| Error::Errno(ENOMEM))?;
    radix_mm::bytes::extend(&mut bytes, &[0; ExecHeader::SIZE])
        .map_err(|_| Error::Errno(ENOMEM))?;
    let mut args = strings(&mut bytes, argv, 0)?;
    if args == 0 {
        // As Linux since 5.18: a program run with no arguments gets an
        // empty first one, so that it does not take its environment's
        // first string for it.
        push(&mut bytes, b"")?;
        args = 1;
    }
    let env = strings(&mut bytes, envp, args)?;
    let cwd = user.process.posix.lock().directory.clone();
    push(&mut bytes, cwd.as_deref().map_or(&b"/"[..], |cwd| &cwd[..]))?;
    push(&mut bytes, path)?;
    for (at, value) in [
        (ExecHeader::OFFSET_VERSION, EXEC_VERSION),
        (ExecHeader::OFFSET_ARGS, args),
        (ExecHeader::OFFSET_ENV, env),
        (ExecHeader::OFFSET_EXTRA, 2),
    ] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}

/// Adds the strings the NULL-ended array of pointers at `array` points
/// to, to `bytes`, after `before` strings, and returns how many there
/// were: E2BIG past what a block holds, EFAULT for one the caller cannot
/// read.
fn strings(bytes: &mut Vec<u8, Kmalloc>, array: u64, before: u32) -> Result<u32> {
    if array == 0 {
        return Ok(0);
    }
    let mut count = 0u32;
    loop {
        let mut pointer = [0u8; 8];
        let at = array.checked_add(u64::from(count) * 8).ok_or(Error::Errno(errno::EFAULT))?;
        copy_in(at, &mut pointer)?;
        let pointer = u64::from_le_bytes(pointer);
        if pointer == 0 {
            return Ok(count);
        }
        // Room for the two extra strings is left.
        if before + count + 2 >= STRINGS_MAX {
            return fail(E2BIG);
        }
        let room = (EXEC_MAX as usize).saturating_sub(bytes.len() + 1);
        let string = paths::read_string(pointer, room).map_err(|err| match err {
            Error::Errno(ENAMETOOLONG) => Error::Errno(E2BIG),
            err => err,
        })?;
        push(bytes, &string)?;
        count += 1;
    }
}

/// Adds `string` and its NUL to `bytes`, or fails with E2BIG past
/// EXEC_MAX.
fn push(bytes: &mut Vec<u8, Kmalloc>, string: &[u8]) -> Result<()> {
    if (bytes.len() + string.len() + 1) as u64 > EXEC_MAX {
        return fail(E2BIG);
    }
    radix_mm::bytes::extend(bytes, string).map_err(|_| Error::Errno(ENOMEM))?;
    radix_mm::bytes::extend(bytes, &[0]).map_err(|_| Error::Errno(ENOMEM))?;
    Ok(())
}

/// `exit_group`: the process ends with `code`'s low byte, and the caller
/// leaves.
pub fn exit_group(me: &ThreadRef, user: &UserRef, code: u64) -> Result<Done> {
    let args = abi::ExitArgs { code: code & 0xff };
    match super::method(me, user, SLOT_PROCESS, abi::EXIT, args.encode())? {
        Answer::Exit(code) => Ok(Done::Exit(code)),
        _ => fail(errno::EIO),
    }
}

/// `wait4`'s options Linux takes: WNOHANG, WUNTRACED, WCONTINUED, and
/// __WNOTHREAD, __WALL and __WCLONE, which ask for nothing different here.
const WAIT4_OPTIONS: u64 = 0x1 | 0x2 | 0x8 | 0x2000_0000 | 0x4000_0000 | 0x8000_0000;

/// `wait4`: waits for a child as `waitpid` does, and writes its status
/// and its CPU time, as the native `wait` gives them.
pub fn wait4(
    me: &ThreadRef,
    user: &UserRef,
    pid: u64,
    status: u64,
    options: u64,
    usage: u64,
) -> Result<u64> {
    let options = options & u64::from(u32::MAX);
    if options & !WAIT4_OPTIONS != 0 {
        return fail(EINVAL);
    }
    let pid = pid as i32;
    if pid == i32::MIN {
        return fail(ESRCH);
    }
    let mut wait = ChildWaitFlags::NONE;
    for (bit, flag) in [
        (0x1, ChildWaitFlags::NO_HANG),
        (0x2, ChildWaitFlags::STOPPED),
        (0x8, ChildWaitFlags::CONTINUED),
    ] {
        if options & bit != 0 {
            wait = wait.union(flag);
        }
    }
    let args = abi::WaitArgs { pid: i64::from(pid), flags: wait };
    let found =
        abi::WaitResults::decode(&results(me, user, SLOT_PROCESS, abi::WAIT, args.encode())?)?;
    // Nothing is written for WNOHANG's 0, as Linux writes nothing.
    if found.pid != 0 {
        if status != 0 {
            copy_out(status, &found.status.to_le_bytes())?;
        }
        if usage != 0 {
            let mut out = [0u8; records::RUSAGE_SIZE];
            records::put_rusage(found.user_ns, found.system_ns, 0, &mut out);
            copy_out(usage, &out)?;
        }
    }
    Ok(found.pid)
}

/// Whether `signal`, a Linux `int`, is a signal or 0; EINVAL if not.
pub(super) fn signal_number(signal: u64) -> Result<u32> {
    match u32::try_from(signal as i32) {
        Ok(signal) if signal <= radix_abi::signal::SIGRTMAX => Ok(signal),
        _ => fail(EINVAL),
    }
}

/// `kill`: the native `kill`, which takes the same process and group
/// numbers.
pub fn kill(me: &ThreadRef, user: &UserRef, pid: u64, signal: u64) -> Result<u64> {
    let signal = signal_number(signal)?;
    let pid = pid as i32;
    if pid == i32::MIN {
        return fail(ESRCH);
    }
    let args = abi::KillArgs { pid: i64::from(pid), signal };
    results(me, user, SLOT_PROCESS, abi::KILL, args.encode()).map(|_| 0)
}

/// `tgkill`, and with no `tgid` `tkill`: the native `signal_thread`,
/// which reaches the threads of the caller's own process.
pub fn tgkill(
    me: &ThreadRef,
    user: &UserRef,
    tgid: Option<u64>,
    tid: u64,
    signal: u64,
) -> Result<u64> {
    let signal = signal_number(signal)?;
    let tid = tid as i32;
    let tgid = tgid.map(|tgid| tgid as i32);
    if tid <= 0 || tgid.is_some_and(|tgid| tgid <= 0) {
        return fail(EINVAL);
    }
    // The threads of other processes are not reached yet
    // (docs/deferred.md): Linux gives ESRCH for a thread it does not find.
    if tgid.is_some_and(|tgid| tgid as u64 != user.process.number()) {
        return fail(ESRCH);
    }
    let args = abi::SignalThreadArgs { thread: tid as u64, signal };
    results(me, user, SLOT_PROCESS, abi::SIGNAL_THREAD, args.encode()).map(|_| 0)
}

/// `arch_prctl`'s codes this answers.
const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;
const ARCH_GET_CPUID: u64 = 0x1011;
const ARCH_SET_CPUID: u64 = 0x1012;

/// `arch_prctl`: the thread pointer (the FS base), and whether CPUID runs,
/// which it always does. Any other code is EINVAL, as Linux gives it for a
/// code it does not know; the GS base is the kernel's.
pub fn arch_prctl(me: &ThreadRef, user: &UserRef, code: u64, address: u64) -> Result<u64> {
    match code {
        ARCH_SET_FS => {
            if address >= USER_TOP {
                return fail(EPERM);
            }
            let args = abi::SetThreadPointerArgs { base: address };
            results(me, user, SLOT_PROCESS, abi::SET_THREAD_POINTER, args.encode()).map(|_| 0)
        }
        ARCH_GET_FS => {
            #[allow(clippy::let_unit_value)]
            let regs = Arch::user_registers().ok_or(error(Kernel::Gone))?;
            copy_out(address, &Arch::thread_pointer(&regs).to_le_bytes()).map(|()| 0)
        }
        ARCH_GET_CPUID => Ok(1),
        ARCH_SET_CPUID if address != 0 => Ok(0),
        // Linux's answer where the CPU cannot make CPUID fault.
        ARCH_SET_CPUID => fail(errno::ENODEV),
        _ => fail(EINVAL),
    }
}

/// `set_tid_address`: the calling thread's clear-child-tid word. Returns
/// its number.
pub fn set_tid_address(me: &ThreadRef, user: &UserRef, word: u64) -> u64 {
    user.linux.clear_child_tid.store(word, Ordering::Relaxed);
    gettid(me)
}

/// The size of `struct robust_list_head`, the only length
/// `set_robust_list` takes.
const ROBUST_LIST_HEAD_SIZE: u64 = 24;

/// `set_robust_list`: kept for `get_robust_list`.
pub fn set_robust_list(user: &UserRef, head: u64, len: u64) -> Result<u64> {
    if len != ROBUST_LIST_HEAD_SIZE {
        return fail(EINVAL);
    }
    user.linux.robust_list.store(head, Ordering::Relaxed);
    user.linux.robust_len.store(len, Ordering::Relaxed);
    Ok(0)
}

/// `get_robust_list` of thread `tid` of the caller's process (0: the
/// caller): writes its head and length at `head` and `len`.
pub fn get_robust_list(user: &UserRef, tid: u64, head: u64, len: u64) -> Result<u64> {
    let tid = tid as i32;
    let found = if tid == 0 {
        Some(user.clone())
    } else {
        u64::try_from(tid)
            .ok()
            .and_then(|tid| crate::user::posix::find_thread(&user.process, tid))
            .map(|(_, user)| user)
    };
    let found = found.ok_or(Error::Errno(ESRCH))?;
    copy_out(head, &found.linux.robust_list.load(Ordering::Relaxed).to_le_bytes())?;
    copy_out(len, &found.linux.robust_len.load(Ordering::Relaxed).to_le_bytes())?;
    Ok(0)
}

/// The file the machine's name is in, from the process's root, as Dukovo's
/// C library reads it (`gethostname`).
const HOSTNAME_FILE: &[u8] = b"system/settings/hostname";

/// The name a machine has until it is given one.
const DEFAULT_HOSTNAME: &[u8] = b"dukovo";

/// The release a Linux program sees: a Linux it can run on.
const RELEASE: &[u8] = b"6.1.0-dukovo";

/// `uname`: "Linux", the machine's name as Dukovo's C library reads it,
/// [`RELEASE`], the kernel's version, and "x86_64" (ADR 0610).
pub fn uname(user: &UserRef, buffer: u64) -> Result<u64> {
    let mut name = [0u8; 66];
    let read = exec::read_from_root(&user.process, HOSTNAME_FILE, &mut name).unwrap_or(0);
    let line = name[..read].split(|&b| b == b'\n').next().unwrap_or(&[]);
    let end = line.iter().rposition(|&b| !matches!(b, b' ' | b'\t' | b'\r')).map_or(0, |at| at + 1);
    // None, or one too long to be a name: the default.
    let nodename = match &line[..end] {
        name if name.is_empty() || name.len() > 64 || name.contains(&0) => DEFAULT_HOSTNAME,
        name => name,
    };
    let version = concat!("#1 SMP Dukovo ", env!("CARGO_PKG_VERSION"));
    let mut out = [0u8; records::UTSNAME_SIZE];
    records::utsname(nodename, RELEASE, version.as_bytes(), &mut out);
    copy_out(buffer, &out).map(|()| 0)
}
