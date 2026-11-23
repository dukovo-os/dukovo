// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files of the kernel's own (ADR 0613): eventfd, timerfd, signalfd and
//! pidfd, over the process's native `event_counter`, `timer_file`,
//! `signal_file` and `process_file` and radix-file(2)'s `timer_set`,
//! `timer_get`, `set_signal_mask` and `process_of`. The flags, records and
//! errors are `radix_linux::kernel_files`'s, which has host tests.
//!
//! Each call that makes a file puts it in the lowest free descriptor
//! (`into_free`), as Linux does, and the file's reads, writes, polls and
//! `close` are the descriptor calls' own.

use radix_abi::errors::Kernel;
use radix_abi::file::{self, DescriptionFlags};
use radix_abi::process::{self, ClockKind};
use radix_abi::startup::SLOT_PROCESS;
use radix_linux::errno::{EBADF, EINVAL};
use radix_linux::kernel_files as lx;

use super::descriptors::status_flags;
use super::files::{delete, descriptor, into_free, other_kind};
use super::process::signal_number;
use super::{Args, Error, Result, copy_in, copy_out, fail, is, method, paths, results};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// Turns the failure of a method that a file of another kind does not have
/// into `errno`: `no_such_method`, `wrong_kind` and `invalid_argument`,
/// which is what the native method says for a file that is not the right
/// one.
fn wrong_kind(err: Error, errno: i32) -> Error {
    match err {
        Error::Native(status) if other_kind(status) || is(status, Kernel::InvalidArgument) => {
            Error::Errno(errno)
        }
        err => err,
    }
}

/// `eventfd` (flags 0) and `eventfd2`: the process's `event_counter`.
///
/// Errors as Linux's: EINVAL for a flag other than EFD_SEMAPHORE,
/// EFD_NONBLOCK and EFD_CLOEXEC, and EMFILE for no free descriptor.
pub fn eventfd2(me: &ThreadRef, user: &UserRef, initial: u64, how: u64) -> Result<u64> {
    let flags = lx::eventfd(how as u32).map_err(Error::Errno)?;
    into_free(user, 0, |slot| {
        let args = process::EventCounterArgs { to: slot, initial: initial as u32, flags };
        results(me, user, SLOT_PROCESS, process::EVENT_COUNTER, args.encode())?;
        Ok(slot)
    })
    .map(descriptor)
}

/// `timerfd_create`: the process's `timer_file`.
///
/// Errors as Linux's: EINVAL for a flag other than TFD_NONBLOCK and
/// TFD_CLOEXEC, or a clock other than CLOCK_REALTIME, CLOCK_MONOTONIC and
/// CLOCK_BOOTTIME, EPERM for the alarm clocks (which need CAP_WAKE_ALARM
/// that no process has), and EMFILE.
pub fn timerfd_create(me: &ThreadRef, user: &UserRef, clock: u64, how: u64) -> Result<u64> {
    let (clock, flags): (ClockKind, _) =
        lx::timerfd_create(clock as i32, how as u32).map_err(Error::Errno)?;
    into_free(user, 0, |slot| {
        let args = process::TimerFileArgs { to: slot, clock, flags };
        results(me, user, SLOT_PROCESS, process::TIMER_FILE, args.encode())?;
        Ok(slot)
    })
    .map(descriptor)
}

/// `timerfd_settime(fd, flags, new, old)`: radix-file(2)'s `timer_set`.
///
/// Errors as Linux's, in its order: EFAULT for a `new` that cannot be
/// read, EINVAL for a flag other than TFD_TIMER_ABSTIME and
/// TFD_TIMER_CANCEL_ON_SET or an `itimerspec` that is not valid, EBADF for
/// a descriptor that is not open, EINVAL for one that is not a timer file,
/// and EFAULT for an `old` that cannot be written, which is found after the
/// timer is set, as on Linux.
pub fn timerfd_settime(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let mut bytes = [0u8; lx::ITIMERSPEC_SIZE];
    copy_in(a[2], &mut bytes)?;
    let flags = lx::timerfd_settime(a[1] as u32).map_err(Error::Errno)?;
    let (value_ns, interval_ns) = lx::itimerspec(&bytes).map_err(Error::Errno)?;
    let slot = paths::slot_of(a[0])?;
    let args = file::TimerSetArgs { flags, value_ns, interval_ns };
    let old = results(me, user, slot, file::TIMER_SET, args.encode())
        .map_err(|err| wrong_kind(err, EINVAL))?;
    let old = file::TimerSetResults::decode(&old)?;
    if a[3] != 0 {
        let mut out = [0u8; lx::ITIMERSPEC_SIZE];
        lx::put_itimerspec(old.value_ns, old.interval_ns, &mut out);
        copy_out(a[3], &out)?;
    }
    Ok(0)
}

/// `timerfd_gettime(fd, curr)`: radix-file(2)'s `timer_get`.
///
/// Errors as Linux's: EBADF for a descriptor that is not open, EINVAL for
/// one that is not a timer file, EFAULT for a `curr` that cannot be
/// written.
pub fn timerfd_gettime(me: &ThreadRef, user: &UserRef, fd: u64, value: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let words = results(me, user, slot, file::TIMER_GET, file::TimerGetArgs {}.encode())
        .map_err(|err| wrong_kind(err, EINVAL))?;
    let now = file::TimerGetResults::decode(&words)?;
    let mut out = [0u8; lx::ITIMERSPEC_SIZE];
    lx::put_itimerspec(now.value_ns, now.interval_ns, &mut out);
    copy_out(value, &out)?;
    Ok(0)
}

/// `signalfd` (flags 0) and `signalfd4(fd, mask, size, flags)`: with `fd`
/// -1 the process's `signal_file`, else radix-file(2)'s `set_signal_mask`
/// on the signal file `fd` is. A new one is put in the lowest free
/// descriptor.
///
/// Errors as Linux's, in its order: EINVAL for a `size` other than 8, EFAULT
/// for a `mask` that cannot be read, EINVAL for a flag other than
/// SFD_NONBLOCK and SFD_CLOEXEC, EBADF for an `fd` that is not open, EINVAL
/// for one that is not a signal file, EMFILE.
pub fn signalfd4(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    if a[2] != lx::SIGSET_SIZE {
        return fail(EINVAL);
    }
    let mut bytes = [0u8; 8];
    copy_in(a[1], &mut bytes)?;
    let mask = u64::from_le_bytes(bytes);
    let flags = lx::signalfd_flags(a[3] as u32).map_err(Error::Errno)?;
    if a[0] as i32 == -1 {
        return into_free(user, 0, |slot| {
            let args = process::SignalFileArgs { to: slot, mask, flags };
            results(me, user, SLOT_PROCESS, process::SIGNAL_FILE, args.encode())?;
            Ok(slot)
        })
        .map(descriptor);
    }
    let slot = paths::slot_of(a[0])?;
    let args = file::SetSignalMaskArgs { mask };
    results(me, user, slot, file::SET_SIGNAL_MASK, args.encode())
        .map_err(|err| wrong_kind(err, EINVAL))?;
    Ok(a[0])
}

/// `pidfd_open(pid, flags)`: the process's `process_file`, always closed by
/// `exec`.
///
/// Errors as Linux's: EINVAL for a flag other than PIDFD_NONBLOCK or a
/// `pid` of 0 or less, ESRCH for a process that is not there (a process
/// that ended and was waited for is not), and EMFILE. A thread that is not
/// its process's first is no process here, so it is ESRCH, as it is on
/// Linux without PIDFD_THREAD.
pub fn pidfd_open(me: &ThreadRef, user: &UserRef, pid: u64, how: u64) -> Result<u64> {
    let flags = lx::pidfd_open(pid as i32, how as u32).map_err(Error::Errno)?;
    into_free(user, 0, |slot| {
        let args = process::ProcessFileArgs { pid: u64::from(pid as u32), to: slot, flags };
        results(me, user, SLOT_PROCESS, process::PROCESS_FILE, args.encode())?;
        Ok(slot)
    })
    .map(descriptor)
}

/// The number of the process the process file in descriptor `fd` is of,
/// and whether the file does not wait: for `pidfd_send_signal` and
/// `waitid`. EBADF if `fd` is not open or not a process file.
pub fn process_of(me: &ThreadRef, user: &UserRef, fd: u64) -> Result<(u64, bool)> {
    let slot = paths::slot_of(fd)?;
    let words = results(me, user, slot, file::PROCESS_OF, file::ProcessOfArgs {}.encode())
        .map_err(|err| wrong_kind(err, EBADF))?;
    let of = file::ProcessOfResults::decode(&words)?;
    let flags = status_flags(me, user, slot, None)?;
    Ok((of.pid, flags.contains(DescriptionFlags::NON_BLOCKING)))
}

/// `pidfd_send_signal(pidfd, sig, info, flags)`: the native `kill`, or
/// `signal_queue` with the value of an `info` that says what to queue, to
/// the process the file is of.
///
/// Errors as Linux's, in its order: EINVAL for `flags` other than 0; EBADF
/// for a descriptor that is not open or not a process file; EFAULT for an
/// `info` that cannot be read; EINVAL for an `info` whose signal is not
/// `sig`; EPERM for an `info` that fakes another sender's signal to another
/// process; EINVAL for an invalid `sig`; EPERM for a process the caller may
/// not signal; ESRCH once the process was waited for (one that exited and
/// was not is signalled in vain, as a zombie is).
pub fn pidfd_send_signal(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    if a[3] != 0 {
        return fail(EINVAL);
    }
    let (pid, _) = process_of(me, user, a[0])?;
    if a[2] != 0 {
        let mut info = [0u8; lx::SIGNALFD_SIGINFO_SIZE];
        copy_in(a[2], &mut info)?;
        let to_self = pid == user.process.number();
        let value = lx::pidfd_signal_info(&info, a[1] as u32, to_self).map_err(Error::Errno)?;
        let signal = signal_number(a[1])?;
        if signal != 0 {
            let args = process::SignalQueueArgs { pid, signal, value };
            results(me, user, SLOT_PROCESS, process::SIGNAL_QUEUE, args.encode())?;
            return Ok(0);
        }
    }
    let args = process::KillArgs { pid: pid as i64, signal: signal_number(a[1])? };
    results(me, user, SLOT_PROCESS, process::KILL, args.encode())?;
    Ok(0)
}

/// Puts a process file for the child `pid` that `clone` or `clone3` made
/// with CLONE_PIDFD in the lowest free descriptor, closed by `exec`, and its
/// number as an `int` at `at` in the caller's memory. The child's thread has
/// not started, so the caller finds a descriptor in no case but a child.
///
/// Returns the descriptor's slot, which the caller closes if the fork then
/// fails.
///
/// Errors as Linux's: EMFILE for no free descriptor, EFAULT for an `at`
/// that cannot be written, in which case the descriptor is closed again.
pub fn clone_pidfd(me: &ThreadRef, user: &UserRef, pid: u64, at: u64) -> Result<u32> {
    let flags = lx::pidfd_open(pid as i32, 0).map_err(Error::Errno)?;
    let slot = into_free(user, 0, |slot| {
        let args = process::ProcessFileArgs { pid, to: slot, flags };
        method(me, user, SLOT_PROCESS, process::PROCESS_FILE, args.encode())?;
        Ok(slot)
    })?;
    if let Err(err) = copy_out(at, &(descriptor(slot) as i32).to_le_bytes()) {
        delete(me, user, slot);
        return Err(err);
    }
    Ok(slot)
}

/// Closes the descriptor [`clone_pidfd`] put in `slot`, for a fork that
/// failed after it.
pub fn clone_pidfd_failed(me: &ThreadRef, user: &UserRef, slot: u32) {
    delete(me, user, slot);
}
