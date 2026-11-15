// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test matrix --list`: the jobs of the update test matrix, as the
//! JSON that .github/workflows/update-matrix.yml gives its
//! `strategy.matrix` (ADR 0536).
//!
//! The nightly run takes a sample: sixteen power cuts of each firmware
//! type, the first-start kills, the update boot tests that ran before the
//! matrix did, the updates from each old release's fixture under each
//! firmware type, the TUF conformance suite, and each fuzz target for ten
//! minutes. `--full`, for a release candidate, takes every power cut, in
//! eight parts of each firmware type that run at once, twenty kills of the
//! first start, and each fuzz target for an hour. The jobs of the updates
//! from an old release run after the workflow's `fixtures` job, which
//! makes the fixtures they boot and keeps them in the Actions cache.

use serde_json::{Value, json};

use super::fuzz;
use super::upgrade::{self, FIXTURE_RELEASES};
use crate::{Args, Result};

/// The power cut replays, by firmware type: the case, and a name for the
/// job.
const REPLAYS: [(&str, &str); 2] =
    [("update-power-replay", "replay-uefi"), ("update-power-replay-bios", "replay-bios")];

/// The first-start kill loops, likewise.
const KILLS: [(&str, &str); 2] = [
    ("update-first-start-kills", "first-start-kills-uefi"),
    ("update-first-start-kills-bios", "first-start-kills-bios"),
];

/// The update boot tests that stood before the matrix, run together.
const UPDATE_CASES: [&str; 13] = [
    "update-stage",
    "update-power-cut",
    "update-panic",
    "update-hang",
    "update-fallback",
    "update-offline",
    "update-online",
    "update-network-down",
    "update-root-missing",
    "update-disk-full",
    "update-bad-network",
    "update-clock-ahead",
    "update-clock-behind",
];

/// How long each fuzz target runs, in seconds, in a nightly run and in a
/// candidate's.
const FUZZ_SECONDS: (u64, u64) = (600, 3600);

/// How many cuts of each firmware type a nightly run boots.
const NIGHTLY_POINTS: usize = 16;

/// How many parts a candidate's full replay of each firmware type is
/// shared out in.
const PARTS: usize = 8;

/// How many times a candidate's run kills the first start.
const FULL_KILLS: &str = "20";

/// One job of the matrix: a name, the `./ob` command it runs, and the
/// environment it runs it in.
#[derive(Debug, PartialEq, Eq)]
struct Job {
    name: String,
    run: String,
    env: Vec<(&'static str, &'static str)>,
}

impl Job {
    fn new(name: impl Into<String>, run: impl Into<String>) -> Job {
        Job { name: name.into(), run: run.into(), env: Vec::new() }
    }

    fn json(&self) -> Value {
        let mut job = json!({ "name": self.name, "run": self.run });
        if !self.env.is_empty() {
            let env: serde_json::Map<String, Value> =
                self.env.iter().map(|&(name, value)| (name.to_string(), json!(value))).collect();
            job["env"] = Value::Object(env);
        }
        job
    }
}

/// The jobs of the nightly sample, or, with `full`, of a candidate's run.
fn jobs(full: bool) -> Vec<Job> {
    let mut jobs = Vec::new();
    for (case, name) in REPLAYS {
        if full {
            for part in 1..=PARTS {
                jobs.push(Job::new(
                    format!("{name}-part-{part}-of-{PARTS}"),
                    format!("./ob test replay --case {case} --points all --part {part}/{PARTS}"),
                ));
            }
        } else {
            jobs.push(Job::new(
                format!("{name}-{NIGHTLY_POINTS}"),
                format!("./ob test replay --case {case} --points {NIGHTLY_POINTS}"),
            ));
        }
    }
    for (case, name) in KILLS {
        // No --retry: a kill loop that fails once must not pass on a
        // second draw of its times.
        let mut job = Job::new(name, format!("./ob test boot --case {case}"));
        if full {
            job.env.push(("OB_TEST_KILLS", FULL_KILLS));
        }
        jobs.push(job);
    }
    let cases: Vec<String> = UPDATE_CASES.iter().map(|case| format!("--case {case}")).collect();
    jobs.push(Job::new("update-cases", format!("./ob test boot {} --retry", cases.join(" "))));
    // Each fixture release under each firmware type, a job of its own: a
    // case boots the old release and this build four times in all.
    for release in FIXTURE_RELEASES {
        for (bios, firmware) in [(false, "uefi"), (true, "bios")] {
            jobs.push(Job::new(
                format!("upgrade-{}-{firmware}", release.replace('.', "-")),
                format!("./ob test boot --case {} --retry", upgrade::case_name(release, bios)),
            ));
        }
    }
    jobs.push(Job::new("tuf-conformance", "./ob test tuf"));
    let seconds = if full { FUZZ_SECONDS.1 } else { FUZZ_SECONDS.0 };
    for target in fuzz::TARGETS {
        jobs.push(Job::new(
            format!("fuzz-{target}"),
            format!("./ob test fuzz --target {target} --seconds {seconds}"),
        ));
    }
    jobs
}

/// The jobs as one line of JSON: an array of `{"name", "run"}`, with an
/// `env` object for a job that needs environment variables.
fn list(full: bool) -> String {
    Value::Array(jobs(full).iter().map(Job::json).collect()).to_string()
}

/// `ob test matrix`.
pub(super) fn command(args: &mut Args) -> Result<()> {
    let full = args.flag("--full");
    let listed = args.flag("--list");
    args.finish()?;
    if !listed {
        crate::bail!("`ob test matrix` lists the update test matrix's jobs; give --list");
    }
    println!("{}", list(full));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::all_cases;
    use super::*;
    use std::collections::HashSet;

    fn parsed(full: bool) -> Vec<Value> {
        let text = list(full);
        assert!(!text.contains('\n'), "the list is not one line");
        serde_json::from_str::<Vec<Value>>(&text).unwrap()
    }

    #[test]
    fn each_job_has_a_name_and_a_command_and_maybe_an_environment() {
        for full in [false, true] {
            for job in parsed(full) {
                let object = job.as_object().unwrap();
                assert!(object["name"].as_str().is_some_and(|name| !name.is_empty()));
                assert!(object["run"].as_str().is_some_and(|run| run.starts_with("./ob test ")));
                for key in object.keys() {
                    assert!(["name", "run", "env"].contains(&key.as_str()), "{key}");
                }
                if let Some(env) = object.get("env") {
                    assert!(env.as_object().unwrap().values().all(Value::is_string));
                }
            }
        }
    }

    #[test]
    fn no_job_is_listed_twice() {
        for full in [false, true] {
            let jobs = jobs(full);
            let names: HashSet<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
            let commands: HashSet<&str> = jobs.iter().map(|job| job.run.as_str()).collect();
            assert_eq!(names.len(), jobs.len());
            assert_eq!(commands.len(), jobs.len());
            // Each name is fit for an artifact's name.
            assert!(
                names
                    .iter()
                    .all(|name| { name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') })
            );
        }
    }

    #[test]
    fn every_case_a_job_names_exists() {
        let known: HashSet<&str> = all_cases().map(|case| case.name).collect();
        for full in [false, true] {
            for job in jobs(full) {
                let mut words = job.run.split_whitespace();
                while let Some(word) = words.next() {
                    if word == "--case" {
                        let case = words.next().unwrap();
                        assert!(known.contains(case), "`{}` names no boot test {case}", job.run);
                    }
                }
            }
        }
    }

    #[test]
    fn the_nightly_sample_is_sixteen_cuts_of_each_firmware_and_the_rest() {
        let runs: Vec<String> = jobs(false).into_iter().map(|job| job.run).collect();
        assert_eq!(runs.len(), 5 + 2 * FIXTURE_RELEASES.len() + 1 + fuzz::TARGETS.len());
        assert_eq!(runs[0], "./ob test replay --case update-power-replay --points 16");
        assert_eq!(runs[1], "./ob test replay --case update-power-replay-bios --points 16");
        assert_eq!(runs[2], "./ob test boot --case update-first-start-kills");
        assert_eq!(runs[3], "./ob test boot --case update-first-start-kills-bios");
        assert_eq!(
            runs[4],
            "./ob test boot --case update-stage --case update-power-cut --case update-panic \
             --case update-hang --case update-fallback --case update-offline \
             --case update-online --case update-network-down --case update-root-missing \
             --case update-disk-full --case update-bad-network --case update-clock-ahead \
             --case update-clock-behind --retry"
        );
        assert_eq!(runs[5], "./ob test boot --case update-from-0.15.0 --retry");
        assert_eq!(runs[6], "./ob test boot --case update-from-0.15.0-bios --retry");
        assert_eq!(runs[7], "./ob test tuf");
        assert_eq!(runs[8], "./ob test fuzz --target release-list --seconds 600");
        assert!(jobs(false).iter().all(|job| job.env.is_empty()));
    }

    #[test]
    fn each_fuzz_target_runs_for_ten_minutes_and_a_candidates_for_an_hour() {
        for (full, seconds) in [(false, 600), (true, 3600)] {
            let jobs = jobs(full);
            for target in fuzz::TARGETS {
                let run = format!("./ob test fuzz --target {target} --seconds {seconds}");
                let job = jobs.iter().find(|job| job.run == run).unwrap();
                assert_eq!(job.name, format!("fuzz-{target}"));
            }
            let fuzzing = jobs.iter().filter(|job| job.run.starts_with("./ob test fuzz ")).count();
            assert_eq!(fuzzing, fuzz::TARGETS.len());
            assert_eq!(jobs.iter().filter(|job| job.run == "./ob test tuf").count(), 1);
        }
    }

    #[test]
    fn the_full_run_boots_every_cut_in_eight_parts_of_each_firmware() {
        let jobs = jobs(true);
        assert_eq!(
            jobs.len(),
            2 * PARTS + 2 + 1 + 2 * FIXTURE_RELEASES.len() + 1 + fuzz::TARGETS.len()
        );
        for (case, _) in REPLAYS {
            for part in 1..=PARTS {
                let run =
                    format!("./ob test replay --case {case} --points all --part {part}/{PARTS}");
                assert_eq!(jobs.iter().filter(|job| job.run == run).count(), 1, "{run}");
            }
        }
        for (case, _) in KILLS {
            let job = jobs.iter().find(|job| job.run.ends_with(case)).unwrap();
            assert_eq!(job.env, [("OB_TEST_KILLS", "20")]);
        }
        // The update boot tests together, and each update from an old
        // release's fixture.
        let retried = jobs.iter().filter(|job| job.run.contains("--retry")).count();
        assert_eq!(retried, 1 + 2 * FIXTURE_RELEASES.len());
    }

    #[test]
    fn each_fixture_release_has_a_job_for_each_firmware_type_in_both_runs() {
        for full in [false, true] {
            let jobs = jobs(full);
            for release in FIXTURE_RELEASES {
                for (bios, firmware) in [(false, "uefi"), (true, "bios")] {
                    let run = format!(
                        "./ob test boot --case {} --retry",
                        upgrade::case_name(release, bios)
                    );
                    let job = jobs.iter().find(|job| job.run == run).unwrap();
                    assert_eq!(job.name, format!("upgrade-0-15-0-{firmware}"));
                    assert!(job.env.is_empty());
                }
            }
        }
    }

    #[test]
    fn the_update_cases_that_need_no_fixture_do_not_name_one() {
        let jobs = jobs(false);
        let plain = jobs.iter().find(|job| job.name == "update-cases").unwrap();
        assert!(!plain.run.contains("update-from-"));
        // The workflow tells the jobs that need the fixtures by this word.
        let workflow = include_str!("../../../../.github/workflows/update-matrix.yml");
        assert!(workflow.contains("*\"--case update-from-\"*"));
    }

    #[test]
    fn the_environment_is_in_the_job_that_has_one_only() {
        let kills = parsed(true)
            .into_iter()
            .filter(|job| job.get("env").is_some())
            .map(|job| job["env"]["OB_TEST_KILLS"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(kills, ["20", "20"]);
        assert!(parsed(false).iter().all(|job| job.get("env").is_none()));
    }
}
