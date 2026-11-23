// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's files of the kernel's own (ADR 0613): `eventfd`, `timerfd`,
//! `signalfd` and `pidfd`. Their flags, their `itimerspec` and
//! `signalfd_siginfo` records, and the rules Linux checks them by, which the
//! Linux table applies before it calls the native methods
//! (`radix_abi::process`'s `event_counter`, `timer_file`, `signal_file` and
//! `process_file`, `radix_abi::file`'s `timer_set`).
//!
//! The order of the checks is Linux's, since a call that breaks two rules
//! gives the error of the first: each function says which it does.

use radix_abi::file::TimerSetFlags;
use radix_abi::process::{ClockKind, CounterFlags, PipeFlags};

use crate::errno::{EINVAL, EPERM};
use crate::flags::{O_CLOEXEC, O_NONBLOCK};
use crate::records::{TIMESPEC_SIZE, Time, put_timespec, timespec};
use crate::sockets::{get, put};

/// `EFD_SEMAPHORE`: each read takes 1.
pub const EFD_SEMAPHORE: u32 = 1;
/// `EFD_CLOEXEC`, which is `O_CLOEXEC`.
pub const EFD_CLOEXEC: u32 = O_CLOEXEC;
/// `EFD_NONBLOCK`, which is `O_NONBLOCK`.
pub const EFD_NONBLOCK: u32 = O_NONBLOCK;

/// The native flags of the event counter `eventfd2` makes with `flags`.
///
/// # Errors
///
/// EINVAL for a bit other than `EFD_SEMAPHORE`, `EFD_CLOEXEC` and
/// `EFD_NONBLOCK`.
pub fn eventfd(flags: u32) -> Result<CounterFlags, i32> {
    if flags & !(EFD_SEMAPHORE | EFD_CLOEXEC | EFD_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    let mut native = CounterFlags::NONE;
    for (asked, flag) in [
        (EFD_SEMAPHORE, CounterFlags::SEMAPHORE),
        (EFD_CLOEXEC, CounterFlags::CLOSE_ON_EXEC),
        (EFD_NONBLOCK, CounterFlags::NON_BLOCKING),
    ] {
        if flags & asked != 0 {
            native = native.union(flag);
        }
    }
    Ok(native)
}

/// `TFD_TIMER_ABSTIME`: the time is on the timer's clock, not from now.
pub const TFD_TIMER_ABSTIME: u32 = 1;
/// `TFD_TIMER_CANCEL_ON_SET`: with an absolute time on `CLOCK_REALTIME`, a
/// read fails once the clock is set.
pub const TFD_TIMER_CANCEL_ON_SET: u32 = 2;
/// `TFD_CLOEXEC`, which is `O_CLOEXEC`.
pub const TFD_CLOEXEC: u32 = O_CLOEXEC;
/// `TFD_NONBLOCK`, which is `O_NONBLOCK`.
pub const TFD_NONBLOCK: u32 = O_NONBLOCK;

/// `CLOCK_REALTIME`.
const CLOCK_REALTIME: i32 = 0;
/// `CLOCK_MONOTONIC`.
const CLOCK_MONOTONIC: i32 = 1;
/// `CLOCK_BOOTTIME`.
const CLOCK_BOOTTIME: i32 = 7;
/// `CLOCK_REALTIME_ALARM`, which wakes a suspended machine.
const CLOCK_REALTIME_ALARM: i32 = 8;
/// `CLOCK_BOOTTIME_ALARM`.
const CLOCK_BOOTTIME_ALARM: i32 = 9;

/// The clock and native flags of the timer file `timerfd_create` makes
/// with `clockid` and `flags`.
///
/// # Errors
///
/// EINVAL for a bit other than `TFD_CLOEXEC` and `TFD_NONBLOCK`, or a
/// clock that is not one of the five timer files have; then EPERM for the
/// two alarm clocks, which need the capability to wake the machine that no
/// program has.
pub fn timerfd_create(clockid: i32, flags: u32) -> Result<(ClockKind, PipeFlags), i32> {
    if flags & !(TFD_CLOEXEC | TFD_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    let clock = match clockid {
        CLOCK_REALTIME => ClockKind::Realtime,
        CLOCK_MONOTONIC => ClockKind::Monotonic,
        CLOCK_BOOTTIME => ClockKind::Boottime,
        CLOCK_REALTIME_ALARM | CLOCK_BOOTTIME_ALARM => return Err(EPERM),
        _ => return Err(EINVAL),
    };
    Ok((clock, description_flags(flags & TFD_NONBLOCK != 0, flags & TFD_CLOEXEC != 0)))
}

/// The native flags of `timerfd_settime`'s `flags`.
///
/// # Errors
///
/// EINVAL for a bit other than `TFD_TIMER_ABSTIME` and
/// `TFD_TIMER_CANCEL_ON_SET`.
pub fn timerfd_settime(flags: u32) -> Result<TimerSetFlags, i32> {
    if flags & !(TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET) != 0 {
        return Err(EINVAL);
    }
    let mut native = TimerSetFlags::NONE;
    if flags & TFD_TIMER_ABSTIME != 0 {
        native = native.union(TimerSetFlags::ABSOLUTE);
    }
    if flags & TFD_TIMER_CANCEL_ON_SET != 0 {
        native = native.union(TimerSetFlags::CANCEL_ON_SET);
    }
    Ok(native)
}

/// The size of `struct itimerspec`: the interval, then the first expiry,
/// each a `struct timespec`.
pub const ITIMERSPEC_SIZE: usize = 2 * TIMESPEC_SIZE;

/// Reads a `struct itimerspec`: the first expiry and the interval, in
/// nanoseconds.
///
/// # Errors
///
/// EINVAL if either `struct timespec` is not valid, as Linux's
/// `itimerspec64_valid` has it: negative seconds, or nanoseconds negative
/// or a second or more.
pub fn itimerspec(bytes: &[u8; ITIMERSPEC_SIZE]) -> Result<(u64, u64), i32> {
    let interval = timespec(&get(bytes, 0))?;
    let value = timespec(&get(bytes, TIMESPEC_SIZE))?;
    Ok((value.as_ns(), interval.as_ns()))
}

/// Writes a `struct itimerspec` for a timer with `value_ns` left and an
/// interval of `interval_ns`.
pub fn put_itimerspec(value_ns: u64, interval_ns: u64, out: &mut [u8; ITIMERSPEC_SIZE]) {
    let mut part = [0u8; TIMESPEC_SIZE];
    put_timespec(Time::from_ns(interval_ns), &mut part);
    put(out, 0, part);
    put_timespec(Time::from_ns(value_ns), &mut part);
    put(out, TIMESPEC_SIZE, part);
}

/// `SFD_CLOEXEC`, which is `O_CLOEXEC`.
pub const SFD_CLOEXEC: u32 = O_CLOEXEC;
/// `SFD_NONBLOCK`, which is `O_NONBLOCK`.
pub const SFD_NONBLOCK: u32 = O_NONBLOCK;
/// The size of the `sigset_t` `signalfd` takes, which its `sizemask` must
/// be: 64 signals in one word.
pub const SIGSET_SIZE: u64 = 8;

/// The native flags of the signal file `signalfd4` makes with `flags`.
///
/// # Errors
///
/// EINVAL for a bit other than `SFD_CLOEXEC` and `SFD_NONBLOCK`.
pub fn signalfd_flags(flags: u32) -> Result<PipeFlags, i32> {
    if flags & !(SFD_CLOEXEC | SFD_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    Ok(description_flags(flags & SFD_NONBLOCK != 0, flags & SFD_CLOEXEC != 0))
}

/// The size of `struct signalfd_siginfo`.
pub const SIGNALFD_SIGINFO_SIZE: usize = 128;

/// `si_code` of a signal sent with `sigqueue`.
const SI_QUEUE: i32 = -1;
/// `si_code` of a POSIX timer's signal.
const SI_TIMER: i32 = -2;
/// `si_code` of a signal sent to a thread.
const SI_TKILL: i32 = -6;
/// `si_code` of a signal the kernel sent: the first code that is no
/// signal's own.
const SI_KERNEL: i32 = 0x80;
/// The signal a child's end sends.
const SIGCHLD: u32 = 17;
/// The signals a fault raises, whose `si_code` then says what the fault
/// was and whose record holds the address.
const FAULT_SIGNALS: [u32; 5] = [4, 5, 7, 8, 11];

/// Writes the `struct signalfd_siginfo` for `signal`, which the process
/// numbered `pid` sent with `si_code` `code` (the native `Info`'s fields),
/// carrying `status`, as Linux's `signalfd_copyinfo` fills it by the
/// layout of the signal's `siginfo_t`:
///
/// - A POSIX timer's signal (`SI_TIMER`) has the timer's number in
///   `ssi_tid` and its overrun, which the native `Info` keeps in the high
///   half of `pid`, in `ssi_overrun`, and the value in `ssi_int` and
///   `ssi_ptr`.
/// - A signal with a negative code (`SI_QUEUE`, `SI_TKILL`) has the sender
///   and, for `SI_QUEUE`, the value.
/// - A child's signal (`SIGCHLD` with a code of its own) has the child and
///   its status; a fault's has the address, which the native `Info` keeps
///   in `pid`.
/// - Any other has the sender.
///
/// `ssi_uid` is 0 whenever there is a sender: the native `Info` does not
/// keep the sender's user. Every field the layout does not name is 0.
pub fn put_signalfd_siginfo(
    signal: u32,
    code: i32,
    pid: u64,
    status: u64,
) -> [u8; SIGNALFD_SIGINFO_SIZE] {
    let mut out = [0u8; SIGNALFD_SIGINFO_SIZE];
    put(&mut out, 0, signal.to_le_bytes());
    put(&mut out, 8, code.to_le_bytes());
    // Process numbers are below 2^32 as Linux's `pid_t` is; the Linux
    // table gives them to programs as 32 bits everywhere.
    let sender = pid as u32;
    let own_code = (1..SI_KERNEL).contains(&code);
    if code == SI_TIMER {
        put(&mut out, 24, sender.to_le_bytes());
        put(&mut out, 32, ((pid >> 32) as u32).to_le_bytes());
        put_value(&mut out, status);
    } else if code < 0 {
        put(&mut out, 12, sender.to_le_bytes());
        if code == SI_QUEUE {
            put_value(&mut out, status);
        }
    } else if own_code && signal == SIGCHLD {
        put(&mut out, 12, sender.to_le_bytes());
        put(&mut out, 40, (status as i32).to_le_bytes());
    } else if own_code && FAULT_SIGNALS.contains(&signal) {
        put(&mut out, 72, pid.to_le_bytes());
    } else {
        put(&mut out, 12, sender.to_le_bytes());
    }
    out
}

/// Writes a `sigqueue` value as `ssi_int`, its low half, and `ssi_ptr`.
fn put_value(out: &mut [u8; SIGNALFD_SIGINFO_SIZE], value: u64) {
    put(out, 44, (value as i32).to_le_bytes());
    put(out, 48, value.to_le_bytes());
}

/// `PIDFD_NONBLOCK`, which is `O_NONBLOCK`.
pub const PIDFD_NONBLOCK: u32 = O_NONBLOCK;
/// `waitid`'s `P_PIDFD`: `id` is a process file's descriptor.
pub const P_PIDFD: u32 = 3;

/// The native flags of the process file `pidfd_open` makes for `pid` with
/// `flags`: the non-blocking one if asked, and always the one that has
/// `exec` close it, as Linux's process files are.
///
/// # Errors
///
/// EINVAL for a bit other than `PIDFD_NONBLOCK` (`PIDFD_THREAD`, which
/// Linux has from 6.9, included), and for a `pid` of 0 or less.
pub fn pidfd_open(pid: i32, flags: u32) -> Result<PipeFlags, i32> {
    if flags & !PIDFD_NONBLOCK != 0 || pid <= 0 {
        return Err(EINVAL);
    }
    Ok(description_flags(flags & PIDFD_NONBLOCK != 0, true))
}

/// The value that `pidfd_send_signal` sends with `signal` when it is given
/// the `siginfo_t` in `info`, as Linux's `pidfd_send_signal` allows it:
/// `sigqueue`'s value, at the end of the record. `to_self` says the
/// process file is of the caller's own process.
///
/// # Errors
///
/// EINVAL if the record's `si_signo` is not `signal`; EPERM for a record
/// whose `si_code` is 0 or more, or `SI_TKILL`, to another process, since
/// Linux lets a program fake only a `sigqueue` signal to others.
pub fn pidfd_signal_info(
    info: &[u8; SIGNALFD_SIGINFO_SIZE],
    signal: u32,
    to_self: bool,
) -> Result<u64, i32> {
    if i32::from_le_bytes(get(info, 0)) != signal as i32 {
        return Err(EINVAL);
    }
    queued_value(info, to_self)
}

/// The value that `rt_sigqueueinfo`, which `sigqueue` makes, sends with
/// the `siginfo_t` in `info`: `sigqueue`'s, at the end of the record. The
/// record's own signal does not count, as Linux puts the call's in its
/// place. `to_self` says the signal is for the caller's own process.
///
/// # Errors
///
/// EPERM for a record whose `si_code` is 0 or more, or `SI_TKILL`, to
/// another process, since Linux lets a program fake only a `sigqueue`
/// signal to others.
pub fn queued_value(info: &[u8; SIGNALFD_SIGINFO_SIZE], to_self: bool) -> Result<u64, i32> {
    let code = i32::from_le_bytes(get(info, 8));
    if !to_self && (code >= 0 || code == SI_TKILL) {
        return Err(EPERM);
    }
    Ok(u64::from_le_bytes(get(info, 24)))
}

/// The native flags of a file made non-blocking and closed by `exec` as
/// the two bits say.
fn description_flags(non_blocking: bool, close_on_exec: bool) -> PipeFlags {
    let mut native = PipeFlags::NONE;
    if non_blocking {
        native = native.union(PipeFlags::NON_BLOCKING);
    }
    if close_on_exec {
        native = native.union(PipeFlags::CLOSE_ON_EXEC);
    }
    native
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_counter_takes_linuxs_three_flags() {
        assert_eq!(eventfd(0), Ok(CounterFlags::NONE));
        let all = CounterFlags::SEMAPHORE
            .union(CounterFlags::CLOSE_ON_EXEC)
            .union(CounterFlags::NON_BLOCKING);
        assert_eq!(eventfd(EFD_SEMAPHORE | EFD_CLOEXEC | EFD_NONBLOCK), Ok(all));
        assert_eq!(eventfd(EFD_SEMAPHORE), Ok(CounterFlags::SEMAPHORE));
        assert_eq!(eventfd(2), Err(EINVAL));
        assert_eq!(eventfd(1 << 20), Err(EINVAL));
    }

    #[test]
    fn the_flags_have_linuxs_numbers() {
        assert_eq!([EFD_SEMAPHORE, EFD_CLOEXEC, EFD_NONBLOCK], [1, 0o2000000, 0o4000]);
        assert_eq!([TFD_TIMER_ABSTIME, TFD_TIMER_CANCEL_ON_SET], [1, 2]);
        assert_eq!([TFD_CLOEXEC, TFD_NONBLOCK], [0o2000000, 0o4000]);
        assert_eq!([SFD_CLOEXEC, SFD_NONBLOCK], [0o2000000, 0o4000]);
        assert_eq!(PIDFD_NONBLOCK, 0o4000);
        assert_eq!(P_PIDFD, 3);
    }

    #[test]
    fn a_timer_file_is_made_on_the_clocks_linux_allows() {
        let plain = PipeFlags::NONE;
        assert_eq!(timerfd_create(0, 0), Ok((ClockKind::Realtime, plain)));
        assert_eq!(timerfd_create(1, 0), Ok((ClockKind::Monotonic, plain)));
        assert_eq!(timerfd_create(7, 0), Ok((ClockKind::Boottime, plain)));
        let both = PipeFlags::NON_BLOCKING.union(PipeFlags::CLOSE_ON_EXEC);
        assert_eq!(timerfd_create(1, TFD_NONBLOCK | TFD_CLOEXEC), Ok((ClockKind::Monotonic, both)));
    }

    #[test]
    fn a_timer_file_refuses_the_flags_before_the_clock() {
        // The process and thread CPU clocks, TAI and a made-up one.
        for clock in [2, 3, 11, 99, -1] {
            assert_eq!(timerfd_create(clock, 0), Err(EINVAL), "clock {clock}");
        }
        assert_eq!(timerfd_create(1, 1), Err(EINVAL));
        // A wrong flag with an alarm clock is EINVAL, not EPERM.
        assert_eq!(timerfd_create(8, 1), Err(EINVAL));
        assert_eq!(timerfd_create(8, 0), Err(EPERM));
        assert_eq!(timerfd_create(9, 0), Err(EPERM));
    }

    #[test]
    fn a_timer_is_set_with_the_two_flags_linux_has() {
        assert_eq!(timerfd_settime(0), Ok(TimerSetFlags::NONE));
        assert_eq!(timerfd_settime(1), Ok(TimerSetFlags::ABSOLUTE));
        let both = TimerSetFlags::ABSOLUTE.union(TimerSetFlags::CANCEL_ON_SET);
        assert_eq!(timerfd_settime(3), Ok(both));
        assert_eq!(timerfd_settime(4), Err(EINVAL));
    }

    #[test]
    fn itimerspec_has_linuxs_layout() {
        let mut bytes = [0u8; ITIMERSPEC_SIZE];
        put_itimerspec(3_000_000_007, 2_500_000_000, &mut bytes);
        assert_eq!(&bytes[0..8], &2i64.to_le_bytes());
        assert_eq!(&bytes[8..16], &500_000_000i64.to_le_bytes());
        assert_eq!(&bytes[16..24], &3i64.to_le_bytes());
        assert_eq!(&bytes[24..32], &7i64.to_le_bytes());
        assert_eq!(itimerspec(&bytes), Ok((3_000_000_007, 2_500_000_000)));
        assert_eq!(ITIMERSPEC_SIZE, 32);
    }

    #[test]
    fn an_itimerspec_with_an_invalid_time_is_refused() {
        let mut bytes = [0u8; ITIMERSPEC_SIZE];
        // A negative second in the interval, then in the first expiry.
        put_itimerspec(1, 1, &mut bytes);
        bytes[0..8].copy_from_slice(&(-1i64).to_le_bytes());
        assert_eq!(itimerspec(&bytes), Err(EINVAL));
        put_itimerspec(1, 1, &mut bytes);
        bytes[16..24].copy_from_slice(&(-1i64).to_le_bytes());
        assert_eq!(itimerspec(&bytes), Err(EINVAL));
        // A second of nanoseconds, and a negative number of them.
        put_itimerspec(1, 1, &mut bytes);
        bytes[8..16].copy_from_slice(&1_000_000_000i64.to_le_bytes());
        assert_eq!(itimerspec(&bytes), Err(EINVAL));
        put_itimerspec(1, 1, &mut bytes);
        bytes[24..32].copy_from_slice(&(-1i64).to_le_bytes());
        assert_eq!(itimerspec(&bytes), Err(EINVAL));
        // A disarming zero is valid.
        assert_eq!(itimerspec(&[0; ITIMERSPEC_SIZE]), Ok((0, 0)));
    }

    #[test]
    fn a_signal_file_takes_two_flags() {
        assert_eq!(signalfd_flags(0), Ok(PipeFlags::NONE));
        assert_eq!(signalfd_flags(SFD_NONBLOCK), Ok(PipeFlags::NON_BLOCKING));
        assert_eq!(signalfd_flags(SFD_CLOEXEC), Ok(PipeFlags::CLOSE_ON_EXEC));
        assert_eq!(signalfd_flags(1), Err(EINVAL));
        assert_eq!(SIGSET_SIZE, 8);
    }

    fn word(record: &[u8; SIGNALFD_SIGINFO_SIZE], at: usize) -> u32 {
        u32::from_le_bytes(get(record, at))
    }

    fn wide(record: &[u8; SIGNALFD_SIGINFO_SIZE], at: usize) -> u64 {
        u64::from_le_bytes(get(record, at))
    }

    #[test]
    fn a_signalfd_record_is_128_bytes() {
        assert_eq!(SIGNALFD_SIGINFO_SIZE, 128);
        assert_eq!(put_signalfd_siginfo(10, 0, 0, 0).len(), 128);
    }

    #[test]
    fn a_signal_from_kill_has_its_sender() {
        let record = put_signalfd_siginfo(10, 0, 4321, 0);
        assert_eq!(word(&record, 0), 10);
        assert_eq!(word(&record, 4), 0);
        assert_eq!(word(&record, 8), 0);
        assert_eq!(word(&record, 12), 4321);
        assert_eq!(word(&record, 16), 0);
        assert!(record[20..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_queued_signal_has_its_value_in_both_places() {
        let record = put_signalfd_siginfo(40, SI_QUEUE, 77, 0x1_2345_6789);
        assert_eq!(word(&record, 0), 40);
        assert_eq!(word(&record, 8) as i32, SI_QUEUE);
        assert_eq!(word(&record, 12), 77);
        assert_eq!(word(&record, 44), 0x2345_6789);
        assert_eq!(wide(&record, 48), 0x1_2345_6789);
    }

    #[test]
    fn a_signal_sent_to_a_thread_has_its_sender_and_no_value() {
        let record = put_signalfd_siginfo(10, SI_TKILL, 9, 55);
        assert_eq!(word(&record, 12), 9);
        assert_eq!(word(&record, 44), 0);
        assert_eq!(wide(&record, 48), 0);
    }

    #[test]
    fn a_timers_signal_has_its_number_overrun_and_value() {
        let record = put_signalfd_siginfo(14, SI_TIMER, (3 << 32) | 5, 0xabc);
        assert_eq!(word(&record, 8) as i32, SI_TIMER);
        assert_eq!(word(&record, 12), 0);
        assert_eq!(word(&record, 24), 5);
        assert_eq!(word(&record, 32), 3);
        assert_eq!(word(&record, 44), 0xabc);
        assert_eq!(wide(&record, 48), 0xabc);
    }

    #[test]
    fn a_childs_signal_has_the_child_and_its_status() {
        let record = put_signalfd_siginfo(17, 1, 600, 3);
        assert_eq!(word(&record, 8), 1);
        assert_eq!(word(&record, 12), 600);
        assert_eq!(word(&record, 40), 3);
    }

    #[test]
    fn a_faults_signal_has_its_address() {
        let record = put_signalfd_siginfo(11, 1, 0x0dea_dbee_f000, 0);
        assert_eq!(word(&record, 12), 0);
        assert_eq!(wide(&record, 72), 0x0dea_dbee_f000);
    }

    #[test]
    fn a_signal_the_kernel_sent_has_no_sender() {
        let record = put_signalfd_siginfo(1, SI_KERNEL, 0, 0);
        assert_eq!(word(&record, 8), 0x80);
        assert!(record[12..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_process_file_is_opened_for_a_process_with_one_flag() {
        let exec = PipeFlags::CLOSE_ON_EXEC;
        assert_eq!(pidfd_open(1, 0), Ok(exec));
        assert_eq!(pidfd_open(1, PIDFD_NONBLOCK), Ok(exec.union(PipeFlags::NON_BLOCKING)));
        assert_eq!(pidfd_open(0, 0), Err(EINVAL));
        assert_eq!(pidfd_open(-5, 0), Err(EINVAL));
        // PIDFD_THREAD, which is O_EXCL, and a made-up flag.
        assert_eq!(pidfd_open(1, 0o200), Err(EINVAL));
        assert_eq!(pidfd_open(1, 1), Err(EINVAL));
    }

    fn record(signal: i32, code: i32, value: u64) -> [u8; SIGNALFD_SIGINFO_SIZE] {
        let mut bytes = [0u8; SIGNALFD_SIGINFO_SIZE];
        bytes[0..4].copy_from_slice(&signal.to_le_bytes());
        bytes[8..12].copy_from_slice(&code.to_le_bytes());
        bytes[24..32].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    #[test]
    fn a_signal_with_a_record_carries_its_value() {
        assert_eq!(pidfd_signal_info(&record(40, SI_QUEUE, 99), 40, false), Ok(99));
        assert_eq!(pidfd_signal_info(&record(40, SI_TIMER, 7), 40, false), Ok(7));
    }

    #[test]
    fn a_record_for_another_signal_is_invalid() {
        assert_eq!(pidfd_signal_info(&record(41, SI_QUEUE, 0), 40, false), Err(EINVAL));
        // Checked before the code, as Linux does.
        assert_eq!(pidfd_signal_info(&record(41, 0, 0), 40, false), Err(EINVAL));
    }

    #[test]
    fn only_a_process_may_fake_any_code_to_itself() {
        for code in [0, 1, SI_KERNEL, SI_TKILL] {
            assert_eq!(pidfd_signal_info(&record(10, code, 0), 10, false), Err(EPERM), "{code}");
            assert_eq!(pidfd_signal_info(&record(10, code, 5), 10, true), Ok(5), "{code}");
        }
    }

    #[test]
    fn a_queued_signal_takes_the_records_value_whatever_its_signal() {
        assert_eq!(queued_value(&record(0, SI_QUEUE, 1234), false), Ok(1234));
        assert_eq!(queued_value(&record(41, SI_TIMER, 7), false), Ok(7));
        for code in [0, 1, SI_KERNEL, SI_TKILL] {
            assert_eq!(queued_value(&record(10, code, 0), false), Err(EPERM), "{code}");
            assert_eq!(queued_value(&record(10, code, 5), true), Ok(5), "{code}");
        }
    }
}
