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

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests};

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
const WORDS: [&str; 4] = [RANGES[0].0, RANGES[1].0, RANGES[2].0, RANGES[3].0];

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

impl Range {
    /// What `--since` and `--every` are for the range, if it is one of
    /// the words.
    fn options(&self) -> Option<(&'static str, &'static str)> {
        RANGES
            .iter()
            .find(|(word, _, _)| *word == self.range)
            .map(|&(_, since, every)| (since, every))
    }
}

impl Args for Range {
    const FIELDS: &'static [Field] =
        &[Field::one_of("range", "How far back the usage goes: 1h, 24h, 7d or 1y.", &WORDS)];

    fn check(&self) -> Result<(), Bad> {
        if self.options().is_some() {
            return Ok(());
        }
        Err(Bad {
            field: "range",
            why: "The usage range is 1h, 24h, 7d or 1y. Nothing was done.".into(),
        })
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

#[cfg(test)]
mod tests {
    use super::super::{Caller, Registered, authorize, find};
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

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
}
