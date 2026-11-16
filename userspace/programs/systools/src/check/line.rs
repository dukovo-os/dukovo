// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a check of dukovo-check(8) found: the area it looks at, how it
//! ended, the detail, the ID that names a warning, and the line, the
//! summary and the exit status that they make.

use serde::{Deserialize, Serialize};

/// The part of the machine a check looks at, in the order the lines of a
/// run come in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Area {
    Network,
    Ssh,
    Accounts,
    Updates,
    Time,
    Services,
    Health,
    Alerts,
    Backups,
}

impl Area {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Area::Network => "network",
            Area::Ssh => "ssh",
            Area::Accounts => "accounts",
            Area::Updates => "updates",
            Area::Time => "time",
            Area::Services => "services",
            Area::Health => "health",
            Area::Alerts => "alerts",
            Area::Backups => "backups",
        }
    }
}

/// How a check ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    Passed,
    Warning,
    /// Only a failure that another tool found, which the check repeats.
    Failed,
    /// It did not run, and the detail says why.
    Skipped,
}

impl Verdict {
    /// The word of a line: `FAILED` stands out as hardware(8)'s does.
    fn word(self) -> &'static str {
        match self {
            Verdict::Passed => "passed",
            Verdict::Warning => "warning",
            Verdict::Failed => "FAILED",
            Verdict::Skipped => "skipped",
        }
    }
}

/// What a warning of dukovo-check itself is. Each has the ID of its page
/// in the support center (docs/errors) and the one sentence the line
/// gives for what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    FirewallAllowsAll,
    FirewallOff,
    SshPasswords,
    SshAnyone,
    BlockingOff,
    EmptyPassword,
    NoAdministrator,
    UpdateWaiting,
    UpdateCheckOld,
    ClockNotSet,
    ServiceGaveUp,
    NoSnapshots,
    NoAlertReached,
}

impl Problem {
    /// Every problem, for the tests that check each one's ID and advice.
    #[cfg(test)]
    pub(crate) const ALL: [Problem; 13] = [
        Problem::FirewallAllowsAll,
        Problem::FirewallOff,
        Problem::SshPasswords,
        Problem::SshAnyone,
        Problem::BlockingOff,
        Problem::EmptyPassword,
        Problem::NoAdministrator,
        Problem::UpdateWaiting,
        Problem::UpdateCheckOld,
        Problem::ClockNotSet,
        Problem::ServiceGaveUp,
        Problem::NoSnapshots,
        Problem::NoAlertReached,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Problem::FirewallAllowsAll => "SEC-101",
            Problem::FirewallOff => "SEC-102",
            Problem::SshPasswords => "SEC-110",
            Problem::SshAnyone => "SEC-111",
            Problem::BlockingOff => "SEC-112",
            Problem::EmptyPassword => "SEC-120",
            Problem::NoAdministrator => "SEC-121",
            Problem::UpdateWaiting => "SEC-130",
            Problem::UpdateCheckOld => "SEC-131",
            Problem::ClockNotSet => "SEC-140",
            Problem::ServiceGaveUp => "SEC-150",
            Problem::NoSnapshots => "SEC-160",
            Problem::NoAlertReached => "SEC-170",
        }
    }

    /// One sentence on what to do.
    pub(crate) fn advice(self) -> &'static str {
        match self {
            Problem::FirewallAllowsAll => {
                "Remove that rule with elevate firewall remove NUMBER, and open only the ports \
                 that need it, as elevate firewall allow tcp 443 does."
            }
            Problem::FirewallOff => {
                "Run service firewall explain to see why the rules are not loaded, then elevate \
                 service firewall start."
            }
            Problem::SshPasswords => {
                "Add your SSH key, then set PasswordAuthentication no in \
                 /system/settings/ssh/sshd_config."
            }
            Problem::SshAnyone => {
                "Name who may sign in, as AllowGroups admins in /system/settings/ssh/sshd_config, \
                 then run elevate service sshd reload."
            }
            Problem::BlockingOff => {
                "Remove on = false from /system/settings/blocking.toml, which turns blocking \
                 back on."
            }
            Problem::EmptyPassword => {
                "Give each of them a password with elevate passwd NAME, or turn the account off \
                 with elevate user disable NAME."
            }
            Problem::NoAdministrator => {
                "Make an account an administrator with user set --admin NAME, from the console \
                 or the serial port."
            }
            Problem::UpdateWaiting => {
                "Stage it with elevate update apply, and restart when it suits you."
            }
            Problem::UpdateCheckOld => {
                "Run elevate update check, which says why the update server cannot be reached, \
                 and see that updates.toml does not turn checks off."
            }
            Problem::ClockNotSet => {
                "Run timedatectl timesync-status to see what timed tried, and check that a time \
                 server can be reached."
            }
            Problem::ServiceGaveUp => {
                "Run service NAME explain to see why it failed, and elevate service NAME start \
                 once that is mended."
            }
            Problem::NoSnapshots => {
                "Put /users and /shared in the places of /system/settings/snap.toml, and start \
                 the snapshots with elevate service snap start."
            }
            Problem::NoAlertReached => {
                "Run elevate alerts status to see why each channel failed and follow the page of \
                 its ID, then run elevate alerts test to try them again."
            }
        }
    }
}

/// How one check ended, with what to say of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Outcome {
    #[serde(rename = "result")]
    pub(crate) verdict: Verdict,
    pub(crate) detail: String,
    /// The ID of a warning's or a failure's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    /// What to do about it, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fix: Option<String>,
}

impl Outcome {
    pub(crate) fn passed(detail: impl Into<String>) -> Outcome {
        Outcome { verdict: Verdict::Passed, detail: detail.into(), id: None, fix: None }
    }

    /// A check that did not run, for `why`.
    pub(crate) fn skipped(why: impl Into<String>) -> Outcome {
        Outcome { verdict: Verdict::Skipped, detail: why.into(), id: None, fix: None }
    }

    pub(crate) fn warning(problem: Problem, detail: impl Into<String>) -> Outcome {
        Outcome {
            verdict: Verdict::Warning,
            detail: detail.into(),
            id: Some(problem.id().to_string()),
            fix: Some(problem.advice().to_string()),
        }
    }

    /// A warning or a failure that another tool found, with that tool's
    /// ID and advice, so that one fault has one page.
    pub(crate) fn repeating(
        verdict: Verdict,
        detail: impl Into<String>,
        id: Option<String>,
        fix: Option<String>,
    ) -> Outcome {
        Outcome { verdict, detail: detail.into(), id, fix }
    }
}

/// One line of a run: what was checked and how it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Line {
    pub(crate) area: Area,
    pub(crate) check: String,
    #[serde(flatten)]
    pub(crate) outcome: Outcome,
}

impl Line {
    /// The line of the check, and under a warning's or a failure's, a
    /// line with its ID and what to do.
    pub(crate) fn text(&self) -> String {
        let Outcome { verdict, detail, id, fix } = &self.outcome;
        let mut text =
            format!("{:<8} {:<21} {:<8} {detail}", self.area.name(), self.check, verdict.word());
        match (id, fix) {
            (Some(id), Some(fix)) => text.push_str(&format!("\n{:<8} {id}: {fix}", "")),
            (Some(id), None) => text.push_str(&format!("\n{:<8} {id}", "")),
            _ => {}
        }
        text
    }
}

/// How many checks ended each way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Counts {
    pub(crate) passed: usize,
    pub(crate) warning: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
}

impl Counts {
    pub(crate) fn of(lines: &[Line]) -> Counts {
        let mut counts = Counts::default();
        for line in lines {
            match line.outcome.verdict {
                Verdict::Passed => counts.passed += 1,
                Verdict::Warning => counts.warning += 1,
                Verdict::Failed => counts.failed += 1,
                Verdict::Skipped => counts.skipped += 1,
            }
        }
        counts
    }

    /// The exit status of a run: 1 if a check failed, else 2 if one
    /// warned, else 3 if no check passed (it could not run), else 0.
    pub(crate) fn status(&self) -> i32 {
        if self.failed > 0 {
            1
        } else if self.warning > 0 {
            2
        } else if self.passed == 0 {
            3
        } else {
            0
        }
    }

    /// The last line of a run: `9 passed, 1 warning, 1 skipped`. A
    /// failure, which is rare, is counted between the two.
    pub(crate) fn summary(&self) -> String {
        let warnings = if self.warning == 1 { "warning" } else { "warnings" };
        let mut text = format!("{} passed, {} {warnings}", self.passed, self.warning);
        if self.failed > 0 {
            text.push_str(&format!(", {} FAILED", self.failed));
        }
        text.push_str(&format!(", {} skipped", self.skipped));
        text
    }
}

/// The IDs of the lines, each once, in order.
pub(crate) fn ids(lines: &[Line]) -> Vec<&str> {
    let mut ids: Vec<&str> = lines.iter().filter_map(|line| line.outcome.id.as_deref()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(area: Area, check: &str, outcome: Outcome) -> Line {
        Line { area, check: check.into(), outcome }
    }

    #[test]
    fn every_problem_has_its_own_id_and_a_sentence_of_advice() {
        let mut ids: Vec<&str> = Problem::ALL.iter().map(|problem| problem.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Problem::ALL.len());
        for problem in Problem::ALL {
            assert!(problem.id().starts_with("SEC-1"), "{problem:?}");
            assert!(problem.advice().ends_with('.'), "{problem:?}");
            assert_eq!(problem.advice().matches(". ").count(), 0, "{problem:?}: one sentence");
        }
    }

    #[test]
    fn a_line_gives_the_area_the_check_the_result_and_the_detail() {
        let passed = line(Area::Time, "clock", Outcome::passed("synchronized 4 min ago"));
        assert_eq!(passed.text(), "time     clock                 passed   synchronized 4 min ago");
        let skipped = line(Area::Health, "hardware", Outcome::skipped("no hardware check yet"));
        assert_eq!(skipped.text(), "health   hardware              skipped  no hardware check yet");
    }

    #[test]
    fn a_warning_adds_its_id_and_what_to_do() {
        let warning = line(
            Area::Ssh,
            "passwords",
            Outcome::warning(
                Problem::SshPasswords,
                "passwords sign in over SSH (sshd_config: PasswordAuthentication yes)",
            ),
        );
        assert_eq!(
            warning.text(),
            "ssh      passwords             warning  passwords sign in over SSH (sshd_config: \
             PasswordAuthentication yes)\n         SEC-110: Add your SSH key, then set \
             PasswordAuthentication no in /system/settings/ssh/sshd_config."
        );
    }

    #[test]
    fn a_failure_another_tool_found_keeps_that_tools_id_and_advice() {
        let outcome = Outcome::repeating(
            Verdict::Failed,
            "/data is 98% full",
            Some("MET-101".into()),
            Some("Free some space.".into()),
        );
        let failed = line(Area::Health, "early warnings", outcome);
        assert_eq!(
            failed.text(),
            "health   early warnings        FAILED   /data is 98% full\n         MET-101: Free \
             some space."
        );
        // An alert that carries no ID has no second line.
        let plain = Outcome::repeating(Verdict::Warning, "a backup is late", None, None);
        assert_eq!(line(Area::Health, "x", plain).text().lines().count(), 1);
    }

    #[test]
    fn the_status_is_the_worst_of_the_checks() {
        let counts = |passed, warning, failed, skipped| Counts { passed, warning, failed, skipped };
        assert_eq!(counts(5, 0, 0, 2).status(), 0);
        assert_eq!(counts(5, 1, 0, 0).status(), 2);
        assert_eq!(counts(5, 1, 1, 0).status(), 1);
        // A run in which nothing could be checked did not run.
        assert_eq!(counts(0, 0, 0, 4).status(), 3);
        assert_eq!(counts(0, 0, 0, 0).status(), 3);
    }

    #[test]
    fn the_summary_counts_each_result_and_names_a_failure_only_when_there_is_one() {
        let counts = |passed, warning, failed, skipped| Counts { passed, warning, failed, skipped };
        assert_eq!(counts(9, 1, 0, 1).summary(), "9 passed, 1 warning, 1 skipped");
        assert_eq!(counts(9, 2, 0, 0).summary(), "9 passed, 2 warnings, 0 skipped");
        assert_eq!(counts(8, 0, 1, 1).summary(), "8 passed, 0 warnings, 1 FAILED, 1 skipped");
    }

    #[test]
    fn the_counts_follow_the_lines() {
        let lines = [
            line(Area::Time, "clock", Outcome::passed("")),
            line(Area::Ssh, "passwords", Outcome::warning(Problem::SshPasswords, "")),
            line(Area::Ssh, "blocking", Outcome::warning(Problem::BlockingOff, "")),
            line(Area::Health, "hardware", Outcome::skipped("")),
        ];
        assert_eq!(Counts::of(&lines), Counts { passed: 1, warning: 2, failed: 0, skipped: 1 });
        assert_eq!(ids(&lines), ["SEC-110", "SEC-112"]);
    }

    #[test]
    fn a_line_is_json_with_its_outcome_beside_its_area() {
        let warning = line(Area::Ssh, "passwords", Outcome::warning(Problem::SshPasswords, "d"));
        let value = serde_json::to_value(&warning).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "area": "ssh", "check": "passwords", "result": "warning", "detail": "d",
                "id": "SEC-110", "fix": Problem::SshPasswords.advice()
            })
        );
        let passed = line(Area::Time, "clock", Outcome::passed("ok"));
        let value = serde_json::to_value(&passed).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"area": "time", "check": "clock", "result": "passed", "detail": "ok"})
        );
        assert_eq!(serde_json::from_value::<Line>(value).unwrap(), passed);
    }
}
