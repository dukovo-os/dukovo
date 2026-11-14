// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admin about` and `admin notices` (admin(8)): what the console's
//! Overview shows of this server, and the notices the consoles show at
//! sign-in. They are the commands of the console's `console.about` and
//! `console.notices` actions, which run them with `--json` as the person
//! who is signed in; at a terminal they print text.
//!
//! Both run as the person, with no rights of their own. Each thing is read
//! from where the machine keeps it, and what the person cannot read, or
//! the machine does not have, is `null` in the JSON and a line left out
//! of the text: a missing piece is never an error. Only a mistake in the
//! command line fails.
//!
//! `admin about` says the machine's name; the addresses of its cards and
//! the port of the console (admin.toml(5)); the fingerprint of the
//! console's certificate, as `admin fingerprint` says it; the release, and
//! whether this start is a fallback to the other slot; the maker and model
//! the firmware names; the processor; the memory the kernel counts, in
//! bytes; the seconds the machine has been up; the time zone, and whether
//! timed has synchronized the clock; the fingerprint of the SSH host key;
//! when the last good start began, from the start records (ADR 0500),
//! which are the person's to read only where the folder lets them; and the
//! person's own last two sign-ins, from their record (ADR 0254), which
//! only they and root may read.
//!
//! `admin notices` lists what is kept for the next sign-in (ADR 0504), one
//! notice each, by its name, with its lines and when it was written: first
//! a failed start's (ADR 0501), then each file of `/system/live/sign-in`.
//!
//! Every place is under a root that the tests set to a folder of their
//! own, as the machine's is `/`.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use accounts::signins::{self, Event};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use serde_json::json;

use crate::admin_settings::{self, Settings};
use crate::admind::ISSUE;
use crate::args::{Arg, Args};
use crate::sign_in_notices::{self, Entry};
use crate::sysupdate::{health, starts};
use crate::util::{self, Fail};
use crate::{ip, proc, sshkeys, timed};

/// The most of a small file that is read: what it holds is a line or a
/// few.
const SMALL: u64 = 64 * 1024;

/// How many of the person's sign-ins are told, the newest first.
const SIGN_INS: usize = 2;

/// Where the machine keeps what `admin about` reads: every path under a
/// root, which is `/` on the machine.
#[derive(Clone, Debug)]
pub struct Sources {
    root: PathBuf,
}

impl Sources {
    /// The machine's own places.
    pub fn machine() -> Sources {
        Sources { root: PathBuf::from("/") }
    }

    /// The places under `root`, a folder with the machine's layout.
    #[cfg(test)]
    pub fn under(root: &Path) -> Sources {
        Sources { root: root.to_path_buf() }
    }

    /// The machine's `path`, here.
    fn at(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        self.root.join(path.strip_prefix("/").unwrap_or(path))
    }

    /// What the file at `path` holds, as text, if the person can read it.
    fn read(&self, path: impl AsRef<Path>) -> Option<String> {
        let mut text = String::new();
        File::open(self.at(path)).ok()?.take(SMALL).read_to_string(&mut text).ok()?;
        Some(text)
    }
}

/// What does not come from a file: the machine's name, its time zone, the
/// processor's name, and who runs the command.
#[derive(Clone, Debug)]
pub struct Live {
    /// The machine's name.
    pub host: String,
    /// The time zone, as `timedatectl` says it.
    pub zone: String,
    /// The processor, as its CPUID names it.
    pub processor: Option<String>,
    /// The person's number, whose sign-in record is read.
    pub uid: u32,
}

impl Live {
    /// What this machine says now, to the person who runs the command.
    fn machine() -> Live {
        Live {
            host: util::hostname(),
            zone: crate::timedatectl::timezone(),
            processor: crate::lscpu::model_name(&crate::lscpu::cpuid)
                .map(|brand| crate::dukofetch::cpu_name(&brand)),
            uid: util::own_user(),
        }
    }
}

/// The maker and model the firmware names.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Machine {
    /// `sys_vendor`.
    pub maker: Option<String>,
    /// `product_name`.
    pub model: Option<String>,
}

/// What timed says of the clock.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Clock {
    /// Whether timed has synchronized it: false also when timed has said
    /// nothing.
    pub synchronized: bool,
}

/// A sign-in of the person's.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SignIn {
    /// When, in seconds since 1970.
    pub at: i64,
    /// The terminal, as `pts/0`.
    pub terminal: String,
    /// Where from, an address; null at the computer itself.
    pub from: Option<String>,
}

/// What `admin about` says: the JSON of `console.about`, in this order.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct About {
    /// The machine's name.
    pub host: String,
    /// The addresses of its cards, as text, the loopback's, the link-local
    /// ones and those still being tested left out.
    pub addresses: Vec<String>,
    /// The console's port, from admin.toml; null when it does not read.
    pub port: Option<u16>,
    /// The certificate's SHA-256 fingerprint, from the issue file admind
    /// keeps; null while the console is not running.
    pub fingerprint: Option<String>,
    /// The release, `Dukovo Server 0.16.0`.
    pub release: Option<String>,
    /// Whether this start is a fallback to the slot that was running.
    pub fallback: bool,
    /// The firmware's maker and model.
    pub machine: Machine,
    /// The processor's name.
    pub processor: Option<String>,
    /// The memory the kernel counts, in bytes.
    pub memory: Option<u64>,
    /// The seconds since the machine started.
    pub up: Option<u64>,
    /// The time zone.
    pub zone: String,
    /// The clock.
    pub clock: Clock,
    /// The SSH host key's fingerprint, `SHA256:...`.
    pub ssh_host_key: Option<String>,
    /// When the last good start began, in seconds since 1970; null when
    /// the records cannot be read or none was good.
    pub last_good_start: Option<i64>,
    /// The person's newest sign-ins, the newest first, at most two; null
    /// when their record cannot be read.
    pub sign_ins: Option<Vec<SignIn>>,
}

/// What this machine says of itself to the person `live` names.
pub fn about(sources: &Sources, live: &Live) -> About {
    let (fingerprint, issue_port) = issue(sources.read(ISSUE).as_deref());
    let boot = slots::cmdline::parse(&sources.read("/kernel/cmdline").unwrap_or_default());
    let memory = sources
        .read(format!("{}/meminfo", proc::PROC))
        .map(|text| proc::parse_memory(text.as_bytes()).total * 1024)
        .filter(|&bytes| bytes > 0);
    About {
        host: live.host.clone(),
        addresses: addresses(sources),
        port: Settings::read_at(&sources.at(admin_settings::PATH))
            .ok()
            .map(|settings| settings.console.listen)
            .or(issue_port),
        fingerprint,
        release: sources.read(crate::dukofetch::VERSION).as_deref().and_then(release),
        fallback: boot.fallback.is_some(),
        machine: Machine { maker: dmi(sources, "sys_vendor"), model: dmi(sources, "product_name") },
        processor: live.processor.clone(),
        memory,
        up: sources.read(format!("{}/uptime", proc::PROC)).and_then(|text| uptime(&text)),
        zone: live.zone.clone(),
        clock: Clock {
            synchronized: sources
                .read(timed::STATUS)
                .is_some_and(|text| text.starts_with("Synchronized: yes")),
        },
        ssh_host_key: host_key(sources),
        last_good_start: last_good_start(sources),
        sign_ins: sign_ins(sources, live.uid),
    }
}

/// The fingerprint and the port the issue file's two lines say:
/// `https://nas:9090` and `SHA-256 fingerprint AB:CD:...`.
fn issue(text: Option<&str>) -> (Option<String>, Option<u16>) {
    let Some(text) = text else { return (None, None) };
    let fingerprint = text
        .lines()
        .find_map(|line| line.strip_prefix("SHA-256 fingerprint "))
        .map(|fingerprint| fingerprint.trim().to_string())
        .filter(|fingerprint| !fingerprint.is_empty());
    let port = text
        .lines()
        .next()
        .and_then(|address| address.trim().rsplit_once(':'))
        .and_then(|(_, port)| port.parse().ok());
    (fingerprint, port)
}

/// The release in `/kernel/version`'s `Radix 0.12.0 (Dukovo Server 0.12.0)`:
/// `Dukovo Server 0.12.0`.
fn release(text: &str) -> Option<String> {
    let (_, (name, version)) = crate::dukofetch::parse_version(text)?;
    Some(format!("{name} {version}"))
}

/// A field of the firmware's SMBIOS table the kernel shows, if it has a
/// value.
fn dmi(sources: &Sources, name: &str) -> Option<String> {
    let text = sources.read(Path::new(crate::hwinfo::DMI).join(name))?;
    Some(text.trim().to_string()).filter(|value| !value.is_empty())
}

/// The seconds up that `/processes/uptime` says in `uptime_ns`.
fn uptime(text: &str) -> Option<u64> {
    let line = text.lines().find_map(|line| line.strip_prefix("uptime_ns:"))?;
    Some(line.trim().parse::<u64>().ok()? / 1_000_000_000)
}

/// The addresses of the cards that reach beyond their own network: IPv4
/// ones, and IPv6 ones that are not link-local or still being tested, each
/// without its prefix, in the order of the cards.
fn addresses(sources: &Sources) -> Vec<String> {
    let cards = ip::read_interfaces(&sources.at(ip::INTERFACES), &sources.at(ip::ROUTES))
        .unwrap_or_default();
    let mut found = Vec::new();
    for card in cards.iter().filter(|card| !card.loopback) {
        found.extend(card.addresses.iter().map(|address| address.ip.to_string()));
        found.extend(
            card.ipv6
                .addresses
                .iter()
                .filter(|address| !address.tentative && !ip::link_local(address.cidr.ip))
                .map(|address| address.cidr.ip.to_string()),
        );
    }
    found
}

/// The fingerprint of the SSH host key, from the public keys sshd keeps in
/// `/system/state/sshd`: the first, by file name, that reads.
fn host_key(sources: &Sources) -> Option<String> {
    let folder = sources.at("/system/state/sshd");
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| Path::new(name).extension().is_some_and(|extension| extension == "pub"))
        .collect();
    names.sort();
    names.iter().find_map(|name| {
        let text = sources.read(Path::new("/system/state/sshd").join(name))?;
        let blob = text.split_ascii_whitespace().nth(1)?;
        let key = STANDARD.decode(blob).ok().filter(|key| !key.is_empty())?;
        Some(sshkeys::fingerprint(&key))
    })
}

/// When the newest good start before this one began, from the records the
/// person can read.
fn last_good_start(sources: &Sources) -> Option<i64> {
    let records = starts::all_in(&sources.at(starts::DIR));
    let this = starts::this_start_in(&sources.at(starts::LIVE)).unwrap_or(u64::MAX);
    starts::last_good(&records, this).map(|start| start.started)
}

/// The person's newest sign-ins, if their record reads.
fn sign_ins(sources: &Sources, uid: u32) -> Option<Vec<SignIn>> {
    let text = sources.read(signins::of(uid))?;
    let mut found: Vec<SignIn> = signins::parse(&text)
        .into_iter()
        .filter_map(|record| match record.event {
            Event::In(seat) => Some(SignIn {
                at: record.time,
                terminal: seat.terminal,
                from: Some(seat.from).filter(|from| !from.is_empty()),
            }),
            _ => None,
        })
        .collect();
    found.sort_by_key(|sign_in| std::cmp::Reverse(sign_in.at));
    found.truncate(SIGN_INS);
    Some(found)
}

impl About {
    /// The text a terminal shows: a line for each thing the machine said,
    /// as `Label  value`. `time` writes a time in seconds since 1970 as the
    /// person reads it.
    pub fn text(&self, time: &dyn Fn(i64) -> String) -> String {
        let mut rows: Vec<(&str, String)> = vec![("Name", self.host.clone())];
        if !self.addresses.is_empty() {
            rows.push(("Addresses", self.addresses.join(", ")));
        }
        if let Some(port) = self.port {
            rows.push(("Console port", port.to_string()));
        }
        if let Some(fingerprint) = &self.fingerprint {
            rows.push(("Fingerprint", format!("SHA-256 {fingerprint}")));
        }
        if let Some(release) = &self.release {
            let fallback = if self.fallback { " (started as a fallback)" } else { "" };
            rows.push(("Release", format!("{release}{fallback}")));
        }
        let machine: Vec<&str> = [&self.machine.maker, &self.machine.model]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if !machine.is_empty() {
            rows.push(("Machine", machine.join(" ")));
        }
        if let Some(processor) = &self.processor {
            rows.push(("Processor", processor.clone()));
        }
        if let Some(bytes) = self.memory {
            rows.push(("Memory", gibibytes(bytes)));
        }
        if let Some(seconds) = self.up {
            rows.push(("Up", duration(seconds)));
        }
        let clock = if self.clock.synchronized { "synchronized" } else { "not synchronized" };
        rows.push(("Time zone", format!("{}, clock {clock}", self.zone)));
        if let Some(key) = &self.ssh_host_key {
            rows.push(("SSH host key", key.clone()));
        }
        if let Some(at) = self.last_good_start {
            rows.push(("Last good start", time(at)));
        }
        for (at, sign_in) in self.sign_ins.iter().flatten().enumerate() {
            let label = if at == 0 { "Last sign-in" } else { "Before that" };
            let from =
                sign_in.from.as_deref().map_or(String::new(), |from| format!(" from {from}"));
            rows.push((label, format!("{} on {}{from}", time(sign_in.at), sign_in.terminal)));
        }
        let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
        rows.iter().fold(String::new(), |mut text, (label, value)| {
            let _ = writeln!(text, "{label:<width$}  {value}");
            text
        })
    }
}

/// `bytes` as `7.8 GiB`.
fn gibibytes(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

/// `seconds` as `3 days 4 hours 5 minutes`: the days, hours and minutes
/// that are not none, and `less than a minute` for none of them.
fn duration(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds % 86_400 / 3600, seconds % 3600 / 60);
    let unit = |number: u64, name: &str| match number {
        0 => None,
        1 => Some(format!("1 {name}")),
        _ => Some(format!("{number} {name}s")),
    };
    let parts: Vec<String> = [unit(days, "day"), unit(hours, "hour"), unit(minutes, "minute")]
        .into_iter()
        .flatten()
        .collect();
    if parts.is_empty() { "less than a minute".into() } else { parts.join(" ") }
}

/// One notice, as `admin notices` tells it.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Notice {
    /// The notice's name: `failed-start`, or the name of its file in
    /// `/system/live/sign-in`.
    pub name: String,
    /// Its lines, as the consoles show them.
    pub lines: Vec<String>,
    /// When it was written, in seconds since 1970; null when the machine
    /// does not say.
    pub at: Option<i64>,
}

/// The name of the failed start's notice.
pub const FAILED_START: &str = "failed-start";

/// What is kept for the next sign-in: the failed start's notice, then the
/// others by their names.
pub fn notices(sources: &Sources) -> Vec<Notice> {
    let notice = |entry: Entry| Notice { name: entry.name, lines: entry.lines, at: entry.at };
    let failed = sign_in_notices::entry_at(&sources.at(health::NOTICE), FAILED_START);
    let kept = sign_in_notices::entries_in(&sources.at(sign_in_notices::DIR));
    failed.into_iter().chain(kept).map(notice).collect()
}

/// The text a terminal shows of `notices`: each with its name and when it
/// was written, and its lines under it, indented; `No notices.` when there
/// are none.
pub fn notices_text(notices: &[Notice], time: &dyn Fn(i64) -> String) -> String {
    if notices.is_empty() {
        return "No notices.\n".into();
    }
    let mut text = String::new();
    for notice in notices {
        match notice.at {
            Some(at) => {
                let _ = writeln!(text, "{} ({})", notice.name, time(at));
            }
            None => {
                let _ = writeln!(text, "{}", notice.name);
            }
        }
        for line in &notice.lines {
            let _ = writeln!(text, "  {line}");
        }
    }
    text
}

/// `admin about [--json]` and `admin notices [--json]`.
pub fn main(args: &mut Args, which: Which) -> Result<i32, Fail> {
    let mut as_json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => as_json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let sources = Sources::machine();
    let local = |at: i64| util::time_text(at, "%F %T %Z", false);
    match which {
        Which::About => {
            let about = about(&sources, &Live::machine());
            if as_json {
                println!("{}", json!(about));
            } else {
                print!("{}", about.text(&local));
            }
        }
        Which::Notices => {
            let found = notices(&sources);
            if as_json {
                println!("{}", json!({ "notices": found }));
            } else {
                print!("{}", notices_text(&found, &local));
            }
        }
    }
    Ok(0)
}

/// Which of the two commands runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// `admin about`.
    About,
    /// `admin notices`.
    Notices,
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use serde_json::{Value, json};

    use super::*;

    /// A folder with the machine's layout, which a test fills in.
    struct Machine {
        root: PathBuf,
    }

    impl Machine {
        fn new(what: &str) -> Machine {
            let root =
                std::env::temp_dir().join(format!("admin-about-{what}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Machine { root }
        }

        fn sources(&self) -> Sources {
            Sources::under(&self.root)
        }

        /// Writes `text` to the machine's `path`.
        fn put(&self, path: &str, text: &str) {
            let file = self.root.join(path.trim_start_matches('/'));
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }

        /// Makes the machine's `path` a folder, which no one can read as a
        /// file.
        fn unreadable(&self, path: &str) {
            fs::create_dir_all(self.root.join(path.trim_start_matches('/'))).unwrap();
        }

        /// Everything `admin about` reads, as a machine that says it all.
        fn full(&self) -> Live {
            self.put(
                "system/live/admind/issue",
                "https://nas:9443\nSHA-256 fingerprint AB:CD:EF\n",
            );
            self.put("system/settings/admin.toml", "[console]\nlisten = 9443\n");
            self.put("kernel/version", "Radix 0.12.0 (Dukovo Server 0.12.0)\n");
            self.put("kernel/cmdline", "root=UUID=x slot=a fallback=b quiet\n");
            self.put("kernel/class/dmi/id/sys_vendor", "Dell Inc.\n");
            self.put("kernel/class/dmi/id/product_name", "OptiPlex 7040\n");
            self.put("processes/meminfo", "total: 8388608 kB\nfree: 100 kB\n");
            self.put("processes/uptime", "uptime_ns: 273900000000000\n");
            self.put("system/live/timed/status", "Synchronized: yes\nServer: x\n");
            self.card("lo", 1, "772", "127.0.0.1/8\n", "");
            self.card(
                "net0",
                2,
                "1",
                "192.168.1.20/24\n10.0.0.5/8\n",
                "address fe80::1/64 link-local preferred forever valid forever\n\
                 address fd00::5/64 stable preferred 100 valid 200\n\
                 address 2001:db8::9/64 given preferred forever valid forever tentative\n",
            );
            let key = STANDARD.encode(b"a public key's bytes");
            self.put(
                "system/state/sshd/ssh_host_ed25519_key.pub",
                &format!("ssh-ed25519 {key} root@nas\n"),
            );
            self.put("system/state/sshd/ssh_host_ed25519_key", "never read\n");
            for (number, outcome, started) in
                [(1, "good", 100), (2, "good", 200), (3, "failed", 300), (4, "unfinished", 400)]
            {
                self.put(
                    &format!("system/state/starts/{number}.json"),
                    &json!({
                        "number": number, "started": started, "slot": "a", "stage": "services",
                        "outcome": outcome, "items": [],
                    })
                    .to_string(),
                );
            }
            self.put("system/live/sysupdate/start", "4\n");
            self.put(
                "system/state/sign-ins/1000",
                "100 boot 0.12.0\n\
                 150 in alice pts/0 - 61\n\
                 200 out alice pts/0 - 61\n\
                 250 failed alice pts/1 10.0.0.9\n\
                 300 in alice pts/1 192.168.1.25 62\n\
                 350 in alice pts/2 - 63\n",
            );
            Live {
                host: "nas".into(),
                zone: "Europe/Prague".into(),
                processor: Some("Intel Core i5-6500".into()),
                uid: 1000,
            }
        }

        /// A card, with the files `ip` reads.
        fn card(&self, name: &str, index: u32, kind: &str, ipv4: &str, ipv6: &str) {
            let base = format!("kernel/class/net/{name}");
            self.put(&format!("{base}/ifindex"), &format!("{index}\n"));
            self.put(&format!("{base}/type"), &format!("{kind}\n"));
            self.put(&format!("{base}/ipv4"), ipv4);
            self.put(&format!("{base}/ipv6"), ipv6);
        }
    }

    impl Drop for Machine {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn nothing() -> Live {
        Live { host: "nas".into(), zone: "UTC".into(), processor: None, uid: 1000 }
    }

    fn the_key() -> String {
        sshkeys::fingerprint(b"a public key's bytes")
    }

    #[test]
    fn a_machine_that_says_everything_is_told_whole() {
        let machine = Machine::new("full");
        let live = machine.full();
        let about = about(&machine.sources(), &live);
        assert_eq!(
            json!(about),
            json!({
                "host": "nas",
                "addresses": ["192.168.1.20", "10.0.0.5", "fd00::5"],
                "port": 9443,
                "fingerprint": "AB:CD:EF",
                "release": "Dukovo Server 0.12.0",
                "fallback": true,
                "machine": { "maker": "Dell Inc.", "model": "OptiPlex 7040" },
                "processor": "Intel Core i5-6500",
                "memory": 8_589_934_592u64,
                "up": 273_900,
                "zone": "Europe/Prague",
                "clock": { "synchronized": true },
                "ssh_host_key": the_key(),
                "last_good_start": 200,
                "sign_ins": [
                    { "at": 350, "terminal": "pts/2", "from": null },
                    { "at": 300, "terminal": "pts/1", "from": "192.168.1.25" },
                ],
            })
        );
        // The text has the keys in the order the design lists them.
        let text = serde_json::to_string(&about).unwrap();
        let at: Vec<usize> = [
            "host",
            "addresses",
            "port",
            "fingerprint",
            "release",
            "fallback",
            "machine",
            "processor",
            "memory",
            "up",
            "zone",
            "clock",
            "ssh_host_key",
            "last_good_start",
            "sign_ins",
        ]
        .iter()
        .map(|key| text.find(&format!("\"{key}\":")).unwrap())
        .collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    }

    #[test]
    fn what_the_machine_does_not_say_is_null_and_nothing_fails() {
        let machine = Machine::new("empty");
        let about = about(&machine.sources(), &nothing());
        assert_eq!(
            json!(about),
            json!({
                "host": "nas",
                "addresses": [],
                // admin.toml is not there: the console's own default.
                "port": 9090,
                "fingerprint": null,
                "release": null,
                "fallback": false,
                "machine": { "maker": null, "model": null },
                "processor": null,
                "memory": null,
                "up": null,
                "zone": "UTC",
                "clock": { "synchronized": false },
                "ssh_host_key": null,
                "last_good_start": null,
                "sign_ins": null,
            })
        );
    }

    #[test]
    fn what_the_person_cannot_read_is_null_where_the_rest_is_still_told() {
        let machine = Machine::new("unreadable");
        let live = machine.full();
        // Each of these, read as a file, fails, as a file with no rights
        // for the person does.
        for path in [
            "system/state/sign-ins/1000",
            "system/state/starts",
            "kernel/version",
            "kernel/class/dmi/id/sys_vendor",
            "processes/meminfo",
            "system/live/admind/issue",
        ] {
            let file = machine.root.join(path);
            if file.is_dir() {
                fs::remove_dir_all(&file).unwrap();
            } else {
                fs::remove_file(&file).unwrap();
            }
            machine.unreadable(path);
        }
        let about = about(&machine.sources(), &live);
        assert_eq!(about.sign_ins, None);
        assert_eq!(about.last_good_start, None);
        assert_eq!(about.release, None);
        assert_eq!(about.machine.maker, None);
        assert_eq!(about.memory, None);
        assert_eq!(about.fingerprint, None);
        // What the person can read is told: the port is admin.toml's, not
        // the issue file's, and the rest is as it was.
        assert_eq!(about.port, Some(9443));
        assert_eq!(about.machine.model.as_deref(), Some("OptiPlex 7040"));
        assert_eq!(about.up, Some(273_900));
        assert!(about.fallback);
    }

    #[test]
    fn the_issue_file_gives_the_fingerprint_and_the_port() {
        assert_eq!(
            issue(Some("https://nas:9443\nSHA-256 fingerprint AB:CD\n")),
            (Some("AB:CD".into()), Some(9443))
        );
        assert_eq!(issue(Some("https://nas\n")), (None, None));
        assert_eq!(issue(Some("https://nas:https\nSHA-256 fingerprint \n")), (None, None));
        assert_eq!(issue(Some("")), (None, None));
        assert_eq!(issue(None), (None, None));
    }

    #[test]
    fn the_port_is_admin_toml_s_and_the_issue_file_s_when_it_does_not_read() {
        let machine = Machine::new("port");
        machine.put("system/live/admind/issue", "https://nas:9443\nSHA-256 fingerprint AB\n");
        machine.put("system/settings/admin.toml", "[console]\nlisten = 443\n");
        assert_eq!(about(&machine.sources(), &nothing()).port, Some(443));
        machine.put("system/settings/admin.toml", "[console]\nlisten = 80\n");
        assert_eq!(about(&machine.sources(), &nothing()).port, Some(9443));
    }

    #[test]
    fn the_release_is_the_name_and_version_the_kernel_gives() {
        assert_eq!(
            release("Radix 0.12.0 (Dukovo Server 0.12.0)\n").as_deref(),
            Some("Dukovo Server 0.12.0")
        );
        for text in ["", "Radix 0.12.0", "Radix (Dukovo)\n"] {
            assert_eq!(release(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_start_is_a_fallback_when_the_command_line_names_the_other_slot() {
        let machine = Machine::new("fallback");
        let fallback = |line: &str| {
            machine.put("kernel/cmdline", line);
            about(&machine.sources(), &nothing()).fallback
        };
        assert!(fallback("slot=a fallback=b\n"));
        assert!(fallback("slot=b fallback=a quiet\n"));
        assert!(!fallback("slot=a\n"));
        assert!(!fallback("slot=a fallback=a\n"));
        assert!(!fallback("slot=a -- fallback=b\n"));
        assert!(!fallback("\n"));
    }

    #[test]
    fn the_seconds_up_and_the_memory_come_from_what_the_kernel_counts() {
        assert_eq!(uptime("uptime_ns: 90000000000\n"), Some(90));
        assert_eq!(uptime("uptime_ns: 999999999\n"), Some(0));
        for text in ["", "uptime_ns: x\n", "idle_ns: 5\n"] {
            assert_eq!(uptime(text), None, "{text:?}");
        }
        let machine = Machine::new("memory");
        machine.put("processes/meminfo", "total: 0\n");
        assert_eq!(about(&machine.sources(), &nothing()).memory, None);
        machine.put("processes/meminfo", "total: 1024\n");
        assert_eq!(about(&machine.sources(), &nothing()).memory, Some(1024 * 1024));
    }

    #[test]
    fn the_clock_is_synchronized_when_timed_says_so() {
        let machine = Machine::new("clock");
        let synchronized = |text: &str| {
            machine.put("system/live/timed/status", text);
            about(&machine.sources(), &nothing()).clock.synchronized
        };
        assert!(synchronized("Synchronized: yes\n"));
        assert!(!synchronized("Synchronized: no\n"));
        assert!(!synchronized(""));
    }

    #[test]
    fn a_card_s_addresses_leave_out_the_loopback_and_the_ones_that_do_not_reach_far() {
        let machine = Machine::new("addresses");
        machine.card(
            "lo",
            1,
            "772",
            "127.0.0.1/8\n",
            "address ::1/128 given preferred forever valid forever\n",
        );
        machine.card("net0", 2, "1", "", "");
        assert_eq!(addresses(&machine.sources()), Vec::<String>::new());
        machine.card(
            "net1",
            3,
            "1",
            "10.1.1.1/24\n",
            "address fe80::2/64 link-local preferred forever valid forever\n",
        );
        machine.card(
            "net0",
            2,
            "1",
            "172.16.0.2/16\n",
            "address fd00::7/64 stable preferred 1 valid 2\n",
        );
        assert_eq!(addresses(&machine.sources()), ["172.16.0.2", "fd00::7", "10.1.1.1"]);
    }

    #[test]
    fn the_ssh_host_key_is_the_first_public_key_that_reads() {
        let machine = Machine::new("hostkey");
        let host_key = || host_key(&machine.sources());
        assert_eq!(host_key(), None);
        machine.put("system/state/sshd/ssh_host_ed25519_key", "private, never read\n");
        assert_eq!(host_key(), None);
        // One that does not read is passed over for the next by name.
        machine.put("system/state/sshd/a.pub", "ssh-ed25519\n");
        machine.put("system/state/sshd/b.pub", "ssh-ed25519 !!!not-base64 x\n");
        machine.put("system/state/sshd/c.pub", "ssh-ed25519  x\n");
        assert_eq!(host_key(), None);
        let key = STANDARD.encode(b"second key");
        machine.put("system/state/sshd/d.pub", &format!("ssh-rsa {key} comment\n"));
        machine.put(
            "system/state/sshd/e.pub",
            &format!("ssh-ed25519 {} x\n", STANDARD.encode(b"third")),
        );
        assert_eq!(host_key(), Some(sshkeys::fingerprint(b"second key")));
        assert!(host_key().unwrap().starts_with("SHA256:"));
    }

    #[test]
    fn the_last_good_start_is_the_newest_good_one_before_this_start() {
        let machine = Machine::new("starts");
        let _ = machine.full();
        let last = || last_good_start(&machine.sources());
        assert_eq!(last(), Some(200));
        // Without a note of which start this is, the newest good one counts.
        fs::remove_file(machine.root.join("system/live/sysupdate/start")).unwrap();
        assert_eq!(last(), Some(200));
        // This start is the second: the first is the one before it.
        machine.put("system/live/sysupdate/start", "2\n");
        assert_eq!(last(), Some(100));
        machine.put("system/live/sysupdate/start", "1\n");
        assert_eq!(last(), None);
        // A record that does not read is passed over.
        machine.put("system/state/starts/9.json", "not json");
        machine.put("system/live/sysupdate/start", "4\n");
        assert_eq!(last(), Some(200));
    }

    #[test]
    fn the_sign_ins_are_the_person_s_newest_two_and_a_new_account_has_none() {
        let machine = Machine::new("signins");
        let signed = || sign_ins(&machine.sources(), 1000);
        assert_eq!(signed(), None);
        machine.put("system/state/sign-ins/1000", "");
        assert_eq!(signed(), Some(Vec::new()));
        machine
            .put("system/state/sign-ins/1000", "100 boot 0.12.0\n250 failed bob pts/1 10.0.0.9\n");
        assert_eq!(signed(), Some(Vec::new()));
        machine.put("system/state/sign-ins/1000", "150 in alice pts/0 - 61\n");
        assert_eq!(signed(), Some(vec![SignIn { at: 150, terminal: "pts/0".into(), from: None }]));
        // The lines are in the order they were written, which is the order
        // of the time unless a clock was set back: the newest come first.
        machine.put(
            "system/state/sign-ins/1000",
            "300 in alice pts/1 192.168.1.25 62\n150 in alice pts/0 - 61\n200 in alice web 10.0.0.9 63\n",
        );
        let found = signed().unwrap();
        assert_eq!(found.iter().map(|sign_in| sign_in.at).collect::<Vec<_>>(), [300, 200]);
        assert_eq!(found[1].terminal, "web");
        // Another account's record is not read.
        assert_eq!(sign_ins(&machine.sources(), 1001), None);
    }

    fn time(at: i64) -> String {
        format!("T{at}")
    }

    #[test]
    fn the_text_has_a_line_for_each_thing_the_machine_said() {
        let machine = Machine::new("text");
        let live = machine.full();
        let text = about(&machine.sources(), &live).text(&time);
        let expected = format!(
            "\
Name             nas
Addresses        192.168.1.20, 10.0.0.5, fd00::5
Console port     9443
Fingerprint      SHA-256 AB:CD:EF
Release          Dukovo Server 0.12.0 (started as a fallback)
Machine          Dell Inc. OptiPlex 7040
Processor        Intel Core i5-6500
Memory           8.0 GiB
Up               3 days 4 hours 5 minutes
Time zone        Europe/Prague, clock synchronized
SSH host key     {}
Last good start  T200
Last sign-in     T350 on pts/2
Before that      T300 on pts/1 from 192.168.1.25
",
            the_key()
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn the_text_of_a_machine_that_says_little_has_only_what_it_said() {
        let machine = Machine::new("little");
        let text = about(&machine.sources(), &nothing()).text(&time);
        assert_eq!(
            text,
            "Name          nas\nConsole port  9090\nTime zone     UTC, clock not synchronized\n"
        );
    }

    #[test]
    fn a_time_up_is_told_in_days_hours_and_minutes_that_are_not_none() {
        assert_eq!(duration(0), "less than a minute");
        assert_eq!(duration(59), "less than a minute");
        assert_eq!(duration(60), "1 minute");
        assert_eq!(duration(3600), "1 hour");
        assert_eq!(duration(86_400 + 3600 + 120), "1 day 1 hour 2 minutes");
        assert_eq!(duration(2 * 86_400 + 59), "2 days");
        assert_eq!(gibibytes(1 << 30), "1.0 GiB");
        assert_eq!(gibibytes(3 << 29), "1.5 GiB");
    }

    #[test]
    fn the_notices_are_the_failed_start_s_and_then_each_one_kept() {
        let machine = Machine::new("notices");
        assert_eq!(notices(&machine.sources()), Vec::new());
        machine.put("system/live/sign-in/backup", "The last backup failed.\n");
        machine.put("system/live/sign-in/bundled", "App X has a known flaw.\nUpdate it.\n");
        machine.put(
            "system/live/sysupdate/failure",
            "Start 7 failed (FW-101).\nThe previous version runs.\n",
        );
        let found = notices(&machine.sources());
        let names: Vec<&str> = found.iter().map(|notice| notice.name.as_str()).collect();
        assert_eq!(names, ["failed-start", "backup", "bundled"]);
        assert_eq!(found[0].lines, ["Start 7 failed (FW-101).", "The previous version runs."]);
        assert_eq!(found[2].lines, ["App X has a known flaw.", "Update it."]);
        assert!(found.iter().all(|notice| notice.at.is_some_and(|at| at > 1_700_000_000)));
        let body = json!({ "notices": found });
        assert_eq!(body["notices"][1]["name"], "backup");
        assert_eq!(body["notices"][1]["lines"], json!(["The last backup failed."]));
        assert!(body["notices"][1]["at"].is_i64());
        // A failed start's notice that says nothing, and a notice that
        // cannot be read, are not told.
        machine.put("system/live/sysupdate/failure", "\n");
        machine.unreadable("system/live/sign-in/odd");
        let names: Vec<String> = notices(&machine.sources()).into_iter().map(|n| n.name).collect();
        assert_eq!(names, ["backup", "bundled"]);
    }

    #[test]
    fn the_text_of_the_notices_names_each_with_its_time_and_says_when_there_are_none() {
        assert_eq!(notices_text(&[], &time), "No notices.\n");
        let found = [
            Notice {
                name: "backup".into(),
                lines: vec!["One.".into(), "Two.".into()],
                at: Some(5),
            },
            Notice { name: "odd".into(), lines: vec!["Three.".into()], at: None },
        ];
        assert_eq!(notices_text(&found, &time), "backup (T5)\n  One.\n  Two.\nodd\n  Three.\n");
    }

    fn args(items: &[&str]) -> Args {
        Args::new("admin", items.iter().map(OsString::from).collect())
    }

    #[test]
    fn both_commands_take_json_and_nothing_else_and_never_fail_on_a_bare_machine() {
        for which in [Which::About, Which::Notices] {
            assert_eq!(main(&mut args(&[]), which).unwrap(), 0);
            assert_eq!(main(&mut args(&["--json"]), which).unwrap(), 0);
            for bad in [["--yaml"], ["now"], ["-j"]] {
                let fail = main(&mut args(&bad), which).unwrap_err();
                assert!(fail.usage, "{bad:?}");
            }
        }
    }

    #[test]
    fn the_json_is_one_value() {
        let machine = Machine::new("json");
        let live = machine.full();
        let text = json!(about(&machine.sources(), &live)).to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["clock"], json!({ "synchronized": true }));
        assert_eq!(parsed["machine"]["maker"], "Dell Inc.");
    }
}
