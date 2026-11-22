// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's calls for clocks, sleeps, interval timers, futexes and random
//! bytes (ADR 0610).
//!
//! The clocks are the process's native `clock` and `cpu_time` methods, the
//! interval timers of `setitimer`, `getitimer` and `alarm` its native
//! `set_timer`, and random bytes the native random capability's `read`. A
//! sleep, and a futex wait, wait on the kernel's clock as the native ones
//! do, until a deadline in it: a relative time from now, a CLOCK_MONOTONIC
//! or CLOCK_BOOTTIME time as it is, and a CLOCK_REALTIME time as far from
//! now as it is from the wall clock's now. A signal ends either wait as the
//! native one's ends it: the call runs again if no handler ran, and fails
//! with EINTR if one did.
//!
//! The futex is the native one (radix-address-space(2)'s `futex_wait` and
//! `futex_wake`), whose waiters any wake of the word wakes: the bit set
//! commands are answered for the bit set that matches all, which is the
//! one the C libraries use, and FUTEX_REQUEUE and FUTEX_CMP_REQUEUE wake
//! the waiters they would move, which then find the word changed and wait
//! again where they meant to. The other commands are ENOSYS.

use radix_abi::errors::Kernel;
use radix_abi::process::{self as abi, ClockKind, TimerKind};
use radix_abi::random;
use radix_abi::startup::{SLOT_PROCESS, SLOT_RANDOM};
use radix_abi::{Status, UserBuffer};
use radix_hal::Timer;
use radix_linux::errno::{EAGAIN, EFAULT, EINVAL, ENOSYS, EOPNOTSUPP};
use radix_linux::futex::{self, Command, FUTEX_BITSET_MATCH_ANY};
use radix_linux::records::{self, ITIMERVAL_SIZE, TIMESPEC_SIZE, Time};

use super::{Args, Error, Result, copy_in, copy_out, fail, is, results};
use crate::Arch;
use crate::sched::{self, ThreadRef, wait};
use crate::user::UserRef;
use crate::user::memory::futex as native;
use crate::user::posix::signal::{Restart, interrupted};

/// Linux's clock ids that are a native clock, as `ClockKind` numbers them
/// alike from CLOCK_REALTIME (0) to CLOCK_BOOTTIME (7). The alarm clocks,
/// which only a timer tells apart, read as the clocks they wake on.
fn clock_kind(clock: i32) -> Result<ClockKind> {
    let raw = match clock {
        0..=7 => clock as u32,
        // CLOCK_REALTIME_ALARM and CLOCK_BOOTTIME_ALARM.
        8 => 0,
        9 => 7,
        _ => return fail(EINVAL),
    };
    ClockKind::from_raw(raw).ok_or(Error::Errno(EINVAL))
}

/// The time on clock `clock`, in nanoseconds: a native clock, or the CPU
/// time of a process or thread that a negative id names as Linux encodes
/// it (`clock_getcpuclockid`): the number, inverted, above three bits
/// whose 4 is a thread's and whose low two are 2 for the scheduler's
/// clock, the only kind kept. Another kind, and a process or thread not
/// there, are EINVAL, as Dukovo's C library says it.
fn read_clock(me: &ThreadRef, user: &UserRef, clock: u64) -> Result<u64> {
    let clock = clock as i32;
    if clock >= 0 {
        let args = abi::ClockArgs { clock: clock_kind(clock)? };
        let words = results(me, user, SLOT_PROCESS, abi::CLOCK, args.encode())?;
        return Ok(abi::ClockResults::decode(&words)?.ns);
    }
    if clock & 3 != 2 {
        return fail(EINVAL);
    }
    let args = abi::CpuTimeArgs { id: !(clock >> 3) as u64, process: clock & 4 == 0 };
    match results(me, user, SLOT_PROCESS, abi::CPU_TIME, args.encode()) {
        Ok(words) => Ok(abi::CpuTimeResults::decode(&words)?.ns),
        Err(Error::Native(status)) if is(status, Kernel::NoSuchProcess) => fail(EINVAL),
        Err(err) => Err(err),
    }
}

/// Reads the `struct timespec` at `address`: EFAULT, or EINVAL for one a
/// wait cannot take.
fn read_time(address: u64) -> Result<Time> {
    let mut bytes = [0u8; TIMESPEC_SIZE];
    copy_in(address, &mut bytes)?;
    records::timespec(&bytes).map_err(Error::Errno)
}

/// Writes `time` as a `struct timespec` at `address`.
fn put_time(address: u64, time: Time) -> Result<u64> {
    let mut bytes = [0u8; TIMESPEC_SIZE];
    records::put_timespec(time, &mut bytes);
    copy_out(address, &bytes).map(|()| 0)
}

/// `clock_gettime(clock, tp)`.
pub fn clock_gettime(me: &ThreadRef, user: &UserRef, clock: u64, tp: u64) -> Result<u64> {
    let ns = read_clock(me, user, clock)?;
    put_time(tp, Time::from_ns(ns))
}

/// `clock_getres(clock, res)`: every clock counts nanoseconds.
pub fn clock_getres(me: &ThreadRef, user: &UserRef, clock: u64, res: u64) -> Result<u64> {
    read_clock(me, user, clock)?;
    if res == 0 {
        return Ok(0);
    }
    put_time(res, Time { sec: 0, nsec: 1 })
}

/// Where a wait until `time` on Linux clock `clock` ends on the kernel's
/// clock: CLOCK_REALTIME's time as far from now as it is from the wall
/// clock's now; the others' as they are.
fn deadline_of(clock: ClockKind, time: Time) -> u64 {
    let ns = time.as_ns();
    match clock {
        ClockKind::Realtime | ClockKind::RealtimeCoarse => {
            let wall = <Arch as Timer>::wall_clock_ns().unwrap_or(0);
            sched::now_ns().saturating_add(ns.saturating_sub(wall))
        }
        _ => ns,
    }
}

/// Sleeps until `deadline` on the kernel's clock, or a signal. Interrupted,
/// writes the time left at `left` (unless it is 0).
fn sleep_until(user: &UserRef, deadline: u64, left: u64) -> Result<u64> {
    // Nothing wakes this key: the wait ends at the deadline, or when a
    // signal interrupts it.
    let key = &raw const user.linux as usize;
    if !wait::wait_until_or_timeout(key, "sleep", deadline, sched::interrupted) {
        return Ok(0);
    }
    if left != 0 {
        put_time(left, Time::from_ns(deadline.saturating_sub(sched::now_ns())))?;
    }
    Err(Error::Native(interrupted(Restart::NoHandler)))
}

/// `nanosleep(req, rem)`.
pub fn nanosleep(user: &UserRef, req: u64, rem: u64) -> Result<u64> {
    let time = read_time(req)?;
    sleep_until(user, sched::now_ns().saturating_add(time.as_ns()), rem)
}

/// `clock_nanosleep`'s flag for a time on the clock rather than from now.
const TIMER_ABSTIME: u64 = 1;

/// `clock_nanosleep(clock, flags, req, rem)`: on CLOCK_REALTIME,
/// CLOCK_MONOTONIC or CLOCK_BOOTTIME. A process's CPU-time clock is
/// EOPNOTSUPP (docs/deferred.md), and a thread's EINVAL, as on Linux.
pub fn clock_nanosleep(user: &UserRef, a: &Args) -> Result<u64> {
    let [clock, flags, req, rem, _, _] = *a;
    let kind = match clock as i32 {
        2 => return fail(EOPNOTSUPP),
        id @ (0 | 1 | 7) => clock_kind(id)?,
        _ => return fail(EINVAL),
    };
    let time = read_time(req)?;
    if flags & TIMER_ABSTIME != 0 {
        // The time left is not written for a time on the clock.
        return sleep_until(user, deadline_of(kind, time), 0);
    }
    sleep_until(user, sched::now_ns().saturating_add(time.as_ns()), rem)
}

/// Sets the process's interval timer `which` (Linux's ITIMER_REAL,
/// ITIMER_VIRTUAL and ITIMER_PROF, which `TimerKind` numbers alike) to
/// expire in `value_ns` (0: never), then every `interval_ns`: the native
/// `set_timer`. Returns what was left of the old one and its interval.
/// Linux takes `which` as an `int`.
fn set_timer(
    me: &ThreadRef,
    user: &UserRef,
    which: u64,
    value_ns: u64,
    interval_ns: u64,
) -> Result<(u64, u64)> {
    let timer = u32::try_from(which as i32)
        .ok()
        .and_then(TimerKind::from_raw)
        .ok_or(Error::Errno(EINVAL))?;
    let args = abi::SetTimerArgs { timer, value_ns, interval_ns };
    let words = results(me, user, SLOT_PROCESS, abi::SET_TIMER, args.encode())?;
    let old = abi::SetTimerResults::decode(&words)?;
    Ok((old.value_ns, old.interval_ns))
}

/// Writes a timer with `value_ns` left and an interval of `interval_ns`
/// as a `struct itimerval` at `address`.
fn put_itimerval(address: u64, value_ns: u64, interval_ns: u64) -> Result<u64> {
    let mut bytes = [0u8; ITIMERVAL_SIZE];
    records::put_itimerval(value_ns, interval_ns, &mut bytes);
    copy_out(address, &bytes).map(|()| 0)
}

/// `setitimer(which, new, old)`. A null `new` turns the timer off, as
/// Linux still takes it. The timer is set before `old` is written, as on
/// Linux, so an `old` that cannot be written is EFAULT with the timer set.
pub fn setitimer(me: &ThreadRef, user: &UserRef, which: u64, new: u64, old: u64) -> Result<u64> {
    let (value_ns, interval_ns) = if new == 0 {
        (0, 0)
    } else {
        let mut bytes = [0u8; ITIMERVAL_SIZE];
        copy_in(new, &mut bytes)?;
        records::itimerval(&bytes).map_err(Error::Errno)?
    };
    let (left, interval) = set_timer(me, user, which, value_ns, interval_ns)?;
    if old == 0 {
        return Ok(0);
    }
    put_itimerval(old, left, interval)
}

/// `getitimer(which, value)`. The native interface reads a timer only by
/// setting it, as Dukovo's C library reads it: off, then on again with
/// what was left and its interval. A timer that was due sends its signal
/// at the first, which then answers the time to its next expiry.
pub fn getitimer(me: &ThreadRef, user: &UserRef, which: u64, value: u64) -> Result<u64> {
    let (left, interval) = set_timer(me, user, which, 0, 0)?;
    if left != 0 {
        set_timer(me, user, which, left, interval)?;
    }
    put_itimerval(value, left, interval)
}

/// `alarm(seconds)`: ITIMER_REAL set to expire in `seconds` (0: turned
/// off), with no interval. Returns the seconds that were left of it,
/// rounded as Linux rounds them. Linux takes `seconds` as an `unsigned
/// int`.
pub fn alarm(me: &ThreadRef, user: &UserRef, seconds: u64) -> Result<u64> {
    let value_ns = u64::from(seconds as u32).saturating_mul(1_000_000_000);
    let (left, _) = set_timer(me, user, TimerKind::Real as u64, value_ns, 0)?;
    Ok(u64::from(records::alarm_left(left)))
}

/// FUTEX_REQUEUE and FUTEX_CMP_REQUEUE.
const FUTEX_REQUEUE: u32 = 3;
const FUTEX_CMP_REQUEUE: u32 = 4;

/// `futex(uaddr, op, val, timeout or val2, uaddr2, val3)`.
pub fn futex(user: &UserRef, a: &Args) -> Result<u64> {
    let [address, op, val, timeout, _, val3] = *a;
    let op = futex::op(op as u32);
    // FUTEX_CLOCK_REALTIME means nothing to a relative FUTEX_WAIT, which
    // Linux takes it with since 4.5, and is ENOSYS with the others.
    if op.realtime && !matches!(op.command, Command::Wait | Command::WaitBitset) {
        return fail(ENOSYS);
    }
    let bitset = || match val3 as u32 {
        0 => fail(EINVAL),
        FUTEX_BITSET_MATCH_ANY => Ok(()),
        _ => fail(ENOSYS),
    };
    match op.command {
        Command::Wait => {
            let deadline = if timeout == 0 {
                None
            } else {
                Some(sched::now_ns().saturating_add(read_time(timeout)?.as_ns()))
            };
            futex_wait(user, address, val as u32, deadline)
        }
        Command::WaitBitset => {
            bitset()?;
            let clock = if op.realtime { ClockKind::Realtime } else { ClockKind::Monotonic };
            let deadline =
                if timeout == 0 { None } else { Some(deadline_of(clock, read_time(timeout)?)) };
            futex_wait(user, address, val as u32, deadline)
        }
        Command::Wake => futex_wake(user, address, val),
        Command::WakeBitset => {
            bitset()?;
            futex_wake(user, address, val)
        }
        Command::Other(FUTEX_CMP_REQUEUE) => {
            let mut word = [0u8; 4];
            copy_in(address, &mut word)?;
            if u32::from_le_bytes(word) != val3 as u32 {
                return fail(EAGAIN);
            }
            futex_wake(
                user,
                address,
                saturating_count(val).saturating_add(saturating_count(timeout)),
            )
        }
        Command::Other(FUTEX_REQUEUE) => futex_wake(
            user,
            address,
            saturating_count(val).saturating_add(saturating_count(timeout)),
        ),
        Command::Other(_) => fail(ENOSYS),
    }
}

/// A count argument, a Linux `int`: 0 for a negative one.
fn saturating_count(count: u64) -> u64 {
    u64::try_from(count as i32).unwrap_or(0)
}

/// Waits while the word at `address` holds `expected`, until a wake,
/// `deadline` on the kernel's clock, or a signal.
fn futex_wait(user: &UserRef, address: u64, expected: u32, deadline: Option<u64>) -> Result<u64> {
    native::wait(&user.space, address, expected, deadline, user).map(|()| 0).map_err(futex_error)
}

/// The error of a futex call, as Linux gives it.
fn futex_error(status: Status) -> Error {
    if is(status, Kernel::BadAddress) {
        Error::Errno(EFAULT)
    } else if is(status, Kernel::WouldBlock) {
        Error::Errno(EAGAIN)
    } else {
        Error::Native(status)
    }
}

/// Wakes up to `count` waiters on the word at `address` (a Linux `int`:
/// at least one, as Linux's loop wakes one before it counts), and returns
/// how many it woke.
fn futex_wake(user: &UserRef, address: u64, count: u64) -> Result<u64> {
    let count = u32::try_from(count as i32).unwrap_or(0).max(1);
    native::wake(&user.space, address, count).map(u64::from).map_err(futex_error)
}

/// `getrandom`'s flags: GRND_NONBLOCK, GRND_RANDOM and GRND_INSECURE.
const GRND_NONBLOCK: u64 = 1;
const GRND_RANDOM: u64 = 2;
const GRND_INSECURE: u64 = 4;

/// The most `getrandom` fills in one call, as Linux's MAX_RW_COUNT.
const GETRANDOM_MAX: u64 = i32::MAX as u64 & !4095;

/// `getrandom(buf, buflen, flags)`: the native random capability's bytes,
/// which never wait once the kernel has started a program, so every flag
/// reads the same.
pub fn getrandom(me: &ThreadRef, user: &UserRef, buffer: u64, len: u64, flags: u64) -> Result<u64> {
    let known = GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE;
    if flags & !known != 0 || flags & (GRND_RANDOM | GRND_INSECURE) == GRND_RANDOM | GRND_INSECURE {
        return fail(EINVAL);
    }
    let len = len.min(GETRANDOM_MAX);
    let mut done = 0;
    while done < len {
        // A long fill stops for a signal, as Linux's does, with what it
        // filled.
        if done > 0 && sched::interrupted() {
            break;
        }
        let chunk = (len - done).min(random::READ_MAX);
        let address = buffer.checked_add(done).ok_or(Error::Errno(EFAULT))?;
        let args = random::ReadArgs { buffer: UserBuffer { address, len: chunk } };
        results(me, user, SLOT_RANDOM, random::READ, args.encode())?;
        done += chunk;
    }
    Ok(done)
}
