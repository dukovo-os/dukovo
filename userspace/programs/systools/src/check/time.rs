// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The time check of dukovo-check(8): whether timed set the clock from a
//! time server in the last day, which TLS certificates and the log's times
//! depend on.

use spark_api::words::span;

use super::context::{Context, SETTLING};
use super::line::{Outcome, Problem};
use crate::timed;

/// How long the clock may go unset: timed asks again within 17 minutes.
const LONGEST: u64 = 86_400;

/// Warns when timed last synced more than a day ago, or on a computer that
/// has been up long enough for timed to have tried, never.
pub(super) fn clock(context: &Context) -> Vec<Outcome> {
    let last = match context.read(timed::LAST) {
        Ok(text) => text.and_then(|text| text.trim().parse::<u64>().ok()),
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let age = last.map(|last| context.now.saturating_sub(last));
    let outcome = match age {
        Some(age) if age <= LONGEST => {
            Outcome::passed(format!("synchronized {} ago", span(age * 1000)))
        }
        _ if context.uptime < SETTLING => Outcome::skipped(format!(
            "this computer started {} ago, and timed has not synchronized yet",
            span(context.uptime * 1000)
        )),
        Some(age) => Outcome::warning(
            Problem::ClockNotSet,
            format!("timed last synchronized the clock {} ago", span(age * 1000)),
        ),
        None => Outcome::warning(Problem::ClockNotSet, "timed has never synchronized the clock"),
    };
    vec![outcome]
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::{NOW, System};
    use super::super::line::Verdict;
    use super::*;

    fn one(outcomes: Vec<Outcome>) -> Outcome {
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        outcomes.into_iter().next().unwrap()
    }

    /// A system whose clock timed last set `ago` seconds before.
    fn synced(ago: u64) -> System {
        let system = System::new(Vec::new());
        system.plant(timed::LAST, &format!("{}\n", NOW - ago));
        system
    }

    #[test]
    fn a_sync_in_the_last_day_passes_and_says_how_long_ago() {
        let outcome = one(clock(&synced(240).context));
        assert_eq!(
            (outcome.verdict, outcome.detail.as_str()),
            (Verdict::Passed, "synchronized 4 min ago")
        );
        assert_eq!(one(clock(&synced(86_400).context)).verdict, Verdict::Passed);
    }

    #[test]
    fn a_sync_over_a_day_ago_warns_with_how_long() {
        let outcome = one(clock(&synced(86_401 + 7200).context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-140")));
        assert_eq!(outcome.detail, "timed last synchronized the clock 1 day ago");
        let older = one(clock(&synced(5 * 86_400).context));
        assert_eq!(older.detail, "timed last synchronized the clock 5 days ago");
    }

    #[test]
    fn a_computer_that_never_synced_warns_once_timed_has_had_time_to() {
        let mut system = System::new(Vec::new());
        let outcome = one(clock(&system.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-140")));
        assert_eq!(outcome.detail, "timed has never synchronized the clock");
        system.context.uptime = 600;
        let outcome = one(clock(&system.context));
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert_eq!(
            outcome.detail,
            "this computer started 10 min ago, and timed has not synchronized yet"
        );
    }

    #[test]
    fn an_old_sync_just_after_the_start_is_not_judged_before_timed_asks_again() {
        let mut system = synced(3 * 86_400);
        system.context.uptime = 120;
        assert_eq!(one(clock(&system.context)).verdict, Verdict::Skipped);
    }

    #[test]
    fn a_file_that_is_no_time_counts_as_no_sync() {
        let system = System::new(Vec::new());
        system.plant(timed::LAST, "soon\n");
        assert_eq!(one(clock(&system.context)).verdict, Verdict::Warning);
    }
}
