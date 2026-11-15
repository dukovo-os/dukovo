// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's usage actions: `metrics.now`, the latest values, and
//! `metrics.usage`, the seven series of the Overview's Usage card over a
//! range. Each runs `metrics`, metrics(1), as the person with `--json`
//! and answers what it printed. The area is `metrics`, the scope a token
//! needs.
//!
//! `metrics` reads the files metricsd writes, which everyone may read:
//! metricsd keeps the history in `/system/state/metrics` and the latest
//! values in `/system/live/metrics/now`, made under the umask of 022 in
//! folders of 0755, and the command asks metricsd nothing. A person who is
//! not an administrator may run `metrics now` and `metrics show` as an
//! administrator does, so both actions are open to every signed-in
//! session, and the Usage card is too.
//!
//! A machine that has not kept a history yet has none to show, which
//! metricsd writes every 10 minutes and when it stops: `metrics show` then
//! says so and ends with status 1, which the console answers as ADM-123
//! with metrics's own words.
//!
//! `metrics.usage` takes `range`, a word of a list, which makes the
//! command's `--since` and `--every`: how far back the chart goes, and the
//! width of each point, which keeps the answer to a few hundred points.
//!
//! `metrics.around` is the chart view's: the same two series of every
//! service, a little before and after a moment the person chose on a chart,
//! at the width of a point that the chart's range had. The moment is a
//! time the history can hold, which is checked against the clock, as a
//! schema cannot say it.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::logs::utc;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests};
use crate::admind::shared::unix_now;

/// The area of these actions: the scope a token needs, `metrics`.
const AREA: &str = "metrics";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::metrics::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// Each range the Usage card offers: its word, and the `--since` and
/// `--every` that make it.
pub const RANGES: [(&str, &str, &str); 4] =
    [("1h", "1h", "1m"), ("24h", "24h", "15m"), ("7d", "7d", "2h"), ("1y", "365d", "1d")];

/// The words of the ranges.
pub(super) const WORDS: [&str; 4] = [RANGES[0].0, RANGES[1].0, RANGES[2].0, RANGES[3].0];

/// The words `range` is checked by, and what a refusal says.
const RANGE_WORDS: &str = "The usage range is 1h, 24h, 7d or 1y. Nothing was done.";

/// The series the Usage card draws, in the order of its charts: the
/// processors' use, the memory used and available, what the disks read and
/// write, and what the network cards receive and send. A name without an
/// instance is every instance of the series.
pub const SERIES: [&str; 7] =
    ["cpu", "memory.used", "memory.available", "disk.read", "disk.write", "net.rx", "net.tx"];

/// `metrics now --json`: the latest values.
pub struct Now;

impl Action for Now {
    const ID: &'static str = "metrics.now";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/metrics/now";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the latest metrics";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["metrics", "now", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "series": "cpu" }), json!({ "all": true })],
            undo: None,
        }
    }
}

/// How far back the usage goes.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    /// `1h`, `24h`, `7d` or `1y`.
    pub range: String,
}

/// What `--since` and `--every` are for `range`, if it is one of the
/// words.
pub(super) fn since_and_every(range: &str) -> Option<(&'static str, &'static str)> {
    RANGES.iter().find(|(word, _, _)| *word == range).map(|&(_, since, every)| (since, every))
}

/// The error of a `range` that is none of the words.
pub(super) fn bad_range() -> Bad {
    Bad { field: "range", why: RANGE_WORDS.into() }
}

impl Range {
    /// What `--since` and `--every` are for the range, if it is one of
    /// the words.
    fn options(&self) -> Option<(&'static str, &'static str)> {
        since_and_every(&self.range)
    }
}

impl Args for Range {
    const FIELDS: &'static [Field] =
        &[Field::one_of("range", "How far back the usage goes: 1h, 24h, 7d or 1y.", &WORDS)];

    fn check(&self) -> Result<(), Bad> {
        if self.options().is_some() { Ok(()) } else { Err(bad_range()) }
    }
}

/// `metrics show SERIES... --since R --every E --json`: the seven series
/// of the Usage card over a range.
pub struct Usage;

impl Action for Usage {
    const ID: &'static str = "metrics.usage";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/metrics/usage/{range}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the usage";
    type Args = Range;

    fn argv(args: &Range) -> Vec<String> {
        let (since, every) = args.options().unwrap_or(("1h", "1m"));
        let mut argv = vec!["metrics".to_string(), "show".into()];
        argv.extend(SERIES.map(String::from));
        argv.extend(["--since", since, "--every", every, "--json"].map(String::from));
        argv
    }

    fn subject(args: &Range) -> Option<String> {
        Some(args.range.clone())
    }

    fn examples() -> Examples<Range> {
        Examples {
            good: vec![Range { range: "24h".into() }],
            bad: vec![
                json!({ "range": "2h" }),
                json!({ "range": "" }),
                json!({ "range": "24H" }),
                json!({ "range": "365d" }),
                json!({ "range": "1h", "series": "cpu" }),
                json!({}),
            ],
            undo: None,
        }
    }
}

/// The tests of `metrics.around`, which `lab/actions.toml` names: the
/// browser test is the chart view's.
const AROUND_TESTS: Tests = Tests {
    checks: "admind::actions::metrics::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/chart-to-logs",
    ansible: "web-console/ansible",
};

/// What `metrics.around` shows for each range: the width of a point in
/// the command's words, and in seconds. A range of a day has the points of
/// a minute and one of a week of 15 minutes, so that a moment is seen in
/// as many points as the chart it was chosen on drew.
const AROUND: [(&str, &str, i64); 4] =
    [("1h", "1m", 60), ("24h", "1m", 60), ("7d", "15m", 900), ("1y", "1h", 3600)];

/// How many points of the range are shown before the moment, and after it.
const POINTS_BEFORE: i64 = 60;
const POINTS_AFTER: i64 = 10;

/// How far back a moment may be: more than the year the history keeps.
const DAYS_BACK: i64 = 400;

/// How far ahead a moment may be, for a clock that is a little fast.
const SECONDS_AHEAD: i64 = 60;

/// The latest time a moment names: the last second of the year 9999.
const TIME_MAX: i64 = 253_402_300_799;

/// A moment on a chart, and the range of the chart.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Moment {
    /// The moment, in seconds since 1970.
    pub at: i64,
    /// `1h`, `24h`, `7d` or `1y`: the range of the chart it was chosen on.
    pub range: String,
}

impl Moment {
    /// The width of a point of the range, in the command's words and in
    /// seconds, if it is one of the words.
    fn step(&self) -> Option<(&'static str, i64)> {
        AROUND
            .iter()
            .find(|(word, _, _)| *word == self.range)
            .map(|&(_, every, secs)| (every, secs))
    }

    /// Whether the arguments can work when it is `now`, in seconds since
    /// 1970.
    fn check_at(&self, now: i64) -> Result<(), Bad> {
        let (oldest, newest) = (now - DAYS_BACK * 86_400, now + SECONDS_AHEAD);
        if !(oldest..=newest).contains(&self.at) {
            return Err(Bad {
                field: "at",
                why: format!(
                    "The moment is not more than {DAYS_BACK} days back, nor more than a minute \
                     ahead. Nothing was done."
                ),
            });
        }
        if self.step().is_none() {
            return Err(bad_range());
        }
        Ok(())
    }
}

impl Args for Moment {
    const FIELDS: &'static [Field] = &[
        Field::number(
            "at",
            "The moment, in seconds since 1970: not more than 400 days back, nor a minute ahead.",
            0,
            TIME_MAX,
        ),
        Field::one_of(
            "range",
            "The range of the chart the moment was chosen on: 1h, 24h, 7d or 1y.",
            &WORDS,
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        self.check_at(unix_now())
    }
}

/// `metrics show service.cpu service.memory ...` around a moment: 60
/// points of the range before it and 10 after, of every service.
pub struct Around;

impl Action for Around {
    const ID: &'static str = "metrics.around";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/metrics/around/{at}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = AROUND_TESTS;
    const TITLE: &'static str = "Show the usage around a time";
    type Args = Moment;

    fn argv(args: &Moment) -> Vec<String> {
        let (every, seconds) = args.step().unwrap_or(("1m", 60));
        let since = utc(args.at - POINTS_BEFORE * seconds);
        let until = utc(args.at + POINTS_AFTER * seconds);
        let mut argv = ["metrics", "show", "service.cpu", "service.memory", "--since"]
            .map(String::from)
            .to_vec();
        argv.extend([since, "--until".into(), until, "--every".into(), every.into()]);
        argv.push("--json".into());
        argv
    }

    fn subject(args: &Moment) -> Option<String> {
        Some(utc(args.at))
    }

    fn examples() -> Examples<Moment> {
        // A moment shortly before now, as the chart view chooses one: the
        // start of the hour, so that the list of actions reads the same a
        // few seconds apart (admin-actions holds GET /api/1/actions to
        // `admin actions --json`), where a fixed time would be too old once
        // 400 days have passed.
        Examples {
            good: vec![Moment { at: unix_now() / 3600 * 3600, range: "24h".into() }],
            bad: vec![
                json!({ "at": -1 }),
                json!({ "at": 1, "range": "1h" }),
                json!({ "at": 1_763_100_000, "range": "2y" }),
                json!({ "at": 1_763_100_000, "range": "" }),
                json!({ "range": "24h" }),
                json!({ "at": 1_763_100_000, "range": "24h", "series": "cpu" }),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, Registered, authorize, find};
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;
    use serde_json::Value;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    #[test]
    fn the_latest_values_are_metrics_now_as_json_for_the_person() {
        let action = action("metrics.now");
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (false, Runs::AsThePerson, "metrics", false)
        );
        assert_eq!(
            (action.path(), action.title()),
            ("/api/1/metrics/now", "Show the latest metrics")
        );
        assert!(action.errors().is_empty());
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["metrics", "now", "--json"]);
        assert_eq!(prepared.probe, None);
        assert_eq!(prepared.ansible.name, "Show the latest metrics");
        assert_eq!(prepared.ansible.params["argv"], json!(["metrics", "now", "--json"]));
        assert!(!prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        for args in action.example_values().1 {
            assert_eq!(action.prepare(args).unwrap_err().kind, Kind::BadRequest);
        }
    }

    #[test]
    fn each_range_makes_its_since_and_every() {
        let action = action("metrics.usage");
        let series = [
            "cpu",
            "memory.used",
            "memory.available",
            "disk.read",
            "disk.write",
            "net.rx",
            "net.tx",
        ];
        for (range, since, every) in
            [("1h", "1h", "1m"), ("24h", "24h", "15m"), ("7d", "7d", "2h"), ("1y", "365d", "1d")]
        {
            let prepared = action.prepare(json!({ "range": range })).unwrap();
            let mut expected = vec!["metrics", "show"];
            expected.extend(series);
            expected.extend(["--since", since, "--every", every, "--json"]);
            assert_eq!(prepared.argv, expected, "{range}");
            assert_eq!(prepared.args, json!({ "range": range }));
            assert_eq!(prepared.probe, None);
            assert_eq!(prepared.ansible.name, format!("Show the usage: {range}"));
            assert_eq!(prepared.ansible.params["argv"], json!(expected));
        }
    }

    #[test]
    fn a_range_is_a_word_of_the_list_and_the_refusal_names_the_field() {
        let action = action("metrics.usage");
        let words = "The usage range is 1h, 24h, 7d or 1y. Nothing was done.";
        for range in ["2h", "", "24H", "365d", "1h ", " 1h", "1y\n", "week", "0", "1m", "*"] {
            let failure = action.prepare(json!({ "range": range })).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("range")), "{range:?}");
            assert_eq!(failure.detail, words, "{range:?}");
        }
        for args in [
            json!({}),
            json!({ "range": 24 }),
            json!({ "range": null }),
            json!({ "range": "1h", "since": "1h" }),
            json!({ "range": ["1h"] }),
        ] {
            assert_eq!(action.prepare(args.clone()).unwrap_err().kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn the_range_is_in_the_address_and_the_schema_lists_the_words() {
        let action = action("metrics.usage");
        assert_eq!(action.path(), "/api/1/metrics/usage/{range}");
        let found = find(REGISTRY, false, "/api/1/metrics/usage/7d");
        let super::super::Found::Action(found, params) = found else { panic!("{found:?}") };
        assert_eq!(found.id(), "metrics.usage");
        assert_eq!(params["range"], "7d");
        assert!(matches!(find(REGISTRY, false, "/api/1/metrics/usage"), super::super::Found::None));
        assert!(matches!(
            find(REGISTRY, true, "/api/1/metrics/usage/7d"),
            super::super::Found::WrongMethod
        ));
        let schema = super::super::schema(action.fields());
        assert_eq!(schema["properties"]["range"]["enum"], json!(["1h", "24h", "7d", "1y"]));
        assert_eq!(schema["required"], json!(["range"]));
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn the_ranges_make_a_few_hundred_points_each() {
        // The width of a point against the length of the range: no answer
        // has more than 400 points of a series.
        let seconds = |text: &str| -> u32 {
            let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap();
            let unit = match &text[digits..] {
                "m" => 60,
                "h" => 3600,
                "d" => 86_400,
                other => panic!("{other}"),
            };
            text[..digits].parse::<u32>().unwrap() * unit
        };
        for (word, since, every) in RANGES {
            let points = seconds(since) / seconds(every);
            assert!((24..=400).contains(&points), "{word}: {points} points");
        }
        assert_eq!(RANGES.map(|(word, _, _)| word), WORDS);
    }

    #[test]
    fn a_token_reads_the_usage_with_the_metrics_scope() {
        for id in ["metrics.now", "metrics.usage"] {
            let call = |list: &str| {
                let scopes = Scopes::parse(list).unwrap();
                authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
            };
            for list in ["metrics", "metrics:read", "*", "services,metrics:read"] {
                assert!(call(list).is_ok(), "{id} {list}");
            }
            for list in ["console", "security:read", "services"] {
                let refused = call(list).unwrap_err();
                assert_eq!(refused.kind, Kind::OutOfScope, "{id} {list}");
                assert_eq!(
                    refused.detail,
                    "the API token \"ci\" may not do this: it needs the scope metrics:read, \
                     metrics or *"
                );
            }
        }
    }

    #[test]
    fn any_signed_in_account_may_read_them_for_it_runs_as_the_person() {
        for id in ["metrics.now", "metrics.usage"] {
            for (admin, unlocked) in [(false, false), (true, false), (true, true)] {
                let caller = Caller::Session { name: "bob", admin, unlocked };
                assert!(authorize(&caller, action(id)).is_ok(), "{id} {admin} {unlocked}");
            }
        }
    }

    /// 2025-11-14 06:00:00 UTC: a moment for the argv tests.
    const AT: i64 = 1_763_100_000;

    fn around(at: i64, range: &str) -> Value {
        json!({ "at": at, "range": range })
    }

    #[test]
    fn a_moment_shows_60_points_before_and_10_after_at_the_width_of_the_range() {
        let action = action("metrics.around");
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (false, Runs::AsThePerson, "metrics", false)
        );
        assert_eq!(
            (action.path(), action.title()),
            ("/api/1/metrics/around/{at}", "Show the usage around a time")
        );
        assert!(action.errors().is_empty());
        // The clock is fixed by the example's own time, so the arguments
        // here use a time that is a few minutes back.
        let at = unix_now() - 600;
        for (range, every, width) in
            [("1h", "1m", 60), ("24h", "1m", 60), ("7d", "15m", 900), ("1y", "1h", 3600)]
        {
            let prepared = action.prepare(around(at, range)).unwrap();
            let since = utc(at - 60 * width);
            let until = utc(at + 10 * width);
            let expected = [
                "metrics",
                "show",
                "service.cpu",
                "service.memory",
                "--since",
                since.as_str(),
                "--until",
                until.as_str(),
                "--every",
                every,
                "--json",
            ];
            assert_eq!(prepared.argv, expected, "{range}");
            assert_eq!(prepared.args, around(at, range));
            assert_eq!(prepared.probe, None);
            assert_eq!(prepared.ansible.params["argv"], json!(expected));
            assert!(!prepared.ansible.become_root);
            assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        }
    }

    #[test]
    fn the_times_are_in_utc_and_the_command_reads_them_back_as_they_were_made() {
        let (since, until) = (utc(AT - 3600), utc(AT + 600));
        assert_eq!(since, "2025-11-14 05:00:00 UTC");
        assert_eq!(until, "2025-11-14 06:10:00 UTC");
        let now_ns = u64::try_from(AT).unwrap() * 1_000_000_000;
        for (text, seconds) in [(since, AT - 3600), (until, AT + 600)] {
            // `metrics show` reads `--since` and `--until` with this.
            let read = crate::log::parse_time(&text, now_ns, false).unwrap();
            assert_eq!(read, u64::try_from(seconds).unwrap() * 1_000_000_000, "{text}");
        }
    }

    #[test]
    fn a_moment_is_in_the_last_400_days_and_not_over_a_minute_ahead() {
        let now = 1_800_000_000;
        let at = |at, range: &str| Moment { at, range: range.into() }.check_at(now);
        for ok in [now - 400 * 86_400, now - 600, now, now + 60] {
            assert!(at(ok, "24h").is_ok(), "{ok}");
        }
        for refused in [now - 400 * 86_400 - 1, now + 61, 0, now + 86_400] {
            let bad = at(refused, "24h").unwrap_err();
            assert_eq!(bad.field, "at", "{refused}");
            assert_eq!(
                bad.why,
                "The moment is not more than 400 days back, nor more than a minute ahead. \
                 Nothing was done."
            );
        }
        // The time is judged first, then the range.
        assert_eq!(at(now, "2y").unwrap_err().field, "range");
        assert_eq!(at(0, "2y").unwrap_err().field, "at");
    }

    #[test]
    fn a_moment_that_cannot_work_is_adm_125_with_the_field_and_nothing_else_is() {
        let action = action("metrics.around");
        let now = unix_now();
        for (args, field) in [
            (around(now - 400 * 86_400 - 60, "24h"), "at"),
            (around(now + 3600, "24h"), "at"),
            (around(-1, "24h"), "at"),
            (around(253_402_300_800, "24h"), "at"),
            (around(now, "2y"), "range"),
            (around(now, ""), "range"),
            (around(now, "24H"), "range"),
        ] {
            let failure = action.prepare(args.clone()).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some(field)), "{args}");
        }
        for args in [
            json!({}),
            json!({ "at": now }),
            json!({ "range": "24h" }),
            json!({ "at": "now", "range": "24h" }),
            json!({ "at": 1.5, "range": "24h" }),
            json!({ "at": now, "range": "24h", "series": "cpu" }),
        ] {
            assert_eq!(action.prepare(args.clone()).unwrap_err().kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn the_moment_is_in_the_address_as_a_number_and_the_schema_says_so() {
        let action = action("metrics.around");
        let found = find(REGISTRY, false, "/api/1/metrics/around/1763100000");
        let super::super::Found::Action(found, params) = found else { panic!("{found:?}") };
        assert_eq!(found.id(), "metrics.around");
        assert_eq!(params["at"], "1763100000");
        // The read takes the part as a number, and the range from the query.
        let at = unix_now() - 600;
        let mut params = super::super::Params::new();
        params.insert("at".into(), json!(at.to_string()));
        let args = super::super::query_arguments(params, "range=7d", action.fields()).unwrap();
        assert_eq!(args, around(at, "7d"));
        assert!(action.prepare(args).is_ok());
        let mut params = super::super::Params::new();
        params.insert("at".into(), json!("soon"));
        let failure =
            super::super::query_arguments(params, "range=7d", action.fields()).unwrap_err();
        assert_eq!(failure.kind, Kind::BadRequest);
        assert!(matches!(
            find(REGISTRY, false, "/api/1/metrics/around"),
            super::super::Found::None
        ));
        assert!(matches!(
            find(REGISTRY, true, "/api/1/metrics/around/1763100000"),
            super::super::Found::WrongMethod
        ));
        let schema = super::super::schema(action.fields());
        assert_eq!(schema["properties"]["at"]["type"], "integer");
        assert_eq!(schema["properties"]["range"]["enum"], json!(["1h", "24h", "7d", "1y"]));
        assert_eq!(schema["required"], json!(["at", "range"]));
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn the_chart_view_reads_the_moment_for_the_person_and_its_test_is_the_charts() {
        let action = action("metrics.around");
        assert_eq!(action.tests().browser, "web-console/chart-to-logs");
        assert_eq!(action.tests().checks, "admind::actions::metrics::tests");
        let scopes = Scopes::parse("metrics:read").unwrap();
        assert!(authorize(&Caller::Token { name: "ci", scopes: &scopes }, action).is_ok());
        let other = Scopes::parse("logs").unwrap();
        assert_eq!(
            authorize(&Caller::Token { name: "ci", scopes: &other }, action).unwrap_err().kind,
            Kind::OutOfScope
        );
        for (admin, unlocked) in [(false, false), (true, false), (true, true)] {
            let caller = Caller::Session { name: "bob", admin, unlocked };
            assert!(authorize(&caller, action).is_ok(), "{admin} {unlocked}");
        }
    }
}
