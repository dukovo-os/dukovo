// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `sysupdate health`: the health check, a service Spark starts at every
//! boot (ADR 0453). It removes a slot a stage left half built, then
//! watches the essential services through Spark's control socket. An
//! update being tried proves itself when they have run for two minutes
//! without a restart, the required checks of health.toml(5) pass, the
//! firmware set it names loaded, and every device the last good start
//! had is there (ADR 0468); any other start is seen through once its
//! services run, and what is wrong with its firmware or devices is only
//! logged. Each start seen through keeps its devices as the record the
//! next try is held to ([`super::devices`]). The result goes into
//! the slot state on every EFI system partition: a proved update becomes
//! the default, and one that failed gives up its tries, and the machine
//! restarts into the version before it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use slots::state::Checked;

use super::place::{self, BUILDING, Root};
use super::{devices, firmware};
use crate::power;
use crate::subvolume;
use crate::util::Fail;

/// health.toml(5), in the settings.
const CONFIG: &str = "/system/settings/sysupdate/health.toml";

/// What became of the CPU microcode GRUB handed the kernel
/// (radix-sysfs(5)).
const MICROCODE: &str = "/kernel/firmware/microcode";

/// Where the last result is kept, for `sysupdate status`.
pub const LAST: &str = "/system/state/sysupdate/health";

/// How often Spark is asked.
const EVERY: Duration = Duration::from_secs(2);

/// How many restarts of an essential service fail the check at once.
const RESTARTS: u64 = 3;

/// How long the health check waits for a stage that holds the slot
/// state's lock, in seconds.
const STAGE_WAIT: u64 = 30 * 60;

/// health.toml(5).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    /// The services the system needs, by name: each that is turned on
    /// must be running.
    essential: Option<Vec<String>>,
    /// Seconds the essential services must run without a restart before
    /// an update counts as proved.
    stable: Option<u64>,
    /// The most seconds an update has to get there.
    deadline: Option<u64>,
    #[serde(default)]
    check: Vec<CheckFile>,
}

/// One `[[check]]`: a command that passes by exiting with 0.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckFile {
    name: String,
    command: Vec<String>,
    /// Whether an update fails when it fails; a wanted check is only
    /// logged.
    #[serde(default)]
    required: bool,
    /// Seconds it may take.
    timeout: Option<u64>,
}

/// What the health check goes by.
#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    pub essential: Vec<String>,
    pub stable: u64,
    pub deadline: u64,
    pub checks: Vec<Check>,
}

/// A check of health.toml(5).
#[derive(Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub command: Vec<String>,
    pub required: bool,
    pub timeout: u64,
}

impl Default for Config {
    /// What the check goes by without health.toml: the services an
    /// administrator needs to reach the machine and read its log.
    fn default() -> Config {
        Config {
            essential: ["logd", "authd", "consoled", "netd", "sshd"].map(String::from).to_vec(),
            stable: 120,
            deadline: 600,
            checks: Vec::new(),
        }
    }
}

impl Config {
    /// health.toml's text, read.
    ///
    /// # Errors
    ///
    /// What is wrong with it.
    pub fn parse(text: &str) -> Result<Config, String> {
        let file: File = toml::from_str(text).map_err(|err| err.to_string())?;
        let default = Config::default();
        let mut checks = Vec::new();
        for check in file.check {
            if check.command.is_empty() {
                return Err(format!("the check {} has no command", check.name));
            }
            checks.push(Check {
                name: check.name,
                command: check.command,
                required: check.required,
                timeout: check.timeout.unwrap_or(60),
            });
        }
        Ok(Config {
            essential: file.essential.unwrap_or(default.essential),
            stable: file.stable.unwrap_or(default.stable),
            deadline: file.deadline.unwrap_or(default.deadline),
            checks,
        })
    }

    /// The settings' health.toml, or the defaults, with a word why.
    fn load() -> Config {
        match std::fs::read_to_string(CONFIG) {
            Ok(text) => Config::parse(&text).unwrap_or_else(|why| {
                println!("sysupdate: {CONFIG}: {why}; going by the defaults");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }
}

/// A service as Spark's `status` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub state: String,
    pub restarts: u64,
    /// How its process last ended, `0x1`, or `-` if it has not.
    pub last: String,
}

/// Spark's `status` answer: `name\tstate\tpid\trestarts\t...` lines, and
/// `#` lines of its own.
pub fn services(answer: &str) -> BTreeMap<String, Service> {
    answer
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?;
            let state = fields.next()?;
            let restarts = fields.nth(1)?.parse().ok()?;
            let last = fields.next().unwrap_or("-").to_string();
            Some((name.to_string(), Service { state: state.to_string(), restarts, last }))
        })
        .collect()
}

/// How the essential services are, at one look.
#[derive(Debug, PartialEq, Eq)]
pub enum Look {
    /// All that are turned on run, with these restarts.
    Running(Vec<u64>),
    /// These do not run yet.
    Waiting(Vec<String>),
    /// One keeps failing, or ended for good in failure: why.
    Failing(String),
}

/// The essential services of `essential` in Spark's `status`: those it
/// has no file for, are turned off, or ended well for good (a service
/// whose `restart` lets it end) do not count.
pub fn look(essential: &[String], services: &BTreeMap<String, Service>) -> Look {
    let ended_well = |s: &Service| s.state == "stopped" && matches!(s.last.as_str(), "-" | "0x0");
    let counted: Vec<(&String, &Service)> = essential
        .iter()
        .filter_map(|name| Some((name, services.get(name)?)))
        .filter(|(_, service)| service.state != "disabled" && !ended_well(service))
        .collect();
    if let Some((name, service)) = counted.iter().find(|(_, s)| s.restarts >= RESTARTS) {
        return Look::Failing(format!("{name} has restarted {} times", service.restarts));
    }
    if let Some((name, service)) = counted.iter().find(|(_, s)| s.state == "stopped") {
        return Look::Failing(format!("{name} ended, with status {}", service.last));
    }
    let waiting: Vec<String> =
        counted.iter().filter(|(_, s)| s.state != "running").map(|(n, _)| (*n).clone()).collect();
    if waiting.is_empty() {
        Look::Running(counted.iter().map(|(_, s)| s.restarts).collect())
    } else {
        Look::Waiting(waiting)
    }
}

/// The health check: the service's program.
///
/// # Errors
///
/// If the slot state cannot be read or written.
pub fn run() -> Result<i32, Fail> {
    let root = match place::root() {
        Ok(root) => root,
        Err(why) => {
            println!("sysupdate: {}; no health check", why.message);
            return Ok(0);
        }
    };
    remove_half_built(&root);
    let (_, state) = place::state(&root)?;
    let trying = state.trying == Some(root.running);
    let config = Config::load();
    let named = place::named(&root, root.running);
    if trying {
        println!(
            "sysupdate: {named} is an update being tried: it proves itself when its essential \
             services run for {} seconds",
            config.stable
        );
    }
    let result = watch(&config, trying)
        .and_then(|()| checks(&config))
        .and_then(|()| hardware(&root, trying));
    let passed = result.is_ok();
    if passed {
        match devices::present(&root) {
            Ok(present) => devices::record(&present),
            Err(why) => println!("sysupdate: cannot list the devices: {}", why.message),
        }
    }
    if let Err(why) = &result {
        println!("sysupdate: {named} failed its health check: {why}");
    }
    record(&named, &result);
    // A stage takes minutes at most.
    let _lock = place::lock_within(Duration::from_secs(STAGE_WAIT))?;
    let (copies, state) = place::state(&root)?;
    let (next, checked) = state.checked(root.running, passed);
    if let Some(next) = next {
        place::write(&copies, next)?;
    }
    // A start seen through: Rescue counts its starts afresh (ADR 0460).
    if !matches!(checked, Checked::Failed(_)) {
        for why in place::reset_rescue(copies.iter().map(|c| c.partition.as_str())) {
            println!("sysupdate: cannot set Rescue's count of starts back: {why}");
        }
    }
    match checked {
        Checked::Proved(slot) => {
            let before = place::named(&root, slot.other());
            println!(
                "sysupdate: {named} proved itself: it is now the default, and {before} the \
                 previous version"
            );
        }
        Checked::Failed(slot) => {
            let before = place::named(&root, slot.other());
            println!("sysupdate: the update gives up its tries; restarting into {before}");
            if let Err(why) = power::request("reboot") {
                println!("sysupdate: cannot ask Spark to restart: {}", why.message);
            }
        }
        Checked::FellBack(slot) => {
            let update = place::named(&root, slot);
            println!(
                "sysupdate: the update in {update} did not prove itself; {named} runs, with the \
                 settings from before the update"
            );
        }
        Checked::Start => println!("sysupdate: this start of {named} is seen through"),
    }
    Ok(0)
}

/// Removes a slot a stage left half built, as a power cut can (ADR
/// 0451). A stage running now holds the lock, and cleans up for itself.
fn remove_half_built(root: &Root) {
    let building = root.path(BUILDING);
    if !building.exists() {
        return;
    }
    let Ok(_lock) = place::lock() else { return };
    match subvolume::remove(&building) {
        Ok(()) => println!(
            "sysupdate: removed {BUILDING}, a slot left half built by a stage that did not finish"
        ),
        Err(why) => println!("sysupdate: cannot remove {BUILDING}: {}", why.message),
    }
}

/// Waits until the essential services run: for `config.stable` seconds
/// with no restart when `trying`, and once otherwise.
fn watch(config: &Config, trying: bool) -> Result<(), String> {
    let start = Instant::now();
    let mut steady: Option<(Instant, Vec<u64>)> = None;
    loop {
        let answer = power::request("status").map_err(|why| format!("Spark: {}", why.message))?;
        match look(&config.essential, &services(&answer)) {
            Look::Failing(why) => return Err(why),
            Look::Running(_) if !trying => return Ok(()),
            Look::Running(restarts) => match &steady {
                Some((since, before)) if *before == restarts => {
                    if since.elapsed() >= Duration::from_secs(config.stable) {
                        return Ok(());
                    }
                }
                _ => steady = Some((Instant::now(), restarts)),
            },
            Look::Waiting(names) => {
                steady = None;
                if start.elapsed() >= Duration::from_secs(config.deadline) {
                    return Err(format!(
                        "{} did not run within {} seconds",
                        names.join(", "),
                        config.deadline
                    ));
                }
            }
        }
        if start.elapsed() >= Duration::from_secs(config.deadline + config.stable) {
            return Err(format!("its services did not settle within {} seconds", config.deadline));
        }
        std::thread::sleep(EVERY);
    }
}

/// Runs health.toml's checks: a required one that fails fails the
/// health check, a wanted one is only logged.
fn checks(config: &Config) -> Result<(), String> {
    for check in &config.checks {
        let passed = run_check(check);
        let kind = if check.required { "required" } else { "wanted" };
        match (&passed, check.required) {
            (Ok(()), _) => println!("sysupdate: the {kind} check {} passed", check.name),
            (Err(why), true) => {
                return Err(format!("the required check {} failed: {why}", check.name));
            }
            (Err(why), false) => {
                println!("sysupdate: the wanted check {} failed: {why}", check.name);
            }
        }
    }
    Ok(())
}

/// The per-device checks (ADR 0468): the firmware set the running slot
/// names loaded, and every device the last good start had is here. While
/// `trying`, the first problem fails the health check; otherwise each is
/// only logged, as a machine that lost a card has to start all the same.
fn hardware(root: &Root, trying: bool) -> Result<(), String> {
    let mut problems: Vec<String> = firmware_problem(root).into_iter().collect();
    match (devices::recorded(), devices::present(root)) {
        (Some(before), Ok(now)) => {
            problems.extend(devices::missing(&before, &now).into_iter().map(|device| {
                format!("{}, there at the last good start, is missing", device.described())
            }));
        }
        (None, _) => {}
        (Some(_), Err(why)) => {
            problems.push(format!("the devices cannot be listed: {}", why.message));
        }
    }
    if trying {
        return problems.into_iter().next().map_or(Ok(()), Err);
    }
    for problem in problems {
        println!("sysupdate: {problem}; as this start is no update's try, it goes on");
    }
    Ok(())
}

/// What `/kernel/firmware/microcode` says went wrong with the CPU
/// microcode of the firmware set the running slot names, if it names one
/// with microcode.
fn firmware_problem(root: &Root) -> Option<String> {
    let named = firmware::named(&root.slot(root.running))?;
    named.microcode.as_ref()?;
    let set = &named.set;
    let Ok(report) = std::fs::read_to_string(MICROCODE) else {
        return Some(format!("the CPU microcode of firmware {set} did not reach the kernel"));
    };
    microcode_problem(&report).map(|why| format!("the CPU microcode of firmware {set} {why}"))
}

/// What went wrong, as the kernel's report of the microcode says: data
/// that failed its checks, or an update the processor refused.
fn microcode_problem(report: &str) -> Option<String> {
    let field = |key: &str| {
        report.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(' ')).map(str::trim)
    };
    match field("result")? {
        "damaged" => Some("has data that failed its checks, so none of it was used".into()),
        "refused" => Some(format!(
            "was refused by the processor: it asked for revision {} and runs {}",
            field("refused").unwrap_or("?"),
            field("revision").unwrap_or("?")
        )),
        _ => None,
    }
}

/// Runs one check, within its time.
fn run_check(check: &Check) -> Result<(), String> {
    let mut child = Command::new(&check.command[0])
        .args(&check.command[1..])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|err| format!("{}: {err}", check.command[0]))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return if status.success() { Ok(()) } else { Err(format!("it ended with {status}")) };
        }
        if start.elapsed() >= Duration::from_secs(check.timeout) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("it took more than {} seconds", check.timeout));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Keeps the result for `sysupdate status`.
fn record(named: &str, result: &Result<(), String>) {
    let when = crate::util::now_ns() / 1_000_000_000;
    let text = match result {
        Ok(()) => format!("{when} passed {named}\n"),
        Err(why) => format!("{when} failed {named}: {why}\n"),
    };
    let path = Path::new(LAST);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_health_toml_and_its_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        let shipped = include_str!("../../../../defaults/sysupdate/health.toml");
        assert_eq!(Config::parse(shipped).unwrap(), Config::default());
        let config = Config::parse(
            "essential = [\"logd\"]\nstable = 10\n\n[[check]]\nname = \"web\"\n\
             command = [\"curl\", \"-f\", \"http://localhost/\"]\nrequired = true\n",
        )
        .unwrap();
        assert_eq!(config.essential, ["logd"]);
        assert_eq!((config.stable, config.deadline), (10, 600));
        assert_eq!(config.checks[0].command, ["curl", "-f", "http://localhost/"]);
        assert!(config.checks[0].required && config.checks[0].timeout == 60);
        assert!(Config::parse("essentials = []").is_err());
        assert!(Config::parse("[[check]]\nname = \"x\"\ncommand = []\n").is_err());
    }

    #[test]
    fn counts_the_essential_services_that_are_turned_on() {
        let answer = "logd\trunning\t12\t0\t-\talways\tSystem log\n\
                      sshd\tdisabled\t-\t0\t-\talways\tSSH\n\
                      netd\twaiting\t-\t1\t0x1\talways\tNetwork\n\
                      #\tshutdown\trestart\t5\n";
        let services = services(answer);
        assert_eq!(services.len(), 3);
        let essential = ["logd", "sshd", "netd", "authd"].map(String::from);
        assert_eq!(look(&essential, &services), Look::Waiting(vec!["netd".into()]));
        assert_eq!(look(&essential[..2], &services), Look::Running(vec![0]));
        let failing = services_with("logd\twaiting\t-\t3\t0x1\talways\tSystem log\n");
        assert_eq!(look(&essential, &failing), Look::Failing("logd has restarted 3 times".into()));
        // Ended well for good, it is not watched; ended in failure, it
        // fails the check.
        let ended = services_with("netd\tstopped\t-\t0\t0x0\ton-failure\tNetwork\n");
        assert_eq!(look(&essential[2..3], &ended), Look::Running(vec![]));
        let ended = services_with("netd\tstopped\t-\t0\t0x2\tnever\tNetwork\n");
        assert_eq!(
            look(&essential[2..3], &ended),
            Look::Failing("netd ended, with status 0x2".into())
        );
    }

    #[test]
    fn reads_what_became_of_the_microcode() {
        let applied = "size 9216\nupdates 2\nresult applied\nrevision 0xf0\n";
        assert_eq!(microcode_problem(applied), None);
        assert_eq!(microcode_problem("size 1\nupdates 1\nresult hypervisor\nrevision 0x0\n"), None);
        assert_eq!(microcode_problem("result unsupported\n"), None);
        assert_eq!(
            microcode_problem("size 9\nupdates 0\nresult damaged\n").unwrap(),
            "has data that failed its checks, so none of it was used"
        );
        let refused = "result refused\nrevision 0xe2\nrefused 0xf0\n";
        assert_eq!(
            microcode_problem(refused).unwrap(),
            "was refused by the processor: it asked for revision 0xf0 and runs 0xe2"
        );
    }

    fn services_with(answer: &str) -> BTreeMap<String, Service> {
        services(answer)
    }
}
