// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The log's actions (ADR 0518): `logs.read`, the records a person may
//! read, and `logs.read-all`, the whole log. Both run `log --all --output
//! json` with the Logs page's filters and answer the records it prints, a
//! line each, as an array. The area is `logs`, the scope a token needs.
//!
//! Root reads every record of the log; anyone else, only the records of
//! their own programs (ADR 0252, which keeps no group that reads the
//! whole log, so that each look at it is an audit line). So `logs.read`
//! runs as the person, and shows a person who is not an administrator, or
//! an administrator whose session is locked, their own records; and
//! `logs.read-all` runs as an administrator, as `elevate log` does: an
//! administrator's unlocked session, or a token with the area's full
//! scope, and one elevated command, and one audit line, for each read.
//!
//! The times are seconds since 1970, which the page makes from the zone it
//! shows, and the command says in UTC. A filter is checked by
//! [`log::console_refusal`], which authd and the follow
//! (`GET /api/1/logs/follow`) check theirs with.

use accounts::console::Filter;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Action, Args, Bad, Examples, Field, Runs, Tests};
use crate::log;
use crate::util;

/// The area of these actions: the scope a token needs, `logs`.
pub const AREA: &str = "logs";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::logs::tests",
    route: "admin-logs",
    same_as_command: "admin-logs",
    browser: "web-console/logs",
    ansible: "web-console/ansible",
};

/// How many records a read shows when it does not say.
const LINES_DEFAULT: u32 = 200;

/// The most records a read shows.
const LINES_MAX: u32 = 1000;

/// The latest time a read names: the last second of the year 9999.
const TIME_MAX: i64 = 253_402_300_799;

/// What a service's name matches, as `manifest::service::check_name`
/// checks it, apart from its length.
const SERVICE_PATTERN: &str = "^[A-Za-z0-9_][A-Za-z0-9_.-]*$";

/// Text with no control character, as `char::is_control` finds them.
const NO_CONTROL_PATTERN: &str = "^[^\u{0}-\u{1f}\u{7f}-\u{9f}]*$";

/// The filters' fields, which a read and a follow both take.
const FILTER_FIELDS: [Field; 4] = [
    Field::text(
        "service",
        "One service's records: its name, as service list shows it.",
        1,
        radix_abi::resource_domain::NAME_MAX as usize,
        Some(SERVICE_PATTERN),
    )
    .optional(),
    Field::flag("kernel", "The kernel's records alone.").optional(),
    Field::one_of(
        "priority",
        "The least urgent level shown: error, warning, info or debug.",
        &log::CONSOLE_LEVELS,
    )
    .optional(),
    Field::text(
        "grep",
        "Words a record's text matches: a pattern as log --grep reads it, without regard to \
         case unless it has a capital.",
        1,
        log::CONSOLE_GREP_MAX,
        Some(NO_CONTROL_PATTERN),
    )
    .optional(),
];

/// What a read shows: the last `lines` records between two times that
/// the filters let through.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    /// How many records, the last ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// One service's records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The kernel's records alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kernel: bool,
    /// The least urgent level shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// The first time shown, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// The last time shown, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    /// Words the records' text matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grep: Option<String>,
}

impl Read {
    /// The filters, as the follow and authd take them.
    fn filter(&self) -> Filter {
        Filter {
            service: self.service.clone(),
            kernel: self.kernel,
            priority: self.priority.clone(),
            grep: self.grep.clone(),
        }
    }
}

/// `seconds` as log(1)'s `--since` and `--until` read a time in UTC.
pub(super) fn utc(seconds: i64) -> String {
    format!("{} UTC", util::time_text(seconds, "%Y-%m-%d %H:%M:%S", true))
}

/// Checks `filter` as [`log::console_refusal`] does, in the words the
/// page shows under the field.
fn check_filter(filter: &Filter) -> Result<(), Bad> {
    match log::console_refusal(filter) {
        Some((field, why)) => Err(Bad { field, why }),
        None => Ok(()),
    }
}

impl Args for Read {
    const FIELDS: &'static [Field] = &[
        Field::number(
            "lines",
            "How many records, the last ones: 1 to 1000; 200 if not said.",
            1,
            1000,
        )
        .optional(),
        FILTER_FIELDS[0],
        FILTER_FIELDS[1],
        FILTER_FIELDS[2],
        Field::number("since", "The first time shown, in seconds since 1970.", 0, TIME_MAX)
            .optional(),
        Field::number("until", "The last time shown, in seconds since 1970.", 0, TIME_MAX)
            .optional(),
        FILTER_FIELDS[3],
    ];

    fn check(&self) -> Result<(), Bad> {
        if self.lines.is_some_and(|lines| !(1..=LINES_MAX).contains(&lines)) {
            return Err(Bad { field: "lines", why: "From 1 to 1000 records.".into() });
        }
        for (field, time) in [("since", self.since), ("until", self.until)] {
            if time.is_some_and(|time| !(0..=TIME_MAX).contains(&time)) {
                return Err(Bad { field, why: "A time from 1970 to the year 9999.".into() });
            }
        }
        if let (Some(since), Some(until)) = (self.since, self.until)
            && until < since
        {
            return Err(Bad { field: "until", why: "The last time comes after the first.".into() });
        }
        check_filter(&self.filter())
    }
}

/// log(1)'s words for `args`: every boot's records as JSON lines, the
/// last ones, with the filters, in the order of the page's fields.
fn read_argv(args: &Read) -> Vec<String> {
    let lines = args.lines.unwrap_or(LINES_DEFAULT).to_string();
    let mut argv = ["log", "--all", "--output", "json", "--lines"].map(String::from).to_vec();
    argv.push(lines);
    argv.extend(log::console_options(&args.filter()));
    for (option, time) in [("--since", args.since), ("--until", args.until)] {
        if let Some(time) = time {
            argv.extend([option.to_string(), utc(time)]);
        }
    }
    if let Some(grep) = &args.grep {
        argv.extend(["--grep".to_string(), grep.clone()]);
    }
    argv
}

/// The examples of both reads.
fn read_examples() -> Examples<Read> {
    Examples {
        good: vec![
            Read { lines: Some(20), priority: Some("info".into()), ..Read::default() },
            Read { lines: Some(5), service: Some("diskhealth".into()), ..Read::default() },
            Read {
                kernel: true,
                since: Some(1_763_100_000),
                until: Some(1_763_103_600),
                grep: Some("usb".into()),
                ..Read::default()
            },
        ],
        bad: vec![
            json!({ "lines": 0 }),
            json!({ "lines": 1001 }),
            json!({ "since": -1 }),
            json!({ "since": "-1h" }),
            json!({ "since": 1_763_103_600, "until": 1_763_100_000 }),
            json!({ "grep": "(" }),
            json!({ "grep": "" }),
            json!({ "priority": "loud" }),
            json!({ "service": "../x" }),
            json!({ "service": "diskhealth", "kernel": true }),
            json!({ "boot": "all" }),
        ],
        undo: None,
    }
}

/// `log --all --output json ...` as the person: the records of their own
/// programs, or every record for root.
pub struct Own;

impl Action for Own {
    const ID: &'static str = "logs.read";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/logs";
    const RUNS: Runs = Runs::AsThePerson;
    const LINES: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Read your programs' log";
    type Args = Read;

    fn argv(args: &Read) -> Vec<String> {
        read_argv(args)
    }

    fn examples() -> Examples<Read> {
        read_examples()
    }
}

/// The same as an administrator: the whole log.
pub struct All;

impl Action for All {
    const ID: &'static str = "logs.read-all";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/logs/all";
    const RUNS: Runs = Runs::AsAdministrator;
    const LINES: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Read the whole log";
    type Args = Read;

    fn argv(args: &Read) -> Vec<String> {
        read_argv(args)
    }

    fn examples() -> Examples<Read> {
        read_examples()
    }
}

/// What a follow of the log shows (`GET /api/1/logs/follow`): the
/// filters, and whether it is the whole log.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    /// One service's records.
    #[serde(default)]
    pub service: Option<String>,
    /// The kernel's records alone.
    #[serde(default)]
    pub kernel: bool,
    /// The least urgent level shown.
    #[serde(default)]
    pub priority: Option<String>,
    /// Words the records' text matches.
    #[serde(default)]
    pub grep: Option<String>,
    /// The whole log, as an administrator.
    #[serde(default)]
    pub all: bool,
}

impl Follow {
    /// The query's fields.
    pub const FIELDS: [Field; 5] = [
        FILTER_FIELDS[0],
        FILTER_FIELDS[1],
        FILTER_FIELDS[2],
        FILTER_FIELDS[3],
        Field::flag("all", "The whole log, as an administrator.").optional(),
    ];

    /// The filters, as authd takes them.
    pub fn filter(&self) -> Filter {
        Filter {
            service: self.service.clone(),
            kernel: self.kernel,
            priority: self.priority.clone(),
            grep: self.grep.clone(),
        }
    }

    /// Whether the follow can work, as a read's filters are checked.
    ///
    /// # Errors
    ///
    /// The first field that cannot, and why.
    pub fn check(&self) -> Result<(), Bad> {
        check_filter(&self.filter())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Found, Registered, find, matches};
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    #[test]
    fn a_person_reads_their_programs_log_and_an_administrator_the_whole_one() {
        let own = action("logs.read");
        let all = action("logs.read-all");
        assert_eq!(
            (own.runs(), own.changes(), own.lines(), own.area()),
            (Runs::AsThePerson, false, true, "logs")
        );
        assert_eq!(
            (all.runs(), all.changes(), all.lines(), all.area()),
            (Runs::AsAdministrator, false, true, "logs")
        );
        for action in [own, all] {
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, ["log", "--all", "--output", "json", "--lines", "200"]);
            assert_eq!(prepared.probe, None);
        }
        assert!(!action("logs.read").prepare(json!({})).unwrap().ansible.become_root);
        assert!(action("logs.read-all").prepare(json!({})).unwrap().ansible.become_root);
        // The whole log's address is not a filter's.
        let found = find(REGISTRY, false, "/api/1/logs/all");
        assert!(matches!(found, Found::Action(action, _) if action.id() == "logs.read-all"));
        assert!(matches(own.path(), "/api/1/logs/follow").is_none());
    }

    #[test]
    fn each_filter_is_log_s_option_in_the_page_s_order_and_a_time_is_said_in_utc() {
        let read = json!({
            "lines": 5,
            "service": "diskhealth",
            "priority": "warning",
            "since": 1_763_100_000,
            "until": 1_763_103_600,
            "grep": "-timed out",
        });
        let prepared = action("logs.read").prepare(read).unwrap();
        assert_eq!(
            prepared.argv,
            [
                "log",
                "--all",
                "--output",
                "json",
                "--lines",
                "5",
                "--service",
                "diskhealth",
                "--priority",
                "warning",
                "--since",
                "2025-11-14 06:00:00 UTC",
                "--until",
                "2025-11-14 07:00:00 UTC",
                "--grep",
                "-timed out",
            ]
        );
        let kernel = action("logs.read").prepare(json!({ "kernel": true })).unwrap();
        assert_eq!(kernel.argv.last().map(String::as_str), Some("--kernel"));
    }

    #[test]
    fn a_read_that_cannot_work_says_which_field_and_why() {
        let refused = |args| {
            let failure = action("logs.read").prepare(args).unwrap_err();
            assert_eq!(failure.kind, Kind::BadValue, "{}", failure.detail);
            failure.detail
        };
        let backwards = refused(json!({ "since": 1_763_103_600, "until": 1_763_100_000 }));
        assert!(backwards.contains("comes after"), "{backwards}");
        assert!(refused(json!({ "grep": "(" })).contains("not a pattern"));
        assert!(refused(json!({ "service": "diskhealth", "kernel": true })).contains("not both"));
        assert!(refused(json!({ "priority": "notice" })).contains("error, warning"));
        // A type that does not read is the request's fault, before any
        // check.
        let word = action("logs.read").prepare(json!({ "since": "-1h" })).unwrap_err();
        assert_eq!(word.kind, Kind::BadRequest);
    }

    #[test]
    fn a_follow_takes_the_filters_and_the_whole_log_and_checks_them_as_a_read_does() {
        let names: Vec<&str> = Follow::FIELDS.iter().map(|field| field.name).collect();
        assert_eq!(names, ["service", "kernel", "priority", "grep", "all"]);
        let follow = Follow {
            service: Some("nginx".into()),
            priority: Some("info".into()),
            all: true,
            ..Follow::default()
        };
        assert!(follow.check().is_ok());
        assert_eq!(follow.filter().service.as_deref(), Some("nginx"));
        let both = Follow { service: Some("nginx".into()), kernel: true, ..Follow::default() };
        assert_eq!(both.check().unwrap_err().field, "kernel");
        let pattern = Follow { grep: Some("a[b".into()), ..Follow::default() };
        assert_eq!(pattern.check().unwrap_err().field, "grep");
    }
}
