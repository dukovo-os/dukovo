// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the kernel says of a process, as values: its `status`, the
//! lines of its `threads` and the lines of its `files` (radix-proc(5)),
//! read once so that the text generators of [`super::task`] never meet
//! the kernel's format.
//!
//! A key the kernel leaves out (a process that is not a POSIX process
//! has no `parent`) reads as 0 or as nothing, as `ps(1)` shows it.

use super::text::{number, pairs, unescape, value, word};

/// What a process or thread is doing, as Linux's `State:` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Running or ready to run (`R`).
    Running,
    /// Waiting for something (`S`).
    Sleeping,
    /// Waiting for a disk (`D`).
    Disk,
    /// Stopped by a signal (`T`).
    Stopped,
    /// Ended, and its parent has not waited for it (`Z`).
    Zombie,
    /// A thread that has ended (`X`).
    Dead,
}

impl State {
    /// The state the kernel's `run` names: a thread that is new is
    /// ready to run.
    fn of(run: &str) -> State {
        match run {
            "running" | "new" => State::Running,
            "disk" => State::Disk,
            "stopped" => State::Stopped,
            "zombie" => State::Zombie,
            "dead" => State::Dead,
            _ => State::Sleeping,
        }
    }

    /// The letter of `/proc/<n>/stat`, and the first of `status`'s.
    pub fn letter(self) -> char {
        match self {
            State::Running => 'R',
            State::Sleeping => 'S',
            State::Disk => 'D',
            State::Stopped => 'T',
            State::Zombie => 'Z',
            State::Dead => 'X',
        }
    }

    /// The words after the letter in `status`.
    pub fn words(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Sleeping => "sleeping",
            State::Disk => "disk sleep",
            State::Stopped => "stopped",
            State::Zombie => "zombie",
            State::Dead => "dead",
        }
    }
}

/// A scheduling policy, with Linux's number for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// SCHED_OTHER.
    Other,
    /// SCHED_FIFO.
    Fifo,
    /// SCHED_RR.
    RoundRobin,
    /// SCHED_BATCH.
    Batch,
    /// SCHED_IDLE.
    Idle,
}

impl Policy {
    fn of(name: &str) -> Policy {
        match name {
            "fifo" => Policy::Fifo,
            "round_robin" => Policy::RoundRobin,
            "batch" => Policy::Batch,
            "idle" => Policy::Idle,
            _ => Policy::Other,
        }
    }

    /// Linux's number for the policy (`sched.h`).
    pub fn number(self) -> u32 {
        match self {
            Policy::Other => 0,
            Policy::Fifo => 1,
            Policy::RoundRobin => 2,
            Policy::Batch => 3,
            Policy::Idle => 5,
        }
    }

    /// `/proc/<n>/stat`'s priority: 20 and the nice level for the
    /// policies of ordinary programs, and -1 less the real-time priority
    /// for the real-time ones.
    pub fn stat_priority(self, priority: u64, nice: i64) -> i64 {
        match self {
            Policy::Fifo | Policy::RoundRobin => {
                i64::try_from(priority).map_or(i64::MIN, |priority| -1 - priority)
            }
            _ => 20 + nice,
        }
    }
}

/// A process: the keys of its `status` that Linux's files show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    /// Its number.
    pub number: u64,
    /// Its name.
    pub name: String,
    /// What it is doing.
    pub state: State,
    /// How many threads it has.
    pub threads: u64,
    /// The memory no file backs, in KiB.
    pub anonymous_kib: u64,
    /// The user it acts for.
    pub user: u32,
    /// The group it acts for.
    pub group: u32,
    /// Its parent's number.
    pub parent: u64,
    /// Its process group's number.
    pub process_group: u64,
    /// Its session's number.
    pub session: u64,
    /// The number *n* of its terminal, `pts/`*n*, if it has one.
    pub terminal: Option<u32>,
    /// The number of its terminal's foreground process group.
    pub terminal_group: Option<u64>,
    /// When it started, in nanoseconds since the kernel did.
    pub started_ns: u64,
    /// The CPU time of its threads in user mode.
    pub user_ns: u64,
    /// The CPU time of its threads in the kernel.
    pub system_ns: u64,
    /// Its first thread's scheduling.
    pub policy: Policy,
    /// The real-time priority.
    pub priority: u64,
    /// The nice level.
    pub nice: i64,
    /// The memory mapped, in KiB.
    pub virtual_kib: u64,
    /// The part of the memory mapped that is in memory, in KiB.
    pub resident_kib: u64,
    /// Page faults that read no disk.
    pub minor_faults: u64,
    /// Page faults that read one.
    pub major_faults: u64,
    /// Times its threads gave up the CPU to wait.
    pub voluntary_switches: u64,
    /// Times another thread was given the CPU.
    pub involuntary_switches: u64,
    /// Its working directory, if it said.
    pub directory: Option<Vec<u8>>,
    /// The path of its program, if the kernel says.
    pub program: Option<Vec<u8>>,
}

impl Process {
    /// The process `status` describes.
    pub fn parse(status: &[u8]) -> Process {
        let count = |key: &str| number(status, key).unwrap_or(0);
        let run = word(status, "run").unwrap_or("sleeping");
        let ended = word(status, "state") == Some("exited");
        let terminal = word(status, "terminal").and_then(|name| name.strip_prefix("pts/"));
        Process {
            number: count("number"),
            name: String::from_utf8_lossy(&unescape(value(status, "name").unwrap_or_default()))
                .into_owned(),
            state: if ended { State::Zombie } else { State::of(run) },
            threads: count("threads"),
            // The kernel counts pages of 4 KiB.
            anonymous_kib: count("anonymous_memory") * 4,
            user: u32::try_from(count("user")).unwrap_or(0),
            group: u32::try_from(count("user_group")).unwrap_or(0),
            parent: count("parent"),
            process_group: count("group"),
            session: count("session"),
            terminal: terminal.and_then(|number| number.parse().ok()),
            terminal_group: number(status, "terminal_group"),
            started_ns: count("started_ns"),
            user_ns: count("user_ns"),
            system_ns: count("system_ns"),
            policy: Policy::of(word(status, "policy").unwrap_or("other")),
            priority: count("priority"),
            nice: word(status, "nice").and_then(|nice| nice.parse().ok()).unwrap_or(0),
            virtual_kib: count("virtual_memory"),
            resident_kib: count("resident_memory"),
            minor_faults: count("minor_faults"),
            major_faults: count("major_faults"),
            voluntary_switches: count("voluntary_switches"),
            involuntary_switches: count("involuntary_switches"),
            directory: value(status, "directory").map(unescape),
            program: value(status, "program").map(unescape),
        }
    }
}

/// A thread: a line of `/processes/<n>/threads`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// Its number.
    pub number: u64,
    /// What it is doing.
    pub state: State,
    /// The CPU it last ran on.
    pub cpu: u64,
    /// Its CPU time in user mode.
    pub user_ns: u64,
    /// Its CPU time in the kernel.
    pub system_ns: u64,
    /// Times it gave up the CPU to wait.
    pub voluntary_switches: u64,
    /// Times another thread was given the CPU.
    pub involuntary_switches: u64,
    /// Its scheduling policy.
    pub policy: Policy,
    /// Its real-time priority.
    pub priority: u64,
    /// Its nice level.
    pub nice: i64,
    /// Its name.
    pub name: String,
}

impl Thread {
    /// The threads `text` lists, in its order.
    pub fn parse_all(text: &[u8]) -> Vec<Thread> {
        text.split(|&byte| byte == b'\n').filter_map(Thread::parse).collect()
    }

    fn parse(line: &[u8]) -> Option<Thread> {
        let (fields, name) = pairs(line, "name");
        let field = |key: &str| fields.iter().find(|(name, _)| *name == key).map(|(_, v)| *v);
        let count = |key: &str| field(key).and_then(|v| v.parse().ok()).unwrap_or(0);
        Some(Thread {
            number: field("number")?.parse().ok()?,
            state: State::of(field("run")?),
            cpu: count("cpu"),
            user_ns: count("user_ns"),
            system_ns: count("system_ns"),
            voluntary_switches: count("voluntary_switches"),
            involuntary_switches: count("involuntary_switches"),
            policy: Policy::of(field("policy").unwrap_or("other")),
            priority: count("priority"),
            nice: field("nice").and_then(|nice| nice.parse().ok()).unwrap_or(0),
            name: String::from_utf8_lossy(&name.unwrap_or_default()).into_owned(),
        })
    }
}

/// A file descriptor: a line of `/processes/<n>/files` that names one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    /// The descriptor's number.
    pub fd: u32,
    /// What it holds: `file`, `directory`, `char`, `pipe`, `socket` and
    /// the others of radix-proc(5).
    pub kind: String,
    /// The node number of its file.
    pub id: u64,
    /// The path it was opened by, or a pipe's or socket's name.
    pub path: Option<Vec<u8>>,
}

impl Descriptor {
    /// The descriptors `text` lists, in its order: the lines for slots
    /// that are not descriptors are left out.
    pub fn parse_all(text: &[u8]) -> Vec<Descriptor> {
        text.split(|&byte| byte == b'\n').filter_map(Descriptor::parse).collect()
    }

    fn parse(line: &[u8]) -> Option<Descriptor> {
        let (fields, path) = pairs(line, "path");
        let field = |key: &str| fields.iter().find(|(name, _)| *name == key).map(|(_, v)| *v);
        Some(Descriptor {
            fd: field("fd")?.parse().ok()?,
            kind: field("kind")?.to_string(),
            id: field("id").and_then(|id| id.parse().ok()).unwrap_or(0),
            path,
        })
    }

    /// What `/proc/<n>/fd/<fd>` links to: the path, `pipe:[n]` or
    /// `socket:[n]` as Linux names them, and `anon_inode:[kind]` for a
    /// file that has no path.
    pub fn target(&self) -> Vec<u8> {
        match (self.kind.as_str(), &self.path) {
            ("pipe", _) => format!("pipe:[{}]", self.id).into_bytes(),
            ("socket", _) => format!("socket:[{}]", self.id).into_bytes(),
            (_, Some(path)) => path.clone(),
            (kind, None) => format!("anon_inode:[{kind}]").into_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "number: 9\nname: nu kesh\nstate: running\nthreads: 2\n\
        anonymous_memory: 212 pages\ndomain: system\nkernel_memory: 40960 bytes\nuser: 1000\n\
        user_group: 100\nparent: 1\ngroup: 9\nsession: 9\nterminal: pts/3\nterminal_group: 9\n\
        run: sleeping\nstarted_ns: 1832004113\ncpu_ns: 41202770\nuser_ns: 30419008\n\
        system_ns: 10783762\npolicy: round_robin\npriority: 5\nnice: -3\n\
        virtual_memory: 9416 KiB\nresident_memory: 3104 KiB\nminor_faults: 811\n\
        major_faults: 37\nvoluntary_switches: 64\ninvoluntary_switches: 3\ntraced: no\n\
        directory: /home/a\\012b\n";

    #[test]
    fn a_status_is_read_by_key() {
        let process = Process::parse(STATUS.as_bytes());
        assert_eq!(process.number, 9);
        assert_eq!(process.name, "nu kesh");
        assert_eq!(process.state, State::Sleeping);
        assert_eq!((process.user, process.group), (1000, 100));
        assert_eq!(process.anonymous_kib, 848);
        assert_eq!(process.terminal, Some(3));
        assert_eq!(process.terminal_group, Some(9));
        assert_eq!((process.policy, process.priority, process.nice), (Policy::RoundRobin, 5, -3));
        assert_eq!(process.directory.as_deref(), Some(&b"/home/a\nb"[..]));
        assert_eq!(process.program, None);
    }

    #[test]
    fn a_status_without_posix_keys_reads_as_zeros() {
        let process = Process::parse(b"number: 2\nname: spark\nstate: running\nthreads: 1\n");
        assert_eq!((process.parent, process.process_group, process.session), (0, 0, 0));
        assert_eq!(process.terminal, None);
        assert_eq!(process.state, State::Sleeping);
    }

    #[test]
    fn an_exited_process_is_a_zombie() {
        let process =
            Process::parse(b"number: 5\nname: x\nstate: exited\ncode: 0x0\nrun: zombie\n");
        assert_eq!(process.state, State::Zombie);
        let process = Process::parse(b"number: 5\nname: x\nstate: exited\nrun: sleeping\n");
        assert_eq!(process.state, State::Zombie);
    }

    #[test]
    fn threads_are_read_a_line_each() {
        let text = "number=9 run=running cpu=2 user_ns=5 system_ns=6 voluntary_switches=7 \
            involuntary_switches=8 policy=other priority=0 nice=0 name=main\n\
            number=10 run=sleeping cpu=1 user_ns=1 system_ns=2 voluntary_switches=3 \
            involuntary_switches=4 policy=batch priority=0 nice=5 waits=futex name=a b\n";
        let threads = Thread::parse_all(text.as_bytes());
        assert_eq!(threads.len(), 2);
        assert_eq!((threads[0].number, threads[0].state, threads[0].cpu), (9, State::Running, 2));
        assert_eq!(threads[1].name, "a b");
        assert_eq!((threads[1].policy, threads[1].nice), (Policy::Batch, 5));
    }

    #[test]
    fn descriptors_become_the_links_linux_shows() {
        let text = "slot=1 kind=thread rights=read+control\n\
            fd=0 kind=char access=rw id=30 size=0 fs=dev mount=7 rights=read offset=0 \
            path=/devices/pts/0\n\
            fd=3 kind=file access=r id=1201 size=5 fs=tmpfs mount=3 rights=read \
            path=/home/my file\n\
            fd=4 kind=pipe access=r id=77 size=0 fs=pipefs mount=2 rights=read path=pipe\n\
            fd=5 kind=socket access=rw id=78 rights=read\n\
            fd=6 kind=eventfd rights=read\n";
        let descriptors = Descriptor::parse_all(text.as_bytes());
        let targets: Vec<(u32, String)> =
            descriptors.iter().map(|d| (d.fd, String::from_utf8(d.target()).unwrap())).collect();
        assert_eq!(
            targets,
            [
                (0, "/devices/pts/0".to_string()),
                (3, "/home/my file".to_string()),
                (4, "pipe:[77]".to_string()),
                (5, "socket:[78]".to_string()),
                (6, "anon_inode:[eventfd]".to_string()),
            ]
        );
    }

    #[test]
    fn real_time_priority_is_negative_in_stat() {
        assert_eq!(Policy::Other.stat_priority(0, 0), 20);
        assert_eq!(Policy::Batch.stat_priority(0, -5), 15);
        assert_eq!(Policy::Fifo.stat_priority(10, 0), -11);
    }
}
