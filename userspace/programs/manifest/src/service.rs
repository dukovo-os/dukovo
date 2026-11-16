// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Service files (service.toml(5), ADR 0216): what Spark starts and keeps
//! running.
//!
//! Each `NAME.toml` in `/system/settings/spark/services` describes the
//! service NAME, in TOML. The file says which version of the format it
//! was written for in `service_version`; a later Dukovo reads every
//! earlier version, and a file for a later one is refused with a message
//! to update Dukovo. An unknown setting is an error, so a misspelt one is
//! never silently ignored.
//!
//! ```toml
//! service_version = 1
//! description = "System log"
//! program = "/system/commands/logd"
//! order = 5
//! restart = "always"
//!
//! [limits]
//! memory = "64M"
//!
//! [permissions]
//! network.listen = [514]
//! ```

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use radix_abi::resource_domain::{
    IO_WEIGHT_DEFAULT, IO_WEIGHT_MAX, NAME_MAX, WEIGHT_DEFAULT, WEIGHT_MAX,
};
use radix_abi::startup::NETWORK_SLOTS;
use serde::Deserialize;
use serde::de::{self, DeserializeOwned, Deserializer, MapAccess, Visitor};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::Error;

/// The version of the format this reads, the newest there is.
pub const VERSION: u32 = 2;

/// Every setting of a service file, a table's after the table: what
/// service.toml(5) must document.
pub const KEYS: &[&str] = &[
    "service_version",
    "description",
    "program",
    "arguments",
    "environment",
    "order",
    "enabled",
    "restart",
    "limits",
    "cpu_weight",
    "memory",
    "oom",
    "io_weight",
    "io_read",
    "io_write",
    "io_read_ops",
    "io_write_ops",
    "threads",
    "open_files",
    "secrets",
    "certificates",
    "permissions",
    "network.outbound",
    "network.listen",
    "files.read",
    "files.read-write",
    "files.mount",
    "files.shared",
    "runs-with-your-access",
    "admin.network",
    "admin.hardware",
    "admin.time",
    "admin.firewall",
    "admin.tune",
    "names",
    "domain",
    "devices.pci",
    "backup",
    "backup.before",
    "backup.after",
    "backup.time_limit",
    "requires",
    "wants",
    "after",
    "give_up_after",
    "ready",
    "ready.when",
    "ready.port",
    "ready.path",
    "ready.timeout",
    "health",
    "health.command",
    "health.every",
    "health.timeout",
    "health.failures",
    "timer",
    "timer.at",
    "timer.every",
    "timer.after_boot",
    "timer.spread",
    "timer.catch_up",
    "timer.time_zone",
    "logs",
    "logs.files",
    "logs.turn_over",
    "logs.keep",
    "logs.compress",
    "logs.reopen",
    "sockets",
    "sockets.port",
    "sockets.address",
    "sockets.udp",
    "sockets.path",
    "sockets.name",
    "sockets.backlog",
    "sockets.non-blocking",
    "sockets.to",
    "sockets.group",
    "start",
    "groups",
    "stop",
    "stop.signal",
    "stop.timeout",
];

/// The keys of service_version 2 (ADR 0470), which a file of version 1
/// may not use.
const VERSION_2_KEYS: &[&str] =
    &["requires", "wants", "after", "give_up_after", "ready", "health", "stop"];

/// The order of a service whose file gives none.
pub const ORDER_DEFAULT: u32 = 50;

/// The threads a service and what it starts may have together when its
/// file gives no `threads`: far more than a service that works needs,
/// and few enough that a fork bomb in one cannot starve the machine.
pub const THREADS_DEFAULT: u32 = 4096;

/// The most `threads` a file may say: a million, as many as a program
/// may have files open ([`OPEN_FILES_MAX`]). A thread costs its stacks,
/// so no machine has the memory for more; a limit above what memory
/// allows is no limit at all, which `threads` has no word for.
pub const THREADS_MAX: u32 = 1 << 20;

/// The highest `open_files`: Linux's `fs.nr_open`. A process holds no
/// more descriptors than its capability space has slots for, whatever
/// its limit (radix-posix(7)), so a program written for Linux may ask
/// for as many as it does there.
pub const OPEN_FILES_MAX: u32 = 1 << 20;

/// When a service that ended is started again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Restart {
    /// Whatever its end.
    #[default]
    Always,
    /// When it failed: an exit code other than 0, or a signal.
    OnFailure,
    /// Never.
    Never,
}

impl Restart {
    pub(crate) const WORDS: &[&str] = &["always", "on-failure", "never"];

    /// The word a file says it with.
    pub fn name(self) -> &'static str {
        Self::WORDS[self as usize]
    }
}

/// Which programs the out-of-memory policy stops first (ADR 0103).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Oom {
    /// A service's: stopped after apps'.
    #[default]
    Service,
    /// An app's.
    App,
}

impl Oom {
    pub(crate) const WORDS: &[&str] = &["service", "app"];

    /// The word a file says it with.
    pub fn name(self) -> &'static str {
        Self::WORDS[self as usize]
    }
}

/// A service file, read.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "File")]
pub struct Service {
    /// What it is, in a few words, for `service status`.
    pub description: String,
    /// The program's absolute path.
    pub program: String,
    /// The arguments after the program's name.
    pub arguments: Vec<String>,
    /// More of the environment, as names and values, by name.
    pub environment: Vec<(String, String)>,
    /// Lower starts first; shutdown stops the highest first.
    pub order: u32,
    /// Whether Spark starts it at boot.
    pub enabled: bool,
    /// When it is started again after it ends.
    pub restart: Restart,
    /// What its resource domain may use.
    pub limits: Limits,
    /// The names of the secrets Spark hands it as read-only files.
    pub secrets: Vec<String>,
    /// The names of the certificates certd keeps that Spark copies for
    /// it, read-only, and has it read again when one changes (ADR 0512).
    pub certificates: Vec<String>,
    /// What it may do beyond running.
    pub permissions: Permissions,
    /// The account it runs as, a bundle's service's own (ADR 0385); when
    /// none, it acts for root, without root's bypass.
    pub account: Option<String>,
    /// What it runs around backups' snapshots (ADR 0463).
    pub backup: Backup,
    /// The services it requires: started first and ready, or it does not
    /// start (ADR 0470).
    pub requires: Vec<String>,
    /// The services it wants: started first; it starts whether or not
    /// they do.
    pub wants: Vec<String>,
    /// The services it starts after, when they are starting too.
    pub after: Vec<String>,
    /// The failed starts in a row after which Spark stops trying; 0 for
    /// never.
    pub give_up_after: u32,
    /// When it is ready, for what waits for it.
    pub ready: Ready,
    /// Its health check, if it has one.
    pub health: Option<Health>,
    /// The sockets Spark makes and holds for it, passed from descriptor
    /// 3 as sd_listen_fds(3) has them (ADR 0477); at most
    /// [`SOCKETS_MAX`].
    pub sockets: Vec<Socket>,
    /// When Spark starts it, once enabled.
    pub start: Start,
    /// How Spark stops it: the signal, and how long it waits.
    pub stop: Stop,
    /// More groups its processes are in, by name: the groups of sockets
    /// other bundles offer it (ADR 0477).
    pub groups: Vec<String>,
    /// When timerd starts it, if at set times (ADR 0478).
    pub timer: Option<Timer>,
    /// Its own log files, which logturn(8) turns over (ADR 0478).
    pub logs: Option<Logs>,
}

/// The most sockets a service file names.
pub const SOCKETS_MAX: usize = 16;

/// A socket's backlog when its file names none: Linux's `somaxconn`.
pub const BACKLOG_DEFAULT: u32 = 4096;

/// The longest `stop.timeout`, in seconds.
pub const STOP_TIMEOUT_MAX: u32 = 3600;

/// `stop.timeout` when the file says none, in seconds: how long Spark
/// waits for a service to end after its stop signal before it ends the
/// service's domain.
pub const STOP_TIMEOUT_DEFAULT: u32 = 5;

/// How Spark stops a service: a service file's `[stop]` (ADR 0477).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    /// The signal that asks it to stop: one of SIGTERM, SIGINT, SIGQUIT,
    /// SIGUSR1, SIGUSR2 and SIGWINCH.
    pub signal: Signal,
    /// Seconds from the signal to ending its domain, 1 to
    /// [`STOP_TIMEOUT_MAX`].
    pub timeout: u32,
}

impl Default for Stop {
    fn default() -> Stop {
        Stop { signal: Signal::Term, timeout: STOP_TIMEOUT_DEFAULT }
    }
}

/// A socket Spark makes and holds for a service (ADR 0477).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socket {
    /// What it is.
    pub kind: SocketKind,
    /// Its name in `LISTEN_FDNAMES`: the file's `name`, or the port's
    /// number or the path.
    pub name: String,
    /// How many connections may wait to be accepted, 1 to 65535.
    pub backlog: u32,
    /// Whether Spark passes it with `O_NONBLOCK` set, as systemd's
    /// `NonBlocking=` does, for a program that takes an inherited socket
    /// as it is and waits for it with poll(2), nginx among them: a
    /// blocking socket leaves each worker that loses the race for a
    /// connection waiting in accept(2).
    pub non_blocking: bool,
    /// Who connects to it, for a Unix socket.
    pub access: SocketAccess,
}

/// Who connects to a Unix socket Spark makes (ADR 0477). A socket offered
/// to others is root's, so that the service cannot widen who reaches it,
/// and its live folder anyone's to look in; the socket's mode keeps the
/// rest out. Views decide which services reach it at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SocketAccess {
    /// The service's own: its account's, mode 0600, or for a service of
    /// the system's, which has none, root's, mode 0666, as systemd's
    /// `SocketMode=` default is.
    #[default]
    Own,
    /// `to = "anyone"`: mode 0666.
    Anyone,
    /// `to = "system"`: root's programs alone, mode 0600.
    System,
    /// `group = NAME`: the group's members, mode 0660, as bundled gives a
    /// socket a bundle offers its publisher's bundles or the programs an
    /// administrator approves.
    Group(String),
}

impl SocketAccess {
    /// Whether others than the service connect to it.
    pub fn offered(&self) -> bool {
        *self != SocketAccess::Own
    }
}

/// What a socket is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketKind {
    /// TCP, listening.
    Stream {
        /// Its port.
        port: u16,
        /// The address it is on.
        address: SocketAddress,
    },
    /// UDP.
    Datagram {
        /// Its port.
        port: u16,
        /// The address it is on.
        address: SocketAddress,
    },
    /// A Unix stream socket of this plain name in the service's live
    /// folder: `/system/live/services/ACCOUNT` for an account's service,
    /// `/system/live/NAME` for the system's.
    Unix {
        /// The socket's name.
        path: String,
    },
}

/// The address a TCP or UDP socket is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SocketAddress {
    /// Every address, IPv6 and IPv4 alike, as IPv4-mapped IPv6: the file
    /// names no `address`.
    #[default]
    Any,
    /// Every IPv4 address: `"0.0.0.0"`.
    AnyV4,
    /// Every IPv6 address and no IPv4 one (`IPV6_V6ONLY`): `"::"`.
    AnyV6,
    /// One IPv4 address.
    V4([u8; 4]),
    /// One IPv6 address.
    V6([u8; 16]),
}

/// When Spark starts an enabled service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Start {
    /// At boot.
    #[default]
    AtBoot,
    /// At the first connection to one of its sockets, or the first
    /// datagram (ADR 0477).
    OnConnection,
}

impl SocketAddress {
    /// The address as a file writes it; none for [`SocketAddress::Any`],
    /// which a file writes by leaving `address` out.
    pub fn text(self) -> Option<String> {
        match self {
            SocketAddress::Any => None,
            SocketAddress::AnyV4 => Some("0.0.0.0".to_owned()),
            SocketAddress::AnyV6 => Some("::".to_owned()),
            SocketAddress::V4(octets) => Some(core::net::Ipv4Addr::from(octets).to_string()),
            SocketAddress::V6(octets) => Some(core::net::Ipv6Addr::from(octets).to_string()),
        }
    }
}

impl Start {
    pub(crate) const WORDS: &[&str] = &["at-boot", "on-connection"];

    /// The word a file says it with.
    pub fn name(self) -> &'static str {
        Self::WORDS[self as usize]
    }
}

/// A signal Spark sends a service: to stop it, or to have it open its
/// log files again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Signal {
    /// SIGTERM, the stop signal unless a file says another.
    #[default]
    Term,
    /// SIGINT.
    Int,
    /// SIGQUIT: nginx's graceful stop.
    Quit,
    /// SIGHUP.
    Hup,
    /// SIGUSR1: nginx's signal to open its log files again.
    Usr1,
    /// SIGUSR2.
    Usr2,
    /// SIGWINCH.
    Winch,
}

impl Signal {
    pub(crate) const WORDS: &[&str] =
        &["SIGTERM", "SIGINT", "SIGQUIT", "SIGHUP", "SIGUSR1", "SIGUSR2", "SIGWINCH"];

    /// Its name, as a file says it.
    pub fn name(self) -> &'static str {
        Self::WORDS[self as usize]
    }

    /// Its number, as Radix and Linux have it on x86-64.
    pub fn number(self) -> u8 {
        match self {
            Signal::Term => 15,
            Signal::Int => 2,
            Signal::Quit => 3,
            Signal::Hup => 1,
            Signal::Usr1 => 10,
            Signal::Usr2 => 12,
            Signal::Winch => 28,
        }
    }
}

/// A service file's `[timer]` (ADR 0478): when timerd starts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timer {
    /// Times of day, as crontab lines' five fields or the words, each
    /// read ([`crate::schedule`]); empty for none.
    pub at: Vec<crate::schedule::Calendar>,
    /// Seconds from its last start to the next; `None` for none.
    pub every: Option<u64>,
    /// Seconds from the machine's start to its first start by `every`;
    /// `None` for `every` itself.
    pub after_boot: Option<u64>,
    /// The most seconds a time is put off by, the same each day for this
    /// machine and timer, so that a fleet does not start at once.
    pub spread: u64,
    /// Whether a time missed while the machine was off runs once, soon
    /// after it starts.
    pub catch_up: bool,
    /// The time zone `at` is read in; `None` for the system's.
    pub time_zone: Option<String>,
    /// The schedule as the file writes it, for a person to read: each
    /// `at` as written, then `every 6h` and `first 10m after boot` where
    /// the file has them, separated by commas.
    pub schedule: String,
}

/// A service file's `[logs]` (ADR 0478): its own log files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logs {
    /// The files, as patterns whose last part may hold `*`: relative to
    /// its data folder, or absolute within its view.
    pub files: Vec<String>,
    /// When a file is turned over.
    pub turn_over: TurnOver,
    /// How many turned-over files are kept, 1 to 100.
    pub keep: u32,
    /// Whether turned-over files are compressed with zstd.
    pub compress: bool,
    /// How the service is told to open its files again.
    pub reopen: Reopen,
}

/// When a log file is turned over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnOver {
    /// Once a day.
    Daily,
    /// Once a week.
    Weekly,
    /// Once it holds this many bytes.
    Size(u64),
}

/// How a service opens its log files again once they are turned over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reopen {
    /// Spark sends it this signal.
    Signal(Signal),
    /// Spark restarts it.
    Restart,
    /// Nothing: it opens its file for each line.
    Nothing,
}

/// When a service is ready, and how long it has to get there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    /// What makes it ready.
    pub when: ReadyWhen,
    /// Seconds from its start to get ready, after which its start has
    /// failed.
    pub timeout: u32,
}

impl Default for Ready {
    fn default() -> Ready {
        Ready { when: ReadyWhen::Started, timeout: READY_TIMEOUT_DEFAULT }
    }
}

/// What makes a service ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadyWhen {
    /// Its process running.
    Started,
    /// A socket of its listening on this port, or on any port of
    /// `network.listen` when it names none.
    Listening(Option<u16>),
    /// This path being a socket made since it started.
    Socket(String),
    /// Its health check passing for the first time.
    Healthy,
    /// Its process ending with status 0, as a setup step's does.
    Ended,
}

impl ReadyWhen {
    /// The word `ready.when` says it with.
    pub fn name(&self) -> &'static str {
        match self {
            ReadyWhen::Started => "started",
            ReadyWhen::Listening(_) => "listening",
            ReadyWhen::Socket(_) => "socket",
            ReadyWhen::Healthy => "healthy",
            ReadyWhen::Ended => "ended",
        }
    }
}

/// How long a service has to get ready by default, in seconds, as
/// systemd's TimeoutStartSec.
pub const READY_TIMEOUT_DEFAULT: u32 = 90;

/// A service's health check: a command run as the service runs, every so
/// often, which passes by exiting with 0 in time, as health.toml(5)'s
/// checks do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    /// A program's absolute path and its arguments.
    pub command: Vec<String>,
    /// Seconds from the end of one check to the start of the next.
    pub every: u32,
    /// Seconds a check may take before it is stopped and fails.
    pub timeout: u32,
    /// Failed checks in a row after which the service is restarted.
    pub failures: u32,
}

impl Service {
    /// What is wrong with the services it names, given its own name,
    /// which its file does not hold: it may not name itself.
    ///
    /// # Errors
    ///
    /// Which list names it, in plain words.
    pub fn check_links(&self, name: &str) -> Result<(), String> {
        for (key, names) in
            [("requires", &self.requires), ("wants", &self.wants), ("after", &self.after)]
        {
            if names.iter().any(|other| other == name) {
                return Err(format!("{key} names {name}, the service itself"));
            }
        }
        Ok(())
    }

    /// Checks its certificates against its own name, `name`: certd, which
    /// makes and keeps them, names none.
    pub fn check_certificates(&self, name: &str) -> Result<(), String> {
        if name == CERTD && !self.certificates.is_empty() {
            return Err(format!(
                "{CERTD} keeps the certificates and names none: leave out certificates"
            ));
        }
        Ok(())
    }
}

/// The commands a service runs around backups' snapshots (ADR 0463),
/// each a program's absolute path and its arguments, as the service runs:
/// `before` as backup(8) is about to make them, to bring what it keeps
/// to a state it can start again from, and `after` once they are made.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    /// Run as the snapshots are about to be made; empty for none.
    #[serde(default)]
    pub before: Vec<String>,
    /// Run once they are made; empty for none.
    #[serde(default)]
    pub after: Vec<String>,
    /// How long each may run, in seconds, before it is stopped.
    #[serde(default = "hook_time_default")]
    pub time_limit: u32,
}

impl Default for Backup {
    fn default() -> Backup {
        Backup { before: Vec::new(), after: Vec::new(), time_limit: HOOK_TIME_DEFAULT }
    }
}

/// How long a backup hook may run by default, in seconds.
pub const HOOK_TIME_DEFAULT: u32 = 300;

/// The longest a backup hook may run, in seconds.
pub const HOOK_TIME_MAX: u32 = 3600;

fn hook_time_default() -> u32 {
    HOOK_TIME_DEFAULT
}

impl Backup {
    /// What is wrong with it, if anything.
    fn check(&self) -> Result<(), Problem> {
        for (key, command) in [("backup.before", &self.before), ("backup.after", &self.after)] {
            if let Some(program) = command.first()
                && !program.starts_with('/')
            {
                return Err(Problem::about(
                    key,
                    format!("{key} starts with a program's absolute path, not `{program}`"),
                ));
            }
            if command.iter().any(|word| word.contains('\0')) {
                return Err(Problem::about(key, format!("{key} holds a NUL")));
            }
        }
        if !(1..=HOOK_TIME_MAX).contains(&self.time_limit) {
            return Err(Problem::about(
                "backup.time_limit",
                format!("backup.time_limit is from 1 to {HOOK_TIME_MAX} seconds"),
            ));
        }
        Ok(())
    }
}

impl Service {
    /// How many network capabilities it gets: one to connect out, one for
    /// each range of ports it listens on, and one for admin.network.
    pub fn network_grants(&self) -> usize {
        self.permissions.network.grants() + usize::from(self.permissions.admin.network)
    }
}

/// What its resource domain may use.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Its share of the CPUs, from 1 to 10000; 100 is everyone's.
    #[serde(default = "weight_default", deserialize_with = "weight")]
    pub cpu_weight: u32,
    /// Its memory limit, in bytes, if it has one.
    #[serde(default, deserialize_with = "memory")]
    pub memory: Option<u64>,
    /// Which programs the out-of-memory policy stops first.
    #[serde(default, deserialize_with = "oom")]
    pub oom: Oom,
    /// Its share of each disk when several services wait for it, from 1
    /// to 10000; 100 is everyone's.
    #[serde(default = "io_weight_default", deserialize_with = "io_weight")]
    pub io_weight: u32,
    /// The bytes it may read from disks each second, if it has a limit.
    #[serde(default, deserialize_with = "io_read")]
    pub io_read: Option<u64>,
    /// The bytes it may write to disks each second, if it has a limit.
    #[serde(default, deserialize_with = "io_write")]
    pub io_write: Option<u64>,
    /// The read requests it may make each second, if it has a limit.
    #[serde(default, deserialize_with = "io_read_ops")]
    pub io_read_ops: Option<u64>,
    /// The write requests it may make each second, if it has a limit.
    #[serde(default, deserialize_with = "io_write_ops")]
    pub io_write_ops: Option<u64>,
    /// The most threads it and what it starts may have together.
    #[serde(default = "threads_default", deserialize_with = "threads")]
    pub threads: u32,
    /// How many files each of its processes may have open at once
    /// (RLIMIT_NOFILE), if not a program's 1024.
    #[serde(default, deserialize_with = "open_files")]
    pub open_files: Option<u32>,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            cpu_weight: WEIGHT_DEFAULT,
            memory: None,
            oom: Oom::Service,
            io_weight: IO_WEIGHT_DEFAULT,
            io_read: None,
            io_write: None,
            io_read_ops: None,
            io_write_ops: None,
            threads: THREADS_DEFAULT,
            open_files: None,
        }
    }
}

/// What it may do beyond running: the permission list's names, in groups.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawPermissions")]
pub struct Permissions {
    /// `network.*`: what of the network it may use.
    pub network: Network,
    /// `files.*`: the places of the tree its view holds, beyond
    /// [`ALWAYS`].
    pub files: Files,
    /// `runs-with-your-access`: the whole tree as Spark holds it, with
    /// the bypass right, in place of a view.
    pub runs_with_your_access: bool,
    /// `admin.*`: what of the machine it may administer.
    pub admin: Admin,
    /// `names`: the names capability (radix-names(2), ADR 0218), which
    /// gives processes and capabilities another user's name: authd's, to
    /// build sessions with.
    pub names: bool,
    /// `domain`: a capability to its own resource domain, with which it
    /// makes domains below it and stops them: authd's, for help sessions
    /// (ADR 0540).
    pub domain: bool,
    /// `devices.*`: the cards it drives, as a driver program.
    pub devices: Devices,
}

impl Permissions {
    /// Every place its view holds beyond [`ALWAYS`], in the order Spark
    /// attaches them.
    pub fn places(&self) -> Vec<Place> {
        let files = &self.files;
        let read = files.read.iter().map(|path| Place { path: path.clone(), write: false });
        let write = files.read_write.iter().chain(&files.mount);
        let write = write.map(|path| Place { path: path.clone(), write: true });
        let shared = files.shared.iter().map(|name| Place { path: shared(name), write: true });
        read.chain(write).chain(shared).collect()
    }
}

/// The administration group: `admin.network`, `admin.hardware`,
/// `admin.time`, `admin.firewall` and `admin.tune`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admin {
    /// Whether it may set the network interfaces up, their addresses and
    /// routers, through a network capability with only the control right
    /// (ADR 0190).
    #[serde(default)]
    pub network: bool,
    /// Whether it gets the platform capability, which sets the machine's
    /// fans, LEDs and ARGB header (ADR 0208).
    #[serde(default)]
    pub hardware: bool,
    /// Whether it gets the clock capability, which sets and slews the
    /// time of day and writes the real-time clock (ADR 0194).
    #[serde(default)]
    pub time: bool,
    /// Whether it gets the firewall capability, which gives the packet
    /// filter its rules (ADR 0334).
    #[serde(default)]
    pub firewall: bool,
    /// Whether it gets the kernel settings capability, which changes the
    /// kernel's settings, as the tune service does (ADR 0476).
    #[serde(default)]
    pub tune: bool,
}

/// The devices group: `devices.pci`, the cards the kernel left for a
/// driver program that the service drives (ADR 0198).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Devices {
    /// `devices.pci`: the PCI cards it drives, by vendor and device ID.
    /// Spark gives it each card the kernel left that one matches, up to
    /// [`DEVICES_MAX`], and does not start it while none does.
    #[serde(default, deserialize_with = "pci_ids")]
    pub pci: Vec<PciId>,
}

/// The most cards a service is given: the slots a program has for them.
pub const DEVICES_MAX: usize = radix_abi::startup::DEVICE_SLOTS as usize;

/// A PCI card's vendor and device IDs, as `devices.pci` names them:
/// `"8086:100e"`, or `"8086:*"` for any of the vendor's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciId {
    /// The vendor's ID.
    pub vendor: u16,
    /// The device's ID, `None` for any.
    pub device: Option<u16>,
}

impl PciId {
    /// Reads one written as `devices.pci` writes it.
    pub fn parse(text: &str) -> Option<PciId> {
        let (vendor, device) = text.split_once(':')?;
        let id = |part: &str| {
            (part.len() == 4 && part.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| u16::from_str_radix(part, 16).ok())
                .flatten()
        };
        let device = if device == "*" { None } else { Some(id(device)?) };
        Some(PciId { vendor: id(vendor)?, device })
    }

    /// Whether the card with these IDs is one it names.
    pub fn matches(self, vendor: u16, device: u16) -> bool {
        self.vendor == vendor && self.device.is_none_or(|own| own == device)
    }
}

impl fmt::Display for PciId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.device {
            Some(device) => write!(f, "{:04x}:{device:04x}", self.vendor),
            None => write!(f, "{:04x}:*", self.vendor),
        }
    }
}

/// `devices.pci`: PCI IDs, each once.
fn pci_ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PciId>, D::Error> {
    let texts = Vec::<String>::deserialize(d)?;
    let mut ids: Vec<PciId> = Vec::new();
    for text in &texts {
        let id = PciId::parse(text).ok_or_else(|| {
            de::Error::custom(format!(
                "devices.pci takes a card's vendor and device IDs in hexadecimal, such as \
                 \"8086:100e\", or \"8086:*\" for any of the vendor's, not `{}`",
                text.escape_debug()
            ))
        })?;
        if ids.contains(&id) {
            return Err(de::Error::custom(format!("devices.pci names {id} twice")));
        }
        ids.push(id);
    }
    Ok(ids)
}

/// The files group: the places of the tree a service's view holds, each
/// at its real path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Files {
    /// `files.read`: places it may read.
    #[serde(default, deserialize_with = "places")]
    pub read: Vec<String>,
    /// `files.read-write`: places it may read and change.
    #[serde(default, rename = "read-write", deserialize_with = "places")]
    pub read_write: Vec<String>,
    /// `files.mount`: places it may read, change and mount filesystems
    /// beneath, which it gets as descriptors from 3 too.
    #[serde(default, deserialize_with = "places")]
    pub mount: Vec<String>,
    /// `files.shared`: folders of `/shared` it may read and change, by
    /// name.
    #[serde(default, deserialize_with = "folders")]
    pub shared: Vec<String>,
    /// `files.disks`: every disk and partition at `/devices`, as they come
    /// and go, with the devices every service has and nothing else of
    /// `/devices` (ADR 0473).
    #[serde(default)]
    pub disks: Option<DeviceAccess>,
    /// `files.efi-system`: the EFI system partitions alone at `/devices`,
    /// by partition type, in the same way.
    #[serde(default, rename = "efi-system")]
    pub efi_system: Option<DeviceAccess>,
    /// `files.root-subvolumes`: names at the top of the Btrfs this system
    /// started from, which its view holds at `/volumes/<disk>` with no
    /// other name there (ADR 0473).
    #[serde(default, rename = "root-subvolumes", deserialize_with = "subvolumes")]
    pub root_subvolumes: Vec<String>,
}

/// How a service holds the disks it is given: `files.disks` and
/// `files.efi-system`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceAccess {
    /// It may read them.
    Read,
    /// It may read and write them.
    ReadWrite,
}

impl DeviceAccess {
    /// Whether it may write them.
    pub fn write(self) -> bool {
        self == DeviceAccess::ReadWrite
    }

    /// Its name in a service file.
    pub fn name(self) -> &'static str {
        match self {
            DeviceAccess::Read => "read",
            DeviceAccess::ReadWrite => "read-write",
        }
    }
}

/// The names at the top of the root's Btrfs that hold the owner's files,
/// which no service's view holds by name: an update or a snapshot never
/// needs them (ADR 0455).
pub const OWNERS_SUBVOLUMES: [&str; 3] = ["@users", "@shared", "@applications"];

/// The most names `files.root-subvolumes` takes, as many as the kernel
/// attaches a directory with (radix-directory(2)).
pub const ROOT_SUBVOLUMES_MAX: usize = 16;

/// The longest name `files.root-subvolumes` takes.
const SUBVOLUME_NAME_MAX: usize = 64;

/// A place of the tree in a service's view: a directory or a file, at its
/// real path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// Its absolute path.
    pub path: String,
    /// Whether the service may change it, or only read it.
    pub write: bool,
}

/// What every service's view holds without asking (ADR 0217): the
/// system's programs, libraries, resources and settings and the kernel's
/// files, read-only; the devices every program uses, and the memory
/// filesystem of shared memory and System V IPC, as Linux's `/dev/shm`
/// is every service's (ADRs 0490, 0522); notifyd's folder, as
/// notifications are always allowed; alertd's, so that the system's
/// services raise alerts (ADR 0485), which alertd takes from root's
/// processes alone; and resolved's,
/// whose resolv.conf `/etc/resolv.conf` leads to (ADR 0193), and netd's,
/// which an `/etc/resolv.conf` from before resolved leads to (ADR 0191).
/// Its program,
/// its own `/tmp`, its own processes at [`PROCESSES`], the root's links
/// and its secrets come on top. A place a file names that is one of these
/// takes its place.
pub const ALWAYS: &[(&str, bool)] = &[
    ("/system/commands", false),
    ("/system/libraries", false),
    ("/system/resources", false),
    ("/system/settings", false),
    ("/system/live/notify", false),
    ("/system/live/alerts", false),
    ("/system/live/resolved", false),
    ("/system/live/netd", false),
    ("/kernel", false),
    ("/devices/null", true),
    ("/devices/zero", false),
    ("/devices/random", false),
    ("/devices/urandom", false),
    ("/devices/shm", true),
];

/// Where a service sees its processes: those of its own domain alone
/// (ADR 0473), unless its file names this place, which holds every
/// process.
pub const PROCESSES: &str = "/processes";

/// Where Spark keeps its own files: no service holds them, or what is
/// in them, the secrets among them.
pub const SPARK_STATE: &str = "/system/state/spark";

/// Where Spark keeps the secrets of service `name`, and where the
/// service finds them, as `SECRETS_DIRECTORY` says.
pub fn secrets_dir(name: &str) -> String {
    format!("{SPARK_STATE}/secrets/{name}")
}

/// The service that keeps the machine's certificates (ADR 0512).
pub const CERTD: &str = "certd";

/// Where certd keeps the certificates: each `NAME` a link to the folder
/// of its current generation, [`certificate_generation`], which holds
/// `fullchain.pem` and `key.pem`.
pub const CERTD_STATE: &str = "/system/state/certd/certificates";

/// The files of a certificate's generation: its chain, the certificate
/// first, and its private key, each PEM.
pub const CERTIFICATE_FILES: [&str; 2] = ["fullchain.pem", "key.pem"];

/// The name of the folder of generation `generation` (from 1) of
/// certificate `name`, where certd and Spark keep it and its link leads: `.NAME.G`,
/// hidden, so that a folder of certificates lists their names alone. No
/// certificate's name starts with a dot, so it is no certificate's name.
pub fn certificate_generation(name: &str, generation: u64) -> String {
    format!(".{name}.{generation}")
}

/// The certificate and generation the folder `entry` is, as
/// [`certificate_generation`] names it, or None if it is no generation's
/// name.
pub fn generation_of(entry: &str) -> Option<(&str, u64)> {
    let (name, digits) = entry.strip_prefix('.')?.rsplit_once('.')?;
    let generation = digits.parse().ok()?;
    let exact = digits.bytes().all(|b| b.is_ascii_digit()) && !digits.starts_with('0');
    (exact && check_certificate(name).is_ok()).then_some((name, generation))
}

/// Where Spark keeps each service's copies of its certificates: no
/// service holds it, or what is in it.
pub const CERTIFICATE_COPIES: &str = "/system/live/certificates";

/// Where Spark keeps the copies of the certificates of service `name`,
/// and where the service finds them, as `CERTIFICATES_DIRECTORY` says:
/// a folder for each, of its name, with `fullchain.pem` and `key.pem`.
pub fn certificates_dir(name: &str) -> String {
    format!("{CERTIFICATE_COPIES}/{name}")
}

/// The file of the accounts, whose lines are
/// `name:x:user:group:full name:home:shell`.
pub const PASSWD: &str = "/system/settings/passwd";

/// The user and group numbers of account `name` in `passwd`, the text of
/// [`PASSWD`], which Spark runs a service with an `account` as (ADR
/// 0385).
pub fn account_numbers(passwd: &str, name: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let user = fields.nth(1)?.parse().ok()?;
        let group = fields.next()?.parse().ok()?;
        Some((user, group))
    })
}

/// The groups, as group(5) has them, whose numbers Spark gives a
/// service's processes for its `groups` (ADR 0477).
pub const GROUP: &str = "/system/settings/group";

/// The number of group `name` in `group`, the text of [`GROUP`].
pub fn group_number(group: &str, name: &str) -> Option<u32> {
    group.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        fields.nth(1)?.parse().ok()
    })
}

/// The most places `files.mount` names: they are descriptors 3 and on.
pub const MOUNT_MAX: usize = 4;

/// The path of the folder `name` of `/shared`.
fn shared(name: &str) -> String {
    format!("/shared/{name}")
}

/// Whether `inner` is `outer` or beneath it.
fn within(inner: &str, outer: &str) -> bool {
    inner.strip_prefix(outer).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The `permissions` table as written, before the checks of the whole.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPermissions {
    #[serde(default)]
    network: Network,
    #[serde(default)]
    files: Files,
    #[serde(default, rename = "runs-with-your-access")]
    runs_with_your_access: bool,
    #[serde(default)]
    admin: Admin,
    #[serde(default)]
    names: bool,
    #[serde(default)]
    domain: bool,
    #[serde(default)]
    devices: Devices,
}

impl TryFrom<RawPermissions> for Permissions {
    type Error = String;

    fn try_from(raw: RawPermissions) -> Result<Permissions, String> {
        let permissions = Permissions {
            network: raw.network,
            files: raw.files,
            runs_with_your_access: raw.runs_with_your_access,
            admin: raw.admin,
            names: raw.names,
            domain: raw.domain,
            devices: raw.devices,
        };
        let grants = permissions.network.grants() + usize::from(permissions.admin.network);
        if grants > NETWORK_SLOTS as usize {
            return Err(format!(
                "it asks for {grants} network capabilities, one to connect out, one for each \
                 range of ports it listens on and one for admin.network; a program gets at most \
                 {NETWORK_SLOTS}"
            ));
        }
        let places = permissions.places();
        let files = &permissions.files;
        let devices = files.disks.is_some() || files.efi_system.is_some();
        let by_name = !files.root_subvolumes.is_empty();
        if permissions.runs_with_your_access && (!places.is_empty() || devices || by_name) {
            return Err("runs-with-your-access holds every file already: leave out files.*".into());
        }
        if files.disks.is_some() && files.efi_system.is_some() {
            return Err("files.disks holds the EFI system partitions already: leave out \
                        files.efi-system"
                .into());
        }
        if let Some(place) = places.iter().find(|place| devices && within(&place.path, "/devices"))
        {
            return Err(format!(
                "files.disks and files.efi-system make its /devices, so its view cannot hold {} \
                 apart: leave it out",
                place.path
            ));
        }
        if permissions.files.mount.len() > MOUNT_MAX {
            return Err(format!("files.mount names at most {MOUNT_MAX} places"));
        }
        for (i, place) in places.iter().enumerate() {
            let path = place.path.as_str();
            if let Some(other) =
                places[..i].iter().find(|o| within(path, &o.path) || within(&o.path, path))
            {
                return Err(if path == other.path {
                    format!("files names {path} twice")
                } else {
                    format!("files names {path} and {}, one within the other: name one", other.path)
                });
            }
            if within(path, SPARK_STATE) || within(SPARK_STATE, path) {
                return Err(format!(
                    "no service holds {SPARK_STATE}, where Spark keeps its files and the \
                     services' secrets, so it cannot hold {path}"
                ));
            }
            if within(path, CERTIFICATE_COPIES) || within(CERTIFICATE_COPIES, path) {
                return Err(format!(
                    "no service holds {CERTIFICATE_COPIES}, where Spark keeps each service's \
                     copies of its certificates' keys, so it cannot hold {path}"
                ));
            }
            if path != PROCESSES && within(path, PROCESSES) {
                return Err(format!(
                    "a service sees its own processes at {PROCESSES} already, so its view cannot \
                     hold {path} apart; name {PROCESSES} with files.read to see every process"
                ));
            }
            if let Some((always, _)) = ALWAYS.iter().find(|(a, _)| path != *a && within(path, a)) {
                return Err(format!(
                    "every service reads {always} already, so its view cannot hold {path} \
                     apart; name {always} with files.read-write to change what is in it"
                ));
            }
        }
        Ok(permissions)
    }
}

/// The network group: `network.outbound` and `network.listen`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawNetwork")]
pub struct Network {
    /// Whether it may connect out, to any address and port.
    pub outbound: bool,
    /// The ranges of ports it may listen on, none overlapping.
    pub listen: Vec<Ports>,
}

impl Network {
    fn grants(&self) -> usize {
        usize::from(self.outbound) + self.listen.len()
    }
}

/// A range of ports, from `first` to `last`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    /// The first port.
    pub first: u16,
    /// The last port, `first` for a range of one.
    pub last: u16,
}

impl Ports {
    /// How many ports it holds.
    pub fn count(self) -> u32 {
        u32::from(self.last) - u32::from(self.first) + 1
    }
}

impl fmt::Display for Ports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.first == self.last {
            write!(f, "{}", self.first)
        } else {
            write!(f, "{}-{}", self.first, self.last)
        }
    }
}

/// The file `service new NAME` writes: every setting, explained, for a
/// service that is not started at boot until it is ready. `NAME` stands
/// for the service's name.
const TEMPLATE: &str = r#"# The NAME service. service.toml(5) explains each setting. Check this file
# with `service check NAME`; `service edit NAME` checks it as you save.
# Start the service with `service NAME start`.

service_version = 1
description = "What NAME does, in a few words"
# The program, by its absolute path, and the arguments after its name.
program = "/system/commands/true"
arguments = []
# Lower starts first at boot, and stops last; the system's own services
# take 5 to 50.
order = 60
# Whether Spark starts it at boot: true once it is ready.
enabled = false
# The account it runs as, for a bundle's service, which bundled makes;
# left out, it acts for root, without root's bypass.
# account = "NAME"
# When it is started again after it ends: always, on-failure or never.
restart = "on-failure"
# With service_version = 2 (Dukovo 1.0 and later), it can wait for other
# services and say when it is ready, and Spark can check its health:
# the services it requires, started first and ready or it does not
# start; those it wants, started first whether or not they start; those
# it starts after when they start too; and the failed starts in a row
# after which Spark stops trying, 0 for never.
# requires = []
# wants = []
# after = []
# give_up_after = 0
# When it is ready, in [ready] or as ready.KEY: started, listening (on
# ready.port, or on any port of network.listen), socket (once ready.path
# is a socket made since it started), healthy (once its health check
# passes) or ended (once it exits with 0, as a setup step does); and the
# seconds it has to get there.
# ready.when = "started"
# ready.port = 8080
# ready.path = "/system/live/services/NAME/NAME.sock"
# ready.timeout = 90
# Its health check, in [health] or as health.KEY: a command run as it
# runs, every so many seconds, which passes by exiting with 0 within its
# timeout; so many failed checks in a row restart it.
# health.command = ["/system/commands/true"]
# health.every = 30
# health.timeout = 10
# health.failures = 3
# When timerd starts it, in [timer] or as timer.KEY: times of day as
# crontab lines' five fields or the words hourly, daily, weekly, monthly
# and yearly; or so long after its last start (every, such as "6h"), the
# first one after the machine starts being after_boot; a delay of up to
# spread that is the same for this machine each day; whether a time
# missed while the machine was off runs once, soon after it starts; and
# the tzdata time zone the times are in, the system's if left out. A
# service with a timer is not started at boot.
# timer.at = ["daily", "30 2 * * 1-5"]
# timer.every = "6h"
# timer.after_boot = "10m"
# timer.spread = "1h"
# timer.catch_up = true
# timer.time_zone = "UTC"
# Its own log files, which logturn(8) turns over, in [logs] or as
# logs.KEY: patterns whose last part may hold a `*`, relative to its data
# folder or absolute; when, daily, weekly or at a size such as "100M";
# how many turned-over files are kept; whether they are compressed; and
# how the service opens its files again: a signal such as "SIGUSR1",
# "restart", or "none" for a program that opens its file for each line.
# logs.files = ["logs/*.log"]
# logs.turn_over = "daily"
# logs.keep = 14
# logs.compress = true
# logs.reopen = "none"
# How Spark stops it, in [stop] or as stop.KEY: the signal that asks it
# to end (SIGTERM, SIGINT, SIGQUIT, SIGUSR1, SIGUSR2 or SIGWINCH; the
# one its daemon takes as a stop request), and the seconds, 1 to 3600,
# after which Spark ends what is left of it.
# stop.signal = "SIGTERM"
# stop.timeout = 5
# When Spark starts it once enabled: at-boot, or on-connection, at the
# first connection to one of its sockets. A socket is a [[sockets]]
# table of the sockets Spark makes and holds, which it passes to the
# program from descriptor 3 as sd_listen_fds(3) says; the lines of one
# (a port of network.listen, or a path in its live folder, which only
# the service reaches unless its to is anyone or system, or a group's
# members connect):
# [[sockets]]
# port = 8080
# address = "0.0.0.0"
# udp = false
# path = "queue"
# name = "web"
# backlog = 4096
# non-blocking = false
# to = "anyone"
# group = "web-socket"
# start = "at-boot"
# More groups its processes are in, by name: those of sockets that
# other bundles offer it.
# groups = []
# The secrets Spark hands it as read-only files, in the folder
# SECRETS_DIRECTORY names, never in its environment; set each with
# `service secret set NAME SECRET`.
secrets = []
# The certificates certd keeps, as `cert list` names them, that Spark
# copies for it, read-only, into the folder CERTIFICATES_DIRECTORY names,
# each in a folder of its name with fullchain.pem and key.pem; it must
# read them again on SIGHUP, which Spark sends when one changes.
certificates = []

# More of its environment, as NAME = "value".
[environment]

# What its domain may use: a CPU weight from 1 to 10000, where 100 is
# everyone's; a memory limit such as "64M", or "none"; and "service" or
# "app", which the out-of-memory policy stops first. Its disks: a weight
# from 1 to 10000, where 100 is everyone's, which shares a disk between
# services whose requests wait for it; the bytes it may read and write
# each second, such as "50M", and the requests it may make, or "none",
# across all disks. The most threads it and what it starts may have
# (1 to 1048576), which a fork bomb in it cannot pass, and the files each
# of its processes may have open (1 to 1048576), which a program has 1024
# of unless this says more.
[limits]
cpu_weight = 100
memory = "none"
oom = "service"
io_weight = 100
io_read = "none"
io_write = "none"
io_read_ops = "none"
io_write_ops = "none"
threads = 4096
# open_files = 1024

# What it may do beyond running: connect out to the network, listen on
# ports and ranges of ports, such as 80 or "8000-8009", set the network
# interfaces up, set the machine's fans, LEDs and ARGB header, and set
# its clock.
[permissions]
network.outbound = false
network.listen = []
# The places it may read, and read and change, beyond the system's
# programs, libraries, resources and settings, which every service reads;
# the places it mounts filesystems beneath; and folders of /shared, by
# name. It sees nothing else of the tree.
files.read = []
files.read-write = []
files.mount = []
files.shared = []
# The whole tree as Spark holds it, in place of files: only for a service
# that starts other people's shells.
runs-with-your-access = false
# Whether it may set the network interfaces' addresses and routers, as
# ip(8) does.
admin.network = false
# Whether it may set the machine's fans, LEDs and ARGB header.
admin.hardware = false
# Whether it may set the time of day and the real-time clock, as timed(8)
# does.
admin.time = false
# Whether it may give the firewall its rules, as firewall(8) does.
admin.firewall = false
# Whether it may change the kernel's settings, as tune(8)'s service does.
admin.tune = false
# Giving processes and files another user's name: only for authd, which
# signs people in.
names = false
# Making resource domains below its own and stopping them: only for
# authd, which runs each help session in one.
domain = false
# The cards it drives, as a driver program does, by their PCI vendor and
# device IDs, such as "8086:100e", or "8086:*" for any of the vendor's.
devices.pci = []

# What it runs around the snapshots of backup(8), each a program's
# absolute path and its arguments, as the service runs: before, to bring
# what it keeps to a state it can start again from, and after, once the
# snapshots are made; and how long each may run, in seconds.
[backup]
before = []
after = []
time_limit = 300
"#;

/// The file `service new` writes for the service `name`.
pub fn template(name: &str) -> String {
    TEMPLATE.replace("NAME", name)
}

/// The error serde found in the service file `text` at `span`, or in no
/// place when it has none: where, the setting it is about, and the nearest
/// of what to put instead.
fn read_error(text: &str, message: &str, span: Option<Range<usize>>) -> Error {
    let error = Error::from_message(text, message, span.clone());
    // An error with no place has no span to look for a setting by.
    let (Some(span), Some(_)) = (span, error.line) else { return error };
    let Some(key) = path_at(text, &span) else { return error };
    // A setting's value of the wrong kind gets the line `service new`
    // writes for it; an item of a list does not, which is no line to copy.
    let whole = find(text, &key).is_some_and(|(_, value)| value == span);
    let fix = match error.fix {
        None if whole && message.starts_with("invalid type: ") => {
            template_line(&key).map(|line| format!("as `service new` writes it: `{line}`"))
        }
        fix => fix,
    };
    Error { fix, ..error }.with_key(key)
}

/// The spans of the key `key`, a dotted path, in the service file `text`,
/// and of its value: through tables written as `[table]`, as `table.key`
/// and as `{ key = value }`.
fn find(text: &str, key: &str) -> Option<(Range<usize>, Range<usize>)> {
    let root = DeTable::parse(text).ok()?;
    let mut table = root.get_ref();
    let mut names = key.split('.').peekable();
    loop {
        let (name, value) = table.get_key_value(names.next()?)?;
        if names.peek().is_none() {
            return Some((name.span(), value.span()));
        }
        table = value.get_ref().as_table()?;
    }
}

/// The span of the key `key`, a dotted path, in the service file `text`.
pub(crate) fn key_span(text: &str, key: &str) -> Option<Range<usize>> {
    find(text, key).map(|(key, _)| key)
}

/// The dotted path of the setting that `span` of `text` is the key of or
/// the value of, or an item of the value of.
fn path_at(text: &str, span: &Range<usize>) -> Option<String> {
    fn walk(table: &DeTable<'_>, prefix: &str, span: &Range<usize>) -> Option<String> {
        table.iter().find_map(|(key, value)| {
            let path = if prefix.is_empty() {
                key.get_ref().to_string()
            } else {
                format!("{prefix}.{}", key.get_ref())
            };
            let inside = match value.get_ref() {
                DeValue::Table(inner) => walk(inner, &path, span),
                DeValue::Array(items) => {
                    items.iter().any(|item| item.span() == *span).then(|| path.clone())
                }
                _ => None,
            };
            inside.or_else(|| (key.span() == *span || value.span() == *span).then_some(path))
        })
    }
    walk(DeTable::parse(text).ok()?.get_ref(), "", span)
}

/// The template's lines for the settings it leaves out until they are
/// wanted, as `service new` writes them in a file that sets them: each
/// line of the form `# KEY = VALUE`, with its `# ` taken off.
pub(crate) fn template_with_settings() -> String {
    let is_setting = |line: &str| {
        line.split_once(" = ").is_some_and(|(key, _)| {
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    };
    let lines: Vec<&str> = TEMPLATE
        .lines()
        .map(|line| line.strip_prefix("# ").filter(|line| is_setting(line)).unwrap_or(line))
        .collect();
    lines.join("\n")
}

/// The line `service new` writes for the setting `key`, a dotted path,
/// without its comment, if it writes one.
pub(crate) fn template_line(key: &str) -> Option<String> {
    let text = template_with_settings();
    let (name, value) = find(&text, key)?;
    let start = text[..name.start].rfind('\n').map_or(0, |newline| newline + 1);
    Some(text[start..value.end].to_owned())
}

/// Reads a service file.
pub fn from_toml(text: &str) -> Result<Service, Error> {
    let file: File =
        toml::from_str(text).map_err(|err| read_error(text, err.message(), err.span()))?;
    check(file).map_err(|problem| problem.placed(text, None))
}

/// The folder, beside the service files, of the files that change them
/// (ADR 0386): `settings.d/NAME.toml` sets keys of `NAME.toml`, so that
/// an administrator's changes never touch the file the system or a
/// bundle wrote, which an update may write again.
pub const OVERRIDES: &str = "settings.d";

/// An override file, read: the keys it sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override(String);

impl Override {
    /// Reads an override file: any keys and tables, checked only once
    /// they are set over a service file.
    pub fn from_toml(text: &str) -> Result<Override, Error> {
        toml::from_str::<toml::Table>(text)
            .map(|_| Override(text.to_owned()))
            .map_err(|err| Error::from_toml(text, &err))
    }
}

/// Reads a `T` from the service file `text` with the keys of `over` set
/// over it. The spans of an error are the file's, or, from the byte after
/// the file's end, the override's: [`over_error`] tells which.
fn read_over<T: DeserializeOwned>(text: &str, over: &Override) -> Result<T, toml::de::Error> {
    // Spaces before the override's text put its spans after the file's,
    // where they cannot be taken for them.
    let shifted = format!("{}{}", " ".repeat(text.len() + 1), over.0);
    let mut table = DeTable::parse(text)?;
    set_over(table.get_mut(), DeTable::parse(&shifted)?.into_inner());
    T::deserialize(toml::de::Deserializer::from(table))
}

/// Sets each key of `over` in `table`: a table's keys one by one, any
/// other value in place of the one there.
fn set_over<'a>(table: &mut DeTable<'a>, over: DeTable<'a>) {
    for (key, value) in over {
        let name: &str = key.get_ref();
        let span = value.span();
        match (table.get_mut(name).map(Spanned::get_mut), value.into_inner()) {
            (Some(DeValue::Table(inner)), DeValue::Table(more)) => set_over(inner, more),
            (_, value) => {
                table.remove(name);
                table.insert(key, Spanned::new(span, value));
            }
        }
    }
}

/// The error `err` of reading the service file `text` with `over` set
/// over it, at its place in the override or in the file.
fn over_error(text: &str, over: &Override, err: &toml::de::Error) -> Error {
    let shift = text.len() + 1;
    match err.span() {
        Some(span) if span.start >= shift => {
            let span = span.start - shift..span.end - shift;
            Error { in_override: true, ..read_error(&over.0, err.message(), Some(span)) }
        }
        span => read_error(text, err.message(), span),
    }
}

/// The keys whose values differ between the service files `a` and `b`,
/// as dotted paths, such as `limits.memory`, in order: what an update's
/// new default beside an administrator's file changes (ADR 0386).
pub fn changed_keys(a: &str, b: &str) -> Result<Vec<String>, Error> {
    let a: toml::Table = toml::from_str(a).map_err(|err| Error::from_toml(a, &err))?;
    let b: toml::Table = toml::from_str(b).map_err(|err| Error::from_toml(b, &err))?;
    let mut out = Vec::new();
    walk("", &a, &b, &mut out);
    Ok(out)
}

/// The keys whose values differ between the service file `old` and `new`,
/// each with the override that sets keys over it, if it has one: the
/// settings that putting `new` to use changes in a service that runs
/// `old`, as dotted paths, in order.
pub fn changed_settings(
    old: (&str, Option<&Override>),
    new: (&str, Option<&Override>),
) -> Result<Vec<String>, Error> {
    let table = |(text, over): (&str, Option<&Override>)| match over {
        Some(over) => read_over(text, over).map_err(|err| over_error(text, over, &err)),
        None => toml::from_str(text).map_err(|err| Error::from_toml(text, &err)),
    };
    let (old, new): (toml::Table, toml::Table) = (table(old)?, table(new)?);
    let mut out = Vec::new();
    walk("", &old, &new, &mut out);
    Ok(out)
}

/// Adds to `out` the dotted path of each key under `prefix` whose value
/// differs between the tables `a` and `b`, in order.
fn walk(prefix: &str, a: &toml::Table, b: &toml::Table, out: &mut Vec<String>) {
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        match (a.get(key), b.get(key)) {
            (Some(toml::Value::Table(x)), Some(toml::Value::Table(y))) => walk(&path, x, y, out),
            (x, y) if x != y => out.push(path),
            _ => {}
        }
    }
}

/// Reads a service file with its override, if it has one, set over it.
/// An error that is in the override has its place there, and says so.
pub fn from_toml_over(text: &str, over: Option<&Override>) -> Result<Service, Error> {
    let Some(over) = over else { return from_toml(text) };
    let file = read_over(text, over).map_err(|err| over_error(text, over, &err))?;
    check(file).map_err(|problem| problem.placed(text, Some(&over.0)))
}

/// Checks an account's name, as Debian's: 1 to 32 lowercase letters,
/// digits, `_`, `-` and `.`, starting with a letter or `_`.
pub fn check_account(name: &str) -> Result<(), String> {
    let fits = (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.".contains(&b))
        && (name.as_bytes()[0].is_ascii_lowercase() || name.starts_with('_'));
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not an account's name: it has 1 to 32 lowercase letters, digits, `_`, `-` \
             and `.`, and starts with a letter or `_`",
            name.escape_debug()
        ))
    }
}

/// `text`, a service file, with its `enabled` line saying `on`, or one
/// added after its `program` line.
pub fn with_enabled(text: &str, on: bool) -> String {
    let line = format!("enabled = {on}");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    // The top-level keys: those before the first table.
    let top = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let key_of = |l: &str| l.split_once('=').map(|(key, _)| key.trim().to_owned());
    if let Some(at) = lines[..top].iter().position(|l| key_of(l).as_deref() == Some("enabled")) {
        lines[at] = line;
    } else {
        let at = lines[..top]
            .iter()
            .position(|l| key_of(l).as_deref() == Some("program"))
            .map_or(0, |at| at + 1);
        lines.insert(at, line);
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Checks a service's name, the name of its file without `.toml`: it
/// names its resource domain and its log lines too.
pub fn check_name(name: &str) -> Result<(), String> {
    let fits = (1..=NAME_MAX as usize).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.starts_with(['.', '-']);
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{name}` is not a service's name: it has 1 to {NAME_MAX} letters, digits, `-`, `_` \
             and `.`, and starts with a letter, a digit or `_`"
        ))
    }
}

/// A service file as it is written, before the checks that need more than
/// one setting.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    service_version: Option<Version>,
    #[serde(default)]
    description: String,
    program: Option<Program>,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    environment: Environment,
    #[serde(default = "order_default")]
    order: u32,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default, deserialize_with = "restart")]
    restart: Restart,
    #[serde(default)]
    limits: Limits,
    #[serde(default, deserialize_with = "secret_names")]
    secrets: Vec<String>,
    #[serde(default, deserialize_with = "certificate_names")]
    certificates: Vec<String>,
    #[serde(default)]
    permissions: Permissions,
    account: Option<String>,
    #[serde(default)]
    backup: Backup,
    #[serde(default, deserialize_with = "requires")]
    requires: Option<Vec<String>>,
    #[serde(default, deserialize_with = "wants")]
    wants: Option<Vec<String>>,
    #[serde(default, deserialize_with = "after")]
    after: Option<Vec<String>>,
    #[serde(default, deserialize_with = "give_up_after")]
    give_up_after: Option<u32>,
    ready: Option<ReadyFile>,
    health: Option<HealthFile>,
    timer: Option<TimerFile>,
    logs: Option<LogsFile>,
    #[serde(default, deserialize_with = "socket_files")]
    sockets: Vec<SocketFile>,
    #[serde(default, deserialize_with = "start")]
    start: Start,
    stop: Option<StopFile>,
    #[serde(default, deserialize_with = "group_names")]
    groups: Vec<String>,
}

/// `[ready]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyFile {
    #[serde(default, deserialize_with = "ready_when")]
    when: ReadyWord,
    #[serde(default, deserialize_with = "ready_port")]
    port: Option<u16>,
    path: Option<String>,
    #[serde(default = "ready_timeout_default", deserialize_with = "ready_timeout")]
    timeout: u32,
}

/// `ready.when` as written.
#[derive(Clone, Copy, Default)]
enum ReadyWord {
    #[default]
    Started,
    Listening,
    Socket,
    Healthy,
    Ended,
}

impl ReadyWord {
    const WORDS: &[&str] = &["started", "listening", "socket", "healthy", "ended"];
}

/// `[health]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthFile {
    #[serde(deserialize_with = "health_command")]
    command: Vec<String>,
    #[serde(default = "health_every_default", deserialize_with = "health_every")]
    every: u32,
    #[serde(default = "health_timeout_default", deserialize_with = "health_timeout")]
    timeout: u32,
    #[serde(default = "health_failures_default", deserialize_with = "health_failures")]
    failures: u32,
}

impl File {
    /// The first key of service_version 2 the file sets, if any.
    fn version_2_key(&self) -> Option<&'static str> {
        let set = [
            self.requires.is_some(),
            self.wants.is_some(),
            self.after.is_some(),
            self.give_up_after.is_some(),
            self.ready.is_some(),
            self.health.is_some(),
            self.stop.is_some(),
        ];
        VERSION_2_KEYS.iter().zip(set).find_map(|(key, set)| set.then_some(*key))
    }
}

/// What is wrong with a file that deserialized, and the setting it is
/// about.
pub(crate) struct Problem {
    /// The dotted key it is about, or none for the file as a whole.
    key: Option<&'static str>,
    /// What is wrong, in plain words.
    message: String,
    /// What to do about it, when there is one thing to do.
    fix: Option<String>,
    /// Which of the `[[sockets]]` tables `key` is in, when it is in one.
    socket: Option<usize>,
}

impl Problem {
    /// A problem with the file as a whole.
    fn new(message: impl Into<String>) -> Problem {
        Problem { key: None, message: message.into(), fix: None, socket: None }
    }

    /// A problem with the setting `key`.
    fn about(key: &'static str, message: impl Into<String>) -> Problem {
        Problem { key: Some(key), ..Problem::new(message) }
    }

    /// The same problem with `fix` as the one thing to do about it.
    fn with_fix(self, fix: impl Into<String>) -> Problem {
        Problem { fix: Some(fix.into()), ..self }
    }

    /// The error it is in `file`, the text of a service file, and in its
    /// override `over`, which sets keys over the file's. It is placed on
    /// its key, in the override when that sets the key and else in the
    /// file; a key neither sets has it placed on the table that holds it.
    fn placed(self, file: &str, over: Option<&str>) -> Error {
        let mut error = Error { checked: true, ..Error::new(self.message) };
        if let Some(fix) = self.fix {
            error = error.with_fix(fix);
        }
        if let Some(index) = self.socket {
            error = error
                .with_key(self.key.map_or("sockets".to_owned(), |key| format!("sockets.{key}")));
            if let Some(text) = over
                && let Some(span) = socket_span(text, index, self.key)
            {
                return Error { in_override: true, ..error.on(text, span) };
            }
            if let Some(span) = socket_span(file, index, self.key) {
                return error.on(file, span);
            }
        }
        let Some(key) = self.socket.map(|_| "sockets").or(self.key) else { return error };
        error = error.with_key(key);
        let mut sought = Some(key);
        while let Some(key) = sought {
            if let Some(text) = over
                && let Some(span) = key_span(text, key)
            {
                return Error { in_override: true, ..error.on(text, span) };
            }
            if let Some(span) = key_span(file, key) {
                return error.on(file, span);
            }
            sought = key.rsplit_once('.').map(|(table, _)| table);
        }
        error
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        match &self.fix {
            Some(fix) => write!(f, ": {fix}"),
            None => Ok(()),
        }
    }
}

impl TryFrom<File> for Service {
    type Error = String;

    fn try_from(file: File) -> Result<Service, String> {
        check(file).map_err(|problem| problem.to_string())
    }
}

/// The service a file says, when its settings fit together.
fn check(file: File) -> Result<Service, Problem> {
    let Some(Version(version)) = file.service_version else {
        // The lowest version the file can be, which more releases read.
        let lowest = if file.version_2_key().is_some() { 2 } else { 1 };
        return Err(Problem::new("it has no service_version")
            .with_fix(format!("add `service_version = {lowest}`")));
    };
    if version < 2
        && let Some(key) = file.version_2_key()
    {
        return Err(Problem::about("service_version", format!("{key} needs service_version = 2"))
            .with_fix("change the file's service_version to 2"));
    }
    let Some(Program(program)) = file.program else {
        return Err(Problem::new("it names no program")
            .with_fix("add a line such as `program = \"/system/commands/logd\"`"));
    };
    if let Some(bad) = file.arguments.iter().find(|a| a.contains('\0')) {
        return Err(Problem::about(
            "arguments",
            format!("the argument `{}` holds a NUL", bad.escape_debug()),
        ));
    }
    if let Some(account) = &file.account {
        check_account(account)
            .map_err(|why| Problem::about("account", format!("account: {why}")))?;
        if account == "root" {
            return Err(Problem::about("account", "account is root's")
                .with_fix("leave it out to act for root"));
        }
        if file.permissions.runs_with_your_access {
            return Err(Problem::about(
                "account",
                format!("it runs as {account}, so it cannot run with your access, which is root's"),
            ));
        }
    }
    file.backup.check()?;
    let requires = file.requires.unwrap_or_default();
    let wants = file.wants.unwrap_or_default();
    let after = file.after.unwrap_or_default();
    check_named_once(&requires, &wants, &after)?;
    let sockets = sockets_from(&file.sockets, Some(&file.permissions.network.listen))
        .map_err(SocketProblem::into_problem)?;
    if file.start == Start::OnConnection && sockets.is_empty() {
        return Err(Problem::about(
            "start",
            "start = \"on-connection\" waits for a connection, and the file names no socket",
        )
        .with_fix("add a [[sockets]] table"));
    }
    let timer = file.timer.as_ref().map(TimerFile::check).transpose()?;
    let logs = file.logs.as_ref().map(Logs::from);
    let health = file.health.map(Health::from_raw).transpose()?;
    let ready = match file.ready {
        Some(ready) => ready.check(&file.permissions, file.restart, health.is_some())?,
        None => Ready::default(),
    };
    Ok(Service {
        description: file.description,
        program,
        arguments: file.arguments,
        environment: file.environment.0,
        order: file.order,
        enabled: file.enabled,
        restart: file.restart,
        limits: file.limits,
        secrets: file.secrets,
        certificates: file.certificates,
        permissions: file.permissions,
        account: file.account,
        backup: file.backup,
        requires,
        wants,
        after,
        give_up_after: file.give_up_after.unwrap_or(0),
        ready,
        health,
        sockets,
        start: file.start,
        stop: file.stop.map_or(Stop::default(), StopFile::into_stop),
        groups: file.groups,
        timer,
        logs,
    })
}

fn order_default() -> u32 {
    ORDER_DEFAULT
}

fn yes() -> bool {
    true
}

pub(crate) fn weight_default() -> u32 {
    WEIGHT_DEFAULT
}

/// `service_version`: a version this reads.
struct Version(u32);

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Version, D::Error> {
        match whole(d, "service_version")? {
            1 => Ok(Version(1)),
            2 => Ok(Version(2)),
            n if n > u64::from(VERSION) => Err(de::Error::custom(format!(
                "this file is for service_version {n}, and this Dukovo reads version {VERSION} \
                 and earlier: update Dukovo to use it"
            ))),
            _ => Err(de::Error::custom(format!("service_version is 1 or {VERSION}"))),
        }
    }
}

/// The most services one of `requires`, `wants` and `after` names.
pub(crate) const LINKS_MAX: usize = 32;

/// The longest a service has to get ready, in seconds.
pub(crate) const READY_TIMEOUT_MAX: u32 = 3600;

/// The longest wait between health checks, in seconds: a day.
pub(crate) const HEALTH_EVERY_MAX: u32 = 86_400;

/// The most failed checks in a row before a restart, and the most failed
/// starts in a row before Spark gives up.
pub(crate) const HEALTH_FAILURES_MAX: u32 = 100;
pub(crate) const GIVE_UP_AFTER_MAX: u32 = 1000;

/// Checks that no service is named in two of `requires`, `wants` and
/// `after`: each says something of its own.
pub(crate) fn check_named_once(
    requires: &[String],
    wants: &[String],
    after: &[String],
) -> Result<(), Problem> {
    let lists = [("requires", requires), ("wants", wants), ("after", after)];
    for (at, (key, names)) in lists.iter().enumerate() {
        for (other_key, others) in &lists[at + 1..] {
            if let Some(name) = names.iter().find(|name| others.contains(name)) {
                return Err(Problem::about(
                    other_key,
                    format!("{name} is in {key} and {other_key}"),
                )
                .with_fix(format!("{key} says it already, so leave it out of {other_key}")));
            }
        }
    }
    Ok(())
}

impl ReadyFile {
    /// The service's readiness, checked against the rest of its file.
    fn check(
        self,
        permissions: &Permissions,
        restart: Restart,
        health: bool,
    ) -> Result<Ready, Problem> {
        if self.port.is_some() && !matches!(self.when, ReadyWord::Listening) {
            return Err(Problem::about(
                "ready.port",
                "ready.port is for ready.when = \"listening\" alone",
            ));
        }
        if !matches!(self.when, ReadyWord::Socket)
            && let Some(path) = &self.path
        {
            return Err(Problem::about(
                "ready.path",
                format!(
                    "ready.path is for ready.when = \"socket\" alone, not `{}`",
                    path.escape_debug()
                ),
            ));
        }
        let when = match self.when {
            ReadyWord::Started => ReadyWhen::Started,
            ReadyWord::Listening => {
                let listen = &permissions.network.listen;
                if listen.is_empty() {
                    return Err(Problem::about(
                        "ready.when",
                        "ready.when = \"listening\" needs the ports it listens on in \
                         network.listen",
                    ));
                }
                if let Some(port) = self.port
                    && !listen.iter().any(|ports| ports.first <= port && port <= ports.last)
                {
                    return Err(Problem::about(
                        "ready.port",
                        format!("ready.port {port} is none of network.listen's ports"),
                    ));
                }
                ReadyWhen::Listening(self.port)
            }
            ReadyWord::Socket => {
                let Some(path) = self.path else {
                    return Err(Problem::about(
                        "ready.when",
                        "ready.when = \"socket\" needs the socket's path in ready.path",
                    ));
                };
                if !path.starts_with('/') || path.contains('\0') {
                    return Err(Problem::about(
                        "ready.path",
                        format!(
                            "ready.path is an absolute path, such as \
                             /system/live/services/web/web.sock, not `{}`",
                            path.escape_debug()
                        ),
                    ));
                }
                ReadyWhen::Socket(path)
            }
            ReadyWord::Healthy if !health => {
                return Err(Problem::about(
                    "ready.when",
                    "ready.when = \"healthy\" needs a [health] check",
                ));
            }
            ReadyWord::Healthy => ReadyWhen::Healthy,
            ReadyWord::Ended if restart == Restart::Always => {
                return Err(Problem::about(
                    "ready.when",
                    "ready.when = \"ended\" is for a service that ends, so its restart is \
                     on-failure or never, not always",
                ));
            }
            ReadyWord::Ended => ReadyWhen::Ended,
        };
        Ok(Ready { when, timeout: self.timeout })
    }
}

impl Health {
    /// The health check `file` says, checked as a whole.
    fn from_raw(file: HealthFile) -> Result<Health, Problem> {
        if file.timeout > file.every {
            return Err(Problem::about(
                "health.timeout",
                format!(
                    "health.timeout is at most health.every, {} seconds, not {}",
                    file.every, file.timeout
                ),
            ));
        }
        Ok(Health {
            command: file.command,
            every: file.every,
            timeout: file.timeout,
            failures: file.failures,
        })
    }
}

/// A whole number from `low` to `high`, for `key`, which counts `unit`.
fn whole_within<'de, D: Deserializer<'de>>(
    d: D,
    key: &'static str,
    (low, high): (u32, u32),
    unit: &str,
) -> Result<u32, D::Error> {
    let n = whole(d, key)?;
    u32::try_from(n)
        .ok()
        .filter(|n| (low..=high).contains(n))
        .ok_or_else(|| de::Error::custom(format!("{key} is from {low} to {high}{unit}, not {n}")))
}

/// The names a link list holds: each a service's name, each once.
fn links<'de, D: Deserializer<'de>>(d: D, key: &str) -> Result<Option<Vec<String>>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    if names.len() > LINKS_MAX {
        return Err(de::Error::custom(format!("{key} names at most {LINKS_MAX} services")));
    }
    for (at, name) in names.iter().enumerate() {
        check_name(name).map_err(|why| de::Error::custom(format!("{key}: {why}")))?;
        if names[..at].contains(name) {
            return Err(de::Error::custom(format!("{key} names {name} twice")));
        }
    }
    Ok(Some(names))
}

fn requires<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    links(d, "requires")
}

fn wants<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    links(d, "wants")
}

fn after<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    links(d, "after")
}

fn give_up_after<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    whole_within(d, "give_up_after", (0, GIVE_UP_AFTER_MAX), " failed starts").map(Some)
}

fn ready_when<'de, D: Deserializer<'de>>(d: D) -> Result<ReadyWord, D::Error> {
    let words = [
        ReadyWord::Started,
        ReadyWord::Listening,
        ReadyWord::Socket,
        ReadyWord::Healthy,
        ReadyWord::Ended,
    ];
    Ok(words[word(d, "ready.when", ReadyWord::WORDS)?])
}

fn ready_port<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    let n = whole(d, "ready.port")?;
    match u16::try_from(n) {
        Ok(port @ 1..) => Ok(Some(port)),
        _ => Err(de::Error::custom(format!("ready.port is from 1 to 65535, not {n}"))),
    }
}

fn ready_timeout_default() -> u32 {
    READY_TIMEOUT_DEFAULT
}

fn ready_timeout<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    whole_within(d, "ready.timeout", (1, READY_TIMEOUT_MAX), " seconds")
}

/// `health.command`: a program's absolute path, then its arguments.
fn health_command<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let words = Vec::<String>::deserialize(d)?;
    match words.first() {
        None => Err(de::Error::custom(
            "health.command names a program, such as [\"/system/commands/true\"]",
        )),
        Some(program) if !program.starts_with('/') => Err(de::Error::custom(format!(
            "health.command starts with a program's absolute path, not `{}`",
            program.escape_debug()
        ))),
        Some(_) if words.iter().any(|word| word.contains('\0')) => {
            Err(de::Error::custom("health.command holds a NUL"))
        }
        Some(_) => Ok(words),
    }
}

fn health_every_default() -> u32 {
    30
}

fn health_every<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    whole_within(d, "health.every", (1, HEALTH_EVERY_MAX), " seconds")
}

fn health_timeout_default() -> u32 {
    10
}

fn health_timeout<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    whole_within(d, "health.timeout", (1, READY_TIMEOUT_MAX), " seconds")
}

fn health_failures_default() -> u32 {
    3
}

fn health_failures<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    whole_within(d, "health.failures", (1, HEALTH_FAILURES_MAX), " failed checks")
}

/// `program`: an absolute path.
struct Program(String);

impl<'de> Deserialize<'de> for Program {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Program, D::Error> {
        let path = String::deserialize(d)?;
        if !path.starts_with('/') || path.contains('\0') {
            return Err(de::Error::custom(format!(
                "program is an absolute path, such as /system/commands/logd, not `{}`",
                path.escape_debug()
            )));
        }
        Ok(Program(path))
    }
}

/// `environment`: a table of names and values, in the order of their
/// names.
#[derive(Default)]
pub(crate) struct Environment(pub(crate) Vec<(String, String)>);

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Environment, D::Error> {
        struct Table;
        impl<'de> Visitor<'de> for Table {
            type Value = Environment;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a table of names and values, such as `TZ = \"UTC\"`")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Environment, A::Error> {
                let mut variables = Vec::new();
                while let Some((Variable(name), value)) = map.next_entry::<Variable, String>()? {
                    if value.contains('\0') {
                        return Err(de::Error::custom(format!("{name}'s value holds a NUL")));
                    }
                    variables.push((name, value));
                }
                Ok(Environment(variables))
            }
        }
        d.deserialize_map(Table)
    }
}

/// A variable's name in `environment`: neither empty nor holding `=`.
struct Variable(String);

impl<'de> Deserialize<'de> for Variable {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Variable, D::Error> {
        let name = String::deserialize(d)?;
        if name.is_empty() || name.contains(['=', '\0']) {
            return Err(de::Error::custom(format!(
                "`{}` cannot be a variable's name",
                name.escape_debug()
            )));
        }
        Ok(Variable(name))
    }
}

/// A whole number, for `key`.
pub(crate) fn whole<'de, D: Deserializer<'de>>(d: D, key: &'static str) -> Result<u64, D::Error> {
    struct Whole(&'static str);
    impl Visitor<'_> for Whole {
        type Value = u64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a whole number for {}", self.0)
        }

        fn visit_u64<E: de::Error>(self, n: u64) -> Result<u64, E> {
            Ok(n)
        }

        fn visit_i64<E: de::Error>(self, n: i64) -> Result<u64, E> {
            u64::try_from(n).map_err(|_| E::custom(format!("{} is not below 0", self.0)))
        }
    }
    d.deserialize_any(Whole(key))
}

/// One of `words`, for `key`: its index.
pub(crate) fn word<'de, D: Deserializer<'de>>(
    d: D,
    key: &'static str,
    words: &'static [&'static str],
) -> Result<usize, D::Error> {
    struct Word(&'static str, &'static [&'static str]);
    impl Word {
        fn list(&self) -> String {
            match self.1 {
                [] => String::new(),
                [one] => (*one).to_owned(),
                [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
            }
        }
    }
    impl Visitor<'_> for Word {
        type Value = usize;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{} for {}", self.list(), self.0)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<usize, E> {
            self.1.iter().position(|w| *w == value).ok_or_else(|| {
                E::custom(format!("{} must be {}, not `{}`", self.0, self.list(), value))
            })
        }
    }
    d.deserialize_str(Word(key, words))
}

pub(crate) fn restart<'de, D: Deserializer<'de>>(d: D) -> Result<Restart, D::Error> {
    Ok([Restart::Always, Restart::OnFailure, Restart::Never][word(d, "restart", Restart::WORDS)?])
}

pub(crate) fn oom<'de, D: Deserializer<'de>>(d: D) -> Result<Oom, D::Error> {
    Ok([Oom::Service, Oom::App][word(d, "oom", Oom::WORDS)?])
}

pub(crate) fn weight<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    match whole(d, "cpu_weight")? {
        n @ 1.. if n <= u64::from(WEIGHT_MAX) => Ok(n as u32),
        n => Err(de::Error::custom(format!("cpu_weight is from 1 to {WEIGHT_MAX}, not {n}"))),
    }
}

/// `memory`: a size such as `"64M"`, a number of bytes, or `"none"`.
pub(crate) fn memory<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    bytes_or_none(d, "memory")
}

/// `io_read`: bytes each second, written as `memory` is.
fn io_read<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    bytes_or_none(d, "io_read")
}

/// `io_write`: bytes each second, written as `memory` is.
fn io_write<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    bytes_or_none(d, "io_write")
}

/// `io_read_ops`: requests each second, or `"none"`.
fn io_read_ops<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    per_second_or_none(d, "io_read_ops")
}

/// `io_write_ops`: requests each second, or `"none"`.
fn io_write_ops<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    per_second_or_none(d, "io_write_ops")
}

/// The size `key` takes: a size such as `"64M"`, a number of bytes, or
/// `"none"`.
fn bytes_or_none<'de, D: Deserializer<'de>>(
    d: D,
    key: &'static str,
) -> Result<Option<u64>, D::Error> {
    struct Bytes(&'static str);
    impl Visitor<'_> for Bytes {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a size such as \"64M\", or \"none\", for {}", self.0)
        }

        fn visit_u64<E: de::Error>(self, bytes: u64) -> Result<Option<u64>, E> {
            match bytes {
                0 => Err(E::custom(format!(
                    "{} is more than 0 bytes; \"none\" sets no limit",
                    self.0
                ))),
                bytes => Ok(Some(bytes)),
            }
        }

        fn visit_i64<E: de::Error>(self, bytes: i64) -> Result<Option<u64>, E> {
            let bytes = u64::try_from(bytes)
                .map_err(|_| E::custom(format!("{} is not below 0", self.0)))?;
            self.visit_u64(bytes)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Option<u64>, E> {
            match size(value) {
                Some(0) => self.visit_u64(0),
                Some(bytes) => Ok(Some(bytes)),
                None if value == "none" => Ok(None),
                None => Err(E::custom(format!(
                    "{} is a size such as \"64M\", \"512K\" or \"2G\", or \"none\", not `{value}`",
                    self.0
                ))),
            }
        }
    }
    d.deserialize_any(Bytes(key))
}

/// The number of requests each second `key` takes: a whole number of 1 or
/// more, or `"none"`.
fn per_second_or_none<'de, D: Deserializer<'de>>(
    d: D,
    key: &'static str,
) -> Result<Option<u64>, D::Error> {
    struct PerSecond(&'static str);
    impl Visitor<'_> for PerSecond {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a whole number, or \"none\", for {}", self.0)
        }

        fn visit_u64<E: de::Error>(self, n: u64) -> Result<Option<u64>, E> {
            match n {
                0 => Err(E::custom(format!(
                    "{} is 1 or more a second; \"none\" sets no limit",
                    self.0
                ))),
                n => Ok(Some(n)),
            }
        }

        fn visit_i64<E: de::Error>(self, n: i64) -> Result<Option<u64>, E> {
            let n =
                u64::try_from(n).map_err(|_| E::custom(format!("{} is not below 0", self.0)))?;
            self.visit_u64(n)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Option<u64>, E> {
            match value {
                "none" => Ok(None),
                _ => Err(E::custom(format!(
                    "{} is a whole number a second, or \"none\", not `{value}`",
                    self.0
                ))),
            }
        }
    }
    d.deserialize_any(PerSecond(key))
}

fn io_weight_default() -> u32 {
    IO_WEIGHT_DEFAULT
}

fn io_weight<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    match whole(d, "io_weight")? {
        n @ 1.. if n <= u64::from(IO_WEIGHT_MAX) => Ok(n as u32),
        n => Err(de::Error::custom(format!("io_weight is from 1 to {IO_WEIGHT_MAX}, not {n}"))),
    }
}

fn threads_default() -> u32 {
    THREADS_DEFAULT
}

fn threads<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    match whole(d, "threads")? {
        n @ 1.. if n <= u64::from(THREADS_MAX) => Ok(n as u32),
        n => Err(de::Error::custom(format!("threads is from 1 to {THREADS_MAX}, not {n}"))),
    }
}

pub(crate) fn open_files<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    match whole(d, "open_files")? {
        n @ 1.. if n <= u64::from(OPEN_FILES_MAX) => Ok(Some(n as u32)),
        n => Err(de::Error::custom(format!("open_files is from 1 to {OPEN_FILES_MAX}, not {n}"))),
    }
}

/// `files.root-subvolumes`: plain names, none twice and none of the
/// owner's ([`OWNERS_SUBVOLUMES`]), at most [`ROOT_SUBVOLUMES_MAX`].
fn subvolumes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    if names.len() > ROOT_SUBVOLUMES_MAX {
        return Err(de::Error::custom(format!(
            "files.root-subvolumes names at most {ROOT_SUBVOLUMES_MAX} subvolumes"
        )));
    }
    for (i, name) in names.iter().enumerate() {
        let plain = !name.is_empty()
            && name.len() <= SUBVOLUME_NAME_MAX
            && name != "."
            && name != ".."
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b));
        if !plain {
            return Err(de::Error::custom(format!(
                "files.root-subvolumes takes names at the top of the root, such as \"@snapshots\": \
                 letters, digits, `@`, `.`, `-` and `_`, not `{}`",
                name.escape_debug()
            )));
        }
        if OWNERS_SUBVOLUMES.contains(&name.as_str()) {
            return Err(de::Error::custom(format!(
                "files.root-subvolumes cannot name {name}, which holds the owner's files: an \
                 update or a snapshot never needs them"
            )));
        }
        if names[..i].contains(name) {
            return Err(de::Error::custom(format!("files.root-subvolumes names {name} twice")));
        }
    }
    Ok(names)
}

/// `files.read`, `files.read-write` and `files.mount`: absolute paths of
/// places beneath `/`, each written once, without `.`, `..` or an empty
/// part.
fn places<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let paths = Vec::<String>::deserialize(d)?;
    for path in &paths {
        let parts = path.strip_prefix('/').map(|p| p.split('/'));
        let fits = parts.is_some_and(|mut parts| {
            parts
                .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\0'))
        });
        if path == "/" {
            return Err(de::Error::custom(
                "files holds places beneath /, not / itself: runs-with-your-access gives a \
                 service the whole tree",
            ));
        }
        if !fits {
            return Err(de::Error::custom(format!(
                "files takes absolute paths without `.`, `..` or `//`, such as /system/state/log, \
                 not `{}`",
                path.escape_debug()
            )));
        }
    }
    Ok(paths)
}

/// `files.shared`: names of folders of `/shared`.
fn folders<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    for name in &names {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
            return Err(de::Error::custom(format!(
                "files.shared takes names of folders in /shared, such as \"media\", not `{}`",
                name.escape_debug()
            )));
        }
    }
    Ok(names)
}

/// `secrets`: names of secrets, each as a service's name is written,
/// and each once.
pub(crate) fn secret_names<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    for (i, name) in names.iter().enumerate() {
        check_secret(name).map_err(de::Error::custom)?;
        if names[..i].contains(name) {
            return Err(de::Error::custom(format!("secrets names {name} twice")));
        }
    }
    Ok(names)
}

/// `certificates`: names of certificates, each once.
pub(crate) fn certificate_names<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    for (i, name) in names.iter().enumerate() {
        check_certificate(name).map_err(de::Error::custom)?;
        if names[..i].contains(name) {
            return Err(de::Error::custom(format!("certificates names {name} twice")));
        }
    }
    Ok(names)
}

/// The longest name of a certificate.
pub const CERTIFICATE_NAME_MAX: usize = 64;

/// Checks a certificate's name, as certd keeps it, `cert` takes it and a
/// service file names it: 1 to [`CERTIFICATE_NAME_MAX`] lowercase
/// letters, digits, `-` and `.`, starting with a letter or a digit, with
/// no `..`. It is one harmless part of a path, and a domain's name fits.
pub fn check_certificate(name: &str) -> Result<(), String> {
    let fits = (1..=CERTIFICATE_NAME_MAX).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.".contains(&b))
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && !name.contains("..");
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a certificate's name: it has 1 to {CERTIFICATE_NAME_MAX} lowercase \
             letters, digits, `-` and `.`, starts with a letter or a digit, and has no `..`",
            name.escape_debug()
        ))
    }
}

/// Checks a secret's name, the name of its file in [`secrets_dir`].
pub fn check_secret(name: &str) -> Result<(), String> {
    let fits = (1..=NAME_MAX as usize).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.starts_with(['.', '-']);
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a secret's name: it has 1 to {NAME_MAX} letters, digits, `-`, `_` and \
             `.`, and starts with a letter, a digit or `_`",
            name.escape_debug()
        ))
    }
}

/// The bytes a size such as `64M`, `512K`, `2G` or `4096` says.
pub fn size(value: &str) -> Option<u64> {
    let at = value.find(|c: char| !c.is_ascii_digit()).unwrap_or(value.len());
    let (digits, unit) = value.split_at(at);
    let scale: u64 = match unit {
        "" => 1,
        "K" | "k" | "KiB" => 1 << 10,
        "M" | "m" | "MiB" => 1 << 20,
        "G" | "g" | "GiB" => 1 << 30,
        _ => return None,
    };
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

/// `network`'s table as written, before the checks of the whole.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNetwork {
    #[serde(default)]
    outbound: bool,
    #[serde(default)]
    listen: Vec<Ports>,
}

impl TryFrom<RawNetwork> for Network {
    type Error = String;

    fn try_from(raw: RawNetwork) -> Result<Network, String> {
        for (i, a) in raw.listen.iter().enumerate() {
            if let Some(b) = raw.listen[..i].iter().find(|b| a.first <= b.last && b.first <= a.last)
            {
                return Err(format!("network.listen: {a} overlaps {b}"));
            }
        }
        let network = Network { outbound: raw.outbound, listen: raw.listen };
        if network.grants() > NETWORK_SLOTS as usize {
            return Err(format!(
                "it asks for {} network capabilities, one to connect out and one for each \
                 range of ports it listens on; a program gets at most {NETWORK_SLOTS}",
                network.grants()
            ));
        }
        Ok(network)
    }
}

impl<'de> Deserialize<'de> for Ports {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Ports, D::Error> {
        struct Item;
        impl Item {
            fn port<E: de::Error>(text: &str) -> Result<u16, E> {
                match text.trim().parse::<u16>() {
                    Ok(port @ 1..) => Ok(port),
                    _ => Err(E::custom(format!(
                        "network.listen takes ports from 1 to 65535 and ranges such as \
                         \"8000-8009\", not `{text}`"
                    ))),
                }
            }
        }
        impl Visitor<'_> for Item {
            type Value = Ports;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a port, or a range such as \"8000-8009\"")
            }

            fn visit_i64<E: de::Error>(self, port: i64) -> Result<Ports, E> {
                let port = Item::port(&format!("{port}"))?;
                Ok(Ports { first: port, last: port })
            }

            fn visit_u64<E: de::Error>(self, port: u64) -> Result<Ports, E> {
                let port = Item::port(&format!("{port}"))?;
                Ok(Ports { first: port, last: port })
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Ports, E> {
                let (first, last) = match value.split_once('-') {
                    Some((first, last)) => (Item::port(first)?, Item::port(last)?),
                    None => (Item::port(value)?, Item::port(value)?),
                };
                if last < first {
                    return Err(E::custom(format!("the range \"{value}\" ends before it starts")));
                }
                Ok(Ports { first, last })
            }
        }
        d.deserialize_any(Item)
    }
}

/// The folder of the live files of the services that run as an account,
/// where their `path` sockets are made.
const LIVE_SERVICES: &str = "/system/live/services/";

/// The longest account name, which names the folder of a service's live
/// files.
const ACCOUNT_NAME_MAX: usize = 32;

/// The longest `path` of a socket: its whole path in a service's live
/// folder is at most as long as a socket's name may be
/// (radix-file(2)'s SOCKET_NAME_MAX).
pub const UNIX_NAME_MAX: usize =
    radix_abi::file::SOCKET_NAME_MAX as usize - LIVE_SERVICES.len() - ACCOUNT_NAME_MAX - 1;

/// The longest `name` of a socket.
pub const SOCKET_WORD_MAX: usize = 64;

/// The most groups a file names.
pub const GROUPS_MAX: usize = 16;

/// A `[[sockets]]` table as written, in a service file or a bundle's
/// manifest: each key alone is checked, the whole by [`sockets_from`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocketFile {
    /// `port`: a TCP or UDP port.
    #[serde(default, deserialize_with = "socket_port")]
    pub port: Option<u16>,
    /// `address`: the address a port socket is on.
    #[serde(default, deserialize_with = "socket_address")]
    pub address: Option<SocketAddress>,
    /// `udp`: a datagram socket, for a port.
    pub udp: Option<bool>,
    /// `path`: the plain name of a Unix socket in the live folder.
    #[serde(default, deserialize_with = "socket_path")]
    pub path: Option<String>,
    /// `name`: its word in `LISTEN_FDNAMES`.
    #[serde(default, deserialize_with = "socket_name")]
    pub name: Option<String>,
    /// `backlog`: how many connections may wait.
    #[serde(default, deserialize_with = "socket_backlog")]
    pub backlog: Option<u32>,
    /// `non-blocking`: passed with `O_NONBLOCK` set.
    #[serde(rename = "non-blocking")]
    pub non_blocking: Option<bool>,
    /// `to`: who connects to a `path` socket besides the service, in a
    /// service file alone: `anyone`, or the `system`'s programs.
    #[serde(default, deserialize_with = "socket_to")]
    pub to: Option<SocketAccess>,
    /// `group`: the group whose members connect to a `path` socket, in a
    /// service file alone.
    #[serde(default, deserialize_with = "socket_group")]
    pub group: Option<String>,
}

impl SocketFile {
    /// Whether it says who connects, which a bundle's manifest leaves to
    /// its `provides`.
    pub fn says_who_connects(&self) -> bool {
        self.to.is_some() || self.group.is_some()
    }
}

/// What is wrong with a socket that read: which table, the key in it
/// the problem is about, and what is wrong.
pub(crate) struct SocketProblem {
    /// The place of the table among the file's sockets, from 0.
    pub(crate) index: usize,
    /// The key to put the error on, or none for the whole table.
    pub(crate) key: Option<&'static str>,
    /// What is wrong, in plain words.
    pub(crate) message: String,
    /// What to do about it.
    pub(crate) fix: Option<String>,
}

impl SocketProblem {
    fn into_problem(self) -> Problem {
        let problem =
            Problem { key: self.key, socket: Some(self.index), ..Problem::new(self.message) };
        match self.fix {
            Some(fix) => problem.with_fix(fix),
            None => problem,
        }
    }
}

/// The sockets the tables say, each checked alone and against the
/// others. `listen` is the ports the file may listen on, which a port
/// socket must be one of; `None` for a file that has none to check
/// against.
pub(crate) fn sockets_from(
    files: &[SocketFile],
    listen: Option<&[Ports]>,
) -> Result<Vec<Socket>, SocketProblem> {
    let mut sockets: Vec<Socket> = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let problem = |key, message: String| SocketProblem { index, key, message, fix: None };
        let kind = match (file.port, &file.path) {
            (Some(_), Some(_)) => {
                return Err(problem(
                    Some("path"),
                    "a socket has a port or a path, not both".into(),
                ));
            }
            (None, None) => {
                return Err(problem(None, "a socket needs a port or a path".into()));
            }
            (Some(port), None) => {
                // The ports of network.listen are for both protocols: the
                // capability that holds one binds TCP and UDP alike.
                let allowed = listen.is_none_or(|ports| {
                    ports.iter().any(|ports| ports.first <= port && port <= ports.last)
                });
                if !allowed {
                    return Err(SocketProblem {
                        fix: Some(format!("add {port} to network.listen")),
                        ..problem(
                            Some("port"),
                            format!("port {port} is none of network.listen's ports"),
                        )
                    });
                }
                let address = file.address.unwrap_or_default();
                if file.udp == Some(true) {
                    SocketKind::Datagram { port, address }
                } else {
                    SocketKind::Stream { port, address }
                }
            }
            (None, Some(path)) => {
                if file.to.is_some() && file.group.is_some() {
                    return Err(SocketProblem {
                        fix: Some("keep one of them".into()),
                        ..problem(
                            Some("group"),
                            "a socket has a `to` or a `group`, not both".into(),
                        )
                    });
                }
                if file.address.is_some() {
                    return Err(problem(
                        Some("address"),
                        "address is for a socket with a port alone".into(),
                    ));
                }
                if file.udp.is_some() {
                    return Err(problem(
                        Some("udp"),
                        "udp is for a socket with a port alone".into(),
                    ));
                }
                SocketKind::Unix { path: path.clone() }
            }
        };
        let name = file.name.clone().unwrap_or_else(|| match &kind {
            SocketKind::Stream { port, .. } | SocketKind::Datagram { port, .. } => port.to_string(),
            SocketKind::Unix { path } => path.clone(),
        });
        if sockets.iter().any(|other| other.name == name) {
            return Err(SocketProblem {
                fix: Some("give one of them a `name`".into()),
                ..problem(Some("name"), format!("two sockets are named {name}"))
            });
        }
        let access = match (&kind, &file.to, &file.group) {
            (SocketKind::Unix { .. }, Some(to), _) => to.clone(),
            (SocketKind::Unix { .. }, None, Some(group)) => SocketAccess::Group(group.clone()),
            (SocketKind::Unix { .. }, None, None) => SocketAccess::Own,
            (_, None, None) => SocketAccess::Own,
            (_, to, _) => {
                let key = if to.is_some() { "to" } else { "group" };
                return Err(SocketProblem {
                    fix: Some("the firewall and network.listen say who reaches a port".into()),
                    ..problem(Some(key), format!("{key} is for a socket with a path alone"))
                });
            }
        };
        let backlog = file.backlog.unwrap_or(BACKLOG_DEFAULT);
        let non_blocking = file.non_blocking.unwrap_or(false);
        sockets.push(Socket { kind, name, backlog, non_blocking, access });
    }
    Ok(sockets)
}

/// The span of `key` in the table `index` of the array `sockets` of the
/// file `text`, or of the whole table when `key` is none or the table
/// does not set it.
fn socket_span(text: &str, index: usize, key: Option<&str>) -> Option<Range<usize>> {
    let root = DeTable::parse(text).ok()?;
    let sockets = root.get_ref().get("sockets")?;
    let table = sockets.get_ref().as_array()?.get(index)?;
    // A key the table leaves at its default has the table for its place.
    let named = key.and_then(|key| table.get_ref().as_table()?.get_key_value(key));
    Some(named.map_or(table.span(), |(name, _)| name.span()))
}

/// `sockets`: tables, at most [`SOCKETS_MAX`].
fn socket_files<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<SocketFile>, D::Error> {
    let files = Vec::<SocketFile>::deserialize(d)?;
    if files.len() > SOCKETS_MAX {
        return Err(de::Error::custom(format!("a file names at most {SOCKETS_MAX} sockets")));
    }
    Ok(files)
}

fn socket_port<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    let n = whole(d, "port")?;
    match u16::try_from(n) {
        Ok(port @ 1..) => Ok(Some(port)),
        _ => Err(de::Error::custom(format!("a socket's port is from 1 to 65535, not {n}"))),
    }
}

/// A socket's `address`: `"0.0.0.0"`, `"::"` or one address.
fn socket_address<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SocketAddress>, D::Error> {
    let text = String::deserialize(d)?;
    if let Ok(address) = text.parse::<core::net::Ipv4Addr>() {
        return Ok(Some(if address.is_unspecified() {
            SocketAddress::AnyV4
        } else {
            SocketAddress::V4(address.octets())
        }));
    }
    if let Ok(address) = text.parse::<core::net::Ipv6Addr>() {
        return Ok(Some(if address.is_unspecified() {
            SocketAddress::AnyV6
        } else {
            SocketAddress::V6(address.octets())
        }));
    }
    Err(de::Error::custom(format!(
        "a socket's address is \"0.0.0.0\", \"::\" or one IPv4 or IPv6 address, not `{}`; \
         leave it out for every address",
        text.escape_debug()
    )))
}

/// Whether `word` is made of letters, digits, `_`, `-` and `.`, which
/// holds no `:`, the separator of `LISTEN_FDNAMES`.
fn is_socket_word(word: &str) -> bool {
    word.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

/// A socket's `path`: a plain name.
fn socket_path<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let path = String::deserialize(d)?;
    let plain = !path.is_empty()
        && path.len() <= UNIX_NAME_MAX
        && path != "."
        && path != ".."
        && is_socket_word(&path);
    if !plain {
        return Err(de::Error::custom(format!(
            "a socket's path is a plain name of 1 to {UNIX_NAME_MAX} letters, digits, `_`, `-` \
             and `.`, made in the service's live folder, not `{}`",
            path.escape_debug()
        )));
    }
    Ok(Some(path))
}

/// A socket's `name`: a word for `LISTEN_FDNAMES`.
fn socket_name<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let name = String::deserialize(d)?;
    if name.is_empty() || name.len() > SOCKET_WORD_MAX || !is_socket_word(&name) {
        return Err(de::Error::custom(format!(
            "a socket's name is 1 to {SOCKET_WORD_MAX} letters, digits, `_`, `-` and `.`, with no \
             `:`, which separates the names in LISTEN_FDNAMES, not `{}`",
            name.escape_debug()
        )));
    }
    Ok(Some(name))
}

fn socket_backlog<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    whole_within(d, "a socket's backlog", (1, 65535), " connections").map(Some)
}

/// A socket's `to`: `anyone` or `system`; a group is named by `group`.
fn socket_to<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SocketAccess>, D::Error> {
    let text = String::deserialize(d)?;
    match text.as_str() {
        "anyone" => Ok(Some(SocketAccess::Anyone)),
        "system" => Ok(Some(SocketAccess::System)),
        _ => Err(de::Error::custom(format!(
            "a socket's to is \"anyone\" or \"system\", not `{}`; `group` names a group whose \
             members connect",
            text.escape_debug()
        ))),
    }
}

/// A socket's `group`: a group's name, as an account's is written.
fn socket_group<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let name = String::deserialize(d)?;
    check_account(&name).map_err(|why| de::Error::custom(format!("a socket's group: {why}")))?;
    Ok(Some(name))
}

fn start<'de, D: Deserializer<'de>>(d: D) -> Result<Start, D::Error> {
    Ok([Start::AtBoot, Start::OnConnection][word(d, "start", Start::WORDS)?])
}

/// `groups`: names of groups, each as an account's name is written, at
/// most [`GROUPS_MAX`], each once.
pub(crate) fn group_names<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let names = Vec::<String>::deserialize(d)?;
    if names.len() > GROUPS_MAX {
        return Err(de::Error::custom(format!("groups names at most {GROUPS_MAX} groups")));
    }
    for (i, name) in names.iter().enumerate() {
        check_account(name).map_err(|why| de::Error::custom(format!("groups: {why}")))?;
        if names[..i].contains(name) {
            return Err(de::Error::custom(format!("groups names {name} twice")));
        }
    }
    Ok(names)
}

/// `[stop]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopFile {
    #[serde(default, deserialize_with = "stop_signal")]
    signal: Signal,
    #[serde(default = "stop_timeout_default", deserialize_with = "stop_timeout")]
    timeout: u32,
}

impl StopFile {
    fn into_stop(self) -> Stop {
        Stop { signal: self.signal, timeout: self.timeout }
    }
}

fn stop_timeout_default() -> u32 {
    STOP_TIMEOUT_DEFAULT
}

pub(crate) fn stop_timeout<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    whole_within(d, "stop.timeout", (1, STOP_TIMEOUT_MAX), " seconds")
}

/// `stop.signal` of a bundle's manifest, which may be left out.
#[cfg(feature = "bundle")]
pub(crate) fn stop_signal_optional<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<Signal>, D::Error> {
    stop_signal(d).map(Some)
}

/// `stop.signal`: one of the signals a daemon takes as a request to stop.
pub(crate) fn stop_signal<'de, D: Deserializer<'de>>(d: D) -> Result<Signal, D::Error> {
    let name = String::deserialize(d)?;
    match name.as_str() {
        "SIGHUP" => Err(de::Error::custom(
            "stop.signal is not SIGHUP, which `service reload` sends to a service to read its \
             settings again",
        )),
        "SIGKILL" | "SIGSTOP" => Err(de::Error::custom(format!(
            "stop.signal is not {name}, which a program cannot take as a request: Spark ends \
             what is left of a service after stop.timeout"
        ))),
        _ => Signal::WORDS
            .iter()
            .position(|word| *word == name)
            .map(|at| {
                [
                    Signal::Term,
                    Signal::Int,
                    Signal::Quit,
                    Signal::Hup,
                    Signal::Usr1,
                    Signal::Usr2,
                    Signal::Winch,
                ][at]
            })
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "stop.signal must be SIGTERM, SIGINT, SIGQUIT, SIGUSR1, SIGUSR2 or SIGWINCH, \
                     not `{}`",
                    name.escape_debug()
                ))
            }),
    }
}

/// The most times of day a `[timer]` names in `at`.
pub const TIMER_AT_MAX: usize = 32;

/// The most patterns `[logs]` names in `files`.
pub const LOG_FILES_MAX: usize = 16;

/// The most turned-over files `[logs]` keeps.
pub const LOG_KEEP_MAX: u32 = 100;

/// How many turned-over files `[logs]` keeps when its file says none.
pub const LOG_KEEP_DEFAULT: u32 = 14;

/// A service file's `[timer]` as written (service.toml(5)), and a bundle
/// manifest's `timer` (manifest.json(5)): the same keys, with the times
/// and durations as the text that was read, which [`TimerFile::check`]
/// turns into a [`Timer`].
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimerFile {
    /// Times of day: crontab lines' five fields, or `hourly`, `daily`,
    /// `weekly`, `monthly`, `yearly`.
    #[serde(default, deserialize_with = "timer_at")]
    pub at: Vec<String>,
    /// A duration, such as `"6h"`: so long after the last start.
    #[serde(default, deserialize_with = "timer_every")]
    pub every: Option<String>,
    /// A duration: the first start by `every` after the machine starts.
    #[serde(default, deserialize_with = "timer_after_boot")]
    pub after_boot: Option<String>,
    /// A duration: the most a time is put off by.
    #[serde(default, deserialize_with = "timer_spread")]
    pub spread: Option<String>,
    /// Whether a time missed while the machine was off runs once; true
    /// when left out.
    #[serde(default)]
    pub catch_up: Option<bool>,
    /// A tzdata name such as `"UTC"`; the system's when left out.
    #[serde(default, deserialize_with = "timer_zone")]
    pub time_zone: Option<String>,
}

impl TimerFile {
    /// The timer it says, when its keys fit together.
    ///
    /// # Errors
    ///
    /// What is wrong, with the key it is about.
    pub(crate) fn check(&self) -> Result<Timer, Problem> {
        if self.at.is_empty() && self.every.is_none() {
            return Err(Problem::about("timer", "a timer needs `at` or `every`").with_fix(
                "add a line such as `timer.at = [\"daily\"]` or `timer.every = \"6h\"`",
            ));
        }
        if self.after_boot.is_some() && self.every.is_none() {
            return Err(Problem::about("timer.after_boot", "after_boot goes with `every`")
                .with_fix("add `timer.every`, or leave after_boot out"));
        }
        let at = self
            .at
            .iter()
            .map(|item| {
                crate::schedule::read(item).map_err(|(_, why)| Problem::about("timer.at", why))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let seconds =
            |text: &Option<String>| text.as_deref().and_then(|t| crate::schedule::duration(t).ok());
        let mut written = self.at.clone();
        written.extend(self.every.iter().map(|every| format!("every {every}")));
        written.extend(self.after_boot.iter().map(|boot| format!("first {boot} after boot")));
        Ok(Timer {
            at,
            every: seconds(&self.every),
            after_boot: seconds(&self.after_boot),
            spread: seconds(&self.spread).unwrap_or(0),
            catch_up: self.catch_up.unwrap_or(true),
            time_zone: self.time_zone.clone(),
            schedule: written.join(", "),
        })
    }
}

fn timer_at<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let items = Vec::<String>::deserialize(d)?;
    if items.len() > TIMER_AT_MAX {
        return Err(de::Error::custom(format!("timer.at names at most {TIMER_AT_MAX} times")));
    }
    for (at, item) in items.iter().enumerate() {
        if item.starts_with('@') {
            return Err(de::Error::custom(format!(
                "timer.at writes `{}` without the @",
                item.escape_debug()
            )));
        }
        if let Err((column, why)) = crate::schedule::read(item) {
            return Err(de::Error::custom(format!(
                "timer.at item {} `{}`, at column {}: {why}",
                at + 1,
                item.escape_debug(),
                column + 1
            )));
        }
    }
    Ok(items)
}

/// A duration for the key `key`.
fn timer_duration<'de, D: Deserializer<'de>>(d: D, key: &str) -> Result<Option<String>, D::Error> {
    let text = String::deserialize(d)?;
    crate::schedule::duration(&text).map_err(|why| de::Error::custom(format!("{key}: {why}")))?;
    Ok(Some(text))
}

fn timer_every<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    timer_duration(d, "timer.every")
}

fn timer_after_boot<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    timer_duration(d, "timer.after_boot")
}

fn timer_spread<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    timer_duration(d, "timer.spread")
}

/// Checks that `name` has the shape of a tzdata name, such as `UTC` or
/// `Europe/Berlin`: letters, digits and `_+-.` in parts that are not
/// empty, `.` or `..`, and no leading `/`, so that it cannot name a file
/// outside tzdata.
///
/// # Errors
///
/// What is wrong, in plain words.
pub fn check_time_zone(name: &str) -> Result<(), String> {
    let part_fits = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_+-.".contains(&b))
    };
    if name.len() <= 64 && name.split('/').all(part_fits) {
        return Ok(());
    }
    Err(format!(
        "`{}` is not a time zone's name: use one of tzdata's, such as \"UTC\" or \
         \"Europe/Berlin\"",
        name.escape_debug()
    ))
}

fn timer_zone<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let name = String::deserialize(d)?;
    check_time_zone(&name).map_err(|why| de::Error::custom(format!("timer.time_zone: {why}")))?;
    Ok(Some(name))
}

/// A service file's `[logs]` as written, and a bundle manifest's `logs`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LogsFile {
    /// Patterns of the files; `*` only in the last part, which ends in
    /// text of its own.
    #[serde(deserialize_with = "log_files")]
    pub files: Vec<String>,
    /// `"daily"`, `"weekly"`, or a size such as `"100M"`; daily when left
    /// out.
    #[serde(default = "turn_over_default", deserialize_with = "log_turn_over")]
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub turn_over: TurnOver,
    /// How many turned-over files are kept, 1 to 100; 14 when left out.
    #[serde(default = "log_keep_default", deserialize_with = "log_keep")]
    pub keep: u32,
    /// Whether turned-over files are compressed; true when left out.
    #[serde(default = "yes")]
    pub compress: bool,
    /// A signal such as `"SIGUSR1"`, `"restart"`, or `"none"`; none when
    /// left out.
    #[serde(default = "reopen_default", deserialize_with = "log_reopen")]
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub reopen: Reopen,
}

impl From<&LogsFile> for Logs {
    fn from(file: &LogsFile) -> Logs {
        Logs {
            files: file.files.clone(),
            turn_over: file.turn_over,
            keep: file.keep,
            compress: file.compress,
            reopen: file.reopen,
        }
    }
}

impl TurnOver {
    /// What a file says: `daily`, `weekly`, or the size in bytes.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            TurnOver::Daily => "daily".to_string(),
            TurnOver::Weekly => "weekly".to_string(),
            TurnOver::Size(bytes) => bytes.to_string(),
        }
    }
}

impl Reopen {
    /// What a file says: a signal's name, `restart` or `none`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Reopen::Signal(signal) => signal.name(),
            Reopen::Restart => "restart",
            Reopen::Nothing => "none",
        }
    }
}

fn turn_over_default() -> TurnOver {
    TurnOver::Daily
}

fn log_keep_default() -> u32 {
    LOG_KEEP_DEFAULT
}

fn reopen_default() -> Reopen {
    Reopen::Nothing
}

/// Checks a log file pattern: relative to the service's data folder or
/// absolute, parts that are plain names, and a `*` only in the last part,
/// which ends in text of its own so that a pattern never takes the files
/// logturn(8) has turned over.
///
/// # Errors
///
/// What is wrong, in plain words.
pub fn check_log_pattern(pattern: &str) -> Result<(), String> {
    let wrong = |why: &str| Err(format!("logs.files: `{}` {why}", pattern.escape_debug()));
    let rest = pattern.strip_prefix('/').unwrap_or(pattern);
    let parts: Vec<&str> = rest.split('/').collect();
    let last = parts.len() - 1;
    for (at, part) in parts.iter().enumerate() {
        if part.is_empty() || *part == "." || *part == ".." || part.contains('\0') {
            return wrong("has an empty part, `.`, `..` or a NUL");
        }
        if part.contains(['?', '[', ']', '{', '}']) {
            return wrong("holds a glob character other than `*`");
        }
        if part.contains('*') && at != last {
            return wrong("has a `*` before its last part");
        }
    }
    if parts[last].ends_with('*') {
        return wrong(
            "ends in `*`, which would take the files turned over already; end it in text such as `.log`",
        );
    }
    Ok(())
}

fn log_files<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let files = Vec::<String>::deserialize(d)?;
    if files.is_empty() || files.len() > LOG_FILES_MAX {
        return Err(de::Error::custom(format!(
            "logs.files names 1 to {LOG_FILES_MAX} files, such as [\"logs/*.log\"]"
        )));
    }
    for (at, pattern) in files.iter().enumerate() {
        check_log_pattern(pattern).map_err(de::Error::custom)?;
        if files[..at].contains(pattern) {
            return Err(de::Error::custom(format!("logs.files names {pattern} twice")));
        }
    }
    Ok(files)
}

fn log_turn_over<'de, D: Deserializer<'de>>(d: D) -> Result<TurnOver, D::Error> {
    struct Words;
    impl Visitor<'_> for Words {
        type Value = TurnOver;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("\"daily\", \"weekly\" or a size such as \"100M\" for logs.turn_over")
        }

        fn visit_u64<E: de::Error>(self, bytes: u64) -> Result<TurnOver, E> {
            match bytes {
                0 => Err(E::custom("logs.turn_over is more than 0 bytes")),
                bytes => Ok(TurnOver::Size(bytes)),
            }
        }

        fn visit_i64<E: de::Error>(self, bytes: i64) -> Result<TurnOver, E> {
            let bytes =
                u64::try_from(bytes).map_err(|_| E::custom("logs.turn_over is not below 0"))?;
            self.visit_u64(bytes)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<TurnOver, E> {
            match (value, size(value)) {
                ("daily", _) => Ok(TurnOver::Daily),
                ("weekly", _) => Ok(TurnOver::Weekly),
                (_, Some(bytes)) => self.visit_u64(bytes),
                _ => Err(E::custom(format!(
                    "logs.turn_over is \"daily\", \"weekly\" or a size such as \"100M\", not \
                     `{}`",
                    value.escape_debug()
                ))),
            }
        }
    }
    d.deserialize_any(Words)
}

fn log_keep<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    whole_within(d, "logs.keep", (1, LOG_KEEP_MAX), " files")
}

fn log_reopen<'de, D: Deserializer<'de>>(d: D) -> Result<Reopen, D::Error> {
    let text = String::deserialize(d)?;
    match text.as_str() {
        "restart" => return Ok(Reopen::Restart),
        "none" => return Ok(Reopen::Nothing),
        _ => {}
    }
    let all = [
        Signal::Term,
        Signal::Int,
        Signal::Quit,
        Signal::Hup,
        Signal::Usr1,
        Signal::Usr2,
        Signal::Winch,
    ];
    match Signal::WORDS.iter().position(|word| *word == text) {
        // These end most programs: a program that wants to open its
        // files again is told with another.
        Some(0..=2) => Err(de::Error::custom(format!(
            "logs.reopen is not {text}, which ends most programs: use a signal such as SIGHUP, \
             SIGUSR1 or SIGUSR2, \"restart\" or \"none\""
        ))),
        Some(at) => Ok(Reopen::Signal(all[at])),
        None => Err(de::Error::custom(format!(
            "logs.reopen is a signal such as SIGUSR1, \"restart\" or \"none\", not `{}`",
            text.escape_debug()
        ))),
    }
}
