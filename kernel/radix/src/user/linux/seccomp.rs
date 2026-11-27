// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's seccomp (ADR 0623): the filters a Linux program installs with
//! `seccomp(2)` or `prctl(PR_SET_SECCOMP)`, which judge each of its own
//! system calls before the table runs it, and strict mode, which allows
//! four. radix-bpf checks and runs the filters.
//!
//! Each thread has its mode, its chain of filters (newest first, each
//! holding the one before it, so that threads and forked children share
//! the filters they have in common), the calls its whole chain allows
//! whatever their arguments (Linux's cache), and `no_new_privs`. A new
//! thread takes them from the program thread that starts it, so `clone`,
//! `fork` and `execve` keep them, as on Linux ([`Seccomp::inherit`], run
//! by `ProcessObject::start` under the process's thread list lock, which a
//! TSYNC holds too, so that no thread started meanwhile misses one).
//!
//! A thread with no filter pays one load and branch per call ([`check`]);
//! one whose chain allows the call by its number makes it after one more
//! load; any other call runs every filter of the chain, and the strictest
//! value wins, ties to the newest filter, as in Linux's
//! `seccomp_run_filters`. The check runs before the table's own calls and
//! before those it forwards to linuxd.
//!
//! A filter speaks Linux's call numbers, so a filtered thread may not
//! `execve` a Dukovo program (`posix::exec` answers EACCES): it would
//! leave its filter behind. A USER_NOTIF listener is not one Dukovo makes:
//! the flags that ask for one are refused as unknown, and a filter's
//! USER_NOTIF, like its TRACE with no tracer, fails the call with ENOSYS,
//! as on Linux.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use radix_abi::signal::{SIGKILL, SIGSYS};
use radix_bpf::calls::{CALLS, WORDS};
use radix_bpf::seccomp::{AUDIT_ARCH_X86_64, Action, RET_ALLOW, RET_KILL_PROCESS, is_stricter};
use radix_bpf::{Calls, INSTRUCTION, Instruction, MAX_INSTRUCTIONS, Program, SeccompData};
use radix_hal::{SyscallRegs, UserMode};
use radix_klog::info;
use radix_linux::errno::{EACCES, EINVAL, ENOMEM, ENOSYS, EOPNOTSUPP, ESRCH};
use radix_linux::seccomp::{
    FILTER_PENALTY, FilterFlags, MAX_INSTRUCTIONS_PER_PATH, SECCOMP_GET_ACTION_AVAIL,
    SECCOMP_MODE_DISABLED, SECCOMP_MODE_FILTER, SECCOMP_MODE_STRICT, SECCOMP_SET_MODE_FILTER,
    SECCOMP_SET_MODE_STRICT, SOCK_FPROG_SIZE, STRICT_CALLS, SYS_SECCOMP, SockFprog, filter_flags,
};
use radix_mm::Kmalloc;
use radix_posix::signal::Info;
use radix_sync::SpinLock;

use super::{Args, Done, Error, Result, copy_in, fail, kmalloc};
use crate::Arch;
use crate::sched::{self, ThreadRef};
use crate::user::{UserRef, posix};

/// A filter, at the head of the chain it ends.
struct Filter {
    /// Its program, which radix-bpf checked.
    program: Program<Vec<Instruction, Kmalloc>>,
    /// The calls this filter and every one before it allow whatever their
    /// arguments.
    allows: Calls,
    /// SECCOMP_FILTER_FLAG_LOG: the calls it refuses are logged.
    log: bool,
    /// The instructions along the chain from this filter, as Linux counts
    /// them against [`MAX_INSTRUCTIONS_PER_PATH`]: those of each filter's
    /// conversion to eBPF ([`Program::converted_len`]), with
    /// [`FILTER_PENALTY`] more for each one before this.
    length: u32,
    /// The filters along the chain from this one.
    count: u32,
    /// The filter installed before it, which runs after it.
    parent: Option<Chain>,
}

/// A chain of filters, by its newest.
type Chain = Arc<Filter, Kmalloc>;

impl Drop for Filter {
    fn drop(&mut self) {
        // One at a time, not by recursion: a chain may hold thousands of
        // filters, more frames than the kernel's stack has room for.
        let mut next = self.parent.take();
        while let Some(filter) = next {
            next = Arc::into_inner(filter).and_then(|mut filter| filter.parent.take());
        }
    }
}

/// A thread's seccomp: its mode, its chain, the calls the chain allows by
/// number, and `no_new_privs`.
pub struct Seccomp {
    /// `SECCOMP_MODE_DISABLED`, `_STRICT` or `_FILTER`: read on every call.
    /// Written last, with Release, after the chain and `allowed`.
    mode: AtomicU8,
    /// Its chain, in filter mode.
    chain: SpinLock<Option<Chain>>,
    /// The chain's `allows`, a word at a time, which [`check`] reads
    /// without the lock. A TSYNC from another thread changes them only to
    /// fewer calls, so a word read while it does allows no call that the
    /// old chain did not.
    allowed: [AtomicU64; WORDS],
    /// PR_SET_NO_NEW_PRIVS, which no call clears.
    no_new_privs: AtomicBool,
}

impl Seccomp {
    /// A thread no filter limits.
    pub const fn new() -> Seccomp {
        Seccomp {
            mode: AtomicU8::new(SECCOMP_MODE_DISABLED),
            chain: SpinLock::new(None),
            allowed: [const { AtomicU64::new(u64::MAX) }; WORDS],
            no_new_privs: AtomicBool::new(false),
        }
    }

    /// Its mode.
    #[inline(always)]
    pub fn mode(&self) -> u8 {
        self.mode.load(Ordering::Acquire)
    }

    /// Whether strict mode or a filter limits its calls.
    pub fn is_on(&self) -> bool {
        self.mode() != SECCOMP_MODE_DISABLED
    }

    /// Whether it has `no_new_privs`.
    pub fn no_new_privs(&self) -> bool {
        self.no_new_privs.load(Ordering::Relaxed)
    }

    /// How many filters its chain has.
    pub fn filters(&self) -> u32 {
        self.chain.lock().as_ref().map_or(0, |chain| chain.count)
    }

    /// Whether its chain allows call `number` whatever its arguments.
    #[inline(always)]
    fn allows(&self, number: u64) -> bool {
        let Ok(index) = usize::try_from(number) else { return false };
        index < CALLS && self.allowed[index / 64].load(Ordering::Relaxed) & (1 << (index % 64)) != 0
    }

    /// Takes `from`'s mode, chain and `no_new_privs`: for a thread that
    /// `from`'s thread starts, before it runs, as Linux's `copy_seccomp`.
    pub fn inherit(&self, from: &Seccomp) {
        let chain = from.chain.lock().clone();
        for (word, from) in self.allowed.iter().zip(&from.allowed) {
            word.store(from.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        *self.chain.lock() = chain;
        self.no_new_privs.store(from.no_new_privs(), Ordering::Relaxed);
        self.mode.store(from.mode(), Ordering::Release);
    }

    /// `chain` is its chain now, in filter mode.
    fn set_chain(&self, chain: &Chain) {
        let words = chain.allows.words();
        // The old chain is the new one's parent, or an ancestor of it for
        // a TSYNC: it is not freed here, under the lock.
        let _old = self.chain.lock().replace(chain.clone());
        for (word, bits) in self.allowed.iter().zip(words) {
            word.store(bits, Ordering::Relaxed);
        }
        self.mode.store(SECCOMP_MODE_FILTER, Ordering::Release);
    }

    /// Whether its chain is `chain` or one of the filters `chain` holds:
    /// what a TSYNC may replace with `chain`. No chain is one.
    fn is_ancestor_of(&self, chain: Option<&Chain>) -> bool {
        let Some(mine) = self.chain.lock().clone() else { return true };
        let mut walker = chain;
        while let Some(filter) = walker {
            if Arc::ptr_eq(filter, &mine) {
                return true;
            }
            walker = filter.parent.as_ref();
        }
        false
    }
}

/// What seccomp makes of the call in `regs`, made by `me`, `user`'s
/// thread: `None` to run it, or its result. The fast path, inlined into
/// the table's `call`.
#[inline(always)]
pub fn check(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs) -> Option<Result<Done>> {
    let seccomp = &user.linux.seccomp;
    match seccomp.mode() {
        SECCOMP_MODE_DISABLED => None,
        SECCOMP_MODE_FILTER if seccomp.allows(regs.number) => None,
        mode => judge(me, user, regs, mode),
    }
}

/// [`check`], for a call strict mode or a filter must judge.
#[cold]
#[inline(never)]
fn judge(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs, mode: u8) -> Option<Result<Done>> {
    let number = regs.number;
    let at = instruction_pointer();
    if mode == SECCOMP_MODE_STRICT {
        if STRICT_CALLS.contains(&number) {
            return None;
        }
        // Linux ends the thread with SIGKILL, and logs it.
        log(me, user, number, at, "kill_thread (strict mode)");
        return Some(end_thread(me, user, SIGKILL, Info::default()));
    }
    let chain = user.linux.seccomp.chain.lock().clone();
    // The number as Linux's `int` holds it.
    let nr = number as i32;
    let data = SeccompData {
        nr,
        arch: AUDIT_ARCH_X86_64,
        instruction_pointer: at,
        args: core::array::from_fn(|i| regs.args[i]),
    };
    // A filter mode with no chain is a fault of the kernel's: fail closed,
    // as Linux does.
    let (value, requested) =
        chain.as_ref().map_or((RET_KILL_PROCESS, false), |chain| run(chain, &data));
    let action = Action::of(value);
    if action.is_logged(requested) {
        log(me, user, number, at, action.name());
    }
    let info = Info {
        code: SYS_SECCOMP,
        errno: i32::from(value as u16),
        pid: at,
        status: (u64::from(AUDIT_ARCH_X86_64) << 32) | u64::from(nr.cast_unsigned()),
    };
    match action {
        Action::Allow | Action::Log => None,
        Action::Errno(0) => Some(Ok(Done::Return(0))),
        Action::Errno(errno) => Some(Err(Error::Errno(i32::from(errno)))),
        // The call is not made, and rax keeps its number, as Linux rolls it
        // back to: the handler's frame shows it.
        Action::Trap(_) => {
            if !posix::signal::force(me, user, SIGSYS, info) {
                posix::signal::kill_current(me, user, SIGSYS, info);
            }
            Some(Ok(Done::Return(number)))
        }
        // No listener and no tracer.
        Action::UserNotif | Action::Trace(_) => Some(Err(Error::Errno(ENOSYS))),
        Action::KillThread => Some(end_thread(me, user, SIGSYS, info)),
        Action::KillProcess => posix::signal::kill_current(me, user, SIGSYS, info),
    }
}

/// The value of the chain from `chain` for `data`, and whether the filter
/// that gave it asked for its refusals to be logged.
fn run(chain: &Chain, data: &SeccompData) -> (u32, bool) {
    let (mut value, mut log) = (RET_ALLOW, false);
    let mut filter = Some(chain);
    while let Some(next) = filter {
        let got = next.program.run(data);
        if is_stricter(got, value) {
            (value, log) = (got, next.log);
        }
        filter = next.parent.as_ref();
    }
    (value, log)
}

/// Ends the calling thread for `signal`, as Linux's `do_exit` does, or its
/// whole process, killed by the signal, when it is the last thread.
fn end_thread(me: &ThreadRef, user: &UserRef, signal: u32, info: Info) -> Result<Done> {
    if user.process.thread_count() > 1 {
        return Ok(Done::ExitThread(u64::from(signal)));
    }
    posix::signal::kill_current(me, user, signal, info)
}

/// Where the call returns to: the address after its `syscall`.
fn instruction_pointer() -> u64 {
    Arch::user_registers().map_or(0, |registers| Arch::frame_registers(&registers)[0])
}

/// The lines [`log`] may write in each [`LOG_WINDOW_NS`], as Linux's
/// `printk` rate limit allows ten in five seconds.
const LOG_BURST: u64 = 10;
const LOG_WINDOW_NS: u64 = 5_000_000_000;

/// The window [`log`] last wrote in, shifted left by 8, and how many lines
/// it wrote there.
static LOGGED: AtomicU64 = AtomicU64::new(0);

/// Logs a call a filter or strict mode judged, as Linux's audit record of
/// it, at most [`LOG_BURST`] lines in a window.
fn log(me: &ThreadRef, user: &UserRef, number: u64, at: u64, action: &str) {
    let window = sched::now_ns() / LOG_WINDOW_NS;
    let taken = LOGGED.try_update(Ordering::Relaxed, Ordering::Relaxed, |word| {
        if word >> 8 != window {
            Some((window << 8) | 1)
        } else if word & 0xff < LOG_BURST {
            Some(word + 1)
        } else {
            None
        }
    });
    if taken.is_ok() {
        info!(
            "seccomp: process {} ({}) thread {}: call {number} at {at:#x}: {action}",
            user.process.number(),
            user.process.name().as_str(),
            super::state::tid(me),
        );
    }
}

/// `seccomp(operation, flags, args)`. The operation and the flags are
/// `unsigned int`s, as Linux reads them.
pub fn seccomp(
    me: &ThreadRef,
    user: &UserRef,
    operation: u64,
    flags: u64,
    args: u64,
) -> Result<u64> {
    let flags = flags as u32;
    match operation as u32 {
        SECCOMP_SET_MODE_STRICT if flags == 0 && args == 0 => set_strict(user),
        SECCOMP_SET_MODE_FILTER => {
            set_filter(me, user, filter_flags(flags).map_err(Error::Errno)?, args)
        }
        SECCOMP_GET_ACTION_AVAIL if flags == 0 => {
            let mut action = [0u8; 4];
            copy_in(args, &mut action)?;
            if Action::is_known(u32::from_le_bytes(action)) { Ok(0) } else { fail(EOPNOTSUPP) }
        }
        // And SECCOMP_GET_NOTIF_SIZES: no listener is one Dukovo makes.
        _ => fail(EINVAL),
    }
}

/// `prctl(PR_SET_SECCOMP, mode, filter)`: `seccomp` with no flags. Strict
/// mode takes no filter, whatever `filter` is.
pub fn prctl_set(me: &ThreadRef, user: &UserRef, mode: u64, filter: u64) -> Result<u64> {
    match mode {
        m if m == u64::from(SECCOMP_MODE_STRICT) => set_strict(user),
        m if m == u64::from(SECCOMP_MODE_FILTER) => {
            set_filter(me, user, FilterFlags::default(), filter)
        }
        _ => fail(EINVAL),
    }
}

/// `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)`.
pub fn set_no_new_privs(user: &UserRef, a: &Args) -> Result<u64> {
    if a[1] != 1 || a[2..5].iter().any(|&arg| arg != 0) {
        return fail(EINVAL);
    }
    user.linux.seccomp.no_new_privs.store(true, Ordering::Relaxed);
    Ok(0)
}

/// `prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0)`.
pub fn no_new_privs(user: &UserRef, a: &Args) -> Result<u64> {
    if a[1..5].iter().any(|&arg| arg != 0) {
        return fail(EINVAL);
    }
    Ok(u64::from(user.linux.seccomp.no_new_privs()))
}

/// `prctl(PR_GET_SECCOMP)`: the calling thread's mode.
pub fn mode(user: &UserRef) -> u64 {
    u64::from(user.linux.seccomp.mode())
}

/// SECCOMP_SET_MODE_STRICT: the calling thread may make only `read`,
/// `write`, `exit` and `rt_sigreturn` from now on. EINVAL once it has a
/// filter.
fn set_strict(user: &UserRef) -> Result<u64> {
    let seccomp = &user.linux.seccomp;
    let _installing = user.process.linux.installing.lock();
    match seccomp.mode() {
        SECCOMP_MODE_FILTER => fail(EINVAL),
        _ => {
            seccomp.mode.store(SECCOMP_MODE_STRICT, Ordering::Release);
            Ok(0)
        }
    }
}

/// SECCOMP_SET_MODE_FILTER: adds the filter `args` describes to the
/// calling thread's chain, and with TSYNC makes that chain every thread's.
/// The checks are made in Linux's order, so that a program probing what
/// the kernel has (a TSYNC with no filter answers EFAULT) learns what it
/// would on Linux.
fn set_filter(me: &ThreadRef, user: &UserRef, flags: FilterFlags, args: u64) -> Result<u64> {
    let mut record = [0u8; SOCK_FPROG_SIZE];
    copy_in(args, &mut record)?;
    let fprog = SockFprog::read(&record);
    let len = usize::from(fprog.len);
    if len == 0 || len > MAX_INSTRUCTIONS {
        return fail(EINVAL);
    }
    // No Linux program holds CAP_SYS_ADMIN here, so a filter needs
    // `no_new_privs`, as Linux asks of a program without it.
    let seccomp = &user.linux.seccomp;
    if !seccomp.no_new_privs() {
        return fail(EACCES);
    }
    let program = read_program(fprog.filter, len)?;
    let allows = program.always_allows(AUDIT_ARCH_X86_64);

    let _installing = user.process.linux.installing.lock();
    if seccomp.mode() == SECCOMP_MODE_STRICT {
        return fail(EINVAL);
    }
    let parent = seccomp.chain.lock().clone();
    let length = program.converted_len()
        + parent.as_ref().map_or(0, |parent| parent.length + FILTER_PENALTY);
    if length > MAX_INSTRUCTIONS_PER_PATH {
        return fail(ENOMEM);
    }
    let filter = Filter {
        program,
        allows: parent.as_ref().map_or(allows, |parent| parent.allows.meet(&allows)),
        log: flags.log,
        length,
        count: parent.as_ref().map_or(1, |parent| parent.count + 1),
        parent,
    };
    let chain: Chain = Arc::try_new_in(filter, kmalloc()).map_err(|_| Error::Errno(ENOMEM))?;
    if !flags.tsync {
        seccomp.set_chain(&chain);
        return Ok(0);
    }
    sync_threads(me, user, &chain, flags)
}

/// TSYNC: every thread of the process takes `chain`, the caller's new one,
/// and the caller's `no_new_privs`, under the lock of the process's list of
/// threads. A thread in strict mode, or whose chain is not one `chain`
/// grew from, stops it: nothing changes, and the answer is that thread's
/// number (ESRCH with TSYNC_ESRCH).
fn sync_threads(me: &ThreadRef, user: &UserRef, chain: &Chain, flags: FilterFlags) -> Result<u64> {
    let mut blocking = None;
    user.process.with_threads(&mut |threads| {
        // The caller's chain before this filter, which the others' may be.
        let before = chain.parent.as_ref();
        blocking = threads.iter().filter(|thread| !Arc::ptr_eq(thread, me)).find_map(|thread| {
            let other = &thread.user()?.linux.seccomp;
            let synced = match other.mode() {
                SECCOMP_MODE_DISABLED => true,
                SECCOMP_MODE_FILTER => other.is_ancestor_of(before),
                _ => false,
            };
            (!synced).then(|| super::state::tid(thread))
        });
        if blocking.is_some() {
            return;
        }
        let no_new_privs = user.linux.seccomp.no_new_privs();
        for thread in threads {
            let Some(other) = thread.user() else { continue };
            if no_new_privs {
                other.linux.seccomp.no_new_privs.store(true, Ordering::Relaxed);
            }
            other.linux.seccomp.set_chain(chain);
        }
    });
    match blocking {
        None => Ok(0),
        Some(_) if flags.tsync_esrch => fail(ESRCH),
        Some(tid) => Ok(u64::from(tid)),
    }
}

/// The program of `len` instructions at `address`, checked: EFAULT if it
/// cannot be read, ENOMEM without the memory for it, EINVAL if Linux would
/// refuse it.
fn read_program(address: u64, len: usize) -> Result<Program<Vec<Instruction, Kmalloc>>> {
    let mut instructions = Vec::new_in(kmalloc());
    instructions.try_reserve_exact(len).map_err(|_| Error::Errno(ENOMEM))?;
    // A few at a time, through the stack.
    let mut chunk = [0u8; 64 * INSTRUCTION];
    for start in (0..len).step_by(64) {
        let count = (len - start).min(64);
        let bytes = &mut chunk[..count * INSTRUCTION];
        copy_in(address.saturating_add((start * INSTRUCTION) as u64), bytes)?;
        for word in bytes.as_chunks::<INSTRUCTION>().0 {
            // Within the capacity reserved for `len`.
            let _ = instructions.push_within_capacity(Instruction::from_bytes(*word));
        }
    }
    let mut masks = Vec::new_in(kmalloc());
    masks.try_reserve_exact(len).map_err(|_| Error::Errno(ENOMEM))?;
    for _ in 0..len {
        // Within the capacity reserved for `len`.
        let _ = masks.push_within_capacity(0u16);
    }
    Program::check(instructions, &mut masks).map_err(|_| Error::Errno(EINVAL))
}
