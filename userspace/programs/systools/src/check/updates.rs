// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The updates checks of dukovo-check(8): whether the last look at the
//! update server found a release that is not installed, and whether the
//! server is still looked at. Both read what the look kept
//! (`sysupdate::checked`).

use spark_api::words::span;

use super::context::{Context, SETTLING};
use super::line::{Outcome, Problem};
use crate::sysupdate::checked::{self, Checked, Found};

/// How long a computer may go without a look at the update server: updated
/// looks once a day.
const OLDEST: u64 = 7 * 86_400;

/// The last look at the update server, or `None` when there has been
/// none.
///
/// # Errors
///
/// A line for a skipped check: the file cannot be read or is not a
/// record.
fn last_look(context: &Context) -> Result<Option<Checked>, String> {
    let Some(text) = context.read(checked::FILE)? else { return Ok(None) };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|why| format!("{} is not a record of a check: {why}", checked::FILE))
}

/// Warns when the last look found a release this system takes and has not
/// taken.
pub(super) fn waiting(context: &Context) -> Vec<Outcome> {
    let look = match last_look(context) {
        Ok(Some(look)) => look,
        Ok(None) => return vec![Outcome::skipped("no update check has run on this computer yet")],
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let when = context.said(look.time);
    // A look from before the system updated says nothing of the release it
    // runs now: the next one does.
    if look.running != context.release {
        return vec![Outcome::skipped(format!(
            "the last check, {when}, was made on Dukovo {}, before this system updated",
            look.running
        ))];
    }
    let offered = &look.offered;
    let outcome = match look.found {
        Found::Update => Outcome::warning(
            Problem::UpdateWaiting,
            format!("Dukovo {offered} is waiting: the last check, {when}, found it"),
        ),
        Found::Current => {
            Outcome::passed(format!("none: the last check, {when}, found Dukovo {offered} current"))
        }
        Found::Waiting => Outcome::passed(format!(
            "none yet: Dukovo {offered} reaches this machine in a later wave"
        )),
        Found::Held => Outcome::passed(format!("none: Dukovo {offered} is held back, as asked")),
        Found::Withdrawn => Outcome::passed(format!(
            "none: Dukovo {offered} was withdrawn, so there is nothing to take"
        )),
    };
    vec![outcome]
}

/// Warns when the update server has not been looked at for a week, or on a
/// computer that has been up long enough to have looked, never.
pub(super) fn last_check(context: &Context) -> Vec<Outcome> {
    let look = match last_look(context) {
        Ok(look) => look,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let Some(look) = look else {
        let outcome = if context.uptime < SETTLING {
            Outcome::skipped(format!(
                "this computer started {} ago, and updated looks 10 minutes after the start",
                span(context.uptime * 1000)
            ))
        } else {
            Outcome::warning(Problem::UpdateCheckOld, "the update server has never been checked")
        };
        return vec![outcome];
    };
    let age = context.now.saturating_sub(look.time);
    if age > OLDEST {
        return vec![Outcome::warning(
            Problem::UpdateCheckOld,
            format!("the last check was {} ago, {}", span(age * 1000), context.said(look.time)),
        )];
    }
    vec![Outcome::passed(context.said(look.time))]
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::{NOW, System};
    use super::super::line::Verdict;
    use super::*;
    use crate::util;

    /// A system whose last look, `ago` seconds before it, found `found`.
    fn looked(ago: u64, running: &str, found: Found) -> System {
        let system = System::new(Vec::new());
        let look =
            Checked { time: NOW - ago, running: running.into(), offered: "0.17.0".into(), found };
        system.plant(checked::FILE, &serde_json::to_string(&look).unwrap());
        system
    }

    fn one(outcomes: Vec<Outcome>) -> Outcome {
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        outcomes.into_iter().next().unwrap()
    }

    fn at(time: u64) -> String {
        util::local_time(i64::try_from(time).unwrap(), "%R")
    }

    #[test]
    fn a_newer_release_that_was_found_and_not_taken_warns() {
        let system = looked(3600, "0.16.0", Found::Update);
        let outcome = one(waiting(&system.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-130")));
        assert_eq!(
            outcome.detail,
            format!(
                "Dukovo 0.17.0 is waiting: the last check, today at {}, found it",
                at(NOW - 3600)
            )
        );
    }

    #[test]
    fn a_look_that_found_nothing_to_take_passes_and_says_what_it_found() {
        let current = looked(60, "0.16.0", Found::Current);
        let outcome = one(waiting(&current.context));
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(
            outcome.detail,
            format!("none: the last check, today at {}, found Dukovo 0.17.0 current", at(NOW - 60))
        );
        for found in [Found::Waiting, Found::Held, Found::Withdrawn] {
            let system = looked(60, "0.16.0", found);
            assert_eq!(one(waiting(&system.context)).verdict, Verdict::Passed, "{found:?}");
        }
    }

    #[test]
    fn a_look_from_before_the_system_updated_is_not_judged() {
        let system = looked(60, "0.15.0", Found::Update);
        let outcome = one(waiting(&system.context));
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert!(outcome.detail.ends_with("was made on Dukovo 0.15.0, before this system updated"));
    }

    #[test]
    fn no_look_at_all_skips_the_waiting_check_and_a_record_that_does_not_read_says_so() {
        let system = System::new(Vec::new());
        assert_eq!(
            one(waiting(&system.context)).detail,
            "no update check has run on this computer yet"
        );
        system.plant(checked::FILE, "{\"time\": 1}\n");
        let outcome = one(waiting(&system.context));
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert!(
            outcome
                .detail
                .starts_with("/system/state/sysupdate/checked is not a record of a check: ")
        );
        assert_eq!(one(last_check(&system.context)).verdict, Verdict::Skipped);
    }

    #[test]
    fn a_look_in_the_last_week_passes_and_an_older_one_warns_with_its_age() {
        let recent = looked(6 * 86_400, "0.16.0", Found::Current);
        assert_eq!(one(last_check(&recent.context)).verdict, Verdict::Passed);
        let today = looked(300, "0.16.0", Found::Current);
        assert_eq!(one(last_check(&today.context)).detail, format!("today at {}", at(NOW - 300)));
        let old = looked(8 * 86_400, "0.16.0", Found::Current);
        let outcome = one(last_check(&old.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-131")));
        assert!(
            outcome.detail.starts_with("the last check was 8 days ago, "),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn a_computer_that_never_looked_warns_once_it_has_had_time_to() {
        let mut system = System::new(Vec::new());
        let outcome = one(last_check(&system.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-131")));
        assert_eq!(outcome.detail, "the update server has never been checked");
        system.context.uptime = 300;
        let outcome = one(last_check(&system.context));
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert_eq!(
            outcome.detail,
            "this computer started 5 min ago, and updated looks 10 minutes after the start"
        );
    }
}
