// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! tune(8): performance profiles (ADR 0476).
//!
//! A profile is a file in [`PROFILES`], a description and the few kernel
//! settings it sets (radix-kernel-settings(2)), each with a line that
//! says why. `tune set NAME` writes the choice to [`CHOSEN`] and
//! restarts the `tune` service, which holds the kernel settings
//! capability and runs `tune apply`: every setting the profile lists gets
//! its value, every other goes back to the value it had when the kernel
//! started, and a setting already at its value is left alone. So a
//! profile changes nothing it does not list, and switching leaves nothing
//! of the one before. A setting this machine cannot have is said in a
//! line and the rest are set. `tune` alone shows the profile in use and
//! each setting's value, from the files everyone reads in
//! `/kernel/settings`.
//!
//! In a virtual machine, the first start after an install tells the
//! sign-in screens that `elevate tune set vm-guest` suits it (ADR 0504);
//! choosing any profile, `balanced` among them, ends that.
//!
//! What needs no kernel is plain functions with host tests: reading a
//! profile and the kernel's lines, the plan of what to set, the report of
//! what came of it, and which profile fits. The boot test `tune` and the
//! lab test `system/tune` run the rest; when a profile does not take
//! effect, `log tune` has the service's lines.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use radix_abi::errors::Kernel;
use radix_abi::kernel_settings::{self as abi, LINE_MAX};
use radix_abi::startup::SLOT_SETTINGS;
use serde::Deserialize;

use crate::args::{Arg, Args};
use crate::sign_in_notices;
use crate::sys::{SYS, status_name};
use crate::util::{Fail, fail, usage};

const USAGE: &str = "Usage: tune\n       tune list\n       tune show NAME\n       elevate tune set \
                     NAME\n       tune recommend";

/// Where the profiles are, one file each.
const PROFILES: &str = "/system/resources/tune";

/// The profile an administrator chose, if one did.
const CHOSEN: &str = "/system/settings/tune.toml";

/// The kernel's settings, a file each, which everyone reads.
const SETTINGS: &str = "/kernel/settings";

/// What the last `tune apply` could not do, a line each, kept in memory
/// for `tune set` to say.
const PROBLEMS: &str = "/system/live/tune/problems";

/// The mark that the notice about virtual machines was given, which a
/// start does only once.
const NOTICE_GIVEN: &str = "/system/state/tune/vm-notice";

/// The profile of a machine nobody chose one for: every setting at its
/// start value.
const DEFAULT: &str = "balanced";

/// What the notice for a virtual machine says.
const VM_NOTICE: &str =
    "This machine runs in a virtual machine: `elevate tune set vm-guest` suits it.";

/// A setting's value in a profile: a word or a whole number.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Value {
    Word(String),
    Number(u64),
}

/// A profile's file as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    description: String,
    #[serde(default)]
    settings: BTreeMap<String, Value>,
}

/// A setting a profile sets, and why.
#[derive(Debug, PartialEq, Eq)]
struct Choice {
    name: String,
    value: String,
    why: String,
}

/// A profile.
#[derive(Debug, PartialEq, Eq)]
struct Profile {
    name: String,
    description: String,
    /// The settings it sets, by name.
    settings: Vec<Choice>,
}

impl Profile {
    /// What the profile sets `name` to, if it lists it.
    fn value_of(&self, name: &str) -> Option<&str> {
        self.settings.iter().find(|c| c.name == name).map(|c| c.value.as_str())
    }
}

/// Reads the profile `name` from its file's `text`.
fn parse_profile(name: &str, text: &str) -> Result<Profile, String> {
    let file: ProfileFile =
        toml::from_str(text).map_err(|err| format!("{name}: {}", err.message()))?;
    let mut why = reasons(text);
    let settings = file
        .settings
        .into_iter()
        .map(|(setting, value)| Choice {
            why: why.remove(&setting).unwrap_or_default(),
            value: match value {
                Value::Word(word) => word,
                Value::Number(number) => number.to_string(),
            },
            name: setting,
        })
        .collect();
    Ok(Profile { name: name.to_string(), description: file.description, settings })
}

/// The reason each setting of a profile's `text` has: the comment lines
/// just above the line that names it. Setting names are quoted keys, as
/// `"sched.slice_us" = 6000`.
fn reasons(text: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut comment: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(said) = line.strip_prefix('#') {
            comment.push(said.trim());
        } else if let Some(key) = line.strip_prefix('"').and_then(|rest| rest.split('"').next()) {
            found.insert(key.to_string(), comment.join(" "));
            comment.clear();
        } else {
            comment.clear();
        }
    }
    found
}

/// The profile an administrator chose, from its file's `text`.
fn parse_choice(text: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Chosen {
        profile: String,
    }
    let chosen: Chosen = toml::from_str(text).map_err(|err| err.message().to_string())?;
    Ok(chosen.profile)
}

/// The file `tune set` writes for profile `name`.
fn choice_text(name: &str) -> String {
    format!(
        "# The performance profile tune(8) applies at each start; `elevate tune set NAME` \
         writes it.\nprofile = \"{name}\"\n"
    )
}

/// A kernel setting, from a line of radix-kernel-settings(2)'s `list`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Setting {
    name: String,
    value: String,
    /// Its value when the kernel started.
    start: String,
}

/// Reads a line of `list`: the name, the value, the value at start and
/// the values the setting takes.
fn parse_setting(line: &str) -> Option<Setting> {
    let mut words = line.split(' ');
    let (name, value, start) = (words.next()?, words.next()?, words.next()?);
    words.next()?;
    Some(Setting { name: name.into(), value: value.into(), start: start.into() })
}

/// A setting to change.
#[derive(Debug, PartialEq, Eq)]
struct Change {
    name: String,
    from: String,
    to: String,
}

/// What it takes to have `profile`: each setting not at the value the
/// profile gives it, or at its start value when it gives none. Also the
/// names the profile lists that the kernel has no setting for.
fn plan(settings: &[Setting], profile: &Profile) -> (Vec<Change>, Vec<String>) {
    let changes = settings
        .iter()
        .filter_map(|setting| {
            let to = profile.value_of(&setting.name).unwrap_or(setting.start.as_str());
            (setting.value != to).then(|| Change {
                name: setting.name.clone(),
                from: setting.value.clone(),
                to: to.to_string(),
            })
        })
        .collect();
    let unknown = profile
        .settings
        .iter()
        .filter(|choice| !settings.iter().any(|s| s.name == choice.name))
        .map(|choice| choice.name.clone())
        .collect();
    (changes, unknown)
}

/// How the kernel took a change.
#[derive(Debug, PartialEq, Eq)]
enum Taken {
    Set,
    /// This machine cannot have the value.
    NotHere,
    /// The kernel refused it, for the reason given.
    Refused(String),
}

/// The lines for what did not come of a plan: a setting this machine
/// cannot have, one the kernel refused, and one the kernel lacks. Whether
/// the profile has a fault of its own: a refused value or a missing
/// setting, which a machine's lack does not make.
fn problems(results: &[(Change, Taken)], unknown: &[String]) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut faulty = false;
    for (change, taken) in results {
        match taken {
            Taken::Set => {}
            Taken::NotHere => lines.push(format!(
                "{}: this machine cannot have {}; it stays {}",
                change.name, change.to, change.from
            )),
            Taken::Refused(why) => {
                faulty = true;
                lines.push(format!("{}: the kernel refused {}: {why}", change.name, change.to));
            }
        }
    }
    for name in unknown {
        faulty = true;
        lines.push(format!("{name}: this kernel has no such setting"));
    }
    (lines, faulty)
}

/// The line that says what applying `profile` did.
fn summary(profile: &str, results: &[(Change, Taken)]) -> String {
    let set: Vec<String> = results
        .iter()
        .filter(|(_, taken)| *taken == Taken::Set)
        .map(|(change, _)| format!("{} to {}", change.name, change.to))
        .collect();
    if set.is_empty() {
        format!("profile {profile}: nothing to change")
    } else {
        format!("profile {profile}: set {}", set.join(", "))
    }
}

/// The profile that fits a machine: in a virtual machine the guest's, on
/// its battery the one that saves power, else the default; and why.
fn recommended(virtual_machine: bool, on_battery: bool) -> (&'static str, &'static str) {
    if virtual_machine {
        ("vm-guest", "this machine runs in a virtual machine, where the host sets the CPUs' speed")
    } else if on_battery {
        ("power-saver", "this machine runs on its battery")
    } else {
        (DEFAULT, "nothing about this machine asks for another")
    }
}

/// Whether the notice about virtual machines is due: in one, with no
/// profile chosen, and not given before.
fn notice_due(virtual_machine: bool, chosen: bool, given: bool) -> bool {
    virtual_machine && !chosen && !given
}

/// The settings table of `tune`: each name with its value, a `*` before
/// those the profile set, and a note on one the profile asks for that is
/// not in effect. `values` are the kernel's, by name.
fn table(profile: &Profile, values: &[(String, String)]) -> Vec<String> {
    let width = values.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    values
        .iter()
        .map(|(name, value)| {
            let (mark, note) = match profile.value_of(name) {
                Some(wanted) if wanted == value.as_str() => ('*', String::new()),
                Some(wanted) => (' ', format!("  (the profile asks for {wanted}; not in effect)")),
                None => (' ', String::new()),
            };
            format!("{mark} {name:width$}  {value}{note}")
        })
        .collect()
}

/// `tune show`'s text for `profile`: what it is for, and each setting it
/// sets with its reason.
fn description(profile: &Profile) -> Vec<String> {
    let mut lines = vec![format!("{}: {}", profile.name, profile.description)];
    if profile.settings.is_empty() {
        lines.push("  It sets nothing: every setting is at its start value.".into());
        return lines;
    }
    let name_width = profile.settings.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let value_width = profile.settings.iter().map(|c| c.value.len()).max().unwrap_or(0);
    for choice in &profile.settings {
        let mut line = format!("  {:name_width$}  {:value_width$}", choice.name, choice.value);
        if !choice.why.is_empty() {
            let _ = write!(line, "  {}", choice.why);
        }
        lines.push(line.trim_end().to_string());
    }
    lines.push("Every other setting is at its start value.".into());
    lines
}

/// The profile named `name`, read from its file.
fn profile(name: &str) -> Result<Profile, Fail> {
    let plain = !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let text = plain.then(|| fs::read_to_string(format!("{PROFILES}/{name}.toml")).ok()).flatten();
    match text {
        Some(text) => parse_profile(name, &text).map_err(fail),
        None => Err(fail(format!("no profile is called {name}; `tune list` names them"))),
    }
}

/// The names of the profiles there are, in order.
fn profile_names() -> Vec<String> {
    let Ok(entries) = fs::read_dir(PROFILES) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".toml").map(String::from))
        .collect();
    names.sort();
    names
}

/// The name of the profile in use: the one an administrator chose, or the
/// default. Whether one was chosen is the second answer.
fn in_use() -> Result<(String, bool), Fail> {
    match fs::read_to_string(CHOSEN) {
        Ok(text) => parse_choice(&text)
            .map(|name| (name, true))
            .map_err(|why| fail(format!("{CHOSEN}: {why}"))),
        Err(_) => Ok((DEFAULT.into(), false)),
    }
}

/// The kernel's settings and their values, from the files everyone reads.
fn values() -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(SETTINGS) else { return Vec::new() };
    let mut values: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let value = fs::read_to_string(entry.path()).ok()?;
            Some((name, value.trim().to_string()))
        })
        .collect();
    values.sort();
    values
}

/// Whether this machine runs under a hypervisor, which CPUID's leaf 1
/// says in bit 31 of ECX.
fn in_virtual_machine() -> bool {
    crate::lscpu::cpuid(1, 0)[2] & (1 << 31) != 0
}

/// Whether the machine runs on a battery: one of its batteries
/// discharges.
fn on_battery() -> bool {
    let Ok(supplies) = fs::read_dir("/kernel/class/power_supply") else { return false };
    supplies.flatten().any(|supply| {
        let read = |file: &str| fs::read_to_string(supply.path().join(file)).unwrap_or_default();
        read("type").trim() == "Battery" && read("status").trim() == "Discharging"
    })
}

/// tune(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = args.subcommand().map(|command| command.to_string_lossy().into_owned());
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    match (command.as_deref(), operands.as_slice()) {
        (None, []) => show_in_use(),
        (Some("list"), []) => list(),
        (Some("show"), [name]) => show(name),
        (Some("set"), [name]) => set(name),
        (Some("recommend"), []) => recommend(),
        (Some("apply"), []) => apply(),
        _ => Err(usage(USAGE.into())),
    }
}

/// `tune`: the profile in use and each setting's value.
fn show_in_use() -> Result<i32, Fail> {
    let (name, chosen) = in_use()?;
    let profile = profile(&name)?;
    let how = if chosen { "chosen by an administrator" } else { "the default" };
    println!("Profile: {name}, {how}; {}", profile.description);
    for line in table(&profile, &values()) {
        println!("{line}");
    }
    println!("\n* set by the profile");
    Ok(0)
}

/// `tune list`: each profile and what it is for.
fn list() -> Result<i32, Fail> {
    let (current, _) = in_use()?;
    for name in profile_names() {
        let profile = profile(&name)?;
        let mark = if name == current { "  (in use)" } else { "" };
        println!("{name:12} {}{mark}", profile.description);
    }
    Ok(0)
}

/// `tune show NAME`: what a profile sets and why.
fn show(name: &str) -> Result<i32, Fail> {
    for line in description(&profile(name)?) {
        println!("{line}");
    }
    Ok(0)
}

/// `tune recommend`: the profile that fits this machine.
fn recommend() -> Result<i32, Fail> {
    let (name, why) = recommended(in_virtual_machine(), on_battery());
    println!("{name}: {why}.");
    let (current, _) = in_use()?;
    if current == name {
        println!("It is the profile in use.");
    } else {
        println!("`elevate tune set {name}` chooses it.");
    }
    Ok(0)
}

/// `elevate tune set NAME`: chooses a profile and has the `tune` service
/// apply it.
fn set(name: &str) -> Result<i32, Fail> {
    profile(name)?;
    let part = format!("{CHOSEN}.new");
    let written = fs::write(&part, choice_text(name)).and_then(|()| fs::rename(&part, CHOSEN));
    if let Err(err) = written {
        let _ = fs::remove_file(&part);
        let why = if err.kind() == std::io::ErrorKind::PermissionDenied {
            "choosing a profile needs an administrator: run it with elevate(8)".to_string()
        } else {
            format!("{CHOSEN}: {err}")
        };
        return Err(fail(why));
    }
    // The notice about virtual machines is for those who chose nothing.
    sign_in_notices::put("tune", &[]);
    // A profile that does not apply well ends the service with a failure,
    // so the restart says so; what it could not set is said either way.
    let restarted = crate::service::change("restart", "tune", &["-q".to_string()]);
    if restarted.is_ok() {
        println!("the profile is {name}");
    }
    if let Ok(text) = fs::read_to_string(PROBLEMS) {
        print!("{text}");
    }
    restarted
}

/// What the kernel's settings are, through the capability the `tune`
/// service holds.
fn held_settings() -> Result<Vec<Setting>, Fail> {
    let mut settings = Vec::new();
    for index in 0.. {
        let mut line = [0u8; LINE_MAX as usize];
        match abi::list(&SYS, SLOT_SETTINGS, index, &mut line) {
            Ok(reply) => {
                let len = usize::try_from(reply.len).unwrap_or(0).min(line.len());
                let text = String::from_utf8_lossy(&line[..len]);
                let setting = parse_setting(&text)
                    .ok_or_else(|| fail(format!("the kernel's line does not read: {text}")))?;
                settings.push(setting);
            }
            Err(status) if matches!(Kernel::from_status(status), Some(Kernel::OutOfRange)) => break,
            Err(status) => {
                return Err(fail(format!(
                    "cannot list the kernel's settings: {}; `tune apply` is for the tune service, \
                     which holds the capability",
                    status_name(status.0)
                )));
            }
        }
    }
    Ok(settings)
}

/// Sets what `change` names, through the capability.
fn take(change: &Change) -> Taken {
    match abi::set(&SYS, SLOT_SETTINGS, change.name.as_bytes(), change.to.as_bytes()) {
        Ok(()) => Taken::Set,
        Err(status) => match Kernel::from_status(status) {
            Some(Kernel::NotSupported) => Taken::NotHere,
            _ => Taken::Refused(status_name(status.0)),
        },
    }
}

/// `tune apply`, which the `tune` service runs at each start and again
/// when a profile is chosen: gives the kernel the settings of the profile
/// in use, and says what it could not.
fn apply() -> Result<i32, Fail> {
    let (name, chosen) = in_use()?;
    let profile = profile(&name)?;
    let (changes, unknown) = plan(&held_settings()?, &profile);
    let results: Vec<(Change, Taken)> = changes
        .into_iter()
        .map(|change| {
            let taken = take(&change);
            (change, taken)
        })
        .collect();
    println!("{}", summary(&name, &results));
    let (lines, faulty) = problems(&results, &unknown);
    for line in &lines {
        eprintln!("{line}");
    }
    keep_problems(&lines);
    tell_virtual_machine(chosen);
    Ok(i32::from(faulty))
}

/// Keeps `lines` for `tune set` to say, or removes what an earlier apply
/// left when there are none.
fn keep_problems(lines: &[String]) {
    if lines.is_empty() {
        let _ = fs::remove_file(PROBLEMS);
        return;
    }
    let text = lines.iter().fold(String::new(), |text, line| text + line + "\n");
    let kept = Path::new(PROBLEMS)
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(PROBLEMS, text));
    if let Err(err) = kept {
        eprintln!("cannot keep what it could not set in {PROBLEMS}: {err}");
    }
}

/// Tells the sign-in screens once, in a virtual machine where no profile
/// was chosen, that the guest's profile suits it.
fn tell_virtual_machine(chosen: bool) {
    let given = Path::new(NOTICE_GIVEN).exists();
    if !notice_due(in_virtual_machine(), chosen, given) {
        return;
    }
    if !sign_in_notices::put("tune", &[VM_NOTICE.to_string()]) {
        return;
    }
    let marked = Path::new(NOTICE_GIVEN)
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(NOTICE_GIVEN, "given\n"));
    if let Err(err) = marked {
        eprintln!("cannot mark the notice as given in {NOTICE_GIVEN}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THROUGHPUT: &str = r#"# SPDX-FileCopyrightText: 2026 The Dukovo contributors

description = "Most work done a second"

[settings]
# Never trades the CPUs' speed for power.
"cpu.energy_preference" = "performance"
# A longer slice means fewer switches between threads that run long.
# They run a little less fairly.
"sched.slice_us" = 6000
"disk.read_ahead_kib" = 1024
"#;

    fn setting(name: &str, value: &str, start: &str) -> Setting {
        Setting { name: name.into(), value: value.into(), start: start.into() }
    }

    fn kernel() -> Vec<Setting> {
        vec![
            setting("cpu.energy_preference", "firmware", "firmware"),
            setting("sched.slice_us", "3000", "3000"),
            setting("disk.read_ahead_kib", "128", "128"),
            setting("net.congestion", "cubic", "cubic"),
            setting("cpu.boost", "on", "on"),
            setting("cpu.speed", "chosen", "chosen"),
        ]
    }

    fn throughput() -> Profile {
        parse_profile("throughput", THROUGHPUT).unwrap()
    }

    #[test]
    fn a_profile_reads_with_its_reasons() {
        let profile = throughput();
        assert_eq!(profile.description, "Most work done a second");
        let seen: Vec<(&str, &str, &str)> = profile
            .settings
            .iter()
            .map(|c| (c.name.as_str(), c.value.as_str(), c.why.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("cpu.energy_preference", "performance", "Never trades the CPUs' speed for power."),
                ("disk.read_ahead_kib", "1024", ""),
                (
                    "sched.slice_us",
                    "6000",
                    "A longer slice means fewer switches between threads that run long. They run \
                     a little less fairly."
                ),
            ]
        );
    }

    #[test]
    fn a_profile_with_no_settings_reads() {
        let profile = parse_profile("balanced", "description = \"The default\"\n").unwrap();
        assert!(profile.settings.is_empty());
        assert_eq!(
            description(&profile),
            ["balanced: The default", "  It sets nothing: every setting is at its start value."]
        );
    }

    #[test]
    fn a_profile_that_does_not_read_says_which() {
        let no_description = parse_profile("odd", "[settings]\n\"sched.slice_us\" = 1\n");
        assert!(no_description.unwrap_err().starts_with("odd: "));
        let flag = parse_profile("odd", "description = \"x\"\n[settings]\n\"net.x\" = true\n");
        assert!(flag.is_err());
        let extra = parse_profile("odd", "description = \"x\"\nmood = 1\n");
        assert!(extra.unwrap_err().contains("mood"));
    }

    #[test]
    fn the_choice_is_written_and_read_back() {
        assert_eq!(parse_choice(&choice_text("vm-guest")), Ok("vm-guest".to_string()));
        assert!(choice_text("low-latency").ends_with("profile = \"low-latency\"\n"));
        assert!(parse_choice("profile = 3\n").is_err());
        assert!(parse_choice("profile = \"a\"\nmore = 1\n").is_err());
    }

    #[test]
    fn the_kernel_s_lines_read() {
        assert_eq!(
            parse_setting("sched.slice_us 6000 3000 500-20000"),
            Some(setting("sched.slice_us", "6000", "3000"))
        );
        assert_eq!(
            parse_setting("net.congestion cubic cubic cubic reno"),
            Some(setting("net.congestion", "cubic", "cubic"))
        );
        // Without the values it takes, the line is not whole.
        assert_eq!(parse_setting("sched.slice_us 6000 3000"), None);
        assert_eq!(parse_setting(""), None);
    }

    #[test]
    fn a_profile_sets_what_it_lists_and_nothing_else() {
        let (changes, unknown) = plan(&kernel(), &throughput());
        let names: Vec<(&str, &str, &str)> =
            changes.iter().map(|c| (c.name.as_str(), c.from.as_str(), c.to.as_str())).collect();
        assert_eq!(
            names,
            [
                ("cpu.energy_preference", "firmware", "performance"),
                ("sched.slice_us", "3000", "6000"),
                ("disk.read_ahead_kib", "128", "1024"),
            ]
        );
        assert!(unknown.is_empty());
    }

    #[test]
    fn a_setting_at_its_value_is_not_set_again() {
        let mut now = kernel();
        now[1].value = "6000".into();
        let (changes, _) = plan(&now, &throughput());
        assert!(changes.iter().all(|c| c.name != "sched.slice_us"));
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn every_other_setting_goes_back_to_its_start_value() {
        // Throughput was applied; low-latency lists the slice alone.
        let mut now = kernel();
        now[0].value = "performance".into();
        now[1].value = "6000".into();
        now[2].value = "1024".into();
        let low_latency = parse_profile(
            "low-latency",
            "description = \"x\"\n[settings]\n\"sched.slice_us\" = 1000\n",
        )
        .unwrap();
        let (changes, _) = plan(&now, &low_latency);
        let seen: Vec<(&str, &str)> =
            changes.iter().map(|c| (c.name.as_str(), c.to.as_str())).collect();
        assert_eq!(
            seen,
            [
                ("cpu.energy_preference", "firmware"),
                ("sched.slice_us", "1000"),
                ("disk.read_ahead_kib", "128")
            ]
        );
        // The default lists nothing: all go back.
        let balanced = parse_profile("balanced", "description = \"x\"\n").unwrap();
        let (changes, _) = plan(&now, &balanced);
        let back: Vec<&str> = changes.iter().map(|c| c.to.as_str()).collect();
        assert_eq!(back, ["firmware", "3000", "128"]);
        // And with everything at its start, there is nothing to do.
        assert_eq!(plan(&kernel(), &balanced), (Vec::new(), Vec::new()));
    }

    #[test]
    fn a_setting_the_kernel_lacks_is_named() {
        let odd = parse_profile(
            "odd",
            "description = \"x\"\n[settings]\n\"net.tcp_buffer_kib\" = 4096\n",
        )
        .unwrap();
        let (changes, unknown) = plan(&kernel(), &odd);
        assert!(changes.is_empty());
        assert_eq!(unknown, ["net.tcp_buffer_kib"]);
    }

    fn change(name: &str, from: &str, to: &str) -> Change {
        Change { name: name.into(), from: from.into(), to: to.into() }
    }

    #[test]
    fn a_setting_this_machine_cannot_have_is_one_line_and_no_fault() {
        let results = [
            (change("cpu.energy_preference", "firmware", "performance"), Taken::NotHere),
            (change("sched.slice_us", "3000", "6000"), Taken::Set),
            (change("disk.read_ahead_kib", "128", "1024"), Taken::Set),
        ];
        let (lines, faulty) = problems(&results, &[]);
        assert_eq!(
            lines,
            ["cpu.energy_preference: this machine cannot have performance; it stays firmware"]
        );
        assert!(!faulty);
        assert_eq!(
            summary("throughput", &results),
            "profile throughput: set sched.slice_us to 6000, disk.read_ahead_kib to 1024"
        );
    }

    #[test]
    fn a_refused_value_and_a_missing_setting_are_faults_of_the_profile() {
        let results = [(
            change("sched.slice_us", "3000", "60000"),
            Taken::Refused("invalid_argument".into()),
        )];
        let (lines, faulty) = problems(&results, &["net.tcp_buffer_kib".to_string()]);
        assert_eq!(
            lines,
            [
                "sched.slice_us: the kernel refused 60000: invalid_argument",
                "net.tcp_buffer_kib: this kernel has no such setting"
            ]
        );
        assert!(faulty);
        assert_eq!(summary("odd", &results), "profile odd: nothing to change");
    }

    #[test]
    fn the_table_marks_what_the_profile_set() {
        let values: Vec<(String, String)> = [
            ("cpu.energy_preference", "firmware"),
            ("disk.read_ahead_kib", "1024"),
            ("net.congestion", "cubic"),
            ("sched.slice_us", "6000"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .into();
        assert_eq!(
            table(&throughput(), &values),
            [
                "  cpu.energy_preference  firmware  (the profile asks for performance; not in effect)",
                "* disk.read_ahead_kib    1024",
                "  net.congestion         cubic",
                "* sched.slice_us         6000",
            ]
        );
    }

    #[test]
    fn show_says_each_setting_with_its_reason() {
        assert_eq!(
            description(&throughput()),
            [
                "throughput: Most work done a second",
                "  cpu.energy_preference  performance  Never trades the CPUs' speed for power.",
                "  disk.read_ahead_kib    1024",
                "  sched.slice_us         6000         A longer slice means fewer switches \
                 between threads that run long. They run a little less fairly.",
                "Every other setting is at its start value.",
            ]
        );
    }

    #[test]
    fn the_profile_that_fits_follows_the_machine() {
        assert_eq!(recommended(true, false).0, "vm-guest");
        // A virtual machine's battery is the host's business.
        assert_eq!(recommended(true, true).0, "vm-guest");
        assert_eq!(recommended(false, true).0, "power-saver");
        assert_eq!(recommended(false, false).0, "balanced");
    }

    #[test]
    fn the_notice_is_for_a_virtual_machine_where_nothing_was_chosen_or_told() {
        assert!(notice_due(true, false, false));
        assert!(!notice_due(false, false, false));
        assert!(!notice_due(true, true, false));
        assert!(!notice_due(true, false, true));
    }

    /// The profiles Dukovo ships, as tune(8) finds them.
    const SHIPPED: [(&str, &str); 7] = [
        ("balanced", include_str!("../../../tune/balanced.toml")),
        ("benchmark", include_str!("../../../tune/benchmark.toml")),
        ("low-latency", include_str!("../../../tune/low-latency.toml")),
        ("power-saver", include_str!("../../../tune/power-saver.toml")),
        ("throughput", include_str!("../../../tune/throughput.toml")),
        ("vm-guest", include_str!("../../../tune/vm-guest.toml")),
        ("vm-host", include_str!("../../../tune/vm-host.toml")),
    ];

    fn shipped(name: &str) -> Profile {
        let (_, text) = SHIPPED.iter().find(|(shipped, _)| *shipped == name).unwrap();
        parse_profile(name, text).unwrap()
    }

    fn sets(name: &str) -> Vec<(String, String)> {
        shipped(name).settings.into_iter().map(|c| (c.name, c.value)).collect()
    }

    #[test]
    fn every_shipped_profile_reads_with_a_reason_for_each_setting() {
        let folder = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tune");
        assert_eq!(fs::read_dir(folder).unwrap().count(), SHIPPED.len(), "a profile is not here");
        for (name, text) in SHIPPED {
            let profile = parse_profile(name, text).unwrap_or_else(|why| panic!("{why}"));
            // `tune list` writes it after the name, on a line of 80.
            assert!(!profile.description.is_empty() && profile.description.len() <= 54, "{name}");
            for choice in &profile.settings {
                assert!(!choice.why.is_empty(), "{name}: {} has no reason", choice.name);
                assert!(choice.why.len() <= 76, "{name}: {}'s reason is one line", choice.name);
            }
        }
    }

    #[test]
    fn every_shipped_setting_is_one_the_kernel_takes() {
        let words = ["firmware", "performance", "balance_performance", "balance_power", "power"];
        let speeds = ["chosen", "highest"];
        for (name, _) in SHIPPED {
            for (setting, value) in sets(name) {
                let number: u64 = value.parse().unwrap_or(0);
                let takes = match setting.as_str() {
                    "cpu.energy_preference" => words.contains(&value.as_str()),
                    "cpu.boost" => ["on", "off"].contains(&value.as_str()),
                    "cpu.speed" => speeds.contains(&value.as_str()),
                    "sched.slice_us" => (500..=20_000).contains(&number),
                    "disk.read_ahead_kib" => number <= 1024 && number.is_multiple_of(4),
                    _ => false,
                };
                assert!(takes, "{name}: {setting} = {value}");
            }
        }
    }

    #[test]
    fn the_profiles_set_what_the_design_says() {
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
        };
        assert_eq!(sets("balanced"), pairs(&[]));
        assert_eq!(
            sets("benchmark"),
            pairs(&[
                ("cpu.boost", "off"),
                ("cpu.energy_preference", "performance"),
                ("cpu.speed", "highest")
            ])
        );
        assert_eq!(
            sets("throughput"),
            pairs(&[
                ("cpu.energy_preference", "performance"),
                ("disk.read_ahead_kib", "1024"),
                ("sched.slice_us", "6000")
            ])
        );
        assert_eq!(
            sets("low-latency"),
            pairs(&[("cpu.energy_preference", "performance"), ("sched.slice_us", "1000")])
        );
        assert_eq!(
            sets("power-saver"),
            pairs(&[("cpu.energy_preference", "power"), ("sched.slice_us", "4000")])
        );
        assert_eq!(
            sets("vm-host"),
            pairs(&[
                ("cpu.energy_preference", "performance"),
                ("disk.read_ahead_kib", "512"),
                ("sched.slice_us", "6000")
            ])
        );
        assert_eq!(
            sets("vm-guest"),
            pairs(&[("disk.read_ahead_kib", "512"), ("sched.slice_us", "4000")])
        );
    }

    #[test]
    fn any_profile_then_the_default_leaves_every_setting_at_its_start() {
        let start = kernel();
        for (name, _) in SHIPPED {
            let mut now = start.clone();
            for profile in [shipped(name), shipped("balanced")] {
                let (changes, unknown) = plan(&now, &profile);
                assert!(unknown.is_empty(), "{name}");
                for change in changes {
                    now.iter_mut().find(|s| s.name == change.name).unwrap().value = change.to;
                }
            }
            assert_eq!(now, start, "{name}");
        }
    }

    #[test]
    fn profile_names_are_plain_words() {
        for name in ["", "../balanced", "Balanced", "a b", "x.toml"] {
            assert!(profile(name).unwrap_err().message.contains("no profile is called"), "{name}");
        }
    }
}
