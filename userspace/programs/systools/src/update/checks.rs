// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `update checks`, which says what a new version of the system must do
//! to prove itself (health.toml(5)), and `update checks add`, `remove` and
//! `essential`, which change the checks and the essential services with
//! [`settings_edit`], so that the comments and the order of the file stay.
//!
//! The two kinds of check a person makes here are a web address that
//! answers and a service that runs; each is one command, `curl` or
//! `systemctl`, and is read back as what it was made of. A check that was
//! written by hand with another command is a command.

use std::path::Path;

use serde::Serialize;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::args::{Arg, Args};
use crate::settings_edit::{self, Changed};
use crate::sysupdate::health::{CONFIG, Check, Config};
use crate::util::{self, Fail, fail, usage};

/// The most seconds a check may be given.
pub(crate) const TIMEOUT_MAX: u64 = 600;

/// The seconds a check is given when it is not said.
const TIMEOUT: u64 = 30;

/// The longest web address.
pub(crate) const URL_MAX: usize = 2048;

/// The longest name of a check.
pub(crate) const NAME_MAX: usize = 64;

/// The members of `update checks --json` after `format`.
#[derive(Debug, Serialize)]
pub(crate) struct Document {
    /// The services the machine needs.
    essential: Vec<String>,
    /// Seconds they must run without a restart.
    stable: u64,
    /// The most seconds an update has to start them.
    deadline: u64,
    checks: Vec<Row>,
}

/// One check, as the health check reads it, and what it was made of.
#[derive(Debug, Serialize)]
struct Row {
    name: String,
    command: Vec<String>,
    required: bool,
    timeout: u64,
    /// `url`, `service` or `command`.
    kind: &'static str,
    /// The address, the service's name, or the command's words in one line.
    target: String,
}

impl Document {
    /// What the health check goes by, `config`.
    pub(crate) fn of(config: &Config) -> Document {
        Document {
            essential: config.essential.clone(),
            stable: config.stable,
            deadline: config.deadline,
            checks: config.checks.iter().map(Row::of).collect(),
        }
    }
}

impl Row {
    fn of(check: &Check) -> Row {
        let (kind, target) = match made_of(&check.command) {
            Made::Url { url, .. } => ("url", url.to_string()),
            Made::Service(name) => ("service", name.to_string()),
            Made::Command => ("command", check.command.join(" ")),
        };
        Row {
            name: check.name.clone(),
            command: check.command.clone(),
            required: check.required,
            timeout: check.timeout,
            kind,
            target,
        }
    }
}

/// What a check's command is made of.
#[derive(Debug, PartialEq, Eq)]
enum Made<'a> {
    /// `curl -fsS [-k] --max-time S URL`.
    Url { url: &'a str, insecure: bool },
    /// `systemctl is-active NAME`.
    Service(&'a str),
    /// Anything else.
    Command,
}

/// What `command` is made of: the words `update checks add` makes, and no
/// others.
fn made_of(command: &[String]) -> Made<'_> {
    let mut words: Vec<&str> = command.iter().map(String::as_str).collect();
    // A check written before `-g` was has none, and reads the same.
    if words.get(..3) == Some(&["curl", "-fsS", "-g"][..]) {
        words.remove(2);
    }
    match words[..] {
        ["curl", "-fsS", "--max-time", seconds, url] if seconds.parse::<u64>().is_ok() => {
            Made::Url { url, insecure: false }
        }
        ["curl", "-fsS", "-k", "--max-time", seconds, url] if seconds.parse::<u64>().is_ok() => {
            Made::Url { url, insecure: true }
        }
        ["systemctl", "is-active", name] => Made::Service(name),
        _ => Made::Command,
    }
}

/// The words `update checks add` makes of a web address: `-g` has curl
/// take the address as it is, so that brackets and braces in it are not a
/// list of addresses to ask for in turn.
fn url_command(url: &str, insecure: bool, timeout: u64) -> Vec<String> {
    let mut words = vec!["curl".to_string(), "-fsS".to_string(), "-g".to_string()];
    if insecure {
        words.push("-k".into());
    }
    words.extend(["--max-time".to_string(), timeout.to_string(), url.to_string()]);
    words
}

/// What `update checks add` was given.
#[derive(Debug, Default)]
struct New {
    name: Option<String>,
    url: Option<String>,
    service: Option<String>,
    insecure: bool,
    required: bool,
    timeout: Option<u64>,
}

/// `update checks`: the checks, in words, or with `json` as the document
/// update(8) describes; and its `add`, `remove` and `essential`.
pub(crate) fn checks(args: &mut Args) -> Result<i32, Fail> {
    match args.subcommand().map(|word| word.to_string_lossy().into_owned()).as_deref() {
        None => show(args),
        Some("add") => add(args),
        Some("remove") => remove(args),
        Some("essential") => essential(args),
        Some(other) => Err(usage(format!("update checks {other}: no such command"))),
    }
}

/// `update checks [--json]`.
fn show(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let (config, why) = Config::read();
    if let Some(why) = why {
        return Err(fail(why));
    }
    let document = Document::of(&config);
    if json {
        return crate::json::print(&document).map(|()| 0);
    }
    println!("essential services: {}", document.essential.join(", "));
    println!("stable for {} s, started within {} s", document.stable, document.deadline);
    for row in &document.checks {
        println!(
            "check {}: {} {}, {} s, {}",
            row.name,
            row.kind,
            row.target,
            row.timeout,
            if row.required { "required" } else { "wanted" }
        );
    }
    if document.checks.is_empty() {
        println!("no checks of your own");
    }
    Ok(0)
}

/// The name of a check, if it is one.
pub(crate) fn check_name(name: &str) -> Result<(), String> {
    let first = name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let fits = (1..=NAME_MAX).contains(&name.len())
        && first
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b" ._-".contains(&b));
    if fits {
        Ok(())
    } else {
        Err(format!(
            "'{name}' is not a check's name: use 1 to {NAME_MAX} letters, digits, spaces, `.`, \
             `_` and `-`, starting with a letter or a digit"
        ))
    }
}

/// The address of a web page, if it is one a check may ask for.
pub(crate) fn check_url(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    let plain = !url.chars().any(|c| c.is_control() || c.is_whitespace());
    if rest.is_some_and(|rest| !rest.is_empty()) && plain && url.len() <= URL_MAX {
        Ok(())
    } else {
        Err(format!(
            "'{url}' is not an address a check may ask for: it starts with http:// or https://, \
             has no spaces, and is at most {URL_MAX} characters"
        ))
    }
}

/// What the command line of `update checks add` asks for.
fn new(args: &mut Args) -> Result<New, Fail> {
    let mut new = New::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "url" && new.url.is_none() => new.url = Some(args.text()?),
            Arg::Long(name) if name == "service" && new.service.is_none() => {
                new.service = Some(args.text()?);
            }
            Arg::Long(name) if name == "insecure" => new.insecure = true,
            Arg::Long(name) if name == "required" => new.required = true,
            Arg::Long(name) if name == "timeout" && new.timeout.is_none() => {
                new.timeout = Some(args.number()?);
            }
            Arg::Operand(name) if new.name.is_none() => {
                new.name = Some(name.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(new)
}

/// The table `new` makes in health.toml.
///
/// # Errors
///
/// What is wrong with it.
fn made(new: &New) -> Result<(String, Vec<String>, bool, u64), String> {
    let name = new.name.clone().ok_or("a check needs a name")?;
    check_name(&name)?;
    let command = match (&new.url, &new.service) {
        (Some(url), None) => {
            check_url(url)?;
            url_command(url, new.insecure, new.timeout.unwrap_or(TIMEOUT))
        }
        (None, Some(service)) => {
            if new.insecure {
                return Err("--insecure is for --url".into());
            }
            manifest::service::check_name(service)?;
            ["systemctl", "is-active", service.as_str()].map(String::from).to_vec()
        }
        _ => return Err("give --url or --service, one of the two".into()),
    };
    let timeout = new.timeout.unwrap_or(TIMEOUT);
    if !(1..=TIMEOUT_MAX).contains(&timeout) {
        return Err(format!("--timeout is from 1 to {TIMEOUT_MAX} seconds"));
    }
    Ok((name, command, new.required, timeout))
}

/// Whether the `[[check]]` table `table` is called `name`.
fn is_called(table: &Table, name: &str) -> bool {
    table.get("name").and_then(Item::as_str) == Some(name)
}

/// Adds the check `(name, command, required, timeout)` to `document`.
///
/// # Errors
///
/// When a check is called that already.
fn added(
    document: &mut DocumentMut,
    (name, command, required, timeout): &(String, Vec<String>, bool, u64),
) -> Result<(), String> {
    let taken = document
        .get("check")
        .and_then(Item::as_array_of_tables)
        .is_some_and(|tables| tables.iter().any(|table| is_called(table, name)));
    if taken {
        return Err(format!("a check is called {name} already"));
    }
    let table = settings_edit::push_table(document, "check")
        .ok_or("`check` in the file is not a list of checks")?;
    table.insert("name", Item::Value(Value::from(name.as_str())));
    let words: Array = command.iter().map(String::as_str).collect();
    table.insert("command", Item::Value(Value::Array(words)));
    table.insert("required", Item::Value(Value::from(*required)));
    table.insert("timeout", Item::Value(Value::from(i64::try_from(*timeout).unwrap_or(0))));
    Config::parse(&document.to_string()).map(|_| ())
}

/// Writes health.toml with `edit`; whether anything changed.
fn write(edit: impl FnOnce(&mut DocumentMut) -> Result<(), String>) -> Result<bool, Fail> {
    let path = Path::new(CONFIG);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|why| util::failed(&folder.display(), why))?;
    }
    settings_edit::change(path, 0o644, edit).map_err(|why| match why {
        Changed::Edit { error, .. } => fail(error),
        other => fail(other.to_string()),
    })
}

/// `update checks add NAME`.
fn add(args: &mut Args) -> Result<i32, Fail> {
    let new = new(args)?;
    let check = made(&new).map_err(fail)?;
    write(|document| added(document, &check))?;
    println!("update: check {} is added", check.0);
    Ok(0)
}

/// `update checks remove NAME`.
fn remove(args: &mut Args) -> Result<i32, Fail> {
    let mut name = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let name = name.ok_or_else(|| usage("update checks remove needs the check's name".into()))?;
    write(|document| removed(document, &name))?;
    println!("update: check {name} is removed");
    Ok(0)
}

/// Takes the check `name` out of `document`.
///
/// # Errors
///
/// When none is called that.
fn removed(document: &mut DocumentMut, name: &str) -> Result<(), String> {
    if settings_edit::remove_table(document, "check", |table| is_called(table, name)) {
        Ok(())
    } else {
        Err(format!("no check is called {name}"))
    }
}

/// `update checks essential SERVICE on|off`.
fn essential(args: &mut Args) -> Result<i32, Fail> {
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if operands.len() < 2 => {
                operands.push(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let [service, on] = &operands[..] else {
        return Err(usage("update checks essential needs a service's name and on or off".into()));
    };
    let on = match on.as_str() {
        "on" => true,
        "off" => false,
        other => return Err(usage(format!("update checks essential: '{other}' is not on or off"))),
    };
    manifest::service::check_name(service).map_err(usage)?;
    let (config, _) = Config::read();
    let mut changed = false;
    write(|document| {
        changed = flipped(document, &config.essential, service, on);
        Config::parse(&document.to_string()).map(|_| ())
    })?;
    match (on, changed) {
        (true, true) => println!("update: {service} is now an essential service"),
        (true, false) => println!("update: {service} is an essential service already"),
        (false, true) => println!("update: {service} is no longer an essential service"),
        (false, false) => println!("update: {service} is not an essential service"),
    }
    Ok(0)
}

/// Makes `service` an essential service of `document`, or not, by the
/// list `listed` the health check goes by while the file has none;
/// whether that changed anything.
fn flipped(document: &mut DocumentMut, listed: &[String], service: &str, on: bool) -> bool {
    let root = document.as_table_mut();
    if root.get("essential").is_none() {
        // The services the check goes by without a list are the list to
        // change, which the file then says.
        let kept = listed.contains(&service.to_string());
        if kept == on {
            return false;
        }
        settings_edit::set_strings(root, "essential", listed);
    }
    if on {
        settings_edit::push_string(root, "essential", service)
    } else {
        settings_edit::remove_string(root, "essential", service)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    /// health.toml as it ships, which has only comments for checks.
    const SHIPPED: &str = "\
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# What a new version of the system must do (health.toml(5)).

# The services the machine needs, by name.
essential = [\"logd\", \"authd\", \"consoled\", \"netd\", \"sshd\"]

# Seconds they must run without a restart.
stable = 120

# The most seconds an update has to start them.
deadline = 600

# Checks of your own: a command that passes by exiting with 0.
#
# [[check]]
# name = \"web site\"
# command = [\"curl\", \"-fsS\", \"http://localhost/\"]
# required = true
# timeout = 30
";

    fn args(items: &[&str]) -> Args {
        Args::new("update", items.iter().map(std::ffi::OsString::from).collect())
    }

    /// The check that `said` asks `update checks add` for, or what is
    /// wrong with it.
    fn asked(said: &[&str]) -> Result<(String, Vec<String>, bool, u64), String> {
        made(&new(&mut args(said)).map_err(|why| why.message)?)
    }

    fn with(text: &str, said: &[&str]) -> Result<String, String> {
        let check = asked(said)?;
        let made = settings_edit::edited(text, |document| added(document, &check)).map_err(
            |why| match why {
                settings_edit::Unedited::Edit(why) => why,
                settings_edit::Unedited::Parse { message, .. } => message,
            },
        )?;
        Ok(made.unwrap_or_else(|| text.to_string()))
    }

    fn without(text: &str, name: &str) -> Result<String, String> {
        settings_edit::edited(text, |document| removed(document, name))
            .map(|made| made.unwrap_or_else(|| text.to_string()))
            .map_err(|why| match why {
                settings_edit::Unedited::Edit(why) => why,
                settings_edit::Unedited::Parse { message, .. } => message,
            })
    }

    fn flip(text: &str, service: &str, on: bool) -> (String, bool) {
        let config = Config::parse(text).unwrap();
        let mut changed = false;
        let made = settings_edit::edited(text, |document| {
            changed = flipped(document, &config.essential, service, on);
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_or_else(|_| unreachable!());
        (made.unwrap_or_else(|| text.to_string()), changed)
    }

    #[test]
    fn a_url_and_a_service_are_each_one_command_that_reads_back_as_what_made_it() {
        let url = asked(&["web site", "--url", "https://localhost/", "--required"]).unwrap();
        assert_eq!(
            url,
            (
                "web site".into(),
                ["curl", "-fsS", "-g", "--max-time", "30", "https://localhost/"]
                    .map(String::from)
                    .to_vec(),
                true,
                30
            )
        );
        let insecure =
            asked(&["web site", "--url", "https://localhost/", "--insecure", "--timeout", "5"])
                .unwrap();
        assert_eq!(insecure.1[..5], ["curl", "-fsS", "-g", "-k", "--max-time"]);
        assert_eq!((insecure.2, insecure.3), (false, 5));
        let service = asked(&["nginx up", "--service", "nginx"]).unwrap();
        assert_eq!(service.1, ["systemctl", "is-active", "nginx"]);
        assert_eq!(made_of(&url.1), Made::Url { url: "https://localhost/", insecure: false });
        assert_eq!(made_of(&insecure.1), Made::Url { url: "https://localhost/", insecure: true });
        assert_eq!(made_of(&service.1), Made::Service("nginx"));
        let own = ["findmnt", "/volumes/BACKUP"].map(String::from);
        assert_eq!(made_of(&own), Made::Command);
        // A check made before `-g` reads as what made it too.
        let older = ["curl", "-fsS", "--max-time", "30", "https://localhost/"].map(String::from);
        assert_eq!(made_of(&older), Made::Url { url: "https://localhost/", insecure: false });
        let by_hand = ["curl", "-fsS", "http://localhost/"].map(String::from);
        assert_eq!(made_of(&by_hand), Made::Command);
    }

    #[test]
    fn a_check_that_is_not_right_is_refused_with_what_is_wrong() {
        for (said, words) in [
            (&["x"][..], "--url or --service"),
            (&["x", "--url", "https://a/", "--service", "nginx"], "--url or --service"),
            (&["x", "--url", "ftp://a/"], "is not an address"),
            (&["x", "--url", "https://"], "is not an address"),
            (&["x", "--url", "https://a b/"], "is not an address"),
            (&["x", "--service", "nginx", "--insecure"], "--insecure"),
            (&["x", "--service", "../x"], "is not a service's name"),
            (&["x", "--url", "https://a/", "--timeout", "0"], "--timeout"),
            (&["x", "--url", "https://a/", "--timeout", "601"], "--timeout"),
            (&["--url", "https://a/"], "needs a name"),
            (&[" x", "--url", "https://a/"], "starting with"),
            (&["--", "-x"], "starting with"),
        ] {
            let why = asked(said).unwrap_err();
            assert!(why.contains(words), "{said:?}: {why}");
        }
        let long = format!("https://example.org/{}", "a".repeat(2040));
        assert!(asked(&["x", "--url", &long]).is_err());
        assert!(asked(&[&"n".repeat(65), "--service", "nginx"]).is_err());
        assert!(asked(&[&"n".repeat(64), "--service", "nginx"]).is_ok());
    }

    #[test]
    fn a_check_goes_after_the_comments_that_end_the_file_and_comes_out_again() {
        let text =
            with(SHIPPED, &["web site", "--url", "https://localhost/", "--required"]).unwrap();
        assert!(Config::parse(&text).is_ok());
        let (head, table) = text.split_once("\n[[check]]\n").unwrap();
        assert!(head.ends_with("# timeout = 30\n"), "{head:?}");
        assert!(head.contains("# Checks of your own"));
        assert_eq!(
            table,
            "name = \"web site\"\ncommand = [\"curl\", \"-fsS\", \"-g\", \"--max-time\", \"30\", \
             \"https://localhost/\"]\nrequired = true\ntimeout = 30\n"
        );
        assert_eq!(without(&text, "web site").unwrap(), SHIPPED);
    }

    #[test]
    fn a_second_check_has_a_blank_line_before_it_and_a_name_in_use_is_refused() {
        let one = with(SHIPPED, &["web site", "--url", "https://localhost/"]).unwrap();
        let two = with(&one, &["nginx", "--service", "nginx", "--required"]).unwrap();
        assert!(two.contains("timeout = 30\n\n[[check]]\nname = \"nginx\"\n"), "{two}");
        let config = Config::parse(&two).unwrap();
        let names: Vec<&str> = config.checks.iter().map(|check| check.name.as_str()).collect();
        assert_eq!(names, ["web site", "nginx"]);
        assert!(config.checks[1].required && !config.checks[0].required);
        let why = with(&two, &["nginx", "--service", "sshd"]).unwrap_err();
        assert_eq!(why, "a check is called nginx already");
        // The comments above a check go with it.
        let back = without(&two, "web site").unwrap();
        assert!(back.contains("[[check]]\nname = \"nginx\"") && !back.contains("web site"));
        assert!(!back.contains("# Checks of your own"));
        assert!(Config::parse(&back).is_ok());
    }

    #[test]
    fn a_check_that_is_not_there_is_not_removed() {
        assert_eq!(without(SHIPPED, "web site").unwrap_err(), "no check is called web site");
        assert_eq!(without("", "x").unwrap_err(), "no check is called x");
    }

    #[test]
    fn a_check_in_a_file_with_nothing_else_makes_the_file() {
        let text = with("", &["web site", "--service", "nginx"]).unwrap();
        assert_eq!(
            text,
            "[[check]]\nname = \"web site\"\ncommand = [\"systemctl\", \"is-active\", \"nginx\"]\n\
             required = false\ntimeout = 30\n"
        );
        assert_eq!(without(&text, "web site").unwrap(), "");
    }

    #[test]
    fn an_essential_service_is_added_and_taken_out_of_the_list_in_its_place() {
        let (text, changed) = flip(SHIPPED, "nginx", true);
        assert!(changed);
        assert_eq!(text, SHIPPED.replace("\"netd\", \"sshd\"]", "\"netd\", \"sshd\", \"nginx\"]"));
        let (back, changed) = flip(&text, "nginx", false);
        assert!(changed);
        assert_eq!(back, SHIPPED);
        let (text, _) = flip(SHIPPED, "logd", false);
        assert!(
            text.contains("essential = [\"authd\", \"consoled\", \"netd\", \"sshd\"]"),
            "{text}"
        );
        let (same, changed) = flip(SHIPPED, "sshd", true);
        assert!(!changed);
        assert_eq!(same, SHIPPED);
        let (same, changed) = flip(SHIPPED, "nginx", false);
        assert!(!changed);
        assert_eq!(same, SHIPPED);
    }

    #[test]
    fn a_file_with_no_list_makes_the_default_one_the_list_to_change() {
        let (text, changed) = flip("", "nginx", true);
        assert!(changed);
        assert_eq!(
            text,
            "essential = [\"logd\", \"authd\", \"consoled\", \"netd\", \"sshd\", \"nginx\"]\n"
        );
        let (text, changed) = flip("stable = 90\n", "sshd", false);
        assert!(changed);
        assert!(text.contains("essential = [\"logd\", \"authd\", \"consoled\", \"netd\"]"));
        let (text, changed) = flip("stable = 90\n", "sshd", true);
        assert!(!changed);
        assert_eq!(text, "stable = 90\n");
    }

    #[test]
    fn the_document_says_each_check_and_what_it_is() {
        let config = Config::parse(
            "essential = [\"logd\", \"sshd\"]\nstable = 90\n\n\
             [[check]]\nname = \"web site\"\ncommand = [\"curl\", \"-fsS\", \"-k\", \"--max-time\", \"20\", \"https://localhost/\"]\nrequired = true\ntimeout = 20\n\n\
             [[check]]\nname = \"nginx\"\ncommand = [\"systemctl\", \"is-active\", \"nginx\"]\n\n\
             [[check]]\nname = \"backup disk\"\ncommand = [\"findmnt\", \"/volumes/BACKUP\"]\n",
        )
        .unwrap();
        let line = json::line(&Document::of(&config)).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/update-checks.json"));
    }
}
