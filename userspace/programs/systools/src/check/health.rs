// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The health checks of dukovo-check(8): the early warnings that are
//! active, and what the last `hardware check` found. Each repeats what
//! another tool found, under that tool's own ID, so that one fault has
//! one page.

use super::context::Context;
use super::line::{Outcome, Verdict};
use crate::alerts::protocol::Severity;
use crate::hardware::check::Outcome as Found;
use crate::hardware::saved::{self, Saved};

/// Whether `text` is an error ID, such as `MET-101`: 2 to 5 capitals, a
/// hyphen and 3 digits.
fn is_error_id(text: &str) -> bool {
    let Some((area, number)) = text.split_once('-') else { return false };
    (2..=5).contains(&area.len())
        && area.bytes().all(|byte| byte.is_ascii_uppercase())
        && number.len() == 3
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// What to do, and the error ID, in an alert's detail. metricsd ends its
/// advice with the ID in parentheses, `Free some space. (MET-101)`;
/// another source's detail has none, and is all advice.
fn advice_and_id(detail: &str) -> (Option<String>, Option<String>) {
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let named = detail
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once('('))
        .filter(|(_, id)| is_error_id(id));
    let (advice, id) = match named {
        Some((advice, id)) => (advice.trim().to_string(), Some(id.to_string())),
        None => (detail, None),
    };
    ((!advice.is_empty()).then_some(advice), id)
}

/// Lists each alert that is active, a warning for one that is a warning
/// and a failure for one that is critical, under the ID its detail ends
/// with.
pub(super) fn early_warnings(context: &Context) -> Vec<Outcome> {
    let alerts = match &context.alerts {
        Ok(alerts) => alerts,
        Err(why) => return vec![Outcome::skipped(why.clone())],
    };
    if alerts.is_empty() {
        return vec![Outcome::passed("none active")];
    }
    alerts
        .iter()
        .map(|active| {
            let verdict = match active.alert.severity {
                Severity::Warning => Verdict::Warning,
                Severity::Critical => Verdict::Failed,
            };
            let (advice, id) = advice_and_id(&active.alert.detail);
            Outcome::repeating(verdict, active.alert.title.clone(), id, advice)
        })
        .collect()
}

/// Lists each warning and failure of the last `hardware check`, as that
/// check said it, under its HW ID. It does not run one, which takes
/// minutes.
pub(super) fn hardware(context: &Context) -> Vec<Outcome> {
    let text = match context.read(saved::FILE) {
        Ok(Some(text)) => text,
        Ok(None) => {
            return vec![Outcome::skipped("hardware check has not run on this computer")];
        }
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let kept: Saved = match serde_json::from_str(&text) {
        Ok(kept) => kept,
        Err(why) => {
            return vec![Outcome::skipped(format!(
                "{} is not the result of a hardware check: {why}",
                saved::FILE
            ))];
        }
    };
    let found: Vec<Outcome> = kept
        .checks
        .iter()
        .filter_map(|check| {
            let verdict = match check.outcome {
                Found::Warning => Verdict::Warning,
                Found::Failed => Verdict::Failed,
                Found::Passed | Found::Skipped => return None,
            };
            let detail = format!("{} {}: {}", check.area.name(), check.name, check.detail);
            Some(Outcome::repeating(verdict, detail, check.id.clone(), check.advice.clone()))
        })
        .collect();
    if found.is_empty() {
        let when = context.said(u64::try_from(kept.time).unwrap_or(0));
        return vec![Outcome::passed(format!(
            "the last hardware check, {when}, found nothing wrong"
        ))];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::{NOW, System};
    use super::*;
    use crate::alerts::protocol::{Active, Alert};
    use crate::hardware::check::{Area, Check};
    use crate::util;

    fn active(key: &str, severity: Severity, title: &str, detail: &str) -> Active {
        let alert = Alert { key: key.into(), severity, title: title.into(), detail: detail.into() };
        Active { source: "metricsd".into(), alert, since: NOW - 600, changed: NOW - 600 }
    }

    #[test]
    fn an_alerts_detail_gives_its_advice_and_the_id_that_ends_it() {
        assert_eq!(
            advice_and_id("Free some space or add a disk. (MET-101)"),
            (Some("Free some space or add a disk.".into()), Some("MET-101".into()))
        );
        assert_eq!(
            advice_and_id("Run `diskhealth`.\nThen replace it (HW-111)"),
            (Some("Run `diskhealth`. Then replace it".into()), Some("HW-111".into()))
        );
        // A bracket that holds no ID is advice.
        assert_eq!(advice_and_id("Wait (a day)"), (Some("Wait (a day)".into()), None));
        assert_eq!(advice_and_id("(MET-102)"), (None, Some("MET-102".into())));
        assert_eq!(advice_and_id(""), (None, None));
    }

    #[test]
    fn no_alert_active_passes() {
        let system = System::new(Vec::new());
        let outcomes = early_warnings(&system.context);
        assert_eq!(
            (outcomes[0].verdict, outcomes[0].detail.as_str()),
            (Verdict::Passed, "none active")
        );
    }

    #[test]
    fn each_active_alert_is_a_line_under_its_own_id_and_a_critical_one_is_a_failure() {
        let mut system = System::new(Vec::new());
        system.context.alerts = Ok(vec![
            active(
                "fs-filling:/data",
                Severity::Warning,
                "/data fills in 9 days",
                "Free space. (MET-102)",
            ),
            active("fs-full:/", Severity::Critical, "/ is 98% full", "Free space now. (MET-101)"),
            active("backup-late", Severity::Warning, "The backup is late", ""),
        ]);
        let outcomes = early_warnings(&system.context);
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].verdict, Verdict::Warning);
        assert_eq!(outcomes[0].id.as_deref(), Some("MET-102"));
        assert_eq!(outcomes[0].detail, "/data fills in 9 days");
        assert_eq!(outcomes[0].fix.as_deref(), Some("Free space."));
        assert_eq!(outcomes[1].verdict, Verdict::Failed);
        assert_eq!(outcomes[1].id.as_deref(), Some("MET-101"));
        assert_eq!((outcomes[2].id.as_deref(), outcomes[2].fix.as_deref()), (None, None));
    }

    #[test]
    fn alertd_that_cannot_say_skips_the_check() {
        let mut system = System::new(Vec::new());
        system.context.alerts = Err("alertd is not running; try service alertd start".into());
        let outcomes = early_warnings(&system.context);
        assert_eq!(outcomes[0].verdict, Verdict::Skipped);
        assert!(outcomes[0].detail.starts_with("alertd is not running"));
    }

    fn kept(checks: Vec<Check>) -> Saved {
        Saved {
            time: i64::try_from(NOW).unwrap() - 7200,
            version: "0.16.0".into(),
            full: false,
            status: 0,
            checks,
        }
    }

    fn plant(system: &System, saved: &Saved) {
        system.plant(saved::FILE, &saved.json().unwrap());
    }

    #[test]
    fn a_computer_whose_hardware_was_never_checked_skips_the_check() {
        let system = System::new(Vec::new());
        let outcomes = hardware(&system.context);
        assert_eq!(
            (outcomes[0].verdict, outcomes[0].detail.as_str()),
            (Verdict::Skipped, "hardware check has not run on this computer")
        );
    }

    #[test]
    fn a_hardware_check_that_found_nothing_passes_and_says_when() {
        let system = System::new(Vec::new());
        let passed = Check::passed(Area::Cpu, "load", "4 CPUs agreed".into());
        let skipped = Check::skipped(Area::Battery, "BAT0", "no battery".into());
        plant(&system, &kept(vec![passed, skipped]));
        let outcomes = hardware(&system.context);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].verdict, Verdict::Passed);
        let hour = util::local_time(i64::try_from(NOW).unwrap() - 7200, "%R");
        assert_eq!(
            outcomes[0].detail,
            format!("the last hardware check, today at {hour}, found nothing wrong")
        );
    }

    #[test]
    fn what_a_hardware_check_warned_of_or_failed_is_repeated_under_its_hw_id() {
        use crate::hardware::check::Problem;
        let system = System::new(Vec::new());
        let failed = Check::failed(
            Area::Disks,
            "sda read scan",
            Problem::DiskUnreadable,
            "sector 20000".into(),
        );
        let warning = Check::warning(
            Area::Cpu,
            "machine checks",
            Problem::CpuCorrected,
            "1 corrected error".into(),
        );
        let passed = Check::passed(Area::Memory, "memory test", "256 MiB".into());
        plant(&system, &kept(vec![passed, failed, warning]));
        let outcomes = hardware(&system.context);
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].verdict, Verdict::Failed);
        assert_eq!(outcomes[0].id.as_deref(), Some("HW-113"));
        assert_eq!(outcomes[0].detail, "disks sda read scan: sector 20000");
        assert_eq!(outcomes[0].fix.as_deref(), Some(Problem::DiskUnreadable.advice()));
        assert_eq!(
            (outcomes[1].verdict, outcomes[1].id.as_deref()),
            (Verdict::Warning, Some("HW-121"))
        );
    }

    #[test]
    fn a_file_that_is_no_hardware_result_skips_the_check() {
        let system = System::new(Vec::new());
        system.plant(saved::FILE, "{\"time\": 1}");
        let outcomes = hardware(&system.context);
        assert_eq!(outcomes[0].verdict, Verdict::Skipped);
        assert!(outcomes[0].detail.contains("is not the result of a hardware check"));
    }
}
