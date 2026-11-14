// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! startup(8): why starting took as long as it did (ADR 0474). Each
//! start's record keeps its times ([`Times`]): how long the firmware and
//! the boot loader took where the firmware says, when the kernel started
//! Spark, when each service started and was ready and what it waited
//! for, when the services settled and when the first person signed in.
//! `startup` shows each step from the firmware to the sign-in, the chain
//! of services that set the time the services settled, as
//! systemd-analyze's critical-chain does, and the ten slowest; with
//! `--compare` it puts a start beside the last good one. The health check
//! uses [`slow_notice`] to say when starting has become slower than it
//! was (BOOT-106).

use serde::Serialize;

use crate::args::{Arg, Args};
use crate::changes;
use crate::sysupdate::starts::{self, Outcome, ServiceTime, Start, Times};
use crate::util::{Fail, fail};

const USAGE: &str = "usage: startup [--start N] [--compare[=N]] [--json]";

/// How many of the slowest services are shown.
const SLOWEST: usize = 10;

/// How many of the last good starts the usual time is the median of, all
/// of which a start needs to be told slow.
const RECENT: usize = 5;

/// How much later than usual, in percent, and at least how many
/// milliseconds, the services settling is a slow start.
const LATER_PERCENT: u64 = 20;
const LATER_MS: u64 = 2000;

/// The least change in a service's time that `--compare` shows, in
/// milliseconds, and the most services it shows.
const CHANGED_MS: u64 = 100;
const CHANGED_SHOWN: usize = 5;

/// `ms` as seconds with a tenth: `7.5 s`.
#[must_use]
pub fn seconds(ms: u64) -> String {
    let tenths = (ms + 50) / 100;
    format!("{}.{} s", tenths / 10, tenths % 10)
}

/// One step of a start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    pub name: &'static str,
    /// How long it took, in milliseconds, if it is known.
    pub ms: Option<u64>,
}

/// The steps of a start with `times`: the firmware and the boot loader
/// where they say, the kernel up to starting Spark, the services up to
/// when they settled, and from then to the first sign-in, none if it was
/// before they settled.
#[must_use]
pub fn steps(times: &Times) -> [Step; 5] {
    [
        Step { name: "firmware", ms: times.firmware_ms },
        Step { name: "boot loader", ms: times.loader_ms },
        Step { name: "kernel", ms: times.spark_ms },
        Step {
            name: "services",
            ms: Some(times.settled_ms.saturating_sub(times.spark_ms.unwrap_or(0))),
        },
        Step {
            name: "sign-in",
            ms: times.sign_in_ms.map(|ms| ms.saturating_sub(times.settled_ms)),
        },
    ]
}

/// Whether `times` hold a start's services, and not only a sign-in.
#[must_use]
pub fn complete(times: &Times) -> bool {
    !times.services.is_empty()
}

/// How long `service` took to be ready after it started.
fn took(service: &ServiceTime) -> Option<u64> {
    Some(service.ready_ms?.saturating_sub(service.started_ms))
}

/// The chain of services that set when the services settled, the first
/// to start first: from the one that was ready last, each the one before
/// it waited longest for, among those it started after, that were ready
/// by the time it started.
#[must_use]
pub fn chain(times: &Times) -> Vec<&ServiceTime> {
    let ready = times.services.iter().filter(|service| service.ready_ms.is_some());
    let Some(mut current) = ready.max_by_key(|service| (service.ready_ms, service.started_ms))
    else {
        return Vec::new();
    };
    let mut found = vec![current];
    while let Some(before) = current
        .after
        .iter()
        .filter_map(|name| times.services.iter().find(|service| service.name == *name))
        .filter(|service| {
            service.ready_ms.is_some_and(|ready| ready <= current.started_ms)
                && !found.iter().any(|seen| seen.name == service.name)
        })
        .max_by_key(|service| service.ready_ms)
    {
        found.push(before);
        current = before;
    }
    found.reverse();
    found
}

/// The [`SLOWEST`] services, by how long each took from start to ready,
/// longest first.
#[must_use]
pub fn slowest(times: &Times) -> Vec<(&ServiceTime, u64)> {
    let mut all: Vec<(&ServiceTime, u64)> =
        times.services.iter().filter_map(|service| Some((service, took(service)?))).collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.cmp(&b.0.name)));
    all.truncate(SLOWEST);
    all
}

/// Rows lined up in columns, the last unpadded, as lines.
fn lined_up(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row).take(columns.saturating_sub(1)) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut text = String::new();
    for row in rows {
        let mut line = String::from("  ");
        for (i, cell) in row.iter().enumerate() {
            if i + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[i]));
            }
        }
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

/// What `startup` says of `start` and its `times`.
#[must_use]
pub fn report(start: &Start, times: &Times) -> String {
    let mut text = format!("{}\n", changes::start_line(start));
    let mut rows = Vec::new();
    for step in steps(times) {
        let how = step.ms.map_or("not known".to_string(), seconds);
        let note = match (step.name, step.ms) {
            ("services", _) => {
                format!("settled {} after the kernel started", seconds(times.settled_ms))
            }
            ("sign-in", Some(0)) => "the first sign-in was before the services settled".into(),
            ("sign-in", Some(_)) => {
                format!(
                    "the first sign-in, {} after the kernel started",
                    seconds(times.sign_in_ms.unwrap_or(0))
                )
            }
            ("sign-in", None) => "nobody has signed in".into(),
            (_, None) => "this computer's firmware does not say".into(),
            _ => String::new(),
        };
        rows.push(vec![step.name.to_string(), how, note]);
    }
    text.push_str(&lined_up(&rows));
    let from_power_on: u64 = steps(times)[..4].iter().filter_map(|step| step.ms).sum();
    text.push_str(&format!(
        "From the computer starting to the services settling: {}\n",
        seconds(from_power_on)
    ));

    let chain = chain(times);
    if chain.is_empty() {
        text.push_str("\nNo service was ready, so there is no chain.\n");
    } else {
        text.push_str(
            "\nThe chain that set when the services settled, each waiting for the one above:\n",
        );
        let rows: Vec<Vec<String>> = chain
            .iter()
            .map(|service| {
                vec![
                    service.name.clone(),
                    format!("started {}", seconds(service.started_ms)),
                    format!("ready {}", seconds(service.ready_ms.unwrap_or(0))),
                    format!("took {}", seconds(took(service).unwrap_or(0))),
                ]
            })
            .collect();
        text.push_str(&lined_up(&rows));
    }
    text.push_str("\nThe slowest services, from started to ready:\n");
    let rows: Vec<Vec<String>> = slowest(times)
        .into_iter()
        .map(|(service, took)| {
            vec![
                service.name.clone(),
                seconds(took),
                format!("started {}", seconds(service.started_ms)),
            ]
        })
        .collect();
    text.push_str(&lined_up(&rows));
    text
}

/// What `startup --compare` says of `start` and `other`.
#[must_use]
pub fn comparison(start: &Start, times: &Times, other: &Start, before: &Times) -> String {
    let kind = if other.outcome == Outcome::Good { "good start " } else { "start " };
    let mut text = format!("Start {} beside the last {kind}{}:\n", start.number, other.number);
    let mut rows = vec![vec![
        String::new(),
        format!("start {}", other.number),
        format!("start {}", start.number),
        "change".to_string(),
    ]];
    for (after, was) in steps(times).iter().zip(steps(before)) {
        let (Some(now), Some(then)) = (after.ms, was.ms) else {
            rows.push(vec![
                after.name.into(),
                was.ms.map_or("-".into(), seconds),
                after.ms.map_or("-".into(), seconds),
                String::new(),
            ]);
            continue;
        };
        rows.push(vec![after.name.into(), seconds(then), seconds(now), signed(now, then)]);
    }
    rows.push(vec![
        "settled".into(),
        seconds(before.settled_ms),
        seconds(times.settled_ms),
        signed(times.settled_ms, before.settled_ms),
    ]);
    text.push_str(&lined_up(&rows));

    let mut changed: Vec<(&str, Option<u64>, u64)> = times
        .services
        .iter()
        .filter_map(|service| {
            let now = took(service)?;
            let then = before.services.iter().find(|old| old.name == service.name).and_then(took);
            Some((service.name.as_str(), then, now))
        })
        .filter(|(_, then, now)| then.is_none_or(|then| then.abs_diff(*now) >= CHANGED_MS))
        .collect();
    changed.sort_by_key(|(name, then, now)| {
        (std::cmp::Reverse(then.map_or(*now, |then| then.abs_diff(*now))), *name)
    });
    changed.truncate(CHANGED_SHOWN);
    if changed.is_empty() {
        text.push_str("\nNo service took a different time.\n");
    } else {
        text.push_str("\nThe services whose time changed most, from started to ready:\n");
        let rows: Vec<Vec<String>> = changed
            .iter()
            .map(|(name, then, now)| match then {
                Some(then) => {
                    vec![(*name).to_string(), seconds(*then), seconds(*now), signed(*now, *then)]
                }
                None => vec![(*name).to_string(), "-".into(), seconds(*now), "new".into()],
            })
            .collect();
        text.push_str(&lined_up(&rows));
    }
    text
}

/// The change from `then` to `now`: `+2.8 s`, `-0.4 s` or `0.0 s`.
fn signed(now: u64, then: u64) -> String {
    match now.cmp(&then) {
        std::cmp::Ordering::Greater => format!("+{}", seconds(now - then)),
        std::cmp::Ordering::Less => format!("-{}", seconds(then - now)),
        std::cmp::Ordering::Equal => seconds(0),
    }
}

/// The middle of `values`, the lower middle of an even number.
fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    values.get(values.len().checked_sub(1)? / 2).copied()
}

/// What to say when `this` start's services settled later than usual
/// (BOOT-106): more than [`LATER_PERCENT`] percent and [`LATER_MS`]
/// milliseconds later than the median of the last [`RECENT`] good starts
/// of `records` before it, which all need to have their times. Says the
/// step that grew most, and the service that did, if the services did.
#[must_use]
pub fn slow_notice(records: &[Start], this: &Start) -> Option<String> {
    let now = this.times.as_ref().filter(|times| complete(times))?;
    let usual: Vec<&Times> = records
        .iter()
        .rev()
        .filter(|start| start.number < this.number && start.outcome == Outcome::Good)
        .filter_map(|start| start.times.as_ref().filter(|times| complete(times)))
        .take(RECENT)
        .collect();
    if usual.len() < RECENT {
        return None;
    }
    let was = median(usual.iter().map(|times| times.settled_ms).collect())?;
    let later = now.settled_ms.checked_sub(was)?;
    if later < LATER_MS || later * 100 <= was * LATER_PERCENT {
        return None;
    }
    let percent = (later * 100 + was / 2) / was.max(1);
    let mut text = format!(
        "This start took {} to settle, {percent}% longer than usual",
        seconds(now.settled_ms)
    );
    // The step that grew most.
    let grew = |step: usize| {
        let ms = steps(now)[step].ms?;
        let baseline = median(usual.iter().filter_map(|times| steps(times)[step].ms).collect())?;
        Some((ms.saturating_sub(baseline), step))
    };
    let Some((by, step)) = (0..4)
        .filter_map(grew)
        .filter(|(by, _)| *by > 0)
        .max_by_key(|(by, step)| (*by, std::cmp::Reverse(*step)))
    else {
        text.push_str(" (BOOT-106)");
        return Some(text);
    };
    let name = ["the firmware", "the boot loader", "the kernel", "the services"][step];
    text.push_str(&format!(": {name} grew by {}", seconds(by)));
    if step == 3
        && let Some(service) = grown_service(now, &usual)
    {
        text.push_str(&format!(", most in {service}"));
    }
    text.push_str(" (BOOT-106)");
    Some(text)
}

/// The service that took longest more than it usually does, if one did.
fn grown_service(now: &Times, usual: &[&Times]) -> Option<String> {
    now.services
        .iter()
        .filter_map(|service| {
            let ms = took(service)?;
            let then = median(
                usual
                    .iter()
                    .filter_map(|times| times.services.iter().find(|old| old.name == service.name))
                    .filter_map(took)
                    .collect(),
            )
            .unwrap_or(0);
            Some((ms.saturating_sub(then), &service.name))
        })
        .filter(|(by, _)| *by > 0)
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(a.1)))
        .map(|(_, name)| name.clone())
}

/// startup(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut asked: Option<u64> = None;
    let (mut json, mut compare) = (false, false);
    let mut other: Option<u64> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "start" => asked = Some(args.number()?),
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "compare" => {
                compare = true;
                if let Some(value) = args.optional_value() {
                    let text = value.to_string_lossy().into_owned();
                    other = Some(
                        text.parse()
                            .map_err(|_| fail(format!("{text:?} is not a start's number")))?,
                    );
                }
            }
            Arg::Long(name) if name == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let records = starts::all();
    let find = |number: u64| {
        records
            .iter()
            .find(|record| record.number == number)
            .ok_or_else(|| fail(format!("no record of start {number} is kept")))
    };
    let Some(start) = (match asked {
        Some(number) => Some(find(number)?),
        None => records.last(),
    }) else {
        println!("startup: no start has been recorded yet");
        return Ok(1);
    };
    let live;
    let times = match start.times.as_ref().filter(|times| complete(times)) {
        Some(times) => times,
        None if records.last().is_some_and(|last| last.number == start.number) => {
            // The health check has not finished the record: this start's
            // times as Spark says them now.
            live = starts::Times::now(start.times.as_ref().and_then(|times| times.sign_in_ms))
                .map_err(|why| fail(format!("Spark: {why}")))?;
            &live
        }
        None => {
            println!(
                "startup: start {} has no times: it began before they were kept, or ended before the health check saw it through",
                start.number
            );
            return Ok(1);
        }
    };
    let against = if compare {
        let other = match other {
            Some(number) => find(number)?,
            None => starts::last_good(&records, start.number)
                .filter(|good| good.times.as_ref().is_some_and(complete))
                .or_else(|| {
                    records.iter().rev().find(|record| {
                        record.number < start.number
                            && record.outcome == Outcome::Good
                            && record.times.as_ref().is_some_and(complete)
                    })
                })
                .ok_or_else(|| fail("no good start with times is kept to compare with".into()))?,
        };
        let before = other
            .times
            .as_ref()
            .filter(|times| complete(times))
            .ok_or_else(|| fail(format!("start {} has no times", other.number)))?;
        Some((other, before))
    } else {
        None
    };
    if json {
        println!("{}", json_text(start, times, against)?);
    } else {
        print!("{}", report(start, times));
        if let Some((other, before)) = against {
            println!();
            print!("{}", comparison(start, times, other, before));
        }
    }
    Ok(0)
}

/// `--json`: the start's steps, its chain and its slowest services, and
/// the steps of the start it is compared with.
fn json_text(
    start: &Start,
    times: &Times,
    against: Option<(&Start, &Times)>,
) -> Result<String, Fail> {
    #[derive(Serialize)]
    struct Said<'a> {
        start: u64,
        outcome: Outcome,
        settled_ms: u64,
        sign_in_ms: Option<u64>,
        steps: [Step; 5],
        chain: Vec<&'a ServiceTime>,
        slowest: Vec<Slow<'a>>,
        compared_with: Option<Compared>,
    }
    #[derive(Serialize)]
    struct Slow<'a> {
        name: &'a str,
        took_ms: u64,
        started_ms: u64,
    }
    #[derive(Serialize)]
    struct Compared {
        start: u64,
        settled_ms: u64,
        steps: [Step; 5],
    }
    let said = Said {
        start: start.number,
        outcome: start.outcome,
        settled_ms: times.settled_ms,
        sign_in_ms: times.sign_in_ms,
        steps: steps(times),
        chain: chain(times),
        slowest: slowest(times)
            .into_iter()
            .map(|(service, took_ms)| Slow {
                name: &service.name,
                took_ms,
                started_ms: service.started_ms,
            })
            .collect(),
        compared_with: against.map(|(other, before)| Compared {
            start: other.number,
            settled_ms: before.settled_ms,
            steps: steps(before),
        }),
    };
    serde_json::to_string(&said).map_err(|err| fail(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysupdate::starts::Item;

    fn service(name: &str, started: u64, ready: Option<u64>, after: &[&str]) -> ServiceTime {
        ServiceTime {
            name: name.into(),
            started_ms: started,
            ready_ms: ready,
            after: after.iter().map(|name| (*name).to_string()).collect(),
        }
    }

    /// The times of a start of an ordinary system, `extra` ms of which are
    /// in the database, which web waits for.
    fn times(extra: u64) -> Times {
        Times {
            firmware_ms: Some(1800),
            loader_ms: Some(900),
            spark_ms: Some(1200),
            settled_ms: 5000 + extra,
            sign_in_ms: None,
            services: vec![
                service("logd", 1300, Some(1400), &[]),
                service("netd", 1500, Some(2500), &["logd"]),
                service("db", 1500, Some(2000 + extra), &["logd"]),
                service("web", 2600 + extra, Some(5000 + extra), &["db", "netd"]),
                service("sshd", 2600, Some(2900), &["netd"]),
                service("hung", 1500, None, &[]),
            ],
        }
    }

    fn start(number: u64, outcome: Outcome, times: Option<Times>) -> Start {
        Start {
            number,
            started: 1_800_000_000 + i64::try_from(number).unwrap() * 100,
            slot: "a".into(),
            stage: "services".into(),
            outcome,
            error: None,
            why: None,
            items: vec![Item::new("system", "Dukovo", "0.15.0", None)],
            times,
        }
    }

    fn names<'a>(found: &[&'a ServiceTime]) -> Vec<&'a str> {
        found.iter().map(|service| service.name.as_str()).collect()
    }

    #[test]
    fn the_steps_run_from_the_firmware_to_the_first_sign_in() {
        let mut times = times(0);
        let ms = |times: &Times| steps(times).map(|step| step.ms);
        assert_eq!(ms(&times), [Some(1800), Some(900), Some(1200), Some(3800), None]);
        times.sign_in_ms = Some(9000);
        assert_eq!(ms(&times)[4], Some(4000));
        // Before the services settled: none after.
        times.sign_in_ms = Some(4000);
        assert_eq!(ms(&times)[4], Some(0));
        // A firmware that says nothing, and a kernel that did not say when.
        times.firmware_ms = None;
        times.loader_ms = None;
        times.spark_ms = None;
        assert_eq!(ms(&times), [None, None, None, Some(5000), Some(0)]);
    }

    #[test]
    fn the_chain_follows_the_service_each_one_waited_for_longest() {
        // web waited for db (ready last) and netd; db for logd.
        assert_eq!(names(&chain(&times(0))), ["logd", "netd", "web"]);
        // When the database is slow, it is the one web waited for.
        assert_eq!(names(&chain(&times(3000))), ["logd", "db", "web"]);
        assert!(chain(&Times::default()).is_empty());
        // One that was not ready does not end the chain, nor start it.
        let stuck = Times { services: vec![service("hung", 100, None, &[])], ..Times::default() };
        assert!(chain(&stuck).is_empty());
        // A service that became ready after the one that follows it started did
        // not hold it up (a want that does not wait), and a loop ends.
        let looped = Times {
            services: vec![
                service("a", 100, Some(500), &["b"]),
                service("b", 100, Some(300), &["a"]),
            ],
            ..Times::default()
        };
        assert_eq!(names(&chain(&looped)), ["a"]);
        let ring = Times {
            services: vec![
                service("a", 400, Some(500), &["b"]),
                service("b", 300, Some(399), &["a", "b"]),
            ],
            ..Times::default()
        };
        assert_eq!(names(&chain(&ring)), ["b", "a"]);
    }

    #[test]
    fn the_slowest_services_are_the_ten_that_took_longest_from_start_to_ready() {
        let slow = times(3000);
        let found: Vec<(&str, u64)> =
            slowest(&slow).iter().map(|(service, took)| (service.name.as_str(), *took)).collect();
        assert_eq!(
            found,
            [("db", 3500), ("web", 2400), ("netd", 1000), ("sshd", 300), ("logd", 100)]
        );
        let many = Times {
            services: (0..15)
                .map(|n| service(&format!("s{n:02}"), 0, Some(100 + n), &[]))
                .collect(),
            ..Times::default()
        };
        let found = slowest(&many);
        assert_eq!(found.len(), 10);
        assert_eq!((found[0].0.name.as_str(), found[9].0.name.as_str()), ("s14", "s05"));
    }

    #[test]
    fn the_report_says_each_step_the_chain_and_the_slowest() {
        let mut times = times(3000);
        times.sign_in_ms = Some(12_000);
        let said = report(&start(7, Outcome::Good, Some(times.clone())), &times);
        assert!(said.starts_with("Start "), "{said}");
        assert!(said.contains(": good\n"), "{said}");
        for line in [
            "firmware     1.8 s",
            "boot loader  0.9 s",
            "kernel       1.2 s",
            "services     6.8 s  settled 8.0 s after the kernel started",
            "sign-in      4.0 s  the first sign-in, 12.0 s after the kernel started",
            "From the computer starting to the services settling: 10.7 s",
            "The chain that set when the services settled, each waiting for the one above:",
            "logd  started 1.3 s  ready 1.4 s  took 0.1 s",
            "db    started 1.5 s  ready 5.0 s  took 3.5 s",
            "web   started 5.6 s  ready 8.0 s  took 2.4 s",
            "The slowest services, from started to ready:",
        ] {
            assert!(said.contains(line), "{line:?} is not in\n{said}");
        }
        // What the firmware does not say, and nobody signed in.
        times.firmware_ms = None;
        times.sign_in_ms = None;
        let said = report(&start(7, Outcome::Good, None), &times);
        assert!(
            said.contains("firmware     not known  this computer's firmware does not say"),
            "{said}"
        );
        assert!(said.contains("nobody has signed in"), "{said}");
    }

    #[test]
    fn the_comparison_says_what_changed_step_by_step_and_service_by_service() {
        // Columns are as wide as their widest cell: only the words are held to.
        let squeezed = |text: String| {
            text.split(' ').filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
        };
        let (old, new) = (times(0), times(3000));
        let said = squeezed(comparison(
            &start(8, Outcome::Failed, Some(new.clone())),
            &new,
            &start(5, Outcome::Good, Some(old.clone())),
            &old,
        ));
        assert!(said.starts_with("Start 8 beside the last good start 5:\n"), "{said}");
        assert!(said.contains("start 5 start 8 change"), "{said}");
        assert!(said.contains("services 3.8 s 6.8 s +3.0 s"), "{said}");
        assert!(said.contains("firmware 1.8 s 1.8 s 0.0 s"), "{said}");
        assert!(said.contains("sign-in - -"), "{said}");
        assert!(said.contains("settled 5.0 s 8.0 s +3.0 s"), "{said}");
        // Only the service whose time changed.
        assert!(said.contains("\n db 0.5 s 3.5 s +3.0 s\n"), "{said}");
        assert!(!said.contains("\n web ") && !said.contains("\n logd "), "{said}");
        // Nothing changed, and a service that is new.
        let same = comparison(
            &start(8, Outcome::Good, Some(old.clone())),
            &old,
            &start(5, Outcome::Good, Some(old.clone())),
            &old,
        );
        assert!(same.contains("No service took a different time."), "{same}");
        let mut more = old.clone();
        more.services.push(service("cups", 3000, Some(3500), &[]));
        let said = squeezed(comparison(
            &start(8, Outcome::Good, Some(more.clone())),
            &more,
            &start(5, Outcome::Good, Some(old.clone())),
            &old,
        ));
        assert!(said.contains("\n cups - 0.5 s new\n"), "{said}");
    }

    #[test]
    fn a_slow_start_is_told_when_the_services_settle_20_percent_and_2_seconds_later() {
        let usual = |n: u64, settled: u64| {
            start(n, Outcome::Good, Some(Times { settled_ms: settled, ..times(0) }))
        };
        let mut records: Vec<Start> = [5000, 5200, 4900, 5100, 5000]
            .iter()
            .zip(1..)
            .map(|(settled, n)| usual(n, *settled))
            .collect();
        let this = |number: u64, extra: u64| start(number, Outcome::Good, Some(times(extra)));
        // The median is 5000: 7.0 s is 40% later and 2.0 s, and 6.9 s is under 2 s.
        records.push(this(6, 2000));
        let said = slow_notice(&records, &this(6, 2000)).unwrap();
        assert_eq!(
            said,
            "This start took 7.0 s to settle, 40% longer than usual: the services grew by 2.0 s, \
             most in db (BOOT-106)"
        );
        assert_eq!(slow_notice(&records, &this(6, 1999)), None);
        // 20% of 12 s is 2.4 s: 2.0 s later is not enough, though it is 2 s.
        let long: Vec<Start> = (1..=5).map(|n| usual(n, 12_000)).collect();
        assert_eq!(slow_notice(&long, &this(6, 9000)), None);
        let mut slow_one = this(6, 0);
        slow_one.times.as_mut().unwrap().settled_ms = 14_000;
        assert_eq!(slow_notice(&long, &slow_one), None);
        slow_one.times.as_mut().unwrap().settled_ms = 14_500;
        assert!(slow_notice(&long, &slow_one).is_some());
        // Five good starts are needed, with their times, before it says anything.
        assert_eq!(slow_notice(&records[1..5], &this(6, 2000)), None);
        let mut four = records[..5].to_vec();
        four[0].times = None;
        assert_eq!(slow_notice(&four, &this(6, 2000)), None);
        four[0].outcome = Outcome::Failed;
        four[0].times = Some(times(0));
        assert_eq!(slow_notice(&four, &this(6, 2000)), None);
        // A start with no times, or one that settled sooner, says nothing.
        assert_eq!(slow_notice(&records, &start(6, Outcome::Good, None)), None);
        assert_eq!(slow_notice(&records, &this(6, 0)), None);
        // Later starts are not what a start is held to.
        let mut later = records.clone();
        later.push(usual(9, 1000));
        assert!(slow_notice(&later, &this(6, 2000)).is_some());
    }

    #[test]
    fn the_notice_names_the_step_that_grew_most_and_the_service_in_it() {
        let usual: Vec<Start> = (1..=5).map(|n| start(n, Outcome::Good, Some(times(0)))).collect();
        // The kernel took 3 s longer to reach Spark; the services as long as ever.
        let mut kernel = times(0);
        kernel.spark_ms = Some(4200);
        kernel.settled_ms = 8000;
        for service in &mut kernel.services {
            service.started_ms += 3000;
            service.ready_ms = service.ready_ms.map(|ready| ready + 3000);
        }
        let said = slow_notice(&usual, &start(6, Outcome::Good, Some(kernel))).unwrap();
        assert_eq!(
            said,
            "This start took 8.0 s to settle, 60% longer than usual: the kernel grew by 3.0 s \
             (BOOT-106)"
        );
        // The firmware is the step when it grew most.
        let mut firmware = times(0);
        firmware.settled_ms = 8000;
        firmware.firmware_ms = Some(6800);
        let said = slow_notice(&usual, &start(6, Outcome::Good, Some(firmware))).unwrap();
        assert!(said.contains(": the firmware grew by 5.0 s (BOOT-106)"), "{said}");
        // Settling later with no step that grew says no more than that.
        let mut vague = times(0);
        vague.settled_ms = 8000;
        vague.spark_ms = Some(1200 + 3000);
        vague.services = Vec::new();
        assert_eq!(slow_notice(&usual, &start(6, Outcome::Good, Some(vague))), None);
    }

    #[test]
    fn the_times_in_seconds_round_to_a_tenth() {
        assert_eq!(seconds(0), "0.0 s");
        assert_eq!(seconds(49), "0.0 s");
        assert_eq!(seconds(50), "0.1 s");
        assert_eq!(seconds(1999), "2.0 s");
        assert_eq!(seconds(61_234), "61.2 s");
        assert_eq!(median(vec![5, 1, 3]), Some(3));
        assert_eq!(median(vec![4, 1, 3, 2]), Some(2));
        assert_eq!(median(Vec::new()), None);
    }

    #[test]
    fn the_json_has_the_steps_the_chain_and_the_comparison() {
        let (old, new) = (times(0), times(3000));
        let (a, b) = (
            start(5, Outcome::Good, Some(old.clone())),
            start(8, Outcome::Good, Some(new.clone())),
        );
        let said = json_text(&b, &new, Some((&a, &old))).unwrap();
        let value: serde_json::Value = serde_json::from_str(&said).unwrap();
        assert_eq!(value["start"], 8);
        assert_eq!(value["settled_ms"], 8000);
        assert_eq!(value["steps"][3]["name"], "services");
        assert_eq!(value["steps"][3]["ms"], 6800);
        assert_eq!(value["chain"][1]["name"], "db");
        assert_eq!(value["slowest"][0]["took_ms"], 3500);
        assert_eq!(value["compared_with"]["start"], 5);
        let alone = json_text(&b, &new, None).unwrap();
        assert!(alone.contains("\"compared_with\":null"), "{alone}");
    }
}
