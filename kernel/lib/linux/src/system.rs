// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's calls for what a process is and has: its limits, its times and
//! its resource usage, its scheduling, the names `prctl` sets, its
//! capabilities (`capget`), the waits of `waitid`, and the `siginfo_t` a
//! signal wait gives.
//!
//! Each call is the one Dukovo's C library makes for the same POSIX
//! function (`ports/mlibc/sysdeps/radix/generic/process.cpp`) with Linux's
//! raw values and records: `getpriority` returns `20 - nice`, a CPU set is
//! as long as the machine's CPUs need, and `waitid` fills a whole
//! `siginfo_t`. Records are built in, or read from, byte slices of their
//! exact size, little-endian, and the host tests check every field's
//! offset.

use radix_abi::process::{LimitKind, UNLIMITED};

use crate::errno::EINVAL;
use crate::sockets::{get, put};

/// The size of `struct rlimit`.
pub const RLIMIT_SIZE: usize = 16;
/// Linux's `RLIM_INFINITY`, all ones, which is the native `UNLIMITED`.
pub const RLIM_INFINITY: u64 = UNLIMITED;

/// A `struct rlimit`: a soft limit and a hard limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rlimit {
    /// `rlim_cur`: the limit in force.
    pub current: u64,
    /// `rlim_max`: the most the limit may be raised to.
    pub max: u64,
}

impl Rlimit {
    /// Reads a `struct rlimit`.
    pub fn read(bytes: &[u8; RLIMIT_SIZE]) -> Rlimit {
        Rlimit {
            current: u64::from_le_bytes(get(bytes, 0)),
            max: u64::from_le_bytes(get(bytes, 8)),
        }
    }

    /// Writes a `struct rlimit`.
    pub fn put(&self, out: &mut [u8; RLIMIT_SIZE]) {
        put(out, 0, self.current.to_le_bytes());
        put(out, 8, self.max.to_le_bytes());
    }
}

/// The native limit that Linux's `RLIMIT_` number `resource` names: the
/// numbers are the same, 0 to 15.
///
/// # Errors
///
/// EINVAL for a number that is not one.
pub fn limit_kind(resource: u64) -> Result<LimitKind, i32> {
    u32::try_from(resource).ok().and_then(LimitKind::from_raw).ok_or(EINVAL)
}

/// Whose resource usage `getrusage` gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// `RUSAGE_SELF`: the process's.
    Own,
    /// `RUSAGE_CHILDREN`: its children's that were waited for.
    Children,
    /// `RUSAGE_THREAD`: the calling thread's.
    Thread,
}

/// What `getrusage`'s `who` asks for.
///
/// # Errors
///
/// EINVAL for any other value.
pub fn usage(who: i32) -> Result<Usage, i32> {
    match who {
        0 => Ok(Usage::Own),
        -1 => Ok(Usage::Children),
        1 => Ok(Usage::Thread),
        _ => Err(EINVAL),
    }
}

/// The clock ticks a second has, `sysconf(_SC_CLK_TCK)`.
pub const CLOCK_TICKS: u64 = 100;
/// The nanoseconds in a clock tick.
const TICK_NS: u64 = 1_000_000_000 / CLOCK_TICKS;
/// The size of `struct tms`.
pub const TMS_SIZE: usize = 32;

/// The clock ticks in `ns` nanoseconds, the unit of `times`.
pub fn ticks(ns: u64) -> u64 {
    ns / TICK_NS
}

/// Writes `struct tms` for a process and its waited-for children, which
/// ran for the nanoseconds given.
pub fn put_tms(
    user_ns: u64,
    system_ns: u64,
    children_user_ns: u64,
    children_system_ns: u64,
) -> [u8; TMS_SIZE] {
    let mut out = [0u8; TMS_SIZE];
    for (at, ns) in
        [user_ns, system_ns, children_user_ns, children_system_ns].into_iter().enumerate()
    {
        put(&mut out, at * 8, ticks(ns).to_le_bytes());
    }
    out
}

/// `prctl`'s `PR_SET_PDEATHSIG`: the signal the caller gets when its
/// parent ends.
pub const PR_SET_PDEATHSIG: u32 = 1;
/// `prctl`'s `PR_GET_PDEATHSIG`.
pub const PR_GET_PDEATHSIG: u32 = 2;
/// `prctl`'s `PR_GET_DUMPABLE`.
pub const PR_GET_DUMPABLE: u32 = 3;
/// `prctl`'s `PR_SET_DUMPABLE`.
pub const PR_SET_DUMPABLE: u32 = 4;
/// `prctl`'s `PR_SET_NAME`.
pub const PR_SET_NAME: u32 = 15;
/// `prctl`'s `PR_GET_NAME`.
pub const PR_GET_NAME: u32 = 16;
/// `prctl`'s `PR_CAPBSET_READ`: whether a capability is in the bounding
/// set.
pub const PR_CAPBSET_READ: u32 = 23;
/// `prctl`'s `PR_CAPBSET_DROP`.
pub const PR_CAPBSET_DROP: u32 = 24;
/// `prctl`'s `PR_SET_TIMERSLACK`.
pub const PR_SET_TIMERSLACK: u32 = 29;
/// `prctl`'s `PR_GET_TIMERSLACK`.
pub const PR_GET_TIMERSLACK: u32 = 30;
/// `prctl`'s `PR_SET_MM`: changes the bounds of the address space's
/// parts, which takes `CAP_SYS_RESOURCE`.
pub const PR_SET_MM: u32 = 35;
/// `prctl`'s `PR_SET_CHILD_SUBREAPER`: the caller adopts its descendants'
/// orphans.
pub const PR_SET_CHILD_SUBREAPER: u32 = 36;
/// `prctl`'s `PR_GET_CHILD_SUBREAPER`.
pub const PR_GET_CHILD_SUBREAPER: u32 = 37;
/// The timer slack Linux gives a thread, in nanoseconds: what
/// `PR_GET_TIMERSLACK` answers.
pub const TIMER_SLACK_NS: u64 = 50_000;
/// The highest signal number `PR_SET_PDEATHSIG` takes: Linux's `_NSIG`.
pub const SIGNAL_MAX: u64 = 64;

/// `capget`'s and `capset`'s first header version, of 32 capabilities.
pub const CAP_VERSION_1: u32 = 0x1998_0330;
/// The second, of 64, which Linux takes and calls deprecated.
pub const CAP_VERSION_2: u32 = 0x2007_1026;
/// The third and current, of 64.
pub const CAP_VERSION_3: u32 = 0x2008_0522;
/// The size of `struct __user_cap_header_struct`: a version and a pid.
pub const CAP_HEADER_SIZE: usize = 8;
/// The size of one `struct __user_cap_data_struct`: the effective,
/// permitted and inheritable sets of 32 capabilities.
pub const CAP_DATA_SIZE: usize = 12;
/// The last capability Linux has: `CAP_CHECKPOINT_RESTORE`.
pub const CAP_LAST_CAP: u64 = 40;
/// Every capability: root's effective and permitted sets, and everyone's
/// bounding set, as `/proc`'s `CapBnd` shows it.
pub const CAP_ALL: u64 = (1 << (CAP_LAST_CAP + 1)) - 1;

/// How many 32-bit data records a capability header of `version` has: 1
/// for the first, 2 for the second and third, none for another, which is
/// EINVAL once the third is written back to the header.
pub fn cap_records(version: u32) -> Option<usize> {
    match version {
        CAP_VERSION_1 => Some(1),
        CAP_VERSION_2 | CAP_VERSION_3 => Some(2),
        _ => None,
    }
}

/// A process's capability sets, each a bit for each capability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapSets {
    /// What it may use now.
    pub effective: u64,
    /// What it may make effective.
    pub permitted: u64,
    /// What a program it runs may keep.
    pub inheritable: u64,
}

impl CapSets {
    /// The sets of a process with user ID `user`, as Dukovo's kernel
    /// treats it: root has every capability, effective and permitted,
    /// and another user none; nothing is inheritable.
    pub fn of_user(user: u32) -> CapSets {
        let all = if user == 0 { CAP_ALL } else { 0 };
        CapSets { effective: all, permitted: all, inheritable: 0 }
    }

    /// The data `capget` writes: `records` records (1 or 2), the low 32
    /// bits of each set first.
    pub fn write(self, records: usize, out: &mut [u8; 2 * CAP_DATA_SIZE]) {
        for record in 0..records.min(2) {
            let shift = 32 * record;
            let at = record * CAP_DATA_SIZE;
            for (n, set) in
                [self.effective, self.permitted, self.inheritable].into_iter().enumerate()
            {
                put(out, at + 4 * n, ((set >> shift) as u32).to_le_bytes());
            }
        }
    }

    /// The sets `capset` was given in `records` records; with 1, the high
    /// bits are `high`'s, which a first-version caller cannot name.
    pub fn read(bytes: &[u8; 2 * CAP_DATA_SIZE], records: usize, high: CapSets) -> CapSets {
        let set = |n: usize, high: u64| {
            let low = u64::from(u32::from_le_bytes(get(bytes, 4 * n)));
            let upper = if records >= 2 {
                u64::from(u32::from_le_bytes(get(bytes, CAP_DATA_SIZE + 4 * n)))
            } else {
                high >> 32
            };
            low | upper << 32
        };
        CapSets {
            effective: set(0, high.effective),
            permitted: set(1, high.permitted),
            inheritable: set(2, high.inheritable),
        }
    }
}
/// The size of a thread's name for `prctl`, with its NUL.
pub const COMM_SIZE: usize = 16;

/// The name `PR_GET_NAME` gives for a thread called `name`: its bytes and
/// NULs to fill the 16 that Linux copies.
pub fn comm(name: &[u8]) -> [u8; COMM_SIZE] {
    let mut out = [0u8; COMM_SIZE];
    let kept = name.len().min(COMM_SIZE - 1);
    out[..kept].copy_from_slice(&name[..kept]);
    out
}

/// What a scheduling policy's priorities are: Linux's `SCHED_FIFO` and
/// `SCHED_RR` have 1 to 99, the other policies only 0.
fn priority_range(policy: i32) -> Result<(i32, i32), i32> {
    match policy {
        // SCHED_FIFO and SCHED_RR.
        1 | 2 => Ok((1, 99)),
        // SCHED_OTHER, SCHED_BATCH, SCHED_IDLE and SCHED_DEADLINE.
        0 | 3 | 5 | 6 => Ok((0, 0)),
        _ => Err(EINVAL),
    }
}

/// `sched_get_priority_max(policy)`.
///
/// # Errors
///
/// EINVAL for a policy that does not exist.
pub fn priority_max(policy: i32) -> Result<i32, i32> {
    priority_range(policy).map(|(_, max)| max)
}

/// `sched_get_priority_min(policy)`.
///
/// # Errors
///
/// EINVAL for a policy that does not exist.
pub fn priority_min(policy: i32) -> Result<i32, i32> {
    priority_range(policy).map(|(min, _)| min)
}

/// What `getpriority` returns for a nice level: `20 - nice`, 1 to 40,
/// which the raw call gives so that no result is negative. The level is
/// -20 to 19, as the kernel keeps it.
pub fn priority_value(nice: i32) -> u64 {
    (20 - i64::from(nice.clamp(-20, 19))) as u64
}

/// The bytes of a CPU set for a machine of `cpus` CPUs, which is what
/// `sched_getaffinity` returns: a whole number of 64-bit words.
pub fn affinity_size(cpus: usize) -> usize {
    cpus.max(1).div_ceil(64) * 8
}

/// `waitid`'s `P_ALL`.
pub const P_ALL: u32 = 0;
/// `waitid`'s `P_PID`.
pub const P_PID: u32 = 1;
/// `waitid`'s `P_PGID`.
pub const P_PGID: u32 = 2;
/// `waitid`'s `WNOHANG`.
pub const WNOHANG: u32 = 0x1;
/// `waitid`'s `WSTOPPED`.
pub const WSTOPPED: u32 = 0x2;
/// `waitid`'s `WEXITED`.
pub const WEXITED: u32 = 0x4;
/// `waitid`'s `WCONTINUED`.
pub const WCONTINUED: u32 = 0x8;
/// `waitid`'s `WNOWAIT`: leave the child waitable.
pub const WNOWAIT: u32 = 0x0100_0000;
/// The options that change nothing here: `__WNOTHREAD`, `__WCLONE` and
/// `__WALL`.
const WAIT_IGNORED: u32 = 0x2000_0000 | 0x4000_0000 | 0x8000_0000;

/// What a `waitid` asks the native `wait` for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitFor {
    /// The native `wait`'s process: a child's number, 0 for the caller's
    /// group, -1 for any child, or the negative of a group's number.
    pub pid: i64,
    /// Return at once if no child has changed state (`WNOHANG`).
    pub no_hang: bool,
    /// Report stopped children (`WSTOPPED`).
    pub stopped: bool,
    /// Report continued children (`WCONTINUED`).
    pub continued: bool,
    /// Leave the child as it was, to be waited for again (`WNOWAIT`).
    pub no_reap: bool,
    /// Skip children that ended: `WEXITED` was not asked for.
    pub skip_ended: bool,
}

/// The wait that `waitid(idtype, id, ..., options)` asks for.
///
/// # Errors
///
/// EINVAL for an option Linux does not take, for none of `WEXITED`,
/// `WSTOPPED` and `WCONTINUED`, for an `idtype` other than `P_ALL`, `P_PID`
/// and `P_PGID` (the table turns `P_PIDFD`, whose `id` is a descriptor,
/// into `P_PID` with the process's number before it asks), for a
/// `P_PID` of no process (an `id` that is 0 or negative as an `int`) and for
/// a negative `P_PGID`.
pub fn waitid(idtype: u32, id: u32, options: u32) -> Result<WaitFor, i32> {
    let known = WNOHANG | WSTOPPED | WEXITED | WCONTINUED | WNOWAIT | WAIT_IGNORED;
    if options & !known != 0 || options & (WEXITED | WSTOPPED | WCONTINUED) == 0 {
        return Err(EINVAL);
    }
    let id = id as i32;
    let pid = match idtype {
        P_ALL => -1,
        P_PID if id > 0 => i64::from(id),
        // Group 0 is the caller's, as `waitpid`'s 0.
        P_PGID if id >= 0 => -i64::from(id),
        _ => return Err(EINVAL),
    };
    Ok(WaitFor {
        pid,
        no_hang: options & WNOHANG != 0,
        stopped: options & WSTOPPED != 0,
        continued: options & WCONTINUED != 0,
        no_reap: options & WNOWAIT != 0,
        skip_ended: options & WEXITED == 0,
    })
}

/// `SIGCHLD`, which `waitid` gives in `si_signo`.
pub const SIGCHLD: i32 = 17;
/// `SIGCONT`, the `si_status` of a child that continued.
const SIGCONT: i32 = 18;
/// `si_code` for a child that exited.
pub const CLD_EXITED: i32 = 1;
/// `si_code` for a child killed by a signal.
pub const CLD_KILLED: i32 = 2;
/// `si_code` for a child killed by a signal that dumped core.
pub const CLD_DUMPED: i32 = 3;
/// `si_code` for a child stopped by a signal.
pub const CLD_STOPPED: i32 = 5;
/// `si_code` for a child that continued.
pub const CLD_CONTINUED: i32 = 6;

/// The `si_code` and `si_status` of a child whose wait status is `status`,
/// as Linux's `waitid` fills them: the exit code of a child that exited,
/// the signal that killed or stopped one, `SIGCONT` for one that continued.
pub fn child_event(status: u32) -> (i32, i32) {
    let low = status & 0x7f;
    let high = ((status >> 8) & 0xff) as i32;
    if low == 0 {
        (CLD_EXITED, high)
    } else if status & 0xff == 0x7f {
        (CLD_STOPPED, high)
    } else if status == 0xffff {
        (CLD_CONTINUED, SIGCONT)
    } else if status & 0x80 != 0 {
        (CLD_DUMPED, low as i32)
    } else {
        (CLD_KILLED, low as i32)
    }
}

/// The size of a `siginfo_t`.
pub const SIGINFO_SIZE: usize = 128;

/// Writes a `siginfo_t` for a signal sent by `pid`, whose user is `uid`,
/// with `code` as its `si_code` and `status` in the union's first word: a
/// child's `si_status`, or a queued signal's value.
pub fn put_siginfo(signal: i32, code: i32, pid: u32, uid: u32, status: u64) -> [u8; SIGINFO_SIZE] {
    let mut out = [0u8; SIGINFO_SIZE];
    put(&mut out, 0, signal.to_le_bytes());
    put(&mut out, 8, code.to_le_bytes());
    put(&mut out, 16, pid.to_le_bytes());
    put(&mut out, 20, uid.to_le_bytes());
    put(&mut out, 24, status.to_le_bytes());
    out
}

/// Writes the `siginfo_t` `waitid` gives for a child `pid` of user `uid`
/// whose wait status is `status` and whose CPU time was `user_ns` and
/// `system_ns` nanoseconds: `SIGCHLD`, the child's event, and the times
/// in clock ticks, as Linux's `si_utime` and `si_stime`.
pub fn put_child_info(
    pid: u32,
    uid: u32,
    status: u32,
    user_ns: u64,
    system_ns: u64,
) -> [u8; SIGINFO_SIZE] {
    let (code, exit) = child_event(status);
    let mut out = put_siginfo(SIGCHLD, code, pid, uid, exit as u64);
    // `si_status` is the low word of the union's first word, which
    // `put_siginfo` wrote with the upper word 0, and the times follow it.
    put(&mut out, 32, ticks(user_ns).to_le_bytes());
    put(&mut out, 40, ticks(system_ns).to_le_bytes());
    out
}

/// The value of the line `key: number` in `text`, as the files under
/// `/processes` write them (`total: 1024 KiB`, `uptime_ns: 5`): the number
/// after the colon.
pub fn field(text: &[u8], key: &[u8]) -> Option<u64> {
    text.split(|&b| b == b'\n').find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(b":")?;
        number(rest)
    })
}

/// The number at the start of `text`, after any spaces: `None` if there is
/// no digit there or it does not fit.
pub fn number(text: &[u8]) -> Option<u64> {
    let start = text.iter().position(|&b| b != b' ' && b != b'\t')?;
    let digits = &text[start..];
    let end = digits.iter().position(|b| !b.is_ascii_digit()).unwrap_or(digits.len());
    let mut value = 0u64;
    if end == 0 {
        return None;
    }
    for &digit in &digits[..end] {
        value = value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlimit_has_linuxs_layout() {
        let mut bytes = [0u8; RLIMIT_SIZE];
        Rlimit { current: 1024, max: RLIM_INFINITY }.put(&mut bytes);
        assert_eq!(&bytes[..8], &1024u64.to_le_bytes());
        assert_eq!(&bytes[8..], &[0xff; 8]);
        assert_eq!(Rlimit::read(&bytes), Rlimit { current: 1024, max: u64::MAX });
        assert_eq!(RLIM_INFINITY, u64::MAX);
    }

    #[test]
    fn rlimit_numbers_are_the_native_ones() {
        // RLIMIT_CPU, RLIMIT_NOFILE, RLIMIT_AS and RLIMIT_RTTIME.
        for (number, kind) in [
            (0, LimitKind::Cpu),
            (7, LimitKind::Files),
            (9, LimitKind::AddressSpace),
            (15, LimitKind::RealtimeTime),
        ] {
            assert_eq!(limit_kind(number), Ok(kind));
        }
        assert_eq!(limit_kind(16), Err(EINVAL));
        assert_eq!(limit_kind(u64::MAX), Err(EINVAL));
        // The high half of a register is not part of an `unsigned int`.
        assert_eq!(limit_kind(0x1_0000_0007), Err(EINVAL));
    }

    #[test]
    fn rusage_scopes_are_linuxs() {
        assert_eq!(usage(0), Ok(Usage::Own));
        assert_eq!(usage(-1), Ok(Usage::Children));
        assert_eq!(usage(1), Ok(Usage::Thread));
        assert_eq!(usage(2), Err(EINVAL));
    }

    #[test]
    fn times_are_in_hundredths() {
        assert_eq!(ticks(0), 0);
        assert_eq!(ticks(9_999_999), 0);
        assert_eq!(ticks(10_000_000), 1);
        assert_eq!(ticks(2_500_000_000), 250);
        let tms = put_tms(10_000_000, 20_000_000, 30_000_000, 40_000_000);
        let words: Vec<u64> =
            tms.chunks(8).map(|w| u64::from_le_bytes(w.try_into().unwrap())).collect();
        assert_eq!(words, [1, 2, 3, 4]);
    }

    #[test]
    fn prctl_options_have_linuxs_numbers() {
        assert_eq!((PR_SET_PDEATHSIG, PR_GET_PDEATHSIG), (1, 2));
        assert_eq!((PR_GET_DUMPABLE, PR_SET_DUMPABLE, PR_SET_NAME, PR_GET_NAME), (3, 4, 15, 16));
        assert_eq!((PR_CAPBSET_READ, PR_CAPBSET_DROP), (23, 24));
        assert_eq!((PR_SET_TIMERSLACK, PR_GET_TIMERSLACK, PR_SET_MM), (29, 30, 35));
        assert_eq!((PR_SET_CHILD_SUBREAPER, PR_GET_CHILD_SUBREAPER), (36, 37));
    }

    #[test]
    fn capability_records_are_linuxs() {
        assert_eq!(cap_records(CAP_VERSION_1), Some(1));
        assert_eq!(cap_records(CAP_VERSION_2), Some(2));
        assert_eq!(cap_records(CAP_VERSION_3), Some(2));
        assert_eq!(cap_records(0), None);
        assert_eq!(CAP_ALL, 0x1ff_ffff_ffff);
        let root = CapSets::of_user(0);
        assert_eq!(root, CapSets { effective: CAP_ALL, permitted: CAP_ALL, inheritable: 0 });
        assert_eq!(CapSets::of_user(1000), CapSets::default());
        let mut out = [0xaa; 2 * CAP_DATA_SIZE];
        root.write(2, &mut out);
        let words: Vec<u32> =
            out.chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        assert_eq!(words, [u32::MAX, u32::MAX, 0, 0x1ff, 0x1ff, 0]);
        assert_eq!(CapSets::read(&out, 2, CapSets::default()), root);
        // A first-version caller names only the low 32.
        let mut out = [0; 2 * CAP_DATA_SIZE];
        root.write(1, &mut out);
        assert_eq!(&out[CAP_DATA_SIZE..], &[0; CAP_DATA_SIZE]);
        assert_eq!(CapSets::read(&out, 1, root), root);
        assert_eq!(CapSets::read(&out, 1, CapSets::default()).effective, u64::from(u32::MAX));
    }

    #[test]
    fn a_name_is_padded_and_cut_to_sixteen_bytes() {
        let name = comm(b"worker");
        assert_eq!(&name[..6], b"worker");
        assert!(name[6..].iter().all(|&b| b == 0));
        assert_eq!(comm(&[b'x'; 40])[15], 0);
        assert_eq!(&comm(&[b'x'; 40])[..15], &[b'x'; 15]);
        assert_eq!(comm(b""), [0; COMM_SIZE]);
    }

    #[test]
    fn priorities_are_linuxs() {
        for policy in [1, 2] {
            assert_eq!((priority_min(policy), priority_max(policy)), (Ok(1), Ok(99)));
        }
        for policy in [0, 3, 5, 6] {
            assert_eq!((priority_min(policy), priority_max(policy)), (Ok(0), Ok(0)));
        }
        for policy in [-1, 4, 7, 100] {
            assert_eq!(priority_max(policy), Err(EINVAL));
            assert_eq!(priority_min(policy), Err(EINVAL));
        }
    }

    #[test]
    fn getpriority_gives_twenty_less_the_nice_level() {
        assert_eq!(priority_value(0), 20);
        assert_eq!(priority_value(-20), 40);
        assert_eq!(priority_value(19), 1);
        assert_eq!(priority_value(i32::MIN), 40);
        assert_eq!(priority_value(i32::MAX), 1);
    }

    #[test]
    fn a_cpu_set_is_whole_words() {
        assert_eq!(affinity_size(0), 8);
        assert_eq!(affinity_size(1), 8);
        assert_eq!(affinity_size(64), 8);
        assert_eq!(affinity_size(65), 16);
        assert_eq!(affinity_size(256), 32);
    }

    #[test]
    fn waitid_asks_for_what_the_options_say() {
        let w = waitid(P_ALL, 0, WEXITED).unwrap();
        assert_eq!(
            w,
            WaitFor {
                pid: -1,
                no_hang: false,
                stopped: false,
                continued: false,
                no_reap: false,
                skip_ended: false
            }
        );
        let w = waitid(P_PID, 42, WSTOPPED | WCONTINUED | WNOHANG | WNOWAIT).unwrap();
        assert_eq!(w.pid, 42);
        assert!(w.no_hang && w.stopped && w.continued && w.no_reap && w.skip_ended);
        assert_eq!(waitid(P_PGID, 7, WEXITED).unwrap().pid, -7);
        assert_eq!(waitid(P_PGID, 0, WEXITED).unwrap().pid, 0);
        // Options that change nothing here are taken.
        assert!(waitid(P_ALL, 0, WEXITED | 0x4000_0000 | 0x2000_0000 | 0x8000_0000).is_ok());
    }

    #[test]
    fn waitid_refuses_what_linux_refuses() {
        assert_eq!(waitid(P_ALL, 0, 0), Err(EINVAL));
        assert_eq!(waitid(P_ALL, 0, WNOHANG), Err(EINVAL));
        assert_eq!(waitid(P_ALL, 0, WEXITED | 0x10), Err(EINVAL));
        assert_eq!(waitid(P_PID, 0, WEXITED), Err(EINVAL));
        assert_eq!(waitid(P_PID, u32::MAX, WEXITED), Err(EINVAL));
        assert_eq!(waitid(P_PGID, u32::MAX, WEXITED), Err(EINVAL));
        // P_PIDFD, which the table has turned into P_PID by now, and a type that
        // does not exist.
        assert_eq!(waitid(3, 5, WEXITED), Err(EINVAL));
        assert_eq!(waitid(9, 5, WEXITED), Err(EINVAL));
    }

    #[test]
    fn a_childs_state_is_told_as_linux_tells_it() {
        assert_eq!(child_event(0), (CLD_EXITED, 0));
        assert_eq!(child_event(3 << 8), (CLD_EXITED, 3));
        assert_eq!(child_event(9), (CLD_KILLED, 9));
        assert_eq!(child_event(0x8b), (CLD_DUMPED, 11));
        assert_eq!(child_event((19 << 8) | 0x7f), (CLD_STOPPED, 19));
        assert_eq!(child_event(0xffff), (CLD_CONTINUED, 18));
    }

    #[test]
    fn siginfo_has_linuxs_layout() {
        let info = put_siginfo(10, -1, 123, 456, 7);
        assert_eq!(&info[0..4], &10i32.to_le_bytes());
        assert_eq!(&info[4..8], &[0; 4]);
        assert_eq!(&info[8..12], &(-1i32).to_le_bytes());
        assert_eq!(&info[12..16], &[0; 4]);
        assert_eq!(&info[16..20], &123u32.to_le_bytes());
        assert_eq!(&info[20..24], &456u32.to_le_bytes());
        assert_eq!(&info[24..32], &7u64.to_le_bytes());
        assert!(info[32..].iter().all(|&b| b == 0));
        assert_eq!(info.len(), 128);
    }

    #[test]
    fn child_info_has_the_childs_times_in_ticks() {
        let info = put_child_info(5, 6, 3 << 8, 20_000_000, 30_000_000);
        assert_eq!(&info[0..4], &17i32.to_le_bytes());
        assert_eq!(&info[8..12], &CLD_EXITED.to_le_bytes());
        assert_eq!(&info[16..20], &5u32.to_le_bytes());
        assert_eq!(&info[20..24], &6u32.to_le_bytes());
        assert_eq!(&info[24..28], &3i32.to_le_bytes());
        assert_eq!(&info[32..40], &2u64.to_le_bytes());
        assert_eq!(&info[40..48], &3u64.to_le_bytes());
    }

    #[test]
    fn process_files_are_read_by_their_keys() {
        let meminfo = b"total: 2048 KiB\nfree: 512 KiB\nused: 1536 KiB\n";
        assert_eq!(field(meminfo, b"total"), Some(2048));
        assert_eq!(field(meminfo, b"free"), Some(512));
        assert_eq!(field(meminfo, b"swap"), None);
        assert_eq!(field(b"uptime_ns: 123456789\n", b"uptime_ns"), Some(123_456_789));
        // A key that only starts a longer one is not it.
        assert_eq!(field(b"free_pages: 9\n", b"free"), None);
        assert_eq!(field(b"total:\n", b"total"), None);
        assert_eq!(field(b"total: x\n", b"total"), None);
    }

    #[test]
    fn numbers_stop_at_the_first_non_digit() {
        assert_eq!(number(b"4\n"), Some(4));
        assert_eq!(number(b"  12 KiB"), Some(12));
        assert_eq!(number(b""), None);
        assert_eq!(number(b"   "), None);
        assert_eq!(number(b"-3"), None);
        assert_eq!(number(b"99999999999999999999999"), None);
    }

    proptest::proptest! {
        #[test]
        fn no_text_makes_a_reader_panic(text in proptest::collection::vec(0u8.., 0..80)) {
            let _ = field(&text, b"total");
            let _ = number(&text);
        }

        #[test]
        fn no_wait_status_makes_a_reader_panic(status: u32, user: u64, system: u64) {
            let _ = child_event(status);
            let _ = put_child_info(1, 2, status, user, system);
        }
    }
}
