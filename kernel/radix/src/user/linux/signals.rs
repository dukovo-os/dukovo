// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's signal calls (ADR 0610): the process's native signal methods,
//! which number signals, masks, action flags and stack flags as Linux
//! does (`radix_linux::signals`), and write the frame a handler runs on as
//! Linux's x86-64 `rt_sigframe`, so that a Linux handler and its
//! restorer's `rt_sigreturn` work as they do on Linux.

use radix_abi::errors::Kernel;
use radix_abi::process as abi;
use radix_abi::signal::{ActionFlags, MaskHow, SIGNALS, StackFlags};
use radix_abi::startup::SLOT_PROCESS;
use radix_hal::UserMode;
use radix_linux::errno::{EAGAIN, EINTR, EINVAL, ENOMEM, EPERM};
use radix_linux::records::{self, SIGACTION_SIZE, SigAction, TIMESPEC_SIZE};
use radix_linux::signals::{SA_RESTORER, SIGSET_SIZE, SS_DISABLE, SS_ONSTACK};
use radix_linux::system::put_siginfo;

use super::process::identity;
use super::{Done, Error, Result, copy_in, copy_out, fail, is, method, results};
use crate::Arch;
use crate::sched::ThreadRef;
use crate::user::UserRef;
use crate::user::methods::Answer;

/// `rt_sigaction(signal, act, oldact, sigsetsize)`: the native
/// `signal_action`. The restorer is kept only with SA_RESTORER, and flags
/// Linux does not define are dropped, as Linux drops them.
pub fn rt_sigaction(
    me: &ThreadRef,
    user: &UserRef,
    signal: u64,
    act: u64,
    oldact: u64,
    size: u64,
) -> Result<u64> {
    if size != SIGSET_SIZE {
        return fail(EINVAL);
    }
    let Ok(signal @ 1..=SIGNALS) = u32::try_from(signal as i32) else {
        return fail(EINVAL);
    };
    // Read before the old action is written, which may be the same record.
    let new = if act == 0 {
        None
    } else {
        let mut bytes = [0u8; SIGACTION_SIZE];
        copy_in(act, &mut bytes)?;
        Some(SigAction::read(&bytes))
    };
    let action = new.unwrap_or_default();
    let args = abi::SignalActionArgs {
        signal,
        set: new.is_some(),
        handler: action.handler,
        flags: action.flags & ActionFlags::ALL.bits(),
        mask: action.mask,
        restorer: if action.flags & SA_RESTORER != 0 { action.restorer } else { 0 },
    };
    let old = abi::SignalActionResults::decode(&results(
        me,
        user,
        SLOT_PROCESS,
        abi::SIGNAL_ACTION,
        args.encode(),
    )?)?;
    if oldact != 0 {
        let old = SigAction {
            handler: old.handler,
            flags: old.flags,
            restorer: old.restorer,
            mask: old.mask,
        };
        let mut bytes = [0u8; SIGACTION_SIZE];
        old.put(&mut bytes);
        copy_out(oldact, &bytes)?;
    }
    Ok(0)
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize)`: the native
/// `signal_mask`, which reads the mask alone with its query, as Linux does
/// for a null `set`.
pub fn rt_sigprocmask(
    me: &ThreadRef,
    user: &UserRef,
    how: u64,
    set: u64,
    oldset: u64,
    size: u64,
) -> Result<u64> {
    if size != SIGSET_SIZE {
        return fail(EINVAL);
    }
    let (how, bits) = if set == 0 {
        (MaskHow::Query, 0)
    } else {
        // SIG_BLOCK, SIG_UNBLOCK and SIG_SETMASK are the native values;
        // the native query is not Linux's.
        let how = match u32::try_from(how) {
            Ok(how @ 0..=2) => MaskHow::from_raw(how).ok_or(Error::Errno(EINVAL))?,
            _ => return fail(EINVAL),
        };
        let mut bytes = [0u8; 8];
        copy_in(set, &mut bytes)?;
        (how, u64::from_le_bytes(bytes))
    };
    let args = abi::SignalMaskArgs { how, set: bits };
    let words = results(me, user, SLOT_PROCESS, abi::SIGNAL_MASK, args.encode())?;
    if oldset != 0 {
        copy_out(oldset, &abi::SignalMaskResults::decode(&words)?.old.to_le_bytes())?;
    }
    Ok(0)
}

/// `rt_sigreturn`: the restorer's `ret` left the stack pointer on the
/// frame's `ucontext_t`, which the native `signal_return` restores.
pub fn rt_sigreturn(me: &ThreadRef, user: &UserRef) -> Result<Done> {
    let args = abi::SignalReturnArgs { context: Arch::user_stack_pointer() };
    match method(me, user, SLOT_PROCESS, abi::SIGNAL_RETURN, args.encode())? {
        Answer::Restore(saved) => Ok(Done::Restore(saved)),
        _ => fail(radix_linux::errno::EIO),
    }
}

/// The size of a `stack_t`: `ss_sp`, `ss_flags` (an `int`, padded) and
/// `ss_size`.
const STACK_T_SIZE: usize = 24;

/// The smallest alternate stack Linux takes, MINSIGSTKSZ.
const MINSIGSTKSZ: u64 = 2048;

/// `sigaltstack(ss, old_ss)`: the native `signal_stack`. SS_ONSTACK in a
/// new stack's flags means nothing, as Linux takes it; SS_AUTODISARM is
/// EINVAL, as a Linux without it gives (docs/deferred.md).
pub fn sigaltstack(me: &ThreadRef, user: &UserRef, new: u64, old: u64) -> Result<u64> {
    let mut args =
        abi::SignalStackArgs { set: false, address: 0, flags: StackFlags::NONE, size: 0 };
    if new != 0 {
        let mut bytes = [0u8; STACK_T_SIZE];
        copy_in(new, &mut bytes)?;
        let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or([0; 8]));
        let flags = word(8) as u32;
        let disable = match flags & !SS_ONSTACK {
            0 => false,
            SS_DISABLE => true,
            _ => return fail(EINVAL),
        };
        let (address, size) = (word(0), word(16));
        if !disable && size < MINSIGSTKSZ {
            return fail(ENOMEM);
        }
        let flags = if disable { StackFlags::DISABLED } else { StackFlags::NONE };
        args = abi::SignalStackArgs { set: true, address, flags, size };
    }
    let words = match results(me, user, SLOT_PROCESS, abi::SIGNAL_STACK, args.encode()) {
        Ok(words) => words,
        // Changed while the thread runs on it.
        Err(Error::Native(status)) if is(status, Kernel::NotPermitted) => return fail(EPERM),
        Err(err) => return Err(err),
    };
    if old != 0 {
        let was = abi::SignalStackResults::decode(&words)?;
        let mut bytes = [0u8; STACK_T_SIZE];
        bytes[0..8].copy_from_slice(&was.address.to_le_bytes());
        bytes[8..12].copy_from_slice(&was.flags.bits().to_le_bytes());
        bytes[16..24].copy_from_slice(&was.size.to_le_bytes());
        copy_out(old, &bytes)?;
    }
    Ok(0)
}

/// The `sigset_t` of `size` bytes at `address`: EINVAL unless `size` is
/// the kernel's, which is 8, then EFAULT.
fn read_set(address: u64, size: u64) -> Result<u64> {
    if size != SIGSET_SIZE {
        return fail(EINVAL);
    }
    let mut bytes = [0u8; 8];
    copy_in(address, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

/// `rt_sigpending(set, sigsetsize)`: the native `signal_pending`, the
/// signals that are pending and blocked, in as many bytes of the set as
/// `sigsetsize` says, which may be fewer than the kernel's, as on Linux.
pub fn rt_sigpending(me: &ThreadRef, user: &UserRef, set: u64, size: u64) -> Result<u64> {
    if size > SIGSET_SIZE {
        return fail(EINVAL);
    }
    let words = results(me, user, SLOT_PROCESS, abi::SIGNAL_PENDING, [0; 8])?;
    let pending = abi::SignalPendingResults::decode(&words)?.set.to_le_bytes();
    copy_out(set, &pending[..size as usize]).map(|()| 0)
}

/// `rt_sigsuspend(mask, sigsetsize)`: the native `signal_suspend`, which
/// waits for a signal with the mask in place and fails with `interrupted`,
/// so that the call runs again if no handler ran and fails with EINTR if
/// one did, as Linux's does.
pub fn rt_sigsuspend(me: &ThreadRef, user: &UserRef, mask: u64, size: u64) -> Result<u64> {
    let args = abi::SignalSuspendArgs { mask: read_set(mask, size)? };
    results(me, user, SLOT_PROCESS, abi::SIGNAL_SUSPEND, args.encode())?;
    // The method never returns otherwise.
    fail(EINTR)
}

/// `pause()`: `rt_sigsuspend` with the mask as it is, which the native
/// `signal_mask` reads.
pub fn pause(me: &ThreadRef, user: &UserRef) -> Result<u64> {
    let args = abi::SignalMaskArgs { how: MaskHow::Query, set: 0 };
    let words = results(me, user, SLOT_PROCESS, abi::SIGNAL_MASK, args.encode())?;
    let args = abi::SignalSuspendArgs { mask: abi::SignalMaskResults::decode(&words)?.old };
    results(me, user, SLOT_PROCESS, abi::SIGNAL_SUSPEND, args.encode())?;
    // The method never returns otherwise.
    fail(EINTR)
}

/// `rt_sigtimedwait(set, info, timeout, sigsetsize)`: the native
/// `signal_wait`, which takes a pending signal of the set without running
/// its handler; EAGAIN when the time runs out, as Linux gives it. A null
/// timeout waits for ever. The `siginfo_t` is the signal's, with the
/// caller's own user, as the signal's sender's is not known.
pub fn rt_sigtimedwait(
    me: &ThreadRef,
    user: &UserRef,
    set: u64,
    info: u64,
    timeout: u64,
    size: u64,
) -> Result<u64> {
    let set = read_set(set, size)?;
    let wait = if timeout == 0 {
        None
    } else {
        let mut bytes = [0u8; TIMESPEC_SIZE];
        copy_in(timeout, &mut bytes)?;
        Some(records::timespec(&bytes).map_err(Error::Errno)?)
    };
    let args = abi::SignalWaitArgs {
        set,
        timeout_ns: wait.map_or(0, |time| time.as_ns()),
        forever: wait.is_none(),
    };
    let words = match results(me, user, SLOT_PROCESS, abi::SIGNAL_WAIT, args.encode()) {
        Err(Error::Native(status)) if is(status, Kernel::TimedOut) => return fail(EAGAIN),
        done => done?,
    };
    let got = abi::SignalWaitResults::decode(&words)?;
    if info != 0 {
        let (uid, _) = identity(me, user)?;
        let record = put_siginfo(got.signal as i32, got.code, got.pid as u32, uid, got.status);
        copy_out(info, &record)?;
    }
    Ok(u64::from(got.signal))
}
