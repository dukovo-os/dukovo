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
//! next try is held to ([`super::devices`]). Each start has a record,
//! begun as the check begins and finished with its outcome
//! ([`super::starts`], ADR 0500), and each failure names its error ID.
//! The result goes into
//! the slot state on every EFI system partition: a proved update becomes
//! the default, and one that failed gives up its tries, and the machine
//! restarts into the version before it.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use slots::state::Checked;
use spark_api::show::{self, HealthState::Failing, State};

use super::place::{self, BUILDING, Root};
use super::starts::{self, Item, Outcome};
use super::{devices, firmware};
use crate::alerts::client::{self as alerts, Screen};
use crate::alerts::protocol::{Alert, Severity};
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

/// What the sign-in screens show of a failed start until the next start
/// (ADR 0501), which consoled(8) reads.
pub const NOTICE: &str = "/system/live/sysupdate/failure";

/// How often Spark is asked.
const EVERY: Duration = Duration::from_secs(2);

/// How many restarts of an essential service fail the check at once.
const RESTARTS: u64 = 3;

/// How long the health check waits for a stage that holds the slot
/// state's lock, in seconds.
const STAGE_WAIT: u64 = 30 * 60;

/// How long the check waits for alertd, which may start after it, to tell
/// it of a failed start.
const ALERTD_WAIT: Duration = Duration::from_secs(60);

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

/// How the essential services are, at one look.
#[derive(Debug, PartialEq, Eq)]
pub enum Look {
    /// All that are turned on run, with these restarts.
    Running(Vec<u64>),
    /// These do not run yet, or fail their health checks and may recover.
    Waiting(Vec<String>),
    /// One keeps failing, or ended for good in failure: why.
    Failing(String),
}

/// The essential services of `essential` among `services`, as Spark's
/// `show` describes them: those it has no file for, are turned off, are
/// off as meant (a driver the kernel left no cards), or ended well for
/// good (a setup step, or a service whose `restart` lets it end) do not
/// count. One that runs but fails its health check
/// (ADR 0470) does not run yet, and fails the check once it has
/// restarted RESTARTS times; one Spark gave up on fails it at once.
pub fn look(essential: &[String], services: &[show::Service]) -> Look {
    let ended_well = |s: &show::Service| match s.state {
        State::Disabled | State::Done => true,
        State::Stopped if s.off_as_meant => true,
        State::Stopped => s.why.is_none() && s.last_status.is_none_or(|status| status == 0),
        _ => false,
    };
    let counted: Vec<&show::Service> = essential
        .iter()
        .filter_map(|name| services.iter().find(|s| &s.name == name))
        .filter(|service| !ended_well(service))
        .collect();
    let unhealthy = |s: &show::Service| s.health.as_ref().is_some_and(|h| h.state == Failing);
    if let Some(service) = counted.iter().find(|s| s.state == State::Failed) {
        let why = service.why.as_deref().unwrap_or("it failed to start");
        return Look::Failing(format!("Spark gave up on {}: {why}", service.name));
    }
    if let Some(service) = counted.iter().find(|s| u64::from(s.restarts) >= RESTARTS) {
        let name = &service.name;
        return Look::Failing(match service.health.as_ref().filter(|_| unhealthy(service)) {
            Some(health) => {
                let last = health.last_failure.as_deref().unwrap_or("no reason given");
                format!("{name} fails its health check: {last}")
            }
            None => format!("{name} has restarted {} times", service.restarts),
        });
    }
    if let Some(service) = counted.iter().find(|s| s.state == State::Stopped) {
        let name = &service.name;
        return Look::Failing(match (&service.why, service.last_status) {
            (Some(why), _) => format!("{name} does not run: {why}"),
            (None, Some(status)) => format!("{name} ended, with status {status:#x}"),
            (None, None) => format!("{name} ended"),
        });
    }
    let waiting: Vec<String> = counted
        .iter()
        .filter(|s| s.state != State::Running || unhealthy(s))
        .map(|s| s.name.clone())
        .collect();
    if waiting.is_empty() {
        Look::Running(counted.iter().map(|s| u64::from(s.restarts)).collect())
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
    let begun = starts::begin_in(
        Path::new(starts::DIR),
        root.running.letter(),
        start_items(&root),
        i64::try_from(crate::util::now_ns() / 1_000_000_000).unwrap_or(0),
    );
    if let Err(err) = &begun {
        println!("sysupdate: cannot keep the record of this start: {err}");
    }
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
    let this = begun.ok().and_then(|start| {
        let outcome = if passed { Outcome::Good } else { Outcome::Failed };
        let why = result.as_ref().err().map(String::as_str);
        starts::finish_in(Path::new(starts::DIR), start, outcome, why)
            .inspect_err(|err| println!("sysupdate: cannot keep the record of this start: {err}"))
            .ok()
    });
    // A stage takes minutes at most.
    let _lock = place::lock_within(Duration::from_secs(STAGE_WAIT))?;
    let (copies, state) = place::state(&root)?;
    let (next, checked) = state.checked(root.running, passed);
    if let Some(text) = failure_notice(&starts::all(), this.as_ref(), checked, &named) {
        tell(&text);
        tell_alertd(&failure_alert(checked, &named, &text), Screen::Shown);
    }
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
            println!(
                "sysupdate: the update gives up its tries; restarting into {before} (UPD-101)"
            );
            if let Err(why) = power::request("reboot") {
                println!("sysupdate: cannot ask Spark to restart: {}", why.message);
                let title =
                    format!("An update failed its health check, and {before} could not be started");
                let detail = format!(
                    "Spark did not restart the machine: {}\n`elevate reboot` starts {before}.",
                    why.message
                );
                let alert = Alert::new("update-failed".into(), Severity::Critical, &title, &detail);
                tell_alertd(&alert, Screen::Tell);
            }
        }
        Checked::FellBack(slot) => {
            let update = place::named(&root, slot);
            println!(
                "sysupdate: the update in {update} did not prove itself; {named} runs, with the \
                 settings from before the update (UPD-101)"
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
        let answer = power::request("show").map_err(|why| format!("Spark: {}", why.message))?;
        let document = show::parse(&answer).map_err(|why| format!("Spark: {why}"))?;
        match look(&config.essential, &document.services) {
            Look::Failing(why) => return Err(format!("{why} (BOOT-102)")),
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
                        "{} did not run within {} seconds (BOOT-102)",
                        names.join(", "),
                        config.deadline
                    ));
                }
            }
        }
        if start.elapsed() >= Duration::from_secs(config.deadline + config.stable) {
            return Err(format!(
                "its services did not settle within {} seconds (BOOT-102)",
                config.deadline
            ));
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
                return Err(format!("the required check {} failed: {why} (BOOT-103)", check.name));
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
                format!(
                    "{}, there at the last good start, is missing (BOOT-101)",
                    device.described()
                )
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
        return Some(format!(
            "the CPU microcode of firmware {set} did not reach the kernel (FW-103)"
        ));
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
        "damaged" => {
            Some("has data that failed its checks, so none of it was used (FW-101)".into())
        }
        "refused" => Some(format!(
            "was refused by the processor: it asked for revision {} and runs {} (FW-102)",
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

/// What this start runs, for its record: the system and its kernel, the
/// firmware set and its microcode, the boot options, the devices and the
/// apps installed for everyone.
fn start_items(root: &Root) -> Vec<Item> {
    let slot = root.slot(root.running);
    let installed = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok())
    };
    let about = slot.join(slots::about::PATH);
    let mut items = Vec::new();
    if let Some(running) = root.about(root.running) {
        items.push(Item::new("system", "Dukovo", &running.version, installed(&about)));
    }
    let kernel = slot.join("startup/radix");
    if let Ok(bytes) = std::fs::read(&kernel) {
        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
        let hex = digest.as_ref()[..8].iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        items.push(Item::new("kernel", "Radix", &hex, installed(&kernel)));
    }
    if let Some(named) = firmware::named(&slot) {
        let list =
            root.path(slots::firmware::STORE).join("sets").join(format!("{}.json", named.set));
        items.push(Item::new("firmware", "firmware set", &named.set, installed(&list)));
        if let Some(hash) = &named.microcode {
            items.push(Item::new("microcode", "CPU microcode", &hash[..hash.len().min(16)], None));
        }
    }
    // Less the slot GRUB named, which changes with every update.
    let line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    let options: Vec<&str> = line
        .split_whitespace()
        .filter(|word| !word.starts_with("slot=") && !word.starts_with("fallback="))
        .collect();
    items.push(Item::new("options", "boot options", &options.join(" "), None));
    if let Ok(present) = devices::present(root) {
        for device in present {
            items.push(Item::new(&device.kind, &device.key, &device.about, None));
        }
    }
    let registry =
        Path::new(crate::bundles::registry::STATE).join(crate::bundles::registry::INSTALLED);
    let text = std::fs::read_to_string(registry).unwrap_or_default();
    for app in crate::bundles::registry::installed(&text).unwrap_or_default() {
        if app.place == crate::bundles::registry::Place::Everyone {
            let when = i64::try_from(app.when).ok();
            items.push(Item::new("app", &app.id, &app.version, when));
        }
    }
    items
}

/// What the sign-in screen says of a failed start (ADR 0501), if this
/// start is one or follows one: the update that fell back, of `records`,
/// or `this` start, which failed its check and goes on; what Dukovo did;
/// what changed since the last good start; and where to read more. A
/// failed try says nothing: the start after it does.
fn failure_notice(
    records: &[starts::Start],
    this: Option<&starts::Start>,
    checked: Checked,
    named: &str,
) -> Option<String> {
    let this_failed = this.filter(|start| start.outcome == Outcome::Failed);
    let (failed, did) = match checked {
        Checked::Failed(_) => return None,
        Checked::FellBack(_) => {
            let before = this.map_or(u64::MAX, |start| start.number);
            let failed = records
                .iter()
                .rev()
                .find(|start| start.number < before && start.outcome == Outcome::Failed)?;
            (
                failed,
                format!("Dukovo went back to {named}, with the settings from before the update."),
            )
        }
        _ => (this_failed?, "As this start is no update's try, it goes on.".to_string()),
    };
    let mut text = format!(
        "Start {} failed its health check: {}\n{did}\n",
        failed.number,
        failed.why.as_deref().unwrap_or("a check failed")
    );
    if let Some(this) = this_failed.filter(|this| this.number != failed.number) {
        let _ = writeln!(
            text,
            "This start failed its health check too: {}",
            this.why.as_deref().unwrap_or("a check failed")
        );
    }
    for line in crate::changes::report(records, None, failed) {
        let _ = writeln!(text, "{line}");
    }
    text.push_str("More: changes --failed, and log -b -1 for the failed start's log.\n");
    if let Some(id) = &failed.error {
        let _ = writeln!(
            text,
            "Help: https://dukovo.org/support/{}/errors/{id}.html",
            env!("CARGO_PKG_VERSION")
        );
    }
    Some(text)
}

/// The alert that tells the owner of a failed start, as `text`, the
/// sign-in notice, does (ADR 0501): an update that fell back is critical,
/// and a start that failed its check and goes on a warning.
fn failure_alert(checked: Checked, named: &str, text: &str) -> Alert {
    let (key, severity, title) = match checked {
        Checked::FellBack(_) => (
            "update-fell-back",
            Severity::Critical,
            format!("An update failed its health check, and Dukovo went back to {named}"),
        ),
        _ => (
            "start-failed",
            Severity::Warning,
            format!("This start of {named} failed its health check, and goes on"),
        ),
    };
    Alert::new(key.into(), severity, &title, text)
}

/// Tells alertd of `alert` as an event, waiting for it to start;
/// `screen` says whether [`tell`] showed it on the consoles already.
fn tell_alertd(alert: &Alert, screen: Screen) {
    if let Err(why) = alerts::event_at_start(alert, screen, ALERTD_WAIT) {
        println!("sysupdate: cannot tell alertd of it: {why}");
    }
}

/// Shows `text` on the sign-in screens until the next start, in
/// [`NOTICE`], and tells whoever is signed in, in a notification shown
/// until a key is pressed.
fn tell(text: &str) {
    let path = Path::new(NOTICE);
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text));
    if let Err(err) = written {
        println!("sysupdate: cannot write {NOTICE}: {err}");
    }
    let first = text.lines().next().unwrap_or_default();
    if let Err(why) = crate::notifyd::send(
        "sysupdate",
        first,
        "changes --failed says what changed",
        crate::notifyd::Urgency::Critical,
    ) {
        println!("sysupdate: cannot tell the consoles: {}", why.message);
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

    /// Service `name` doing `state`, with `restarts`.
    fn service(name: &str, state: State, restarts: u32) -> show::Service {
        show::Service { name: name.into(), state, restarts, ..show::Service::default() }
    }

    #[test]
    fn counts_the_essential_services_that_are_turned_on() {
        let services = [
            service("logd", State::Running, 0),
            service("sshd", State::Disabled, 0),
            service("netd", State::Waiting, 1),
        ];
        let essential = ["logd", "sshd", "netd", "authd"].map(String::from);
        assert_eq!(look(&essential, &services), Look::Waiting(vec!["netd".into()]));
        assert_eq!(look(&essential[..2], &services), Look::Running(vec![0]));
        // Pending and starting do not run yet either.
        for state in [State::Pending, State::Starting, State::Stopping] {
            let services = [service("netd", state, 0)];
            assert_eq!(look(&essential[2..3], &services), Look::Waiting(vec!["netd".into()]));
        }
        let failing = [service("logd", State::Waiting, 3)];
        assert_eq!(look(&essential, &failing), Look::Failing("logd has restarted 3 times".into()));
    }

    #[test]
    fn a_service_that_ended_counts_as_its_end_says() {
        let essential = ["netd".to_string()];
        // Ended well for good, or done, it is not watched.
        let ended = show::Service { last_status: Some(0), ..service("netd", State::Stopped, 0) };
        assert_eq!(look(&essential, &[ended]), Look::Running(vec![]));
        assert_eq!(look(&essential, &[service("netd", State::Done, 0)]), Look::Running(vec![]));
        // Off as meant, as a driver the kernel left no cards is, the same.
        let idle = show::Service {
            why: Some("the kernel left it none of the cards it drives".into()),
            off_as_meant: true,
            ..service("netd", State::Stopped, 0)
        };
        assert_eq!(look(&essential, &[idle]), Look::Running(vec![]));
        // Ended in failure, or refused, it fails the check.
        let ended =
            show::Service { last_status: Some(0x200), ..service("netd", State::Stopped, 0) };
        assert_eq!(
            look(&essential, &[ended]),
            Look::Failing("netd ended, with status 0x200".into())
        );
        let refused = show::Service {
            why: Some("it requires resolved, which is stopped".into()),
            ..service("netd", State::Stopped, 0)
        };
        assert_eq!(
            look(&essential, &[refused]),
            Look::Failing("netd does not run: it requires resolved, which is stopped".into())
        );
        let given_up = show::Service {
            why: Some("it failed to start 5 times in a row".into()),
            ..service("netd", State::Failed, 4)
        };
        assert_eq!(
            look(&essential, &[given_up]),
            Look::Failing("Spark gave up on netd: it failed to start 5 times in a row".into())
        );
    }

    #[test]
    fn a_running_service_that_fails_its_health_check_does_not_count_as_running() {
        let essential = ["web".to_string()];
        let health = |state| show::Health {
            state,
            failures: 2,
            last_failure: Some("exited with code 0x1: no answer on 8080".into()),
            checked_ms: Some(1_000),
        };
        let unhealthy = |restarts| show::Service {
            health: Some(health(Failing)),
            ..service("web", State::Running, restarts)
        };
        assert_eq!(look(&essential, &[unhealthy(1)]), Look::Waiting(vec!["web".into()]));
        assert_eq!(
            look(&essential, &[unhealthy(3)]),
            Look::Failing(
                "web fails its health check: exited with code 0x1: no answer on 8080".into()
            )
        );
        let healthy = show::Service {
            health: Some(health(show::HealthState::Passing)),
            ..service("web", State::Running, 1)
        };
        assert_eq!(look(&essential, &[healthy]), Look::Running(vec![1]));
    }

    #[test]
    fn reads_what_became_of_the_microcode() {
        let applied = "size 9216\nupdates 2\nresult applied\nrevision 0xf0\n";
        assert_eq!(microcode_problem(applied), None);
        assert_eq!(microcode_problem("size 1\nupdates 1\nresult hypervisor\nrevision 0x0\n"), None);
        assert_eq!(microcode_problem("result unsupported\n"), None);
        assert_eq!(
            microcode_problem("size 9\nupdates 0\nresult damaged\n").unwrap(),
            "has data that failed its checks, so none of it was used (FW-101)"
        );
        let refused = "result refused\nrevision 0xe2\nrefused 0xf0\n";
        assert_eq!(
            microcode_problem(refused).unwrap(),
            "was refused by the processor: it asked for revision 0xf0 and runs 0xe2 (FW-102)"
        );
    }

    #[test]
    fn a_failed_start_is_told_with_what_changed() {
        let dir = std::env::temp_dir().join(format!("notice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let set = |value: &str| vec![Item::new("firmware", "set", value, Some(200))];
        let good = starts::begin_in(&dir, "b", set("2026.1.1"), 1000).unwrap();
        starts::finish_in(&dir, good, Outcome::Good, None).unwrap();
        let tried = starts::begin_in(&dir, "a", set("2026.2.1"), 2000).unwrap();
        let why = "the network card vio0 (52:54:00:15:0c:01), there at the last good start, \
                   is missing (BOOT-101)";
        starts::finish_in(&dir, tried, Outcome::Failed, Some(why)).unwrap();
        let this = starts::begin_in(&dir, "b", set("2026.1.1"), 3000).unwrap();
        let this = starts::finish_in(&dir, this, Outcome::Good, None).unwrap();
        let records = starts::all_in(&dir);
        let named = "slot B (Dukovo 0.15.0, firmware 2026.1.1)";
        // A failed try says nothing; the start after it does.
        assert_eq!(failure_notice(&records, None, Checked::Failed(slots::Slot::A), named), None);
        let text = failure_notice(&records, Some(&this), Checked::FellBack(slots::Slot::A), named)
            .unwrap();
        assert!(text.starts_with(&format!("Start 2 failed its health check: {why}\n")), "{text}");
        assert!(text.contains(&format!("Dukovo went back to {named}")), "{text}");
        assert!(text.contains("  firmware set: 2026.1.1 to 2026.2.1;"), "{text}");
        assert!(text.contains("/errors/BOOT-101.html"), "{text}");
        // A good start that follows no failure says nothing.
        assert_eq!(failure_notice(&records, Some(&this), Checked::Start, named), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_start_is_told_to_alertd_as_its_notice_says() {
        let text = "Start 7 failed its health check: netd did not start\n";
        let fell = failure_alert(Checked::FellBack(slots::Slot::B), "Dukovo 0.16.0 (slot A)", text);
        assert_eq!((fell.key.as_str(), fell.severity), ("update-fell-back", Severity::Critical));
        assert_eq!(
            fell.title,
            "An update failed its health check, and Dukovo went back to Dukovo 0.16.0 (slot A)"
        );
        assert_eq!(fell.detail, text.trim_end());
        assert_eq!(fell.invalid(), None);
        let goes_on = failure_alert(Checked::Start, "Dukovo 0.16.0 (slot A)", text);
        assert_eq!((goes_on.key.as_str(), goes_on.severity), ("start-failed", Severity::Warning));
    }
}
