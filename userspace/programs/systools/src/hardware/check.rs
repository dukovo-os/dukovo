// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a check of `hardware check` found: its area, its result and its
//! detail, the ID that names a warning or a failure, and the line and the
//! exit status they make.

use serde::{Deserialize, Serialize};

/// The part of the machine a check looks at, in the order the lines of a
/// run come in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Area {
    Memory,
    Disks,
    Cpu,
    Sensors,
    Battery,
    Network,
}

impl Area {
    /// Every area.
    pub(crate) const ALL: [Area; 6] =
        [Area::Memory, Area::Disks, Area::Cpu, Area::Sensors, Area::Battery, Area::Network];

    /// The area `name` names on the command line.
    pub(crate) fn parse(name: &str) -> Option<Area> {
        Area::ALL.into_iter().find(|area| area.name() == name)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Area::Memory => "memory",
            Area::Disks => "disks",
            Area::Cpu => "cpu",
            Area::Sensors => "sensors",
            Area::Battery => "battery",
            Area::Network => "network",
        }
    }
}

/// How a check ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome {
    Passed,
    Warning,
    Failed,
    /// It did not run, and the detail says why.
    Skipped,
}

impl Outcome {
    /// The word of a line: `FAILED` stands out as diskhealth's `FAILING`
    /// does.
    fn word(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Warning => "warning",
            Outcome::Failed => "FAILED",
            Outcome::Skipped => "skipped",
        }
    }
}

/// What a warning or a failure is. Each has the ID of its page in the
/// support center (docs/errors) and the one sentence the line gives for
/// what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    MemoryCorrected,
    MemoryUncorrected,
    MemoryTest,
    DiskFailing,
    DiskSelfTest,
    DiskUnreadable,
    DiskSlow,
    DiskEarlyWarning,
    CpuCorrected,
    CpuWrongResult,
    CpuHot,
    SensorHot,
    FanStopped,
    BatteryWorn,
    LinkHalfDuplex,
    LinkErrors,
}

impl Problem {
    /// Every problem, for the tests that check each one's ID and advice.
    #[cfg(test)]
    pub(crate) const ALL: [Problem; 16] = [
        Problem::MemoryCorrected,
        Problem::MemoryUncorrected,
        Problem::MemoryTest,
        Problem::DiskFailing,
        Problem::DiskSelfTest,
        Problem::DiskUnreadable,
        Problem::DiskSlow,
        Problem::DiskEarlyWarning,
        Problem::CpuCorrected,
        Problem::CpuWrongResult,
        Problem::CpuHot,
        Problem::SensorHot,
        Problem::FanStopped,
        Problem::BatteryWorn,
        Problem::LinkHalfDuplex,
        Problem::LinkErrors,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Problem::MemoryCorrected => "HW-101",
            Problem::MemoryUncorrected => "HW-102",
            Problem::MemoryTest => "HW-103",
            Problem::DiskFailing => "HW-111",
            Problem::DiskSelfTest => "HW-112",
            Problem::DiskUnreadable => "HW-113",
            Problem::DiskSlow => "HW-114",
            Problem::DiskEarlyWarning => "HW-115",
            Problem::CpuCorrected => "HW-121",
            Problem::CpuWrongResult => "HW-122",
            Problem::CpuHot => "HW-123",
            Problem::SensorHot => "HW-131",
            Problem::FanStopped => "HW-132",
            Problem::BatteryWorn => "HW-141",
            Problem::LinkHalfDuplex => "HW-152",
            Problem::LinkErrors => "HW-153",
        }
    }

    /// One sentence on what to do.
    pub(crate) fn advice(self) -> &'static str {
        match self {
            Problem::MemoryCorrected => {
                "Run Test the memory from the start menu, and replace the memory module if the \
                 count keeps rising."
            }
            Problem::MemoryUncorrected => {
                "Back up what matters and replace the memory module: it gave wrong data."
            }
            Problem::MemoryTest => {
                "Do not trust this machine's data: run Test the memory from the start menu to \
                 confirm, then replace the memory module."
            }
            Problem::DiskFailing | Problem::DiskUnreadable => {
                "Back up what is on the disk now, and replace it."
            }
            Problem::DiskSelfTest => {
                "Back up what is on the disk now, and replace it; the self-test names the first \
                 sector it could not read."
            }
            Problem::DiskSlow => {
                "Check the disk's cable and port; a disk that reads slowly may be failing."
            }
            Problem::DiskEarlyWarning => {
                "Make sure what is on the disk is backed up, and plan to replace it."
            }
            Problem::CpuCorrected => {
                "Check the processor's cooling and settings; if the count keeps rising, plan to \
                 replace the processor."
            }
            Problem::CpuWrongResult => {
                "Do not rely on this processor: undo any overclock, check the power supply and \
                 the cooling, and replace the processor if it still fails."
            }
            Problem::CpuHot | Problem::SensorHot => {
                "Check that the fans turn, the vents are clear and the cooler is seated."
            }
            Problem::FanStopped => {
                "Check that the fan is plugged in and turns freely, and replace it if it does not."
            }
            Problem::BatteryWorn => "Replace the battery to get back a full charge's use.",
            Problem::LinkHalfDuplex => {
                "Check the cable and the switch port: both ends must agree on full duplex."
            }
            Problem::LinkErrors => "Replace the cable, and try another port of the switch.",
        }
    }
}

/// One line of a run: what was checked and how it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Check {
    pub(crate) area: Area,
    #[serde(rename = "check")]
    pub(crate) name: String,
    #[serde(rename = "result")]
    pub(crate) outcome: Outcome,
    pub(crate) detail: String,
    /// The ID of a warning's or a failure's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    /// What to do about it, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) advice: Option<String>,
}

impl Check {
    fn make(
        area: Area,
        name: &str,
        outcome: Outcome,
        detail: String,
        problem: Option<Problem>,
    ) -> Check {
        Check {
            area,
            name: name.to_string(),
            outcome,
            detail,
            id: problem.map(|problem| problem.id().to_string()),
            advice: problem.map(|problem| problem.advice().to_string()),
        }
    }

    pub(crate) fn passed(area: Area, name: &str, detail: String) -> Check {
        Check::make(area, name, Outcome::Passed, detail, None)
    }

    /// A check that did not run, for `why`.
    pub(crate) fn skipped(area: Area, name: &str, why: String) -> Check {
        Check::make(area, name, Outcome::Skipped, why, None)
    }

    pub(crate) fn warning(area: Area, name: &str, problem: Problem, detail: String) -> Check {
        Check::make(area, name, Outcome::Warning, detail, Some(problem))
    }

    pub(crate) fn failed(area: Area, name: &str, problem: Problem, detail: String) -> Check {
        Check::make(area, name, Outcome::Failed, detail, Some(problem))
    }

    /// The line of the check, and under a warning's or a failure's, a
    /// line with its ID and what to do.
    pub(crate) fn line(&self) -> String {
        let mut line = format!(
            "{:<8} {:<22} {:<8} {}",
            self.area.name(),
            self.name,
            self.outcome.word(),
            self.detail
        );
        if let (Some(id), Some(advice)) = (&self.id, &self.advice) {
            line.push_str(&format!("\n{:<8} {id}: {advice}", ""));
        }
        line
    }
}

/// `count` of `noun`, with an s unless it is one: `1 corrected error`, `3
/// corrected errors`.
pub(crate) fn counted(count: u64, noun: &str) -> String {
    let s = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{s}")
}

/// The exit status of a run: 1 if a check failed, else 2 if one warned,
/// else 3 if the run was stopped before it ended or every check was
/// skipped (it could not run), else 0.
pub(crate) fn status(checks: &[Check], stopped: bool) -> i32 {
    let any = |outcome: Outcome| checks.iter().any(|check| check.outcome == outcome);
    if any(Outcome::Failed) {
        1
    } else if any(Outcome::Warning) {
        2
    } else if stopped || !any(Outcome::Passed) {
        3
    } else {
        0
    }
}

/// How many checks ended each way: `11 passed, 1 warning, 0 FAILED, 2
/// skipped`.
pub(crate) fn summary(checks: &[Check]) -> String {
    let count = |outcome: Outcome| checks.iter().filter(|check| check.outcome == outcome).count();
    let warnings = count(Outcome::Warning);
    format!(
        "{} passed, {warnings} {}, {} FAILED, {} skipped",
        count(Outcome::Passed),
        if warnings == 1 { "warning" } else { "warnings" },
        count(Outcome::Failed),
        count(Outcome::Skipped)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_problem_has_its_own_id_and_a_sentence_of_advice() {
        let mut ids: Vec<&str> = Problem::ALL.iter().map(|problem| problem.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Problem::ALL.len());
        for problem in Problem::ALL {
            assert!(problem.id().starts_with("HW-1"), "{problem:?}");
            assert!(problem.advice().ends_with('.'), "{problem:?}");
            assert_eq!(problem.advice().matches(". ").count(), 0, "{problem:?}: one sentence");
        }
    }

    #[test]
    fn a_line_gives_the_area_the_check_the_result_and_the_detail() {
        let check = Check::passed(Area::Memory, "test", "256 MiB tested".into());
        assert_eq!(check.line(), "memory   test                   passed   256 MiB tested");
        let check = Check::skipped(Area::Battery, "BAT0", "no battery".into());
        assert_eq!(check.line(), "battery  BAT0                   skipped  no battery");
    }

    #[test]
    fn a_warning_or_a_failure_adds_its_id_and_what_to_do() {
        let check =
            Check::failed(Area::Disks, "sda read scan", Problem::DiskUnreadable, "sector 7".into());
        assert_eq!(
            check.line(),
            format!(
                "disks    sda read scan          FAILED   sector 7\n         HW-113: {}",
                Problem::DiskUnreadable.advice()
            )
        );
        let check = Check::warning(Area::Cpu, "heat", Problem::CpuHot, "99 °C".into());
        assert!(check.line().contains("warning"));
        assert!(check.line().contains("\n         HW-123: "));
    }

    #[test]
    fn the_status_is_the_worst_of_the_checks() {
        let passed = Check::passed(Area::Cpu, "load", String::new());
        let skipped = Check::skipped(Area::Battery, "BAT0", String::new());
        let warning = Check::warning(Area::Cpu, "heat", Problem::CpuHot, String::new());
        let failed = Check::failed(Area::Cpu, "load", Problem::CpuWrongResult, String::new());
        assert_eq!(status(&[passed.clone(), skipped.clone()], false), 0);
        assert_eq!(status(&[passed.clone(), warning.clone()], false), 2);
        assert_eq!(status(&[warning.clone(), failed.clone()], false), 1);
        // A run that was stopped before its end tells what it found, and
        // says it did not finish when it found nothing.
        assert_eq!(status(std::slice::from_ref(&passed), true), 3);
        assert_eq!(status(&[passed, warning], true), 2);
        assert_eq!(status(&[failed], true), 1);
        // A run in which nothing could be checked did not run.
        assert_eq!(status(&[skipped], false), 3);
        assert_eq!(status(&[], false), 3);
    }

    #[test]
    fn a_count_is_said_in_the_singular_for_one() {
        assert_eq!(counted(1, "corrected error"), "1 corrected error");
        assert_eq!(counted(0, "corrected error"), "0 corrected errors");
        assert_eq!(counted(12, "read"), "12 reads");
    }

    #[test]
    fn the_summary_counts_each_result() {
        let checks = [
            Check::passed(Area::Cpu, "a", String::new()),
            Check::passed(Area::Cpu, "b", String::new()),
            Check::warning(Area::Cpu, "c", Problem::CpuHot, String::new()),
            Check::skipped(Area::Cpu, "d", String::new()),
        ];
        assert_eq!(summary(&checks), "2 passed, 1 warning, 0 FAILED, 1 skipped");
        assert_eq!(summary(&[]), "0 passed, 0 warnings, 0 FAILED, 0 skipped");
    }

    #[test]
    fn areas_are_named_as_the_command_line_names_them() {
        for area in Area::ALL {
            assert_eq!(Area::parse(area.name()), Some(area));
        }
        assert_eq!(Area::parse("disk"), None);
    }
}
