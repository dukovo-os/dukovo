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
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use radix_abi::resource_domain::{NAME_MAX, WEIGHT_DEFAULT, WEIGHT_MAX};
use radix_abi::startup::NETWORK_SLOTS;
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

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
    "secrets",
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
    "names",
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
];

/// The keys of service_version 2 (ADR 0470), which a file of version 1
/// may not use.
const VERSION_2_KEYS: &[&str] = &["requires", "wants", "after", "give_up_after", "ready", "health"];

/// The order of a service whose file gives none.
pub const ORDER_DEFAULT: u32 = 50;

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
    fn check(&self) -> Result<(), String> {
        for (name, command) in [("before", &self.before), ("after", &self.after)] {
            if let Some(program) = command.first()
                && !program.starts_with('/')
            {
                return Err(format!(
                    "backup.{name} starts with a program's absolute path, not `{program}`"
                ));
            }
            if command.iter().any(|word| word.contains('\0')) {
                return Err(format!("backup.{name} holds a NUL"));
            }
        }
        if !(1..=HOOK_TIME_MAX).contains(&self.time_limit) {
            return Err(format!("backup.time_limit is from 1 to {HOOK_TIME_MAX} seconds"));
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
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { cpu_weight: WEIGHT_DEFAULT, memory: None, oom: Oom::Service }
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
/// `admin.time` and `admin.firewall`.
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
}

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
/// system's programs, libraries, resources and settings, the processes'
/// and the kernel's files, read-only; the devices every program uses;
/// notifyd's folder, as notifications are always allowed; and resolved's,
/// whose resolv.conf `/etc/resolv.conf` leads to (ADR 0193), and netd's,
/// which an `/etc/resolv.conf` from before resolved leads to (ADR 0191).
/// Its program,
/// its own `/tmp`, the root's links and its secrets come on top. A place
/// a file names that is one of these takes its place.
pub const ALWAYS: &[(&str, bool)] = &[
    ("/system/commands", false),
    ("/system/libraries", false),
    ("/system/resources", false),
    ("/system/settings", false),
    ("/system/live/notify", false),
    ("/system/live/resolved", false),
    ("/system/live/netd", false),
    ("/processes", false),
    ("/kernel", false),
    ("/devices/null", true),
    ("/devices/zero", false),
    ("/devices/random", false),
    ("/devices/urandom", false),
];

/// Where Spark keeps its own files: no service holds them, or what is
/// in them, the secrets among them.
pub const SPARK_STATE: &str = "/system/state/spark";

/// Where Spark keeps the secrets of service `name`, and where the
/// service finds them, as `SECRETS_DIRECTORY` says.
pub fn secrets_dir(name: &str) -> String {
    format!("{SPARK_STATE}/secrets/{name}")
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
        if permissions.runs_with_your_access && !places.is_empty() {
            return Err("runs-with-your-access holds every file already: leave out files.*".into());
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
# The secrets Spark hands it as read-only files, in the folder
# SECRETS_DIRECTORY names, never in its environment; set each with
# `service secret set NAME SECRET`.
secrets = []

# More of its environment, as NAME = "value".
[environment]

# What its domain may use: a CPU weight from 1 to 10000, where 100 is
# everyone's; a memory limit such as "64M", or "none"; and "service" or
# "app", which the out-of-memory policy stops first.
[limits]
cpu_weight = 100
memory = "none"
oom = "service"

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
# Giving processes and files another user's name: only for authd, which
# signs people in.
names = false
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

/// Reads a service file.
pub fn from_toml(text: &str) -> Result<Service, Error> {
    toml::from_str(text).map_err(|err| Error::from_toml(text, &err))
}

/// The folder, beside the service files, of the files that change them
/// (ADR 0386): `settings.d/NAME.toml` sets keys of `NAME.toml`, so that
/// an administrator's changes never touch the file the system or a
/// bundle wrote, which an update may write again.
pub const OVERRIDES: &str = "settings.d";

/// An override file, read: the keys it sets.
#[derive(Clone, Debug, PartialEq)]
pub struct Override(toml::Table);

impl Override {
    /// Reads an override file: any keys and tables, checked only once
    /// they are set over a service file.
    pub fn from_toml(text: &str) -> Result<Override, Error> {
        toml::from_str(text).map(Override).map_err(|err| Error::from_toml(text, &err))
    }
}

/// Sets each key of `over` in `table`: a table's keys one by one, any
/// other value in place of the one there.
fn set_over(table: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (table.get_mut(&key), value) {
            (Some(toml::Value::Table(inner)), toml::Value::Table(more)) => set_over(inner, more),
            (_, value) => {
                table.insert(key, value);
            }
        }
    }
}

/// The keys whose values differ between the service files `a` and `b`,
/// as dotted paths, such as `limits.memory`, in order: what an update's
/// new default beside an administrator's file changes (ADR 0386).
pub fn changed_keys(a: &str, b: &str) -> Result<Vec<String>, Error> {
    fn walk(prefix: &str, a: &toml::Table, b: &toml::Table, out: &mut Vec<String>) {
        let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
        keys.sort();
        keys.dedup();
        for key in keys {
            let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
            match (a.get(key), b.get(key)) {
                (Some(toml::Value::Table(x)), Some(toml::Value::Table(y))) => {
                    walk(&path, x, y, out);
                }
                (x, y) if x != y => out.push(path),
                _ => {}
            }
        }
    }
    let a: toml::Table = toml::from_str(a).map_err(|err| Error::from_toml(a, &err))?;
    let b: toml::Table = toml::from_str(b).map_err(|err| Error::from_toml(b, &err))?;
    let mut out = Vec::new();
    walk("", &a, &b, &mut out);
    Ok(out)
}

/// Reads a service file with its override, if it has one, set over it.
pub fn from_toml_over(text: &str, over: Option<&Override>) -> Result<Service, Error> {
    let Some(Override(over)) = over else { return from_toml(text) };
    let mut table: toml::Table =
        toml::from_str(text).map_err(|err| Error::from_toml(text, &err))?;
    set_over(&mut table, over.clone());
    table.try_into().map_err(|err: toml::de::Error| Error::new(crate::plain(err.message())))
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
        ];
        VERSION_2_KEYS.iter().zip(set).find_map(|(key, set)| set.then_some(*key))
    }
}

impl TryFrom<File> for Service {
    type Error = String;

    fn try_from(file: File) -> Result<Service, String> {
        let Some(Version(version)) = file.service_version else {
            // The lowest version the file can be, which more releases read.
            let lowest = if file.version_2_key().is_some() { 2 } else { 1 };
            return Err(format!("it has no service_version: add `service_version = {lowest}`"));
        };
        if version < 2
            && let Some(key) = file.version_2_key()
        {
            return Err(format!(
                "{key} needs service_version = 2: change the file's service_version to 2"
            ));
        }
        let Some(Program(program)) = file.program else {
            return Err("it names no program: add a line such as \
                        `program = \"/system/commands/logd\"`"
                .into());
        };
        if let Some(bad) = file.arguments.iter().find(|a| a.contains('\0')) {
            return Err(format!("the argument `{}` holds a NUL", bad.escape_debug()));
        }
        if let Some(account) = &file.account {
            check_account(account).map_err(|why| format!("account: {why}"))?;
            if account == "root" {
                return Err("account is root's: leave it out to act for root".into());
            }
            if file.permissions.runs_with_your_access {
                return Err(format!(
                    "it runs as {account}, so it cannot run with your access, which is root's"
                ));
            }
        }
        file.backup.check()?;
        let requires = file.requires.unwrap_or_default();
        let wants = file.wants.unwrap_or_default();
        let after = file.after.unwrap_or_default();
        check_named_once(&requires, &wants, &after)?;
        let health = file.health.map(Health::try_from).transpose()?;
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
            permissions: file.permissions,
            account: file.account,
            backup: file.backup,
            requires,
            wants,
            after,
            give_up_after: file.give_up_after.unwrap_or(0),
            ready,
            health,
        })
    }
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
) -> Result<(), String> {
    let lists = [("requires", requires), ("wants", wants), ("after", after)];
    for (at, (key, names)) in lists.iter().enumerate() {
        for (other_key, others) in &lists[at + 1..] {
            if let Some(name) = names.iter().find(|name| others.contains(name)) {
                return Err(format!(
                    "{name} is in {key} and {other_key}: {key} says it already, so leave it out \
                     of {other_key}"
                ));
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
    ) -> Result<Ready, String> {
        if self.port.is_some() && !matches!(self.when, ReadyWord::Listening) {
            return Err("ready.port is for ready.when = \"listening\" alone".into());
        }
        if !matches!(self.when, ReadyWord::Socket)
            && let Some(path) = &self.path
        {
            return Err(format!(
                "ready.path is for ready.when = \"socket\" alone, not `{}`",
                path.escape_debug()
            ));
        }
        let when = match self.when {
            ReadyWord::Started => ReadyWhen::Started,
            ReadyWord::Listening => {
                let listen = &permissions.network.listen;
                if listen.is_empty() {
                    return Err("ready.when = \"listening\" needs the ports it listens on in \
                                network.listen"
                        .into());
                }
                if let Some(port) = self.port
                    && !listen.iter().any(|ports| ports.first <= port && port <= ports.last)
                {
                    return Err(format!("ready.port {port} is none of network.listen's ports"));
                }
                ReadyWhen::Listening(self.port)
            }
            ReadyWord::Socket => {
                let Some(path) = self.path else {
                    return Err(
                        "ready.when = \"socket\" needs the socket's path in ready.path".into()
                    );
                };
                if !path.starts_with('/') || path.contains('\0') {
                    return Err(format!(
                        "ready.path is an absolute path, such as \
                         /system/live/services/web/web.sock, not `{}`",
                        path.escape_debug()
                    ));
                }
                ReadyWhen::Socket(path)
            }
            ReadyWord::Healthy if !health => {
                return Err("ready.when = \"healthy\" needs a [health] check".into());
            }
            ReadyWord::Healthy => ReadyWhen::Healthy,
            ReadyWord::Ended if restart == Restart::Always => {
                return Err("ready.when = \"ended\" is for a service that ends, so its restart \
                            is on-failure or never, not always"
                    .into());
            }
            ReadyWord::Ended => ReadyWhen::Ended,
        };
        Ok(Ready { when, timeout: self.timeout })
    }
}

impl TryFrom<HealthFile> for Health {
    type Error = String;

    fn try_from(file: HealthFile) -> Result<Health, String> {
        if file.timeout > file.every {
            return Err(format!(
                "health.timeout is at most health.every, {} seconds, not {}",
                file.every, file.timeout
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
    struct Memory;
    impl Visitor<'_> for Memory {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a size such as \"64M\", or \"none\", for memory")
        }

        fn visit_u64<E: de::Error>(self, bytes: u64) -> Result<Option<u64>, E> {
            match bytes {
                0 => Err(E::custom("memory is more than 0 bytes; \"none\" sets no limit")),
                bytes => Ok(Some(bytes)),
            }
        }

        fn visit_i64<E: de::Error>(self, bytes: i64) -> Result<Option<u64>, E> {
            let bytes = u64::try_from(bytes).map_err(|_| E::custom("memory is not below 0"))?;
            self.visit_u64(bytes)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Option<u64>, E> {
            match size(value) {
                Some(0) => self.visit_u64(0),
                Some(bytes) => Ok(Some(bytes)),
                None if value == "none" => Ok(None),
                None => Err(E::custom(format!(
                    "memory is a size such as \"64M\", \"512K\" or \"2G\", or \"none\", not `{value}`"
                ))),
            }
        }
    }
    d.deserialize_any(Memory)
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
