// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! systemctl(1): the verbs an administrator knows from Linux, over the
//! services Spark supervises and their timers (ADR 0478). Each verb is
//! `service`'s: `start`, `stop` and `restart` are `service::change`,
//! `status` is `service::status`, `enable` and `disable` are
//! `servicefiles::enable`, and the rest read Spark's `show` document
//! (`service::shown`) and say it in systemd's words and statuses:
//! `is-active`, `is-enabled`, `is-failed`, `list-units`, `show -p` and
//! `cat`. `list-timers` and the `.timer` names ask timerd(8).
//!
//! A unit is a service: a `.service` suffix is taken and left off, and
//! `NAME.timer` is the timer in NAME's service file. Every other kind of
//! unit, option and verb says what to run instead and ends with status 1:
//! `advice` and `elsewhere` hold the sentences. The boot test
//! `admin-commands` runs the verbs against Spark.

use std::fs;
use std::path::Path;

use spark_api::show::{Service, State};
use spark_api::words::span;

use crate::args::{Arg, Args};
use crate::timerd::protocol::{self, Reply, Request};
use crate::util::{self, Fail, fail, fail_with, usage};
use crate::{service, servicefiles};

const USAGE: &str = "Usage: systemctl start | stop | restart | reload | status | enable | disable NAME...\n       \
                     systemctl is-active | is-enabled | is-failed NAME...\n       \
                     systemctl list-units [--type=service|timer] [--all] [--failed]\n       \
                     systemctl list-timers | daemon-reload\n       \
                     systemctl cat NAME... | show [-p PROPERTY[,PROPERTY]] [--value] NAME...";

/// The kinds of unit `--type` takes.
const TYPES: [&str; 2] = ["service", "timer"];

/// What `show` says of a unit Spark does not know, as systemd does of a
/// unit it has no file for.
const UNKNOWN: &str = "not-found";

/// A timer as timerd(8) lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TimerRow {
    /// The service it starts.
    name: String,
    description: String,
    /// When it runs next and last, in seconds since the epoch.
    next: Option<i64>,
    last: Option<i64>,
}

/// The timers timerd(8) keeps, each with its next and last run and its
/// service's description.
fn timer_rows() -> Result<Vec<TimerRow>, Fail> {
    let reply = protocol::answer(&Request::Timers).map_err(fail)?;
    let Reply::Timers { timers } = reply else {
        return Err(fail("timerd answered something other than its timers".into()));
    };
    // The description is the service's, which Spark knows; a timer is
    // listed without one while Spark does not answer.
    let services = service::shown(None).map_or_default(|document| document.services);
    Ok(timers
        .into_iter()
        .map(|timer| TimerRow {
            description: services
                .iter()
                .find(|s| s.name == timer.name)
                .map_or_default(|s| s.description.clone()),
            name: timer.name,
            next: timer.next,
            last: timer.last,
        })
        .collect())
}

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// The kinds of unit `list-units` lists: all of them when empty.
    types: Vec<String>,
    all: bool,
    failed_only: bool,
    /// `show`'s properties to print: all of them when empty.
    properties: Vec<String>,
    value: bool,
    no_block: bool,
    quiet: bool,
    no_legend: bool,
    /// `status -n`'s log lines.
    lines: Option<String>,
}

/// systemctl(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut options = Options::default();
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('t') => options.types.extend(list(&args.text()?)),
            Arg::Long(l) if l == "type" => options.types.extend(list(&args.text()?)),
            Arg::Short('a') => options.all = true,
            Arg::Long(l) if l == "all" => options.all = true,
            Arg::Long(l) if l == "failed" => options.failed_only = true,
            Arg::Long(l) if l == "state" => options.failed_only = args.text()? == "failed",
            Arg::Long(l) if l == "value" => options.value = true,
            Arg::Short('p') => options.properties.extend(list(&args.text()?)),
            Arg::Long(l) if l == "property" => options.properties.extend(list(&args.text()?)),
            Arg::Long(l) if l == "no-block" => options.no_block = true,
            Arg::Short('q') => options.quiet = true,
            Arg::Long(l) if l == "quiet" => options.quiet = true,
            Arg::Long(l) if l == "no-legend" => options.no_legend = true,
            Arg::Short('n') => options.lines = Some(args.text()?),
            Arg::Long(l) if l == "lines" => options.lines = Some(args.text()?),
            // What systemctl takes that changes nothing here: Dukovo has no
            // pager to turn off, no password to ask for, and a service
            // that is enabled is started too (`service enable`).
            Arg::Short('l') => {}
            Arg::Long(l)
                if ["no-pager", "no-ask-password", "full", "plain", "now", "system", "help"]
                    .contains(&l.as_str()) => {}
            Arg::Long(l) if ["user", "global"].contains(&l.as_str()) => {
                return Err(usage(format!(
                    "--{l}: Dukovo has no services per person; `crontab` runs a person's jobs \
                     (crontab(1))"
                )));
            }
            Arg::Short('H' | 'M') => return Err(remote()),
            Arg::Long(l) if l == "host" || l == "machine" => return Err(remote()),
            Arg::Operand(word) => operands.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some((verb, names)) = operands.split_first() else {
        return Err(usage(USAGE.into()));
    };
    run(verb, names, &options)
}

/// What to say of an option that names another machine.
fn remote() -> Fail {
    usage("systemctl runs on this machine; run it on another with `ssh HOST systemctl ...`".into())
}

/// `value` as the words between commas: what `-p` and `--type` take.
fn list(value: &str) -> impl Iterator<Item = String> + '_ {
    util::list(value).map(str::to_string)
}

/// The verb `verb` over `names`.
fn run(verb: &str, names: &[String], options: &Options) -> Result<i32, Fail> {
    match verb {
        "start" | "stop" | "restart" => each_changing(verb, names, options),
        "reload" => each_changing(verb, names, options),
        "enable" | "disable" => each_changing(verb, names, options),
        "status" => each(names, verb, |unit| status(unit, options)),
        "is-active" => is_active(names, options),
        "is-enabled" => is_enabled(names, options),
        "is-failed" => is_failed(names, options),
        "list-units" => list_units(names, options),
        "list-timers" => list_timers(names),
        "cat" => each(names, verb, cat),
        "show" => show_units(names, options),
        "daemon-reload" => daemon_reload(),
        other => Err(usage(elsewhere(other))),
    }
}

/// What to run instead of the verb `verb`, or what systemctl takes.
fn elsewhere(verb: &str) -> String {
    let instead = match verb {
        "mask" | "unmask" => {
            "Dukovo has no masking: `elevate service NAME disable` stops a service and keeps it \
             from starting at boot"
        }
        "edit" | "set-property" | "revert" | "link" | "add-wants" | "add-requires" => {
            "`elevate service edit NAME` changes a service's file, and checks it before it is put \
             to use"
        }
        "isolate" | "default" | "rescue" | "emergency" | "get-default" | "set-default" => {
            "Dukovo has no targets: `service list` shows what runs, and `service NAME start` or \
             `stop` changes it"
        }
        "poweroff" => "`elevate poweroff` ends the machine",
        "reboot" | "kexec" => "`elevate reboot` restarts the machine",
        "halt" => "`elevate halt` stops the machine",
        "suspend" | "hibernate" | "hybrid-sleep" | "suspend-then-hibernate" => {
            "Dukovo does not suspend; `elevate poweroff` ends the machine"
        }
        "list-dependencies" => {
            "`service NAME status` shows what a service needs, and `service NAME explain` what \
             it waits for"
        }
        "list-unit-files" => {
            "`service list` lists the services; their files are in \
             /system/settings/spark/services"
        }
        "list-sockets" => "`ss -l` lists what listens",
        "list-jobs" | "cancel" => {
            "Dukovo has no job queue: `service NAME start` waits until the service has settled"
        }
        "kill" => "`elevate service NAME stop` stops a service, and `kill` signals a process",
        "try-restart"
        | "condition-restart"
        | "reload-or-restart"
        | "try-reload-or-restart"
        | "reenable" => "`elevate service NAME restart` starts a service that is stopped, too",
        "reset-failed" => "`elevate service NAME start` starts a service Spark gave up on again",
        "preset" | "preset-all" => {
            "services are set at install; `elevate service NAME enable` or `disable` changes one"
        }
        "show-environment" | "set-environment" | "unset-environment" | "import-environment" => {
            "Dukovo keeps no environment for services as a whole; a service's own is in its file \
             (`systemctl cat NAME`)"
        }
        "daemon-reexec" => "Spark stays as it is; `systemctl daemon-reload` checks the files",
        _ => {
            return format!(
                "no command '{verb}': `service` (service(8)) does the rest; systemctl takes \
                 start, stop, restart, reload, status, enable, disable, is-active, is-enabled, \
                 is-failed, list-units, list-timers, cat, show and daemon-reload"
            );
        }
    };
    format!("{verb}: {instead}")
}

/// A unit, as the command line names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit<'a> {
    Service(&'a str),
    Timer(&'a str),
}

impl<'a> Unit<'a> {
    /// The unit `text` names: `NAME`, `NAME.service` or `NAME.timer`.
    fn of(text: &'a str) -> Result<Unit<'a>, Fail> {
        if let Some(name) = text.strip_suffix(".timer") {
            return Ok(Unit::Timer(name));
        }
        if let Some((name, kind)) = text.rsplit_once('.')
            && [
                "socket",
                "mount",
                "automount",
                "target",
                "path",
                "slice",
                "scope",
                "swap",
                "device",
            ]
            .contains(&kind)
        {
            return Err(fail(format!(
                "{text}: Dukovo has services and their timers; a service's sockets and what it \
                 needs are in its file (`systemctl cat {name}`)"
            )));
        }
        Ok(Unit::Service(text.strip_suffix(".service").unwrap_or(text)))
    }
}

/// Runs `one` for each of `names`, as a service or its timer, and gives
/// the first status that is not 0, as systemctl does, after the last.
fn each(
    names: &[String],
    verb: &str,
    mut one: impl FnMut(Unit<'_>) -> Result<i32, Fail>,
) -> Result<i32, Fail> {
    if names.is_empty() {
        return Err(usage(format!("systemctl {verb} takes the name of a service")));
    }
    let mut status = 0;
    for name in names {
        let code = Unit::of(name).and_then(&mut one).unwrap_or_else(|failed| {
            if !failed.message.is_empty() {
                eprintln!("systemctl: {}", failed.message);
            }
            failed.code
        });
        if status == 0 {
            status = code;
        }
    }
    Ok(status)
}

/// `start`, `stop`, `restart`, `reload`, `enable` and `disable`: the
/// services change as `service` changes them, and these verbs need root
/// as `service`'s do.
fn each_changing(verb: &str, names: &[String], options: &Options) -> Result<i32, Fail> {
    each(names, verb, |unit| {
        let name = match unit {
            Unit::Service(name) => name,
            Unit::Timer(name) => {
                return Err(usage(format!(
                    "{name}.timer: a timer is part of {name}'s service file, which `elevate \
                     service edit {name}` changes; `systemctl list-timers` says when it runs"
                )));
            }
        };
        if util::own_user() != 0 {
            return Err(fail(format!(
                "only root may {verb} a service: elevate first, as `sudo systemctl {verb} {name}`"
            )));
        }
        match verb {
            "enable" | "disable" => servicefiles::enable(name, verb == "enable"),
            "reload" => service::ask(&format!("reload {name}")).map(|_| 0).map_err(not_found(name)),
            _ => {
                let mut flags = vec!["-q".to_string()];
                if options.no_block {
                    flags.push("--no-wait".into());
                }
                service::change(verb, name, &flags).map_err(not_found(name))
            }
        }
    })
}

/// Spark's refusal of a name it does not know as systemctl says it, with
/// its status, 5.
fn not_found(name: &str) -> impl FnOnce(Fail) -> Fail + '_ {
    move |failed| {
        if failed.message.starts_with("no service is called") {
            fail_with(5, format!("Unit {name}.service not found."))
        } else {
            failed
        }
    }
}

/// `status NAME`: service(8)'s, and a timer's next and last run.
fn status(unit: Unit<'_>, options: &Options) -> Result<i32, Fail> {
    match unit {
        Unit::Service(name) => {
            let lines: Vec<String> =
                options.lines.iter().flat_map(|n| ["-n".to_string(), n.clone()]).collect();
            let code = service::status(name, &lines)?;
            if let Some(line) = known(name)?.as_ref().and_then(triggered_by) {
                println!("{line}");
            }
            Ok(code)
        }
        Unit::Timer(name) => {
            let row = timer_row(name)?;
            print!("{}", timer_status_text(&row, now()));
            Ok(0)
        }
    }
}

/// The `TriggeredBy:` line of a service that waits for its sockets: what
/// systemd says of the units that start a service.
fn triggered_by(service: &Service) -> Option<String> {
    let waits = service.state == State::Listening || service.start == "on-connection";
    (waits && !service.holds.sockets.is_empty())
        .then(|| format!("TriggeredBy: its sockets ({})", service.holds.sockets.join(", ")))
}

/// The timer of service `name`; its absence is status 4, as an unknown
/// unit's.
fn timer_row(name: &str) -> Result<TimerRow, Fail> {
    timer_rows()?
        .into_iter()
        .find(|row| row.name == name)
        .ok_or_else(|| fail_with(4, format!("{name}.timer: no timer is set in {name}'s file")))
}

/// Seconds since the epoch, now.
fn now() -> i64 {
    i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(i64::MAX)
}

/// The lines of `status NAME.timer`.
fn timer_status_text(row: &TimerRow, now: i64) -> String {
    let mut text = format!("{}.timer - {}\n", row.name, row.description);
    text.push_str("   Active: active (waiting)\n");
    text.push_str(&format!("  Trigger: {}\n", when(row.next, now, "left")));
    text.push_str(&format!("     Last: {}\n", when(row.last, now, "ago")));
    text
}

/// A time as `list-timers` writes it, and how far it is from `now`:
/// `Sat 2026-11-14 02:30:00 UTC; 5 h 12 min left`, or `n/a`.
fn when(at: Option<i64>, now: i64, direction: &str) -> String {
    let Some(at) = at else { return "n/a".into() };
    format!("{}; {} {direction}", clock(at), span(at.abs_diff(now).saturating_mul(1000)))
}

/// A time in local time, with its zone.
fn clock(at: i64) -> String {
    util::local_time(at, "%a %Y-%m-%d %H:%M:%S %Z")
}

/// systemd's word for what a service is doing.
fn active_state(state: State) -> &'static str {
    match state {
        State::Running | State::Done => "active",
        State::Starting | State::Pending | State::Waiting => "activating",
        State::Stopping => "deactivating",
        State::Failed => "failed",
        // A service that waits for its first connection is an idle socket
        // unit to systemd: inactive until a client comes.
        State::Stopped | State::Disabled | State::Listening => "inactive",
    }
}

/// systemd's word for the detail of what a service is doing: the unit's
/// `SubState`.
fn sub_state(state: State) -> &'static str {
    match state {
        State::Running => "running",
        State::Done => "exited",
        State::Starting => "start",
        State::Pending => "start-pre",
        State::Waiting => "auto-restart",
        State::Stopping => "stop-sigterm",
        State::Failed => "failed",
        State::Stopped | State::Disabled | State::Listening => "dead",
    }
}

/// The service `name`, or none when Spark does not know it. Other
/// failures end the command.
fn known(name: &str) -> Result<Option<Service>, Fail> {
    match service::shown(Some(name)) {
        Ok(document) => Ok(document.services.into_iter().next()),
        Err(failed) if failed.code == 4 => Ok(None),
        Err(failed) => Err(failed),
    }
}

/// The text of `is-active`, `is-failed` and `is-enabled` for a service,
/// or for one Spark does not know, which is `inactive` as systemctl says.
fn answer_line(options: &Options, line: &str) {
    if !options.quiet {
        println!("{line}");
    }
}

/// `is-active NAME...`: each one's `ActiveState`; 0 if one is active,
/// 3 if none is, as systemctl.
fn is_active(names: &[String], options: &Options) -> Result<i32, Fail> {
    let mut active = false;
    each(names, "is-active", |unit| {
        let state = match unit {
            Unit::Service(name) => known(name)?.map_or("inactive", |s| active_state(s.state)),
            Unit::Timer(name) => {
                if timer_row(name).is_ok() {
                    "active"
                } else {
                    "inactive"
                }
            }
        };
        answer_line(options, state);
        active |= state == "active";
        Ok(0)
    })?;
    Ok(if active { 0 } else { 3 })
}

/// `is-failed NAME...`: each one's `ActiveState`; 0 if one has failed,
/// 1 if none has, as systemctl.
fn is_failed(names: &[String], options: &Options) -> Result<i32, Fail> {
    let mut failed = false;
    each(names, "is-failed", |unit| {
        let Unit::Service(name) = unit else {
            answer_line(options, "inactive");
            return Ok(0);
        };
        let state = known(name)?.map_or("inactive", |s| active_state(s.state));
        answer_line(options, state);
        failed |= state == "failed";
        Ok(0)
    })?;
    Ok(i32::from(!failed))
}

/// `is-enabled NAME...`: `enabled` or `disabled` for each; 0 if every
/// one is enabled, 1 if any is not, as systemctl. A service Spark does
/// not know is said as systemctl says a unit without a file.
fn is_enabled(names: &[String], options: &Options) -> Result<i32, Fail> {
    let mut all = true;
    each(names, "is-enabled", |unit| {
        let Unit::Service(name) = unit else {
            return Err(usage(
                "a timer runs while its service is enabled: `systemctl is-enabled NAME` says"
                    .into(),
            ));
        };
        let Some(service) = known(name)? else {
            return Err(fail(format!(
                "Failed to get unit file state for {name}.service: No such file or directory"
            )));
        };
        answer_line(options, if service.enabled { "enabled" } else { "disabled" });
        all &= service.enabled;
        Ok(0)
    })?;
    Ok(i32::from(!all))
}

/// One row of `list-units`.
#[derive(Debug, PartialEq, Eq)]
struct UnitRow {
    unit: String,
    active: &'static str,
    sub: &'static str,
    description: String,
}

/// The rows of the services `list-units` shows: those that are on their
/// way, running or failed, or all of them with `--all`; with `--failed`
/// those that failed alone.
fn service_rows(services: &[Service], options: &Options) -> Vec<UnitRow> {
    services
        .iter()
        .filter(|s| {
            if options.failed_only {
                s.state == State::Failed
            } else {
                options.all
                    || !matches!(s.state, State::Stopped | State::Disabled | State::Listening)
            }
        })
        .map(|s| UnitRow {
            unit: format!("{}.service", s.name),
            active: active_state(s.state),
            sub: sub_state(s.state),
            description: s.description.clone(),
        })
        .collect()
}

/// `list-units`' table: UNIT, LOAD, ACTIVE, SUB and DESCRIPTION, then
/// the count, as systemctl lays them out.
fn units_text(rows: &[UnitRow], options: &Options) -> String {
    let width = rows.iter().map(|r| r.unit.len()).chain([4]).max().unwrap_or(4);
    let mut text = String::new();
    if !options.no_legend {
        text.push_str(&format!("{:width$}  LOAD    ACTIVE     SUB          DESCRIPTION\n", "UNIT"));
    }
    for row in rows {
        text.push_str(&format!(
            "{:width$}  loaded  {:10} {:12} {}\n",
            row.unit, row.active, row.sub, row.description
        ));
    }
    if !options.no_legend {
        text.push_str(&format!("\n{} loaded units listed.", rows.len()));
        if !options.all && !options.failed_only {
            text.push_str(" Pass --all to see loaded but inactive units, too.");
        }
        text.push('\n');
    }
    text
}

/// `list-units [--type=service|timer] [--all] [--failed]`.
fn list_units(names: &[String], options: &Options) -> Result<i32, Fail> {
    if !names.is_empty() {
        return Err(usage(
            "systemctl list-units takes no name; `systemctl status NAME` says of one".into(),
        ));
    }
    if let Some(other) = options.types.iter().find(|t| !TYPES.contains(&t.as_str())) {
        return Err(usage(format!(
            "--type={other}: Dukovo has services and timers; `--type=service` or `--type=timer`"
        )));
    }
    let wants = |kind: &str| options.types.is_empty() || options.types.iter().any(|t| t == kind);
    let mut rows = Vec::new();
    if wants("service") {
        let mut document = service::shown(None)?;
        document.services.sort_by(|a, b| a.name.cmp(&b.name));
        rows.extend(service_rows(&document.services, options));
    }
    if wants("timer") && !options.failed_only {
        // Without `--type`, a system whose timerd does not answer still
        // lists its services.
        match timer_rows() {
            Ok(timers) => rows.extend(timers.into_iter().map(|row| UnitRow {
                unit: format!("{}.timer", row.name),
                active: "active",
                sub: "waiting",
                description: row.description,
            })),
            Err(failed) if !options.types.is_empty() => return Err(failed),
            Err(_) => {}
        }
    }
    print!("{}", units_text(&rows, options));
    Ok(0)
}

/// `list-timers [NAME...]`: when each timer runs next and last.
fn list_timers(names: &[String]) -> Result<i32, Fail> {
    let wanted: Vec<&str> = names
        .iter()
        .map(|n| {
            let n = n.strip_suffix(".timer").unwrap_or(n);
            n.strip_suffix(".service").unwrap_or(n)
        })
        .collect();
    let mut rows = timer_rows()?;
    rows.retain(|row| wanted.is_empty() || wanted.contains(&row.name.as_str()));
    rows.sort_by_key(|row| row.next.unwrap_or(i64::MAX));
    print!("{}", timers_text(&rows, now()));
    Ok(0)
}

/// `list-timers`' table: NEXT, LEFT, LAST, PASSED, UNIT and ACTIVATES.
fn timers_text(rows: &[TimerRow], now: i64) -> String {
    let cells: Vec<[String; 6]> = rows
        .iter()
        .map(|row| {
            let apart = |at: Option<i64>, direction: &str| {
                at.map_or_else(
                    || "n/a".to_string(),
                    |at| {
                        let words = span(at.abs_diff(now).saturating_mul(1000));
                        format!("{words} {direction}")
                    },
                )
            };
            [
                row.next.map_or_else(|| "n/a".to_string(), clock),
                apart(row.next, "left"),
                row.last.map_or_else(|| "n/a".to_string(), clock),
                apart(row.last, "ago"),
                format!("{}.timer", row.name),
                format!("{}.service", row.name),
            ]
        })
        .collect();
    let titles = ["NEXT", "LEFT", "LAST", "PASSED", "UNIT", "ACTIVATES"].map(String::from);
    let mut widths = [0; 5];
    for row in cells.iter().chain([&titles]) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut text = String::new();
    for row in [&titles].into_iter().chain(&cells) {
        for (cell, width) in row.iter().zip(widths) {
            text.push_str(&format!("{cell:<width$} "));
        }
        text.push_str(&row[5]);
        text.push('\n');
    }
    text.push_str(&format!("\n{} timers listed.\n", rows.len()));
    text
}

/// `cat NAME`: the service's file under a line that names it, as
/// systemctl cat shows a unit's.
fn cat(unit: Unit<'_>) -> Result<i32, Fail> {
    let (Unit::Service(name) | Unit::Timer(name)) = unit;
    let own = format!("{}/{name}.toml", servicefiles::DIR);
    let path = if Path::new(&own).exists() {
        own
    } else {
        format!("{}/{name}.toml", servicefiles::DEFAULTS)
    };
    let text = fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            fail_with(1, format!("No files found for {name}.service."))
        } else {
            util::failed(&path, e)
        }
    })?;
    println!("# {path}");
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    Ok(0)
}

/// The `show` properties of `service`, by name, in the order systemctl
/// gives them: what Spark's document says, in systemd's words. A service
/// Spark does not know has the properties of a unit without a file.
fn properties(name: &str, service: Option<&Service>) -> Vec<(&'static str, String)> {
    let id = format!("{name}.service");
    let Some(s) = service else {
        return vec![
            ("Id", id.clone()),
            ("Description", id),
            ("LoadState", UNKNOWN.into()),
            ("ActiveState", "inactive".into()),
            ("SubState", "dead".into()),
            ("UnitFileState", String::new()),
            ("MainPID", "0".into()),
            ("ExecMainStatus", "0".into()),
            ("NRestarts", "0".into()),
            ("MemoryCurrent", "[not set]".into()),
        ];
    };
    vec![
        ("Id", id),
        ("Description", s.description.clone()),
        ("LoadState", "loaded".into()),
        ("ActiveState", active_state(s.state).into()),
        ("SubState", sub_state(s.state).into()),
        ("UnitFileState", if s.enabled { "enabled" } else { "disabled" }.into()),
        ("MainPID", s.pid.unwrap_or(0).to_string()),
        ("ExecMainStatus", exit_status(s.last_status).to_string()),
        ("NRestarts", s.restarts.to_string()),
        (
            "MemoryCurrent",
            s.usage.as_ref().map_or_else(|| "[not set]".into(), |u| u.memory_bytes.to_string()),
        ),
    ]
}

/// What `ExecMainStatus` says of a wait status as waitpid(2) gives it:
/// the exit code, or for a process a signal ended, the signal's number.
fn exit_status(wait_status: Option<u32>) -> u32 {
    match wait_status {
        None => 0,
        Some(status) if status & 0x7f == 0 => status >> 8 & 0xff,
        Some(status) => status & 0x7f,
    }
}

/// `show`'s text of one unit: `Name=value` lines of the properties asked
/// for, or all of them; with `--value`, the values alone.
fn show_text(properties: &[(&'static str, String)], wanted: &[String], value_only: bool) -> String {
    let mut text = String::new();
    for (name, value) in properties {
        if !wanted.is_empty() && !wanted.iter().any(|w| w == name) {
            continue;
        }
        if value_only {
            text.push_str(&format!("{value}\n"));
        } else {
            text.push_str(&format!("{name}={value}\n"));
        }
    }
    text
}

/// `show [-p PROPERTY[,PROPERTY]] [--value] NAME...`, a blank line
/// between units, as systemctl.
fn show_units(names: &[String], options: &Options) -> Result<i32, Fail> {
    if names.is_empty() {
        return Err(usage("systemctl show takes the name of a service".into()));
    }
    let mut texts = Vec::new();
    for name in names {
        let Unit::Service(name) = Unit::of(name)? else {
            return Err(usage(format!(
                "{name}: show gives a service's properties; a timer's times are `systemctl \
                 list-timers`"
            )));
        };
        let service = known(name)?;
        texts.push(show_text(
            &properties(name, service.as_ref()),
            &options.properties,
            options.value,
        ));
    }
    print!("{}", texts.join("\n"));
    Ok(0)
}

/// `daemon-reload`: Spark takes a changed service file as it is written,
/// so this checks every file and says what Spark does with each.
fn daemon_reload() -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail(
            "only root may check the service files: elevate first, as `sudo systemctl \
             daemon-reload`"
                .into(),
        ));
    }
    servicefiles::check(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_api::show;

    fn run_args(items: &[&str]) -> Result<i32, Fail> {
        main(&mut Args::new("systemctl", items.iter().map(Into::into).collect()))
    }

    fn nginx() -> Service {
        Service {
            name: "nginx".into(),
            description: "nginx web server".into(),
            enabled: true,
            state: State::Running,
            pid: Some(57),
            restarts: 2,
            last_status: Some(0x0100),
            ..Service::default()
        }
    }

    #[test]
    fn a_service_state_has_systemds_words() {
        let words = |state| (active_state(state), sub_state(state));
        assert_eq!(words(State::Running), ("active", "running"));
        assert_eq!(words(State::Done), ("active", "exited"));
        assert_eq!(words(State::Starting), ("activating", "start"));
        assert_eq!(words(State::Pending), ("activating", "start-pre"));
        assert_eq!(words(State::Waiting), ("activating", "auto-restart"));
        assert_eq!(words(State::Stopping), ("deactivating", "stop-sigterm"));
        assert_eq!(words(State::Failed), ("failed", "failed"));
        assert_eq!(words(State::Stopped), ("inactive", "dead"));
        assert_eq!(words(State::Disabled), ("inactive", "dead"));
    }

    #[test]
    fn a_service_waiting_for_a_connection_is_inactive_as_an_idle_socket_unit_is() {
        assert_eq!(active_state(State::Listening), "inactive");
        assert_eq!(sub_state(State::Listening), "dead");
        let mut s = nginx();
        s.state = State::Listening;
        s.pid = None;
        let shown = show_text(
            &properties("nginx", Some(&s)),
            &["ActiveState".into(), "SubState".into()],
            false,
        );
        assert_eq!(shown, "ActiveState=inactive\nSubState=dead\n");
        let hidden = Options::default();
        assert!(service_rows(&[s.clone()], &hidden).is_empty());
        assert_eq!(
            service_rows(&[s.clone()], &Options { all: true, ..Options::default() }).len(),
            1
        );
        assert_eq!(triggered_by(&s), None);
        s.holds.sockets = vec!["port 80".into(), "/system/live/web/db".into()];
        assert_eq!(
            triggered_by(&s).as_deref(),
            Some("TriggeredBy: its sockets (port 80, /system/live/web/db)")
        );
        s.state = State::Running;
        s.start = "on-connection".into();
        assert!(triggered_by(&s).is_some());
        s.start = "at-boot".into();
        assert_eq!(triggered_by(&s), None);
    }

    #[test]
    fn show_gives_the_properties_spark_knows() {
        let s = nginx();
        let all = show_text(&properties("nginx", Some(&s)), &[], false);
        assert_eq!(
            all,
            "Id=nginx.service\nDescription=nginx web server\nLoadState=loaded\n\
             ActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=57\n\
             ExecMainStatus=1\nNRestarts=2\nMemoryCurrent=[not set]\n"
        );
        let wanted = ["MainPID".to_string(), "ActiveState".to_string()];
        let two = show_text(&properties("nginx", Some(&s)), &wanted, false);
        assert_eq!(two, "ActiveState=active\nMainPID=57\n");
        assert_eq!(show_text(&properties("nginx", Some(&s)), &wanted, true), "active\n57\n");
        let unknown = show_text(&properties("nope", None), &["LoadState".into()], false);
        assert_eq!(unknown, "LoadState=not-found\n");
        // A property systemctl knows and Spark does not say is left out.
        assert_eq!(show_text(&properties("nginx", Some(&s)), &["FragmentPath".into()], false), "");
    }

    #[test]
    fn memory_is_what_the_domain_holds_when_spark_says() {
        let mut s = nginx();
        s.usage = Some(show::Usage { memory_bytes: 4096, ..show::Usage::default() });
        let text = show_text(&properties("nginx", Some(&s)), &["MemoryCurrent".into()], true);
        assert_eq!(text, "4096\n");
    }

    #[test]
    fn the_main_status_is_the_exit_code_or_the_signal() {
        assert_eq!(exit_status(None), 0);
        assert_eq!(exit_status(Some(0)), 0);
        assert_eq!(exit_status(Some(0x0300)), 3);
        assert_eq!(exit_status(Some(9)), 9);
        assert_eq!(exit_status(Some(0x8b)), 11);
    }

    #[test]
    fn list_units_shows_what_runs_and_with_all_what_does_not() {
        let mut stopped = nginx();
        stopped.name = "db".into();
        stopped.state = State::Stopped;
        let mut failed = nginx();
        failed.name = "mail".into();
        failed.state = State::Failed;
        let services = [nginx(), stopped, failed];
        let plain = Options::default();
        let names = |options: &Options| -> Vec<String> {
            service_rows(&services, options).into_iter().map(|r| r.unit).collect()
        };
        assert_eq!(names(&plain), ["nginx.service", "mail.service"]);
        assert_eq!(
            names(&Options { all: true, ..Options::default() }),
            ["nginx.service", "db.service", "mail.service"]
        );
        assert_eq!(names(&Options { failed_only: true, ..Options::default() }), ["mail.service"]);
        let text = units_text(&service_rows(&services, &plain), &plain);
        assert!(
            text.starts_with("UNIT           LOAD    ACTIVE     SUB          DESCRIPTION\n"),
            "{text}"
        );
        assert!(
            text.contains("nginx.service  loaded  active     running      nginx web server\n"),
            "{text}"
        );
        assert!(
            text.ends_with(
                "\n2 loaded units listed. Pass --all to see loaded but inactive units, too.\n"
            ),
            "{text}"
        );
        let bare = Options { no_legend: true, ..Options::default() };
        assert!(!units_text(&service_rows(&services, &bare), &bare).contains("LOAD"));
    }

    #[test]
    fn timers_are_listed_by_their_next_run() {
        let rows = [TimerRow {
            name: "logturn".into(),
            description: "turns the logs over".into(),
            next: Some(1_000_000_000 + 7_500),
            last: None,
        }];
        let text = timers_text(&rows, 1_000_000_000);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("NEXT") && lines[0].ends_with("UNIT          ACTIVATES"),
            "{text}"
        );
        assert!(lines[1].contains(" 2 h 5 min left ") && lines[1].contains(" n/a "), "{text}");
        assert!(lines[1].ends_with("logturn.timer logturn.service"), "{text}");
        assert!(text.ends_with("\n1 timers listed.\n"), "{text}");
        let status = timer_status_text(&rows[0], 1_000_000_000);
        assert!(
            status
                .starts_with("logturn.timer - turns the logs over\n   Active: active (waiting)\n"),
            "{status}"
        );
        assert!(
            status.contains("; 2 h 5 min left\n") && status.ends_with("     Last: n/a\n"),
            "{status}"
        );
    }

    #[test]
    fn a_service_suffix_is_taken_and_a_timer_is_a_timer() {
        assert_eq!(Unit::of("nginx").unwrap(), Unit::Service("nginx"));
        assert_eq!(Unit::of("nginx.service").unwrap(), Unit::Service("nginx"));
        assert_eq!(Unit::of("logturn.timer").unwrap(), Unit::Timer("logturn"));
        let socket = Unit::of("nginx.socket").unwrap_err();
        assert!(socket.message.contains("systemctl cat nginx"), "{}", socket.message);
        // A dot that is no suffix of a kind stays in the name, as bundles' do.
        assert_eq!(Unit::of("org.nginx.nginx").unwrap(), Unit::Service("org.nginx.nginx"));
    }

    #[test]
    fn each_verb_systemctl_has_and_dukovo_lacks_says_what_to_run() {
        for (verb, said) in [
            ("mask", "elevate service NAME disable"),
            ("edit", "elevate service edit NAME"),
            ("poweroff", "elevate poweroff"),
            ("reboot", "elevate reboot"),
            ("isolate", "no targets"),
            ("list-dependencies", "service NAME explain"),
            ("try-restart", "elevate service NAME restart"),
            ("list-unit-files", "service list"),
            ("suspend", "elevate poweroff"),
            ("frobnicate", "no command 'frobnicate'"),
        ] {
            let text = elsewhere(verb);
            assert!(text.contains(said), "{verb}: {text}");
            let failed = run_args(&[verb]).unwrap_err();
            assert!(failed.usage && failed.code == 1, "{verb}");
        }
    }

    #[test]
    fn options_dukovo_lacks_say_what_to_run() {
        for (items, said) in [
            (&["--user", "status", "x"][..], "crontab"),
            (&["-H", "web", "status"], "ssh HOST"),
            (&["--type=socket", "list-units"], "--type=service"),
            (&["status"], "takes the name of a service"),
        ] {
            let failed = run_args(items).unwrap_err();
            assert!(failed.message.contains(said), "{items:?}: {}", failed.message);
        }
    }

    #[test]
    fn a_timer_is_changed_in_its_services_file() {
        for verb in ["start", "stop", "enable", "restart", "reload", "disable"] {
            let status = run_args(&[verb, "logturn.timer"]).unwrap();
            assert_eq!(status, 1, "{verb}");
        }
    }
}
