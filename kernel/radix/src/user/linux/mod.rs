// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Linux table (Phase 18, ADR 0610): the system calls of a Linux
//! program, served by the methods of the program's own capabilities.
//!
//! `exec` runs every x86-64 program that is not Dukovo's as a Linux
//! program (radix-linux(7)): its process's [`LinuxProcess`] is on, and
//! each `syscall` it makes comes here in place of Radix's twelve calls
//! (`syscall::handle`). The call's number is in rax and its arguments in
//! rdi, rsi, rdx, r10, r8 and r9, as on Linux; its result, or a negative
//! error number, goes back in rax, and every other register comes back as
//! it went in (but rcx and r11, which `sysret` takes).
//!
//! Each call ends in a method the program could call itself, on a
//! capability in its own slots: a file's `read`, a directory's `open`, its
//! process's `fork`, its address space's `map_anonymous`. The table finds
//! the slots a Linux call means (a descriptor n is slot SLOT_FD_BASE + n;
//! a path is resolved from SLOT_ROOT and the working directory,
//! `paths`), turns Linux's arguments into the method's, and the method's
//! answer into Linux's. So a Linux program has no authority a Dukovo
//! program in its place would not have, and every check a method makes
//! is made for it too. What Linux keeps for a program that Radix leaves
//! to the C library (the working directory, the lowest free descriptor, a
//! directory's listing cursor, the break, the thread words of `clone`) the
//! kernel keeps for a Linux program (`state`, ADR 0611).
//!
//! A call a signal interrupts fails with EINTR, or runs again, as the
//! method's `interrupted` status says (Linux's ERESTARTSYS and
//! ERESTARTNOHAND): the registers it was made with are kept, so the way
//! back makes the `syscall` instruction run again.
//!
//! Calls Linux has that Dukovo will not give a program answer as Linux
//! answers a program without the privilege: ENOSYS for io_uring, BPF,
//! perf events, ptrace, kexec and modules, EPERM for mounts, reboot,
//! swap and setting the clock or the host's name. A number the table
//! does not know answers ENOSYS.

mod descriptors;
mod events;
mod files;
mod kernel_files;
mod memory;
mod names;
mod notify;
mod paths;
mod poll;
mod process;
mod robust;
mod signals;
mod sockets;
mod state;
mod system;
mod time;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_hal::{SyscallRegs, UserMode};
use radix_ipc::info::Info;
use radix_linux::errno::{self, EINTR, ENOSYS, EPERM};
use radix_linux::nr;
use radix_mm::{Flags, HEAP, Kmalloc};

pub use robust::ending;
pub use state::{LinuxProcess, LinuxThread, starting};

use super::methods::{self, Answer};
use super::objects;
use super::process::Ending;
use super::{UserRef, posix};
use crate::Arch;
use crate::sched::ThreadRef;

/// Why a Linux call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// It fails with this error number.
    Errno(i32),
    /// The method it called failed with this status, which becomes an
    /// error number as Dukovo's C library makes it one
    /// (`radix_linux::errno::from_status`), unless it is `interrupted`,
    /// whose detail says whether the call runs again. Kept as a status
    /// until then, so that a call can tell one method's failure from
    /// another's.
    Native(Status),
}

impl From<Status> for Error {
    fn from(status: Status) -> Error {
        Error::Native(status)
    }
}

/// Whether `status` is `kind`'s, whatever its detail.
fn is(status: Status, kind: Kernel) -> bool {
    let wanted = kind.status(0);
    status.domain() == wanted.domain() && status.code() == wanted.code()
}

/// A Linux call's result: what goes in rax, or why it failed.
pub type Result<T> = core::result::Result<T, Error>;

/// The heap, for what a call may sleep to allocate.
fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// Fails with error number `errno`.
fn fail<T>(errno: i32) -> Result<T> {
    Err(Error::Errno(errno))
}

impl From<radix_abi::Invalid> for Error {
    fn from(invalid: radix_abi::Invalid) -> Error {
        Error::Native(Status::from(invalid))
    }
}

/// Copies `out.len()` bytes from `address` in the caller's memory, or
/// fails with EFAULT.
fn copy_in(address: u64, out: &mut [u8]) -> Result<()> {
    if Arch::copy_from_user(out, address) { Ok(()) } else { fail(errno::EFAULT) }
}

/// Copies `bytes` to `address` in the caller's memory, or fails with
/// EFAULT.
fn copy_out(address: u64, bytes: &[u8]) -> Result<()> {
    if Arch::copy_to_user(address, bytes) { Ok(()) } else { fail(errno::EFAULT) }
}

/// What a Linux call did.
pub enum Done {
    /// It returned this, which goes in rax.
    Return(u64),
    /// The calling thread exits with this code: Linux's `exit`.
    ExitThread(u64),
    /// The calling thread leaves with this code, as a native method's
    /// `Answer::Exit` makes it: after `exit_group`, whose process has
    /// begun to end, or after `execve`, whose new program's thread has
    /// taken its place.
    Exit(u64),
    /// It returned from a signal handler: the registers are what the
    /// handler's frame saved.
    Restore(SyscallRegs),
}

/// A call's six arguments, in Linux's order: rdi, rsi, rdx, r10, r8, r9.
pub type Args = [u64; 6];

/// Runs the Linux call in `regs`, made by `me`, `user`'s thread, and puts
/// its result in rax. Leaves the other registers as they are; an `exit`
/// does not return.
pub fn call(me: &ThreadRef, user: &UserRef, regs: &mut SyscallRegs) {
    let number = regs.number;
    match dispatch(me, user, regs) {
        Ok(Done::Return(value)) => regs.number = value,
        Ok(Done::Restore(saved)) => *regs = saved,
        Ok(Done::Exit(code)) => super::exit_current(user.clone(), code, Ending::Exit),
        Ok(Done::ExitThread(code)) => {
            let word = user.linux.clear_child_tid.load(core::sync::atomic::Ordering::Relaxed);
            if word == 0 {
                super::exit_current(user.clone(), code, Ending::Exit)
            }
            super::exit_clearing(user.clone(), code, word)
        }
        Err(Error::Errno(errno)) => regs.number = negative(errno),
        Err(Error::Native(status)) if is(status, Kernel::Interrupted) => {
            // Kept with the registers it was made with, which this leaves
            // as they were, to run again if the signal allows; a `poll`
            // keeps its timeout as what is left of it.
            let mut args = regs.args;
            if let Some(left) = user.linux.take_poll_left() {
                args[2] = left;
            }
            posix::signal::call_failed(me, user, number, &args, status);
            regs.number = negative(EINTR);
        }
        Err(Error::Native(status)) => regs.number = negative(errno::from_status(status)),
    }
}

/// rax for a call that failed with `errno`.
fn negative(errno: i32) -> u64 {
    (-i64::from(errno)) as u64
}

/// The Linux call in `regs`: its number, and its arguments in the first
/// six words. The calls that make a thread or a process give it all ten,
/// which it returns with.
#[allow(clippy::too_many_lines)]
fn dispatch(me: &ThreadRef, user: &UserRef, regs: &SyscallRegs) -> Result<Done> {
    let a: &Args = &core::array::from_fn(|i| regs.args[i]);
    let value = match regs.number {
        // Files and descriptors.
        nr::READ => files::read(me, user, a[0], a[1], a[2])?,
        nr::WRITE => files::write(me, user, a[0], a[1], a[2])?,
        nr::PREAD64 => files::pread(me, user, a[0], a[1], a[2], a[3])?,
        nr::PWRITE64 => files::pwrite(me, user, a[0], a[1], a[2], a[3])?,
        nr::OPEN => files::openat(me, user, paths::AT_FDCWD, a[0], a[1], a[2])?,
        nr::CREAT => files::openat(me, user, paths::AT_FDCWD, a[0], files::CREAT_FLAGS, a[1])?,
        nr::OPENAT => files::openat(me, user, a[0], a[1], a[2], a[3])?,
        nr::CLOSE => files::close(me, user, a[0])?,
        nr::STAT => files::stat_at(me, user, paths::AT_FDCWD, a[0], a[1], 0)?,
        nr::LSTAT => files::stat_at(me, user, paths::AT_FDCWD, a[0], a[1], files::NO_FOLLOW)?,
        nr::FSTAT => files::fstat(me, user, a[0], a[1])?,
        nr::NEWFSTATAT => files::stat_at(me, user, a[0], a[1], a[2], a[3])?,
        nr::STATX => files::statx(me, user, a[0], a[1], a[2], a[3], a[4])?,
        nr::GETDENTS64 => files::getdents64(user, a[0], a[1], a[2])?,
        nr::GETCWD => files::getcwd(user, a[0], a[1])?,
        nr::CHDIR => files::chdir(user, a[0])?,
        nr::FCHDIR => files::fchdir(user, a[0])?,
        // Descriptors and names (descriptors.rs, names.rs).
        nr::READV => descriptors::readv(me, user, a[0], a[1], a[2])?,
        nr::WRITEV => descriptors::writev(me, user, a[0], a[1], a[2])?,
        nr::PREADV => descriptors::preadv(me, user, a[0], a[1], a[2], a[3])?,
        nr::PWRITEV => descriptors::pwritev(me, user, a[0], a[1], a[2], a[3])?,
        nr::CLOSE_RANGE => descriptors::close_range(me, user, a[0], a[1], a[2])?,
        nr::LSEEK => descriptors::lseek(me, user, a[0], a[1], a[2])?,
        nr::IOCTL => descriptors::ioctl(me, user, a[0], a[1], a[2])?,
        nr::ACCESS => names::faccessat(user, paths::AT_FDCWD, a[0], a[1], 0)?,
        nr::FACCESSAT => names::faccessat(user, a[0], a[1], a[2], 0)?,
        nr::FACCESSAT2 => names::faccessat(user, a[0], a[1], a[2], a[3])?,
        nr::PIPE => descriptors::pipe(me, user, a[0])?,
        nr::PIPE2 => descriptors::pipe2(me, user, a[0], a[1])?,
        nr::DUP => descriptors::dup(me, user, a[0])?,
        nr::DUP2 => descriptors::dup2(me, user, a[0], a[1])?,
        nr::DUP3 => descriptors::dup3(me, user, a[0], a[1], a[2])?,
        nr::FCNTL => descriptors::fcntl(me, user, a[0], a[1], a[2])?,
        nr::FLOCK => descriptors::flock(me, user, a[0], a[1])?,
        nr::FSYNC | nr::FDATASYNC => descriptors::fsync(me, user, a[0])?,
        nr::TRUNCATE => names::truncate(me, user, a[0], a[1])?,
        nr::FTRUNCATE => descriptors::ftruncate(me, user, a[0], a[1])?,
        nr::FALLOCATE => descriptors::fallocate(me, user, a[0], a[1], a[2], a[3])?,
        nr::RENAME => names::rename(user, paths::AT_FDCWD, a[0], paths::AT_FDCWD, a[1], 0)?,
        nr::RENAMEAT => names::rename(user, a[0], a[1], a[2], a[3], 0)?,
        nr::RENAMEAT2 => names::rename(user, a[0], a[1], a[2], a[3], a[4])?,
        nr::MKDIR => names::mkdirat(user, paths::AT_FDCWD, a[0], a[1])?,
        nr::MKDIRAT => names::mkdirat(user, a[0], a[1], a[2])?,
        nr::RMDIR => names::unlinkat(user, paths::AT_FDCWD, a[0], names::REMOVE_DIRECTORY)?,
        nr::LINK => names::linkat(user, paths::AT_FDCWD, a[0], paths::AT_FDCWD, a[1], 0)?,
        nr::LINKAT => names::linkat(user, a[0], a[1], a[2], a[3], a[4])?,
        nr::UNLINK => names::unlinkat(user, paths::AT_FDCWD, a[0], 0)?,
        nr::UNLINKAT => names::unlinkat(user, a[0], a[1], a[2])?,
        nr::SYMLINK => names::symlinkat(user, a[0], paths::AT_FDCWD, a[1])?,
        nr::SYMLINKAT => names::symlinkat(user, a[0], a[1], a[2])?,
        nr::READLINK => names::readlinkat(user, paths::AT_FDCWD, a[0], a[1], a[2])?,
        nr::READLINKAT => names::readlinkat(user, a[0], a[1], a[2], a[3])?,
        nr::CHMOD => names::fchmodat(user, paths::AT_FDCWD, a[0], a[1])?,
        nr::FCHMOD => descriptors::fchmod(me, user, a[0], a[1])?,
        nr::FCHMODAT => names::fchmodat(user, a[0], a[1], a[2])?,
        nr::CHOWN => names::fchownat(me, user, paths::AT_FDCWD, a[0], a[1], a[2], 0)?,
        nr::FCHOWN => descriptors::fchown(me, user, a[0], a[1], a[2])?,
        nr::LCHOWN => {
            names::fchownat(me, user, paths::AT_FDCWD, a[0], a[1], a[2], files::NO_FOLLOW)?
        }
        nr::FCHOWNAT => names::fchownat(me, user, a[0], a[1], a[2], a[3], a[4])?,
        nr::UMASK => descriptors::umask(me, user, a[0])?,
        nr::UTIMENSAT => names::utimensat(me, user, a[0], a[1], a[2], a[3])?,
        nr::UTIMES => names::futimesat(me, user, paths::AT_FDCWD, a[0], a[1])?,
        nr::FUTIMESAT => names::futimesat(me, user, a[0], a[1], a[2])?,
        nr::MKNOD => names::mknodat(user, paths::AT_FDCWD, a[0], a[1])?,
        nr::MKNODAT => names::mknodat(user, a[0], a[1], a[2])?,
        // Polling, sockets and the system (poll.rs, sockets.rs, system.rs).
        nr::POLL => poll::poll(user, a[0], a[1], a[2])?,
        nr::SELECT => poll::select(user, a)?,
        nr::SOCKET => sockets::socket(me, user, a[0], a[1], a[2])?,
        nr::CONNECT => sockets::connect(me, user, a[0], a[1], a[2])?,
        nr::ACCEPT => sockets::accept(me, user, a[0], a[1], a[2], 0)?,
        nr::SENDTO => sockets::sendto(me, user, a)?,
        nr::RECVFROM => sockets::recvfrom(me, user, a)?,
        nr::SENDMSG => sockets::sendmsg(me, user, a[0], a[1], a[2])?,
        nr::RECVMSG => sockets::recvmsg(me, user, a[0], a[1], a[2])?,
        nr::SHUTDOWN => sockets::shutdown(me, user, a[0], a[1])?,
        nr::BIND => sockets::bind(me, user, a[0], a[1], a[2])?,
        nr::LISTEN => sockets::listen(me, user, a[0], a[1])?,
        nr::GETSOCKNAME => sockets::getsockname(user, a[0], a[1], a[2])?,
        nr::GETPEERNAME => sockets::getpeername(user, a[0], a[1], a[2])?,
        nr::SOCKETPAIR => sockets::socketpair(me, user, a[0], a[1], a[2], a[3])?,
        nr::SETSOCKOPT => sockets::setsockopt(me, user, a[0], a[1], a[2], a[3], a[4])?,
        nr::GETSOCKOPT => sockets::getsockopt(me, user, a[0], a[1], a[2], a[3], a[4])?,
        nr::GETTIMEOFDAY => system::gettimeofday(me, user, a[0], a[1])?,
        nr::GETRLIMIT => system::getrlimit(me, user, a[0], a[1])?,
        nr::GETRUSAGE => system::getrusage(me, user, a[0], a[1])?,
        nr::SYSINFO => system::sysinfo(user, a[0])?,
        nr::TIMES => system::times(me, user, a[0])?,
        nr::SETUID => system::set_id(me, user, a[0], false)?,
        nr::SETGID => system::set_id(me, user, a[0], true)?,
        nr::SETPGID => system::setpgid(me, user, a[0], a[1])?,
        nr::GETPGRP => system::getpgrp(me, user)?,
        nr::SETSID => system::setsid(me, user)?,
        nr::SETREUID => system::set_ids(me, user, &a[..2], false)?,
        nr::SETREGID => system::set_ids(me, user, &a[..2], true)?,
        nr::GETGROUPS => system::getgroups(me, user, a[0], a[1])?,
        nr::SETGROUPS => system::setgroups(me, user, a[0], a[1])?,
        nr::SETRESUID => system::set_ids(me, user, &a[..3], false)?,
        nr::GETRESUID => system::get_res_ids(me, user, &a[..3], false)?,
        nr::SETRESGID => system::set_ids(me, user, &a[..3], true)?,
        nr::GETRESGID => system::get_res_ids(me, user, &a[..3], true)?,
        nr::GETPGID => system::getpgid(me, user, a[0])?,
        nr::SETFSUID => system::get_fs_id(me, user, false)?,
        nr::SETFSGID => system::get_fs_id(me, user, true)?,
        nr::GETSID => system::getsid(me, user, a[0])?,
        nr::RT_SIGPENDING => signals::rt_sigpending(me, user, a[0], a[1])?,
        nr::RT_SIGTIMEDWAIT => signals::rt_sigtimedwait(me, user, a[0], a[1], a[2], a[3])?,
        nr::RT_SIGSUSPEND => signals::rt_sigsuspend(me, user, a[0], a[1])?,
        nr::PAUSE => signals::pause(me, user)?,
        nr::GETPRIORITY => system::getpriority(me, user, a[0], a[1])?,
        nr::SETPRIORITY => system::setpriority(me, user, a[0], a[1], a[2])?,
        nr::SCHED_GETPARAM => system::sched_getparam(me, user, a[0], a[1])?,
        nr::SCHED_GETSCHEDULER => system::sched_getscheduler(me, user, a[0])?,
        nr::SCHED_GET_PRIORITY_MAX => system::sched_get_priority_max(a[0])?,
        nr::SCHED_GET_PRIORITY_MIN => system::sched_get_priority_min(a[0])?,
        nr::PRCTL => system::prctl(me, user, a[0], a[1])?,
        nr::SETRLIMIT => system::setrlimit(me, user, a[0], a[1])?,
        nr::TIME => system::time(me, user, a[0])?,
        nr::SCHED_SETAFFINITY => system::sched_setaffinity(me, user, a[0], a[1], a[2])?,
        nr::SCHED_GETAFFINITY => system::sched_getaffinity(user, a[0], a[1], a[2])?,
        nr::WAITID => system::waitid(me, user, a)?,
        nr::PSELECT6 => poll::pselect6(user, a)?,
        nr::PPOLL => poll::ppoll(user, a)?,
        nr::ACCEPT4 => sockets::accept(me, user, a[0], a[1], a[2], a[3])?,
        nr::RECVMMSG => sockets::recvmmsg(me, user, a[0], a[1], a[2], a[3], a[4])?,
        nr::PRLIMIT64 => system::prlimit64(me, user, a)?,
        nr::SENDMMSG => sockets::sendmmsg(me, user, a[0], a[1], a[2], a[3])?,
        nr::PREADV2 => descriptors::preadv2(me, user, a)?,
        nr::PWRITEV2 => descriptors::pwritev2(me, user, a)?,
        nr::GETCPU => system::getcpu(a[0], a[1])?,
        // epoll (ADR 0612).
        nr::EPOLL_CREATE => events::epoll_create(me, user, a[0])?,
        nr::EPOLL_CREATE1 => events::epoll_create1(me, user, a[0])?,
        nr::EPOLL_CTL => events::epoll_ctl(user, a)?,
        nr::EPOLL_WAIT | nr::EPOLL_PWAIT | nr::EPOLL_PWAIT2 => {
            events::epoll_wait(user, a, regs.number)?
        }
        // Files of the kernel's own (ADR 0613).
        nr::EVENTFD => kernel_files::eventfd2(me, user, a[0], 0)?,
        nr::EVENTFD2 => kernel_files::eventfd2(me, user, a[0], a[1])?,
        nr::TIMERFD_CREATE => kernel_files::timerfd_create(me, user, a[0], a[1])?,
        nr::TIMERFD_SETTIME => kernel_files::timerfd_settime(me, user, a)?,
        nr::TIMERFD_GETTIME => kernel_files::timerfd_gettime(me, user, a[0], a[1])?,
        nr::SIGNALFD => kernel_files::signalfd4(me, user, &[a[0], a[1], a[2], 0, 0, 0])?,
        nr::SIGNALFD4 => kernel_files::signalfd4(me, user, a)?,
        nr::PIDFD_OPEN => kernel_files::pidfd_open(me, user, a[0], a[1])?,
        nr::PIDFD_SEND_SIGNAL => kernel_files::pidfd_send_signal(me, user, a)?,
        // Memory files (ADR 0614).
        nr::MEMFD_CREATE => memory::memfd_create(me, user, a[0], a[1])?,
        // Watches (ADR 0615).
        nr::INOTIFY_INIT => notify::inotify_init1(me, user, 0)?,
        nr::INOTIFY_INIT1 => notify::inotify_init1(me, user, a[0])?,
        nr::INOTIFY_ADD_WATCH => notify::inotify_add_watch(me, user, a[0], a[1], a[2])?,
        nr::INOTIFY_RM_WATCH => notify::inotify_rm_watch(me, user, a[0], a[1])?,
        // Processes and threads.
        nr::GETPID => process::getpid(user),
        nr::GETTID => process::gettid(me),
        nr::GETPPID => process::getppid(me, user)?,
        nr::CLONE => process::clone(me, user, regs)?,
        nr::CLONE3 => process::clone3(me, user, regs)?,
        nr::FORK => process::fork(me, user, regs, false)?,
        nr::VFORK => process::fork(me, user, regs, true)?,
        nr::EXECVE => return process::execve(me, user, paths::AT_FDCWD, a[0], a[1], a[2], 0),
        nr::EXECVEAT => return process::execve(me, user, a[0], a[1], a[2], a[3], a[4]),
        nr::EXIT => return Ok(Done::ExitThread(a[0] & 0xff)),
        nr::EXIT_GROUP => return process::exit_group(me, user, a[0]),
        nr::WAIT4 => process::wait4(me, user, a[0], a[1], a[2], a[3])?,
        nr::KILL => process::kill(me, user, a[0], a[1])?,
        nr::TGKILL => process::tgkill(me, user, Some(a[0]), a[1], a[2])?,
        nr::TKILL => process::tgkill(me, user, None, a[0], a[1])?,
        nr::ARCH_PRCTL => process::arch_prctl(me, user, a[0], a[1])?,
        nr::SET_TID_ADDRESS => process::set_tid_address(me, user, a[0]),
        nr::SET_ROBUST_LIST => process::set_robust_list(user, a[0], a[1])?,
        nr::GET_ROBUST_LIST => process::get_robust_list(user, a[0], a[1], a[2])?,
        nr::FUTEX => time::futex(user, a)?,
        nr::UNAME => process::uname(user, a[0])?,
        nr::GETUID | nr::GETEUID => u64::from(process::identity(me, user)?.0),
        nr::GETGID | nr::GETEGID => u64::from(process::identity(me, user)?.1),
        // Memory.
        nr::MMAP => memory::mmap(me, user, a)?,
        nr::MUNMAP => memory::munmap(me, user, a[0], a[1])?,
        nr::MPROTECT => memory::mprotect(me, user, a[0], a[1], a[2])?,
        nr::MADVISE => memory::madvise(me, user, a[0], a[1], a[2])?,
        nr::BRK => memory::brk(me, user, a[0]),
        nr::MREMAP => memory::mremap(me, user, a)?,
        // Signals.
        nr::RT_SIGACTION => signals::rt_sigaction(me, user, a[0], a[1], a[2], a[3])?,
        nr::RT_SIGPROCMASK => signals::rt_sigprocmask(me, user, a[0], a[1], a[2], a[3])?,
        nr::RT_SIGRETURN => return signals::rt_sigreturn(me, user),
        nr::SIGALTSTACK => signals::sigaltstack(me, user, a[0], a[1])?,
        // Time.
        nr::CLOCK_GETTIME => time::clock_gettime(me, user, a[0], a[1])?,
        nr::CLOCK_GETRES => time::clock_getres(me, user, a[0], a[1])?,
        nr::NANOSLEEP => time::nanosleep(user, a[0], a[1])?,
        nr::CLOCK_NANOSLEEP => time::clock_nanosleep(user, a)?,
        nr::SETITIMER => time::setitimer(me, user, a[0], a[1], a[2])?,
        nr::GETITIMER => time::getitimer(me, user, a[0], a[1])?,
        nr::ALARM => time::alarm(me, user, a[0])?,
        nr::GETRANDOM => time::getrandom(me, user, a[0], a[1], a[2])?,
        nr::SCHED_YIELD => {
            crate::sched::yield_now();
            0
        }
        // Refused for good, as Linux refuses a program without the
        // privilege (ADR 0610).
        nr::IO_URING_SETUP
        | nr::IO_URING_ENTER
        | nr::IO_URING_REGISTER
        | nr::BPF
        | nr::PERF_EVENT_OPEN
        | nr::PTRACE
        | nr::KEXEC_LOAD
        | nr::KEXEC_FILE_LOAD
        | nr::INIT_MODULE
        | nr::FINIT_MODULE => return fail(ENOSYS),
        nr::REBOOT
        | nr::MOUNT
        | nr::UMOUNT2
        | nr::PIVOT_ROOT
        | nr::SWAPON
        | nr::SWAPOFF
        | nr::SETHOSTNAME
        | nr::SETDOMAINNAME
        | nr::SETTIMEOFDAY
        | nr::CLOCK_SETTIME => return fail(EPERM),
        _ => return fail(ENOSYS),
    };
    Ok(Done::Return(value))
}

/// Calls the method `label` names on the capability in the process's slot
/// `slot`, with `words`, as the program could call it with `cap_call`:
/// the same lookup, the same rights, the same checks.
fn method(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    label: u32,
    words: [u64; 8],
) -> Result<Answer> {
    let info = Info { label, words: 8, caps: 0, move_caps: false, no_block: false };
    if info.is_reserved() {
        return Ok(methods::call_capability(user, slot, info, &words)?);
    }
    let mut place = objects::Lent::default();
    let found =
        objects::find_holding(user.cspace, slot, methods::takes_object, &user.process, &mut place);
    let answer = found.and_then(|found| methods::call(me, user, slot, found, info, &words));
    place.end();
    Ok(answer?)
}

/// [`method`], for a method that answers with words: its results.
fn results(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    label: u32,
    words: [u64; 8],
) -> Result<[u64; 8]> {
    match method(me, user, slot, label, words)? {
        Answer::Reply(_, results) => Ok(results),
        // Only the process's exits and `signal_return` answer otherwise,
        // and the table calls those with [`method`].
        _ => fail(errno::EIO),
    }
}
