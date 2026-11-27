// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The files of a process and of each of its threads, in Linux's text
//! (proc(5)): `comm`, `status`, `stat`, `statm` and `limits`, made from
//! what [`super::records`] read of `/processes`.
//!
//! A thread's files are those of a [`Task`] made from the thread; the
//! process's own are those of a task made from its status, as Linux's
//! `/proc/<n>/stat` counts every thread's time and `/proc/<n>/task/<n>`
//! only the main thread's. Fields the kernel does not keep are the
//! values Linux shows for a process that has none of the thing, never a
//! made-up number.
//!
//! When a program misreads one of these, compare the text with a real
//! Linux's: the tests here hold golden strings of each file.

use super::records::{Policy, Process, State, Thread};
use super::text::Page;

/// The most signals a process may have queued: the kernel's
/// `QUEUE_MAX`, which `limits` shows as the soft and hard limit of
/// pending signals.
const SIGNALS_PENDING_MAX: u64 = 1024;

/// The capabilities Linux's `CapBnd` lists for a kernel with 41 of them.
const CAPABILITIES_ALL: &str = "000001ffffffffff";

/// What `/proc/<n>/stat` counts a time in: Linux's `USER_HZ`, 100 a
/// second.
const NS_PER_TICK: u64 = 10_000_000;

/// A page, in KiB.
const PAGE_KIB: u64 = 4;

/// The longest name Linux gives a thread, without its NUL.
const COMM_MAX: usize = 15;

/// The process or thread whose files are made, with what they say of it.
#[derive(Debug)]
pub struct Task<'a> {
    /// The process it belongs to.
    pub process: &'a Process,
    /// Its number: the process's, or the thread's.
    pub id: u64,
    /// Its name, as Linux's `comm` is: at most 15 bytes.
    pub comm: String,
    /// What it is doing.
    pub state: State,
    /// Its CPU time in user mode.
    pub user_ns: u64,
    /// Its CPU time in the kernel.
    pub system_ns: u64,
    /// Times it gave up the CPU to wait.
    pub voluntary_switches: u64,
    /// Times another thread was given the CPU.
    pub involuntary_switches: u64,
    /// The CPU it last ran on.
    pub cpu: u64,
    /// Its scheduling policy.
    pub policy: Policy,
    /// Its real-time priority.
    pub priority: u64,
    /// Its nice level.
    pub nice: i64,
    /// Its seccomp mode and filters, and `no_new_privs`: the main
    /// thread's, for the process.
    pub seccomp: (u8, u32),
    /// Whether it has `no_new_privs`.
    pub no_new_privs: bool,
}

impl<'a> Task<'a> {
    /// The process as a whole: its totals, and the CPU its main thread,
    /// the one that has the process's number, last ran on.
    pub fn of_process(process: &'a Process, threads: &[Thread]) -> Task<'a> {
        let main = threads.iter().find(|thread| thread.number == process.number);
        let first = main.or(threads.first());
        Task {
            process,
            id: process.number,
            comm: comm_of(&process.name),
            state: process.state,
            user_ns: process.user_ns,
            system_ns: process.system_ns,
            voluntary_switches: process.voluntary_switches,
            involuntary_switches: process.involuntary_switches,
            cpu: first.map_or(0, |thread| thread.cpu),
            policy: process.policy,
            priority: process.priority,
            nice: process.nice,
            seccomp: first.map_or((0, 0), |thread| thread.seccomp),
            no_new_privs: first.is_some_and(|thread| thread.no_new_privs),
        }
    }

    /// One thread of `process`.
    pub fn of_thread(process: &'a Process, thread: &Thread) -> Task<'a> {
        Task {
            process,
            id: thread.number,
            comm: comm_of(&thread.name),
            state: if process.state == State::Zombie { State::Zombie } else { thread.state },
            user_ns: thread.user_ns,
            system_ns: thread.system_ns,
            voluntary_switches: thread.voluntary_switches,
            involuntary_switches: thread.involuntary_switches,
            cpu: thread.cpu,
            policy: thread.policy,
            priority: thread.priority,
            nice: thread.nice,
            seccomp: thread.seccomp,
            no_new_privs: thread.no_new_privs,
        }
    }
}

/// `name`, cut to what Linux's `comm` holds, at a character's end.
fn comm_of(name: &str) -> String {
    let mut end = name.len().min(COMM_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// `/proc/<n>/comm`.
pub fn comm(task: &Task<'_>) -> String {
    format!("{}\n", task.comm)
}

/// The hexadecimal of a mask with the first `cpus` bits set, as the
/// kernel prints a CPU mask: eight digits to a group, the groups
/// separated by commas, the first without leading zeros.
fn cpu_mask(cpus: u32) -> String {
    let digits = cpus.div_ceil(4).max(1);
    let top = match cpus % 4 {
        0 => 0xf,
        bits => (1 << bits) - 1,
    };
    let mut text = String::new();
    for place in (0..digits).rev() {
        let digit = if place == digits - 1 { top } else { 0xf };
        text.push_str(&format!("{digit:x}"));
        if place % 8 == 0 && place != 0 {
            text.push(',');
        }
    }
    text
}

/// The CPUs `0-3` of a machine of `cpus` CPUs, as Linux lists them.
pub fn cpu_list(cpus: u32) -> String {
    if cpus <= 1 { "0".to_string() } else { format!("0-{}", cpus - 1) }
}

/// `/proc/<n>/status` of a machine with `cpus` CPUs.
pub fn status(task: &Task<'_>, cpus: u32) -> String {
    let process = task.process;
    let rss = process.resident_kib;
    let file = rss.saturating_sub(process.anonymous_kib);
    let (user, group) = (process.user, process.group);
    let caps = if user == 0 { CAPABILITIES_ALL } else { "0000000000000000" };
    let mut page = Page::new();
    page.line(format_args!("Name:\t{}", task.comm));
    page.line(format_args!("Umask:\t0022"));
    page.line(format_args!("State:\t{} ({})", task.state.letter(), task.state.words()));
    page.line(format_args!("Tgid:\t{}", process.number));
    page.line(format_args!("Ngid:\t0"));
    page.line(format_args!("Pid:\t{}", task.id));
    page.line(format_args!("PPid:\t{}", process.parent));
    page.line(format_args!("TracerPid:\t0"));
    page.line(format_args!("Uid:\t{user}\t{user}\t{user}\t{user}"));
    page.line(format_args!("Gid:\t{group}\t{group}\t{group}\t{group}"));
    // The kernel keeps no table size: Linux's smallest one after a
    // program has started is 256.
    page.line(format_args!("FDSize:\t256"));
    // A name has its groups only where a program asks for them.
    page.line(format_args!("Groups:\t"));
    page.line(format_args!("NStgid:\t{}", process.number));
    page.line(format_args!("NSpid:\t{}", task.id));
    page.line(format_args!("NSpgid:\t{}", process.process_group));
    page.line(format_args!("NSsid:\t{}", process.session));
    // Linux's peak figures are the highest the process has reached; the
    // kernel keeps the current ones only. Locked, pinned, shared-memory,
    // stack, program, library and page-table sizes are not counted.
    for (label, kib) in [
        ("VmPeak", process.virtual_kib),
        ("VmSize", process.virtual_kib),
        ("VmLck", 0),
        ("VmPin", 0),
        ("VmHWM", rss),
        ("VmRSS", rss),
        ("RssAnon", process.anonymous_kib),
        ("RssFile", file),
        ("RssShmem", 0),
        ("VmData", process.anonymous_kib),
        ("VmStk", 0),
        ("VmExe", 0),
        ("VmLib", 0),
        ("VmPTE", 0),
        ("VmSwap", 0),
        ("HugetlbPages", 0),
    ] {
        page.line(format_args!("{label}:\t{kib:>8} kB"));
    }
    page.line(format_args!("CoreDumping:\t0"));
    page.line(format_args!("THP_enabled:\t1"));
    page.line(format_args!("Threads:\t{}", process.threads));
    page.line(format_args!("SigQ:\t0/{SIGNALS_PENDING_MAX}"));
    for label in ["SigPnd", "ShdPnd", "SigBlk", "SigIgn", "SigCgt", "CapInh"] {
        page.line(format_args!("{label}:\t0000000000000000"));
    }
    page.line(format_args!("CapPrm:\t{caps}"));
    page.line(format_args!("CapEff:\t{caps}"));
    page.line(format_args!("CapBnd:\t{CAPABILITIES_ALL}"));
    page.line(format_args!("CapAmb:\t0000000000000000"));
    page.line(format_args!("NoNewPrivs:\t{}", u8::from(task.no_new_privs)));
    page.line(format_args!("Seccomp:\t{}", task.seccomp.0));
    page.line(format_args!("Seccomp_filters:\t{}", task.seccomp.1));
    page.line(format_args!("Cpus_allowed:\t{}", cpu_mask(cpus)));
    page.line(format_args!("Cpus_allowed_list:\t{}", cpu_list(cpus)));
    page.line(format_args!("Mems_allowed:\t00000001"));
    page.line(format_args!("Mems_allowed_list:\t0"));
    page.line(format_args!("voluntary_ctxt_switches:\t{}", task.voluntary_switches));
    page.line(format_args!("nonvoluntary_ctxt_switches:\t{}", task.involuntary_switches));
    page.finish()
}

/// `tty_nr` of a terminal `pts/`*n*: Linux's device number, major 136
/// and up for the slaves of its pseudo-terminals.
fn terminal_device(number: u32) -> u64 {
    (u64::from(136 + number / 256) << 8) | u64::from(number % 256)
}

/// `/proc/<n>/stat`: one line of the 52 fields proc(5) lists.
pub fn stat(task: &Task<'_>) -> String {
    let process = task.process;
    let tty = process.terminal.map_or(0, terminal_device);
    let foreground = match (process.terminal, process.terminal_group) {
        (Some(_), Some(group)) => i64::try_from(group).unwrap_or(-1),
        _ => -1,
    };
    let priority = task.policy.stat_priority(task.priority, task.nice);
    let rss_pages = process.resident_kib / PAGE_KIB;
    // Fields 1 to 52, in proc(5)'s order. Addresses, signal masks and
    // swap counts the kernel does not keep are 0, as Linux shows them
    // to a reader who may not see them.
    let fields: [String; 52] = [
        task.id.to_string(),
        format!("({})", task.comm),
        task.state.letter().to_string(),
        process.parent.to_string(),
        process.process_group.to_string(),
        process.session.to_string(),
        tty.to_string(),
        foreground.to_string(),
        // flags: PF_RANDOMIZE and the one Linux sets for a program that
        // has made its address space.
        "4194560".to_string(),
        process.minor_faults.to_string(),
        "0".to_string(),
        process.major_faults.to_string(),
        "0".to_string(),
        (task.user_ns / NS_PER_TICK).to_string(),
        (task.system_ns / NS_PER_TICK).to_string(),
        "0".to_string(),
        "0".to_string(),
        priority.to_string(),
        task.nice.to_string(),
        process.threads.to_string(),
        "0".to_string(),
        (process.started_ns / NS_PER_TICK).to_string(),
        (process.virtual_kib * 1024).to_string(),
        rss_pages.to_string(),
        // rsslim: RLIM_INFINITY.
        u64::MAX.to_string(),
        // startcode, endcode, startstack, kstkesp, kstkeip.
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        // signal, blocked, sigignore, sigcatch.
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        // wchan, nswap, cnswap.
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        // exit_signal: SIGCHLD.
        "17".to_string(),
        task.cpu.to_string(),
        match task.policy {
            Policy::Fifo | Policy::RoundRobin => task.priority.to_string(),
            _ => "0".to_string(),
        },
        task.policy.number().to_string(),
        // delayacct_blkio_ticks, guest_time, cguest_time.
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        // start_data, end_data, start_brk, arg_start, arg_end,
        // env_start, env_end, exit_code.
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
    ];
    format!("{}\n", fields.join(" "))
}

/// `/proc/<n>/statm`: sizes in pages, as proc(5) lists them: the whole
/// size, the resident part, the part that files back, program text,
/// libraries (always 0), data and stack, and dirty pages (always 0).
pub fn statm(task: &Task<'_>) -> String {
    let process = task.process;
    let resident = process.resident_kib / PAGE_KIB;
    let anonymous = process.anonymous_kib / PAGE_KIB;
    let size = process.virtual_kib / PAGE_KIB;
    let shared = resident.saturating_sub(anonymous);
    format!("{size} {resident} {shared} 0 0 {anonymous} 0\n")
}

/// One row of `/proc/<n>/limits`.
fn limit(page: &mut Page, name: &str, soft: &str, hard: &str, units: &str) {
    page.line(format_args!("{name:<25} {soft:<20} {hard:<20} {units:<10}"));
}

/// `/proc/<n>/limits`. The kernel keeps each process's limits and
/// `/processes` does not show them, so these are the limits a process
/// starts with (radix-posix(7)).
pub fn limits() -> String {
    let mut page = Page::new();
    limit(&mut page, "Limit", "Soft Limit", "Hard Limit", "Units");
    limit(&mut page, "Max cpu time", "unlimited", "unlimited", "seconds");
    limit(&mut page, "Max file size", "unlimited", "unlimited", "bytes");
    limit(&mut page, "Max data size", "unlimited", "unlimited", "bytes");
    limit(&mut page, "Max stack size", "8388608", "unlimited", "bytes");
    limit(&mut page, "Max core file size", "0", "unlimited", "bytes");
    limit(&mut page, "Max resident set", "unlimited", "unlimited", "bytes");
    limit(&mut page, "Max processes", "unlimited", "unlimited", "processes");
    limit(&mut page, "Max open files", "1024", "4096", "files");
    limit(&mut page, "Max locked memory", "8388608", "8388608", "bytes");
    limit(&mut page, "Max address space", "unlimited", "unlimited", "bytes");
    limit(&mut page, "Max file locks", "unlimited", "unlimited", "locks");
    let queued = SIGNALS_PENDING_MAX.to_string();
    limit(&mut page, "Max pending signals", &queued, &queued, "signals");
    limit(&mut page, "Max msgqueue size", "819200", "819200", "bytes");
    limit(&mut page, "Max nice priority", "0", "0", "");
    limit(&mut page, "Max realtime priority", "0", "0", "");
    limit(&mut page, "Max realtime timeout", "unlimited", "unlimited", "us");
    page.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process() -> Process {
        Process::parse(
            b"number: 4242\nname: a-very-long-program-name\nstate: running\nthreads: 3\n\
            anonymous_memory: 100 pages\nuser: 1000\nuser_group: 100\nparent: 4000\ngroup: 4242\n\
            session: 4000\nterminal: pts/2\nterminal_group: 4242\nrun: sleeping\n\
            started_ns: 5000000000\nuser_ns: 123456789\nsystem_ns: 23456789\npolicy: other\n\
            priority: 0\nnice: 0\nvirtual_memory: 20000 KiB\nresident_memory: 1000 KiB\n\
            minor_faults: 12\nmajor_faults: 3\nvoluntary_switches: 40\ninvoluntary_switches: 5\n",
        )
    }

    fn threads() -> Vec<Thread> {
        Thread::parse_all(
            b"number=4242 run=sleeping cpu=1 user_ns=100000000 system_ns=20000000 \
            voluntary_switches=30 involuntary_switches=2 policy=other priority=0 nice=0 name=main\n\
            number=4250 run=running cpu=3 user_ns=30000000 system_ns=3000000 \
            voluntary_switches=10 involuntary_switches=3 policy=other priority=0 nice=0 \
            name=worker\n",
        )
    }

    #[test]
    fn a_name_is_cut_to_fifteen_bytes() {
        assert_eq!(comm_of("short"), "short");
        assert_eq!(comm_of("a-very-long-program-name"), "a-very-long-pro");
        // A cut inside a character falls back to the one before it.
        assert_eq!(comm_of("aaaaaaaaaaaaaaé"), "aaaaaaaaaaaaaa");
    }

    #[test]
    fn cpu_masks_are_grouped_like_the_kernels() {
        assert_eq!(cpu_mask(1), "1");
        assert_eq!(cpu_mask(4), "f");
        assert_eq!(cpu_mask(6), "3f");
        assert_eq!(cpu_mask(8), "ff");
        assert_eq!(cpu_mask(32), "ffffffff");
        assert_eq!(cpu_mask(36), "f,ffffffff");
        assert_eq!(cpu_mask(64), "ffffffff,ffffffff");
        assert_eq!(cpu_list(1), "0");
        assert_eq!(cpu_list(6), "0-5");
    }

    #[test]
    fn the_status_of_a_process_is_linuxs() {
        let process = process();
        let task = Task::of_process(&process, &threads());
        let golden = "Name:\ta-very-long-pro
Umask:\t0022
State:\tS (sleeping)
Tgid:\t4242
Ngid:\t0
Pid:\t4242
PPid:\t4000
TracerPid:\t0
Uid:\t1000\t1000\t1000\t1000
Gid:\t100\t100\t100\t100
FDSize:\t256
Groups:\t
NStgid:\t4242
NSpid:\t4242
NSpgid:\t4242
NSsid:\t4000
VmPeak:\t   20000 kB
VmSize:\t   20000 kB
VmLck:\t       0 kB
VmPin:\t       0 kB
VmHWM:\t    1000 kB
VmRSS:\t    1000 kB
RssAnon:\t     400 kB
RssFile:\t     600 kB
RssShmem:\t       0 kB
VmData:\t     400 kB
VmStk:\t       0 kB
VmExe:\t       0 kB
VmLib:\t       0 kB
VmPTE:\t       0 kB
VmSwap:\t       0 kB
HugetlbPages:\t       0 kB
CoreDumping:\t0
THP_enabled:\t1
Threads:\t3
SigQ:\t0/1024
SigPnd:\t0000000000000000
ShdPnd:\t0000000000000000
SigBlk:\t0000000000000000
SigIgn:\t0000000000000000
SigCgt:\t0000000000000000
CapInh:\t0000000000000000
CapPrm:\t0000000000000000
CapEff:\t0000000000000000
CapBnd:\t000001ffffffffff
CapAmb:\t0000000000000000
NoNewPrivs:\t0
Seccomp:\t0
Seccomp_filters:\t0
Cpus_allowed:\tf
Cpus_allowed_list:\t0-3
Mems_allowed:\t00000001
Mems_allowed_list:\t0
voluntary_ctxt_switches:\t40
nonvoluntary_ctxt_switches:\t5
";
        assert_eq!(status(&task, 4), golden);
    }

    #[test]
    fn root_has_every_capability() {
        let mut process = process();
        process.user = 0;
        let text = status(&Task::of_process(&process, &[]), 1);
        assert!(text.contains("CapEff:\t000001ffffffffff\n"));
        assert!(text.contains("CapPrm:\t000001ffffffffff\n"));
        assert!(text.contains("CapInh:\t0000000000000000\n"));
    }

    #[test]
    fn a_thread_has_its_own_number_name_and_switches() {
        let process = process();
        let threads = threads();
        let task = Task::of_thread(&process, &threads[1]);
        let text = status(&task, 4);
        assert!(text.starts_with("Name:\tworker\n"));
        assert!(text.contains("State:\tR (running)\n"));
        assert!(text.contains("Tgid:\t4242\n"));
        assert!(text.contains("Pid:\t4250\n"));
        assert!(text.contains("NSpid:\t4250\n"));
        assert!(text.contains("voluntary_ctxt_switches:\t10\n"));
        assert!(text.contains("nonvoluntary_ctxt_switches:\t3\n"));
    }

    #[test]
    fn the_stat_of_a_process_has_fifty_two_fields() {
        let process = process();
        let task = Task::of_process(&process, &threads());
        let text = stat(&task);
        assert_eq!(
            text,
            "4242 (a-very-long-pro) S 4000 4242 4000 34818 4242 4194560 12 0 3 0 12 2 0 0 20 0 3 0 \
             500 20480000 250 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 1 0 0 0 0 0 0 0 0 0 \
             0 0 0 0\n"
        );
        assert_eq!(text.trim_end().split(' ').count(), 52);
    }

    #[test]
    fn the_stat_of_a_thread_counts_its_own_time() {
        let process = process();
        let threads = threads();
        let text = stat(&Task::of_thread(&process, &threads[1]));
        let fields: Vec<&str> = text.trim_end().split(' ').collect();
        assert_eq!(&fields[..3], ["4250", "(worker)", "R"]);
        // utime and stime in ticks of a hundredth of a second, and the CPU.
        assert_eq!((fields[13], fields[14], fields[38]), ("3", "0", "3"));
    }

    #[test]
    fn a_process_with_no_terminal_has_none() {
        let process = Process::parse(b"number: 7\nname: x\nthreads: 1\nrun: running\n");
        let text = stat(&Task::of_process(&process, &[]));
        let fields: Vec<&str> = text.split(' ').collect();
        assert_eq!((fields[6], fields[7], fields[2]), ("0", "-1", "R"));
    }

    #[test]
    fn real_time_threads_show_their_priority() {
        let process =
            Process::parse(b"number: 7\nname: x\nthreads: 1\npolicy: fifo\npriority: 10\n");
        let text = stat(&Task::of_process(&process, &[]));
        let fields: Vec<&str> = text.split(' ').collect();
        assert_eq!((fields[17], fields[39], fields[40]), ("-11", "10", "1"));
    }

    #[test]
    fn statm_is_in_pages() {
        let process = process();
        assert_eq!(statm(&Task::of_process(&process, &[])), "5000 250 150 0 0 100 0\n");
    }

    #[test]
    fn limits_are_linuxs_table() {
        let golden = concat!(
            "Limit                     Soft Limit           Hard Limit           Units     \n",
            "Max cpu time              unlimited            unlimited            seconds   \n",
            "Max file size             unlimited            unlimited            bytes     \n",
            "Max data size             unlimited            unlimited            bytes     \n",
            "Max stack size            8388608              unlimited            bytes     \n",
            "Max core file size        0                    unlimited            bytes     \n",
            "Max resident set          unlimited            unlimited            bytes     \n",
            "Max processes             unlimited            unlimited            processes \n",
            "Max open files            1024                 4096                 files     \n",
            "Max locked memory         8388608              8388608              bytes     \n",
            "Max address space         unlimited            unlimited            bytes     \n",
            "Max file locks            unlimited            unlimited            locks     \n",
            "Max pending signals       1024                 1024                 signals   \n",
            "Max msgqueue size         819200               819200               bytes     \n",
            "Max nice priority         0                    0                              \n",
            "Max realtime priority     0                    0                              \n",
            "Max realtime timeout      unlimited            unlimited            us        \n",
        );
        assert_eq!(limits(), golden);
    }
}
