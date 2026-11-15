// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukovo-check(8): looks at this computer's security and health, prints
//! a line as each check ends, and keeps the result (ADR 0479).
//!
//! It looks for what a person would want to know before it hurts: a rule
//! that lets every port in, SSH that takes passwords from anyone, an
//! account with no password, an update that is waiting, a clock that was
//! never set, a service that gave up, a warning that is still active, and
//! no snapshots of the people's files. A default install has none of
//! these, so a warning is real. It changes nothing but the kept result.
//!
//! Where to look: `line` holds what a check finds, the IDs of its
//! warnings (SEC-101 to SEC-160, each with a page in docs/errors) and the
//! exit status they make; `context` what the checks read, which the unit
//! tests plant in a folder; `saved` the result kept in
//! `/system/state/dukovo-check/last.json`; and one module for each area:
//! `network`, `ssh`, `accounts`, `updates`, `time`, `services`, `health`
//! and `backups`. A check is a function of the context that gives its
//! lines, and the list in [`CHECKS`] is where a new one is added.

mod accounts;
mod backups;
mod context;
mod health;
mod line;
mod network;
mod saved;
mod services;
mod ssh;
mod time;
mod updates;

use std::path::Path;

// The accounts crate, not this folder's module of the same name.
use ::accounts::ADMINS_GROUP;

use crate::args::{Arg, Args};
use crate::logger;
use crate::util::{self, Fail, fail, usage};
use context::Context;
use line::{Area, Counts, Line, Outcome, Verdict};
use saved::Saved;

const USAGE: &str = "usage: dukovo-check [--json]\n       dukovo-check --last [--json]";

/// One check: where it is in the report, its name, and what it does.
struct Check {
    area: Area,
    name: &'static str,
    /// Its lines, at least one: the check's result, or why it did not run.
    run: fn(&Context) -> Vec<Outcome>,
}

/// The checks, in the order of the report.
const CHECKS: [Check; 15] = [
    Check { area: Area::Network, name: "reachable", run: network::reachable },
    Check { area: Area::Network, name: "firewall rules", run: network::rules },
    Check { area: Area::Network, name: "firewall on", run: network::service_on },
    Check { area: Area::Ssh, name: "passwords", run: ssh::passwords },
    Check { area: Area::Ssh, name: "who may sign in", run: ssh::who_may_sign_in },
    Check { area: Area::Ssh, name: "blocking", run: ssh::blocking },
    Check { area: Area::Accounts, name: "empty passwords", run: accounts::empty_passwords },
    Check { area: Area::Accounts, name: "administrators", run: accounts::administrators },
    Check { area: Area::Updates, name: "waiting", run: updates::waiting },
    Check { area: Area::Updates, name: "last check", run: updates::last_check },
    Check { area: Area::Time, name: "clock", run: time::clock },
    Check { area: Area::Services, name: "failed", run: services::failed },
    Check { area: Area::Health, name: "early warnings", run: health::early_warnings },
    Check { area: Area::Health, name: "hardware", run: health::hardware },
    Check { area: Area::Backups, name: "snapshots", run: backups::snapshots },
];

/// What `dukovo-check` was asked.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// Print the result as JSON, as the file keeps it.
    json: bool,
    /// Print the kept result without checking again.
    last: bool,
}

/// dukovo-check(8). A run that could not start ends with status 3: 1 is a
/// check that failed.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let ended = parse(args).and_then(|options| {
        if options.last { last_command(&options) } else { check_command(&options) }
    });
    ended.map_err(|fail| Fail { code: 3, ..fail })
}

/// The options.
fn parse(args: &mut Args) -> Result<Options, Fail> {
    let mut options = Options::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => options.json = true,
            Arg::Long(name) if name == "last" => options.last = true,
            Arg::Long(name) if name == "help" => return Err(usage(USAGE.into())),
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(options)
}

/// Runs each check of `checks` against `context`. `show` is given each
/// line as it ends, so that a person watching sees the report grow.
fn run(checks: &[Check], context: &Context, mut show: impl FnMut(&Line)) -> Vec<Line> {
    let mut lines = Vec::new();
    for check in checks {
        for outcome in (check.run)(context) {
            let line = Line { area: check.area, check: check.name.to_string(), outcome };
            show(&line);
            lines.push(line);
        }
    }
    lines
}

/// The checks, run now, and the services Spark knows, as the checks read
/// them: for a canary's report (ADR 0533), which keeps no result and
/// logs no line. A check that cannot ask its source says so in its own
/// line, as in a run of `dukovo-check`.
pub(crate) fn run_now() -> (Vec<Line>, Result<Vec<spark_api::show::Service>, String>) {
    let context = Context::gather();
    let lines = run(&CHECKS, &context, |_| {});
    (lines, context.services)
}

/// What a canary reports of `lines`, a run of the checks: how many
/// failed and how many warned, and each that failed as `area: check:
/// detail`. `None` when no check could run, as dukovo-check ends with
/// status 3 then.
pub(crate) fn for_canary(lines: &[Line]) -> Option<system_release::canary::Check> {
    let counts = Counts::of(lines);
    if counts.status() == 3 {
        return None;
    }
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Some(system_release::canary::Check {
        failures: count(counts.failed),
        warnings: count(counts.warning),
        failed: lines
            .iter()
            .filter(|line| line.outcome.verdict == Verdict::Failed)
            .map(|line| format!("{}: {}: {}", line.area.name(), line.check, line.outcome.detail))
            .collect(),
    })
}

/// Keeps the result of a run and logs its line. Neither fails the run:
/// what it found is shown all the same.
fn keep(saved: &Saved) {
    if let Err(error) = saved.save(Path::new(saved::DIR), ADMINS_GROUP) {
        eprintln!(
            "dukovo-check: cannot keep the result in {}: {}",
            saved::FILE,
            util::strip_os_error(&error)
        );
    }
    let (level, line) = saved.log_line();
    if let Err(fail) = logger::log("dukovo-check", level, &line) {
        eprintln!("dukovo-check: cannot write to the log: {}", fail.message);
    }
}

/// `dukovo-check`: checks, and keeps the result.
fn check_command(options: &Options) -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail("needs an administrator: run elevate dukovo-check".into()));
    }
    if !options.json {
        println!("Checking this computer's security and health. Ctrl-C stops it.");
    }
    let context = Context::gather();
    // A run that is stopped leaves the kept result as it was: it is only
    // written, whole, after the last check.
    let lines = run(&CHECKS, &context, |line| {
        if !options.json {
            println!("{}", line.text());
        }
    });
    let saved = Saved::of(&context, lines);
    keep(&saved);
    if options.json {
        println!("{}", saved.json()?);
    } else {
        println!("{}", saved.counts.summary());
    }
    Ok(saved.counts.status())
}

/// `dukovo-check --last`: the kept result, which a group of administrators
/// reads, as the file's mode lets them.
fn last_command(options: &Options) -> Result<i32, Fail> {
    let saved = Saved::load(Path::new(saved::FILE))?;
    if options.json {
        println!("{}", saved.json()?);
    } else {
        println!("{}", saved.heading());
        for line in &saved.results {
            println!("{}", line.text());
        }
        println!("{}", Counts::of(&saved.results).summary());
    }
    Ok(Counts::of(&saved.results).status())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::context::planted::System;
    use super::*;

    fn options(items: &[&str]) -> Result<Options, Fail> {
        parse(&mut Args::new("dukovo-check", items.iter().map(OsString::from).collect()))
    }

    #[test]
    fn the_options_are_read() {
        assert_eq!(options(&[]).unwrap(), Options::default());
        assert_eq!(options(&["--json"]).unwrap(), Options { json: true, last: false });
        assert_eq!(options(&["--last"]).unwrap(), Options { json: false, last: true });
        assert_eq!(options(&["--last", "--json"]).unwrap(), Options { json: true, last: true });
    }

    #[test]
    fn a_mistake_in_the_command_line_says_what_it_is() {
        let message = |items: &[&str]| options(items).unwrap_err().message;
        assert_eq!(message(&["--fast"]), "unrecognized option '--fast'");
        assert_eq!(message(&["now"]), "unexpected argument 'now'");
        assert_eq!(message(&["-j"]), "invalid option -- 'j'");
        assert_eq!(message(&["--help"]), USAGE);
    }

    #[test]
    fn a_run_that_cannot_start_ends_with_status_3_not_the_1_of_a_failed_check() {
        let ended = |items: &[&str]| {
            main(&mut Args::new("dukovo-check", items.iter().map(OsString::from).collect()))
                .map_err(|fail| fail.code)
        };
        assert_eq!(ended(&["--fast"]), Err(3));
        assert_eq!(ended(&["now"]), Err(3));
    }

    #[test]
    fn every_check_has_its_own_name_in_its_area_and_gives_a_line() {
        let mut names: Vec<(&str, &str)> =
            CHECKS.iter().map(|check| (check.area.name(), check.name)).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CHECKS.len());
        // The column of the report is 21 wide.
        assert!(CHECKS.iter().all(|check| check.name.len() <= 21));
        // A system with nothing planted and Spark that cannot be asked: each
        // check says why it did not run, instead of saying nothing.
        let mut system = System::new(Vec::new());
        system.context.services = Err("no Spark".into());
        system.context.reached = Err("no firewall".into());
        system.context.alerts = Err("no alertd".into());
        for check in &CHECKS {
            assert!(!(check.run)(&system.context).is_empty(), "{}", check.name);
        }
    }

    #[test]
    fn a_line_is_shown_as_each_check_ends_and_all_are_kept_in_order() {
        let system = System::new(Vec::new());
        let mut shown = Vec::new();
        let lines = run(&CHECKS, &system.context, |line| shown.push(line.check.clone()));
        assert_eq!(shown, lines.iter().map(|line| line.check.clone()).collect::<Vec<_>>());
        let first: Vec<&str> = lines.iter().take(3).map(|line| line.check.as_str()).collect();
        assert_eq!(first, ["reachable", "firewall rules", "firewall on"]);
        assert_eq!(lines.last().map(|line| line.check.as_str()), Some("snapshots"));
    }

    #[test]
    fn a_canary_reports_the_failures_and_warnings_of_a_run_and_how_each_failure_reads() {
        let entry = |area, check: &str, outcome| Line { area, check: check.into(), outcome };
        let lines = [
            entry(Area::Time, "clock", Outcome::passed("ok")),
            entry(
                Area::Ssh,
                "passwords",
                Outcome::warning(line::Problem::SshPasswords, "passwords sign in"),
            ),
            entry(
                Area::Health,
                "hardware",
                Outcome::repeating(Verdict::Failed, "disk sda: worn out", None, None),
            ),
            entry(Area::Backups, "snapshots", Outcome::skipped("no snapshots")),
        ];
        let found = for_canary(&lines).unwrap();
        assert_eq!((found.failures, found.warnings), (1, 1));
        assert_eq!(found.failed, ["health: hardware: disk sda: worn out"]);
        // Nothing that ran: the checks did not run.
        assert_eq!(for_canary(&[lines[3].clone()]), None);
        assert_eq!(for_canary(&[]), None);
    }
}
