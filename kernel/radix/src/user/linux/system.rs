// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's calls for what a process is and has (ADR 0610): its process
//! group and session, the user and groups it acts for, its limits, times
//! and scheduling, the names `prctl` sets, the clock `time` reads and the
//! waits of `waitid`. Each ends in a method of the process's own, as
//! Dukovo's C library makes the same calls
//! (`ports/mlibc/sysdeps/radix/generic/process.cpp`), with Linux's raw
//! values and records (`radix_linux::system`).
//!
//! A process acts for one user and one group, which it is started with and
//! keeps, and has no set-user-ID: the calls that set an identity succeed
//! when they change nothing and fail with EPERM when they would change it.

use radix_abi::UserBuffer;
use radix_abi::address_space::PAGE_SIZE;
use radix_abi::names::GROUPS_MAX;
use radix_abi::process::{
    self as abi, ChildWaitFlags, ClockKind, LimitKind, PriorityWhich, SchedulingPolicy,
};
use radix_abi::startup::SLOT_PROCESS;
use radix_cap::Rights;
use radix_hal::CpuLocal;
use radix_linux::errno::{EINVAL, EPERM, ESRCH};
use radix_linux::records::{self, RUSAGE_SIZE, SYSINFO_SIZE, Time};
use radix_linux::system::{self as lx, Rlimit, Usage};

use super::files::on_object;
use super::process::identity;
use super::{Args, Error, Result, copy_in, copy_out, fail, results};
use crate::Arch;
use crate::sched::{self, ThreadRef};
use crate::user::UserRef;
use crate::user::posix::exec;

/// The process whose number is `pid`, a Linux `pid_t`, for the native
/// `ids`: 0 is the caller's, and a number below 0 is no process's.
fn ids(me: &ThreadRef, user: &UserRef, pid: u64) -> Result<abi::IdsResults> {
    let Ok(pid) = u64::try_from(pid as i32) else { return fail(ESRCH) };
    let words = results(me, user, SLOT_PROCESS, abi::IDS, abi::IdsArgs { pid }.encode())?;
    Ok(abi::IdsResults::decode(&words)?)
}

/// `getpgid(pid)`: the native `ids`' process group.
pub fn getpgid(me: &ThreadRef, user: &UserRef, pid: u64) -> Result<u64> {
    ids(me, user, pid).map(|ids| ids.group)
}

/// `getpgrp()`: the native `ids`' process group, of the caller.
pub fn getpgrp(me: &ThreadRef, user: &UserRef) -> Result<u64> {
    getpgid(me, user, 0)
}

/// `getsid(pid)`: the native `ids`' session.
pub fn getsid(me: &ThreadRef, user: &UserRef, pid: u64) -> Result<u64> {
    ids(me, user, pid).map(|ids| ids.session)
}

/// `setpgid(pid, pgid)`: the native `set_group`. A negative group is
/// EINVAL and a negative process ESRCH, as on Linux, which the libc
/// wrapper turns both into EINVAL.
pub fn setpgid(me: &ThreadRef, user: &UserRef, pid: u64, group: u64) -> Result<u64> {
    let (pid, group) = (pid as i32, group as i32);
    let Ok(group) = u64::try_from(group) else { return fail(EINVAL) };
    let Ok(pid) = u64::try_from(pid) else { return fail(ESRCH) };
    results(me, user, SLOT_PROCESS, abi::SET_GROUP, abi::SetGroupArgs { pid, group }.encode())
        .map(|_| 0)
}

/// `setsid()`: the native `new_session`, which gives the new session's
/// number.
pub fn setsid(me: &ThreadRef, user: &UserRef) -> Result<u64> {
    let words = results(me, user, SLOT_PROCESS, abi::NEW_SESSION, [0; 8])?;
    Ok(abi::NewSessionResults::decode(&words)?.session)
}

/// `setuid(uid)`, and `setgid(gid)` with `group`: EPERM unless `id` is
/// the user's (the group's) own, and EINVAL for -1, which is no id. The
/// native `identity` says what the process acts for.
pub fn set_id(me: &ThreadRef, user: &UserRef, id: u64, group: bool) -> Result<u64> {
    let id = id as u32;
    if id == u32::MAX {
        return fail(EINVAL);
    }
    let (own_user, own_group) = identity(me, user)?;
    if id == if group { own_group } else { own_user } { Ok(0) } else { fail(EPERM) }
}

/// `setreuid`, `setresuid` and with `group` `setregid` and `setresgid`
/// for the ids in `ids`: each is -1 or the same as the process's own, else
/// EPERM. The native `identity` says what the process acts for.
pub fn set_ids(me: &ThreadRef, user: &UserRef, ids: &[u64], group: bool) -> Result<u64> {
    let (own_user, own_group) = identity(me, user)?;
    let own = if group { own_group } else { own_user };
    if ids.iter().all(|&id| id as u32 == u32::MAX || id as u32 == own) {
        Ok(0)
    } else {
        fail(EPERM)
    }
}

/// `setfsuid(uid)`, and `setfsgid(gid)` with `group`: the user's (group's)
/// own id, which is what it was and stays, as Linux answers a caller that
/// may not change it. The native `identity`.
pub fn get_fs_id(me: &ThreadRef, user: &UserRef, group: bool) -> Result<u64> {
    let (user_id, group_id) = identity(me, user)?;
    Ok(u64::from(if group { group_id } else { user_id }))
}

/// `getresuid(ruid, euid, suid)`, and `getresgid` with `group`: the id
/// the process acts for, three times, which `identity` gives.
pub fn get_res_ids(me: &ThreadRef, user: &UserRef, at: &[u64], group: bool) -> Result<u64> {
    let (user_id, group_id) = identity(me, user)?;
    let id = if group { group_id } else { user_id };
    for &address in at {
        copy_out(address, &id.to_le_bytes())?;
    }
    Ok(0)
}

/// How many other groups the process has: the native `identity`, asked
/// for no list.
fn group_count(me: &ThreadRef, user: &UserRef) -> Result<u64> {
    let args = abi::IdentityArgs { groups: UserBuffer::default() };
    let words = results(me, user, SLOT_PROCESS, abi::IDENTITY, args.encode())?;
    Ok(u64::from(abi::IdentityResults::decode(&words)?.count))
}

/// `getgroups(size, list)`: the native `identity`. A size of 0 only
/// counts the groups, and one too small for them is EINVAL with nothing
/// written, as on Linux.
pub fn getgroups(me: &ThreadRef, user: &UserRef, size: u64, list: u64) -> Result<u64> {
    let Ok(size) = u64::try_from(size as i32) else { return fail(EINVAL) };
    let count = group_count(me, user)?;
    if size == 0 || count == 0 {
        return Ok(count);
    }
    if size < count {
        return fail(EINVAL);
    }
    let args = abi::IdentityArgs { groups: UserBuffer { address: list, len: count * 4 } };
    results(me, user, SLOT_PROCESS, abi::IDENTITY, args.encode())?;
    Ok(count)
}

/// `setgroups(size, list)`: EPERM unless the list holds the groups the
/// process has, in any order. They are the groups the native `identity`
/// gives, read with the check it makes.
pub fn setgroups(me: &ThreadRef, user: &UserRef, size: u64, list: u64) -> Result<u64> {
    let count = group_count(me, user)?;
    if u64::from(size as u32) != count {
        return fail(EPERM);
    }
    let mut given = [0u8; GROUPS_MAX as usize * 4];
    let given = &mut given[..(count as usize * 4).min(GROUPS_MAX as usize * 4)];
    copy_in(list, given)?;
    let has = on_object(user, SLOT_PROCESS, |found| {
        let process = crate::user::process::own_process(user, found, Rights::READ)?;
        let identity = process.identity();
        let own = identity.groups();
        let has = |group: &[u8; 4]| own.contains(&u32::from_le_bytes(*group));
        Ok(given.as_chunks::<4>().0.iter().all(has))
    })?;
    if has { Ok(0) } else { fail(EPERM) }
}

/// The limit `kind` of the process: the native `limits`, which with `new`
/// sets it too. The limit as it was.
fn limits(me: &ThreadRef, user: &UserRef, kind: LimitKind, new: Option<Rlimit>) -> Result<Rlimit> {
    let new_limit = new.unwrap_or_default();
    let args = abi::LimitsArgs {
        resource: kind,
        set: new.is_some(),
        current: new_limit.current,
        max: new_limit.max,
    };
    let words = results(me, user, SLOT_PROCESS, abi::LIMITS, args.encode())?;
    let was = abi::LimitsResults::decode(&words)?;
    Ok(Rlimit { current: was.current, max: was.max })
}

/// The limit `kind` of process `pid`: the native `limits_of`, which with
/// `new` sets it too. The limit as it was.
fn limits_of(
    me: &ThreadRef,
    user: &UserRef,
    pid: u64,
    kind: LimitKind,
    new: Option<Rlimit>,
) -> Result<Rlimit> {
    let new_limit = new.unwrap_or_default();
    let args = abi::LimitsOfArgs {
        pid,
        resource: kind,
        set: new.is_some(),
        current: new_limit.current,
        max: new_limit.max,
    };
    let words = results(me, user, SLOT_PROCESS, abi::LIMITS_OF, args.encode())?;
    let was = abi::LimitsOfResults::decode(&words)?;
    Ok(Rlimit { current: was.current, max: was.max })
}

/// The limit `resource` names, a Linux `unsigned int`, or EINVAL.
fn limit_kind(resource: u64) -> Result<LimitKind> {
    lx::limit_kind(u64::from(resource as u32)).map_err(Error::Errno)
}

/// The `struct rlimit` at `address`.
fn read_rlimit(address: u64) -> Result<Rlimit> {
    let mut bytes = [0u8; lx::RLIMIT_SIZE];
    copy_in(address, &mut bytes)?;
    Ok(Rlimit::read(&bytes))
}

/// Writes `limit` as a `struct rlimit` at `address`.
fn put_rlimit(address: u64, limit: Rlimit) -> Result<()> {
    let mut bytes = [0u8; lx::RLIMIT_SIZE];
    limit.put(&mut bytes);
    copy_out(address, &bytes)
}

/// `getrlimit(resource, rlim)`: the native `limits`.
pub fn getrlimit(me: &ThreadRef, user: &UserRef, resource: u64, rlim: u64) -> Result<u64> {
    let limit = limits(me, user, limit_kind(resource)?, None)?;
    put_rlimit(rlim, limit).map(|()| 0)
}

/// `setrlimit(resource, rlim)`: the native `limits` with a new limit.
pub fn setrlimit(me: &ThreadRef, user: &UserRef, resource: u64, rlim: u64) -> Result<u64> {
    let kind = limit_kind(resource)?;
    limits(me, user, kind, Some(read_rlimit(rlim)?)).map(|_| 0)
}

/// `prlimit64(pid, resource, new_limit, old_limit)`: the native `limits`
/// for the caller (0, or its own number), else `limits_of`. The old limit
/// is written once the new one is set.
pub fn prlimit64(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let (pid, kind) = (a[0] as i32, limit_kind(a[1])?);
    let new = if a[2] == 0 { None } else { Some(read_rlimit(a[2])?) };
    let Ok(pid) = u64::try_from(pid) else { return fail(ESRCH) };
    let old = if pid == 0 || pid == user.process.number() {
        limits(me, user, kind, new)?
    } else {
        limits_of(me, user, pid, kind, new)?
    };
    if a[3] != 0 {
        put_rlimit(a[3], old)?;
    }
    Ok(0)
}

/// The time on native clock `kind`, in nanoseconds: the native `clock`.
fn clock(me: &ThreadRef, user: &UserRef, kind: ClockKind) -> Result<u64> {
    let words =
        results(me, user, SLOT_PROCESS, abi::CLOCK, abi::ClockArgs { clock: kind }.encode())?;
    Ok(abi::ClockResults::decode(&words)?.ns)
}

/// The process's CPU times and its waited-for children's: the native
/// `times`.
fn process_times(me: &ThreadRef, user: &UserRef) -> Result<abi::TimesResults> {
    let words = results(me, user, SLOT_PROCESS, abi::TIMES, [0; 8])?;
    Ok(abi::TimesResults::decode(&words)?)
}

/// `getrusage(who, usage)`: the native `times`, or for `RUSAGE_THREAD` the
/// native `clock` of the thread's CPU time. What Radix does not count is 0.
pub fn getrusage(me: &ThreadRef, user: &UserRef, who: u64, usage: u64) -> Result<u64> {
    let (user_ns, system_ns) = match lx::usage(who as i32).map_err(Error::Errno)? {
        Usage::Own => {
            let times = process_times(me, user)?;
            (times.user_ns, times.system_ns)
        }
        Usage::Children => {
            let times = process_times(me, user)?;
            (times.children_user_ns, times.children_system_ns)
        }
        Usage::Thread => (clock(me, user, ClockKind::Thread)?, 0),
    };
    let mut out = [0u8; RUSAGE_SIZE];
    records::put_rusage(user_ns, system_ns, 0, &mut out);
    copy_out(usage, &out).map(|()| 0)
}

/// `times(buf)`: the native `times`, in clock ticks, and the ticks since
/// boot the native monotonic `clock` counts, which is what `times` returns.
pub fn times(me: &ThreadRef, user: &UserRef, buf: u64) -> Result<u64> {
    let times = process_times(me, user)?;
    if buf != 0 {
        let out = lx::put_tms(
            times.user_ns,
            times.system_ns,
            times.children_user_ns,
            times.children_system_ns,
        );
        copy_out(buf, &out)?;
    }
    Ok(lx::ticks(clock(me, user, ClockKind::Monotonic)?))
}

/// The number at the start of the line `key` of file `path` beneath the
/// process's root (`total: 1024 KiB`), or none if it cannot be read: the
/// files under `/processes` and `/kernel`, which Dukovo's C library reads.
fn proc_field(user: &UserRef, path: &[u8], key: &[u8]) -> Option<u64> {
    // The lines it wants come first in the file.
    let mut text = [0u8; 256];
    let read = exec::read_from_root(&user.process, path, &mut text).ok()?;
    lx::field(&text[..read], key)
}

/// How many CPUs the machine has, from `/kernel/cpus`: 1 if it does not
/// say.
fn cpus(user: &UserRef) -> usize {
    let mut text = [0u8; 32];
    let read = exec::read_from_root(&user.process, b"kernel/cpus", &mut text).unwrap_or(0);
    lx::number(&text[..read]).filter(|&n| n > 0).map_or(1, |n| n as usize)
}

/// `sysinfo(info)`: the memory `/processes/meminfo` says the machine has
/// and has free, the time since boot the kernel's clock counts, and one
/// process (docs/deferred.md).
pub fn sysinfo(user: &UserRef, info: u64) -> Result<u64> {
    let kib =
        |key: &[u8]| proc_field(user, b"processes/meminfo", key).unwrap_or(0).saturating_mul(1024);
    let uptime = (sched::now_ns() / 1_000_000_000) as i64;
    let mut out = [0u8; SYSINFO_SIZE];
    records::put_sysinfo(uptime, kib(b"total"), kib(b"free"), 1, &mut out);
    copy_out(info, &out).map(|()| 0)
}

/// `sched_getaffinity(pid, len, mask)`: the CPU set of the thread (0 is
/// the caller's) the native `affinity` gives, with the checks it makes,
/// of which the bytes the machine's CPUs need go to `mask`. The raw call
/// returns their number, as Linux does, where the libc wrapper returns 0.
pub fn sched_getaffinity(user: &UserRef, pid: u64, len: u64, mask: u64) -> Result<u64> {
    let Ok(thread) = u64::try_from(pid as i32) else { return fail(ESRCH) };
    let wanted = lx::affinity_size(cpus(user));
    if len < wanted as u64 || !len.is_multiple_of(8) {
        return fail(EINVAL);
    }
    let set = on_object(user, SLOT_PROCESS, |found| {
        crate::user::process::own_process(user, found, Rights::CONTROL)?;
        crate::user::posix::priority::affinity_of(user, thread)
    })?;
    let kept = wanted.min(set.len());
    copy_out(mask, &set[..kept])?;
    Ok(kept as u64)
}

/// `sched_setaffinity(pid, len, mask)`: the native `set_affinity` of the
/// thread (0 is the caller's), with the mask the program gave, of which
/// the kernel reads as many bytes as it has CPUs for.
pub fn sched_setaffinity(
    me: &ThreadRef,
    user: &UserRef,
    pid: u64,
    len: u64,
    mask: u64,
) -> Result<u64> {
    let Ok(thread) = u64::try_from(pid as i32) else { return fail(ESRCH) };
    let args = abi::SetAffinityArgs {
        thread,
        cpus: UserBuffer { address: mask, len: len.min(PAGE_SIZE) },
    };
    results(me, user, SLOT_PROCESS, abi::SET_AFFINITY, args.encode()).map(|_| 0)
}

/// `getpriority(which, who)`: the native `priority`, and `20 - nice`, as
/// the raw call returns it so that no answer is negative.
pub fn getpriority(me: &ThreadRef, user: &UserRef, which: u64, who: u64) -> Result<u64> {
    let args = priority_args(which, who, None)?;
    let words = results(me, user, SLOT_PROCESS, abi::PRIORITY, args.encode())?;
    Ok(lx::priority_value(abi::PriorityResults::decode(&words)?.nice))
}

/// `setpriority(which, who, prio)`: the native `priority` with a new nice
/// level, which the kernel cuts to -20 to 19.
pub fn setpriority(me: &ThreadRef, user: &UserRef, which: u64, who: u64, nice: u64) -> Result<u64> {
    let args = priority_args(which, who, Some(nice as i32))?;
    results(me, user, SLOT_PROCESS, abi::PRIORITY, args.encode()).map(|_| 0)
}

/// The native `priority`'s arguments for `which` (`PRIO_PROCESS`,
/// `PRIO_PGRP`, `PRIO_USER`, as `int`) and `who` (an `id_t`), setting
/// `nice` if there is one.
fn priority_args(which: u64, who: u64, nice: Option<i32>) -> Result<abi::PriorityArgs> {
    let Some(which) = PriorityWhich::from_raw(which as u32) else { return fail(EINVAL) };
    Ok(abi::PriorityArgs {
        which,
        who: u64::from(who as u32),
        set: nice.is_some(),
        nice: nice.unwrap_or(0),
    })
}

/// The scheduling policy and priority of the thread `pid` names (0 is the
/// caller's): the native `scheduling`. EINVAL for a negative number, as
/// Linux gives for the `sched_` calls.
fn scheduling(me: &ThreadRef, user: &UserRef, pid: u64) -> Result<abi::SchedulingResults> {
    let Ok(thread) = u64::try_from(pid as i32) else { return fail(EINVAL) };
    let args =
        abi::SchedulingArgs { thread, set: false, policy: SchedulingPolicy::Other, priority: 0 };
    let words = results(me, user, SLOT_PROCESS, abi::SCHEDULING, args.encode())?;
    Ok(abi::SchedulingResults::decode(&words)?)
}

/// `sched_getscheduler(pid)`: the policy of the native `scheduling`.
pub fn sched_getscheduler(me: &ThreadRef, user: &UserRef, pid: u64) -> Result<u64> {
    scheduling(me, user, pid).map(|found| u64::from(found.policy.raw()))
}

/// `sched_getparam(pid, param)`: the priority of the native `scheduling`,
/// as the `int` of a `struct sched_param`.
pub fn sched_getparam(me: &ThreadRef, user: &UserRef, pid: u64, param: u64) -> Result<u64> {
    if param == 0 {
        return fail(EINVAL);
    }
    let found = scheduling(me, user, pid)?;
    copy_out(param, &found.priority.to_le_bytes()).map(|()| 0)
}

/// `sched_get_priority_max(policy)`: the policy's highest priority. It
/// ends in no method: the ranges are the kernel's policies'.
pub fn sched_get_priority_max(policy: u64) -> Result<u64> {
    lx::priority_max(policy as i32).map(|max| max as u64).map_err(Error::Errno)
}

/// `sched_get_priority_min(policy)`: the policy's lowest priority. It
/// ends in no method.
pub fn sched_get_priority_min(policy: u64) -> Result<u64> {
    lx::priority_min(policy as i32).map(|min| min as u64).map_err(Error::Errno)
}

/// The name `PR_SET_NAME` gives: the bytes at `address` up to the NUL, at
/// most 15 (`NAME_MAX`), as Linux keeps the first 15. Read a byte at a
/// time, so that a name at the end of the program's memory is read.
fn read_name(address: u64) -> Result<usize> {
    for len in 0..abi::NAME_MAX {
        let mut byte = [0u8; 1];
        copy_in(address.saturating_add(len), &mut byte)?;
        if byte[0] == 0 {
            return Ok(len as usize);
        }
    }
    Ok(abi::NAME_MAX as usize)
}

/// `PR_SET_NAME`: the native `set_thread_name` of the calling thread, with
/// the bytes of the name where the program has them.
fn set_name(me: &ThreadRef, user: &UserRef, address: u64) -> Result<u64> {
    let name = UserBuffer { address, len: read_name(address)? as u64 };
    let args = abi::SetThreadNameArgs { thread: 0, name };
    results(me, user, SLOT_PROCESS, abi::SET_THREAD_NAME, args.encode()).map(|_| 0)
}

/// `PR_GET_NAME`: the native `thread_name` of the calling thread, which
/// writes the name where the program wants it, and the NULs that fill the
/// 16 bytes Linux writes follow.
fn get_name(me: &ThreadRef, user: &UserRef, address: u64) -> Result<u64> {
    let buffer = UserBuffer { address, len: abi::NAME_MAX };
    let args = abi::ThreadNameArgs { thread: 0, buffer };
    let words = results(me, user, SLOT_PROCESS, abi::THREAD_NAME, args.encode())?;
    let length = abi::ThreadNameResults::decode(&words)?.length.min(abi::NAME_MAX);
    let padding = [0u8; lx::COMM_SIZE];
    let rest = lx::COMM_SIZE - length as usize;
    copy_out(address.saturating_add(length), &padding[..rest]).map(|()| 0)
}

/// `prctl(option, arg2, ...)`: the thread's name (the native
/// `set_thread_name` and `thread_name`) and whether the process dumps core,
/// which is always on and may be set to what it is. Every other option is
/// EINVAL, as Linux answers an option it does not know.
pub fn prctl(me: &ThreadRef, user: &UserRef, option: u64, arg: u64) -> Result<u64> {
    match option as u32 {
        lx::PR_SET_NAME => set_name(me, user, arg),
        lx::PR_GET_NAME => get_name(me, user, arg),
        lx::PR_GET_DUMPABLE => Ok(1),
        lx::PR_SET_DUMPABLE if matches!(arg, 0 | 1) => Ok(0),
        _ => fail(EINVAL),
    }
}

/// `gettimeofday(tv, tz)`: the native `clock`'s wall clock, as a
/// `timeval`; the time zone is always UTC. Either may be null.
pub fn gettimeofday(me: &ThreadRef, user: &UserRef, tv: u64, tz: u64) -> Result<u64> {
    if tv != 0 {
        let mut out = [0u8; records::TIMESPEC_SIZE];
        records::put_timeval(Time::from_ns(clock(me, user, ClockKind::Realtime)?), &mut out);
        copy_out(tv, &out)?;
    }
    if tz != 0 {
        // `struct timezone`: minutes west of Greenwich and the DST rule.
        copy_out(tz, &[0u8; 8])?;
    }
    Ok(0)
}

/// `time(tloc)`: the native `clock`'s wall clock in whole seconds, also
/// at `tloc` if it is not null.
pub fn time(me: &ThreadRef, user: &UserRef, tloc: u64) -> Result<u64> {
    let seconds = clock(me, user, ClockKind::Realtime)? / 1_000_000_000;
    if tloc != 0 {
        copy_out(tloc, &seconds.to_le_bytes())?;
    }
    Ok(seconds)
}

/// The native flags for a `waitid` of `wait`.
fn child_flags(wait: &lx::WaitFor) -> ChildWaitFlags {
    let mut flags = ChildWaitFlags::NONE;
    for (asked, flag) in [
        (wait.no_hang, ChildWaitFlags::NO_HANG),
        (wait.stopped, ChildWaitFlags::STOPPED),
        (wait.continued, ChildWaitFlags::CONTINUED),
        (wait.no_reap, ChildWaitFlags::NO_REAP),
        (wait.skip_ended, ChildWaitFlags::SKIP_ENDED),
    ] {
        if asked {
            flags = flags.union(flag);
        }
    }
    flags
}

/// `waitid(idtype, id, infop, options, rusage)`: the native `wait`, and a
/// whole `siginfo_t` for the child it gave, as Linux fills it: `SIGCHLD`,
/// what happened to the child and its CPU time. For `WNOHANG` and no child
/// the record is zeros. A child's user is the caller's, as Radix has no
/// set-user-ID.
pub fn waitid(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let (idtype, id, options, eagain) = if a[0] as u32 == radix_linux::kernel_files::P_PIDFD {
        // The options first, as Linux checks them before the descriptor.
        lx::waitid(lx::P_PID, 1, a[3] as u32).map_err(Error::Errno)?;
        // The process file's process, which waits like a `P_PID` one, and
        // not at all for a file that does not wait, which then fails with
        // EAGAIN where a plain WNOHANG gives an empty record (ADR 0613).
        let (pid, non_blocking) = super::kernel_files::process_of(me, user, a[1])?;
        let no_hang = a[3] as u32 & lx::WNOHANG != 0;
        (
            lx::P_PID,
            pid as u32,
            a[3] as u32 | if non_blocking { lx::WNOHANG } else { 0 },
            non_blocking && !no_hang,
        )
    } else {
        (a[0] as u32, a[1] as u32, a[3] as u32, false)
    };
    let wait = lx::waitid(idtype, id, options).map_err(Error::Errno)?;
    let args = abi::WaitArgs { pid: wait.pid, flags: child_flags(&wait) };
    let found =
        abi::WaitResults::decode(&results(me, user, SLOT_PROCESS, abi::WAIT, args.encode())?)?;
    if found.pid == 0 && eagain {
        return fail(radix_linux::errno::EAGAIN);
    }
    let info = if found.pid == 0 {
        [0u8; lx::SIGINFO_SIZE]
    } else {
        let (uid, _) = identity(me, user)?;
        lx::put_child_info(found.pid as u32, uid, found.status, found.user_ns, found.system_ns)
    };
    if a[2] != 0 {
        copy_out(a[2], &info)?;
    }
    if a[4] != 0 && found.pid != 0 {
        let mut out = [0u8; RUSAGE_SIZE];
        records::put_rusage(found.user_ns, found.system_ns, 0, &mut out);
        copy_out(a[4], &out)?;
    }
    Ok(0)
}

/// `getcpu(cpu, node, cache)`: the number of the CPU the thread runs on at
/// `cpu` and the NUMA node, always 0, at `node`, each a 32-bit number a
/// null pointer leaves out. EFAULT for one that cannot be written, as
/// Linux's is; the third argument is unused, as it is there. The thread
/// may move to another CPU before it reads the answer, as on Linux.
pub fn getcpu(cpu: u64, node: u64) -> Result<u64> {
    let here = Arch::current_cpu() as u32;
    if cpu != 0 {
        copy_out(cpu, &here.to_le_bytes())?;
    }
    if node != 0 {
        copy_out(node, &0u32.to_le_bytes())?;
    }
    Ok(0)
}
