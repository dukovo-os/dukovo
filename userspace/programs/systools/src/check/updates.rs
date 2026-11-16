// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The updates checks of dukovo-check(8): whether the last look at the
//! update server found a release that is not installed, and whether the
//! server is still looked at. Both read what the look kept
//! (`sysupdate::checked`); the first also reads whether the update was put
//! off (`update later`, ADR 0565).

use spark_api::words::span;

use super::context::{Context, SETTLING};
use super::line::{Outcome, Problem};
use crate::sysupdate::checked::{self, Checked, Found};
use crate::sysupdate::later::{self, Fixes, Later};
use crate::update::last;
use crate::util;

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

/// What the reminder says when `update later` put off the update `offered`
/// of the system and still does: with the last check root kept for its
/// advisories, which is all this check, with no network, can go by. The
/// rule is the one every reader asks ([`later::stands`]), so a record that
/// is missing, or whose advisories were not read, leaves the reminder to
/// stand for nothing.
fn put_off(context: &Context, offered: &str) -> Option<String> {
    let later = Later::parse(&context.read(later::FILE).ok()??);
    let entry = later.entry("system")?;
    let record = context.read(last::FILE).ok().flatten();
    let fixes = Fixes::in_record(record.as_deref(), "system", offered);
    if !later::stands(entry, offered, fixes, context.now.saturating_mul(1000)) {
        return None;
    }
    let until = util::local_time(i64::try_from(entry.until_ms / 1000).unwrap_or(0), "%Y-%m-%d");
    Some(format!("none: Dukovo {offered} is put off until {until}, as asked (`update later`)"))
}

/// Warns when the last look found a release this system takes and has not
/// taken, unless it is put off or held back on purpose.
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
        Found::Update => put_off(context, offered).map_or_else(
            || {
                Outcome::warning(
                    Problem::UpdateWaiting,
                    format!("Dukovo {offered} is waiting: the last check, {when}, found it"),
                )
            },
            Outcome::passed,
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

    /// What `update check --json` printed and root kept, `by` the daily
    /// check, with a `security` part that fixes `fixed`, and, when
    /// `unread`, the note that the advisories could not be read.
    fn record(fixed: &[&str], unread: bool) -> String {
        let system: Vec<serde_json::Value> = fixed
            .iter()
            .map(|release| serde_json::json!({"id": "DKSA-2026-0007", "release": release}))
            .collect();
        let unchecked = if unread {
            serde_json::json!([{"item": "security", "why": "the server did not answer"}])
        } else {
            serde_json::json!([])
        };
        serde_json::json!({
            "format": 1,
            "checked_ms": (NOW - 600) * 1000,
            "by": "updated",
            "updates": {
                "format": 1,
                "offers": [{"item": "system", "from": "0.16.0", "to": "0.17.0", "state": "update"}],
                "unchecked": unchecked,
                "security": {"system": system, "app_flaws": 0},
            },
        })
        .to_string()
    }

    /// The reminder of the system's update to `version`, put off `until` seconds from now.
    fn put_off_to(version: &str, until: i64) -> String {
        let until_ms = (i64::try_from(NOW).unwrap() + until) * 1000;
        serde_json::json!({
            "format": 1,
            "later": [{
                "item": "system", "version": version, "until_ms": until_ms, "by": 1000,
                "at_ms": (NOW - 60) * 1000,
            }],
        })
        .to_string()
    }

    #[test]
    fn an_update_put_off_passes_while_the_reminder_stands_and_warns_once_it_does_not() {
        let system = looked(3600, "0.16.0", Found::Update);
        system.plant(last::FILE, &record(&[], false));
        let until = util::local_time(i64::try_from(NOW).unwrap() + 86_400, "%Y-%m-%d");
        // Put off for a day, and nothing fixed: passes, and says until when.
        system.plant(later::FILE, &put_off_to("0.17.0", 86_400));
        let outcome = one(waiting(&system.context));
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(
            outcome.detail,
            format!("none: Dukovo 0.17.0 is put off until {until}, as asked (`update later`)")
        );
        // Its time has come.
        system.plant(later::FILE, &put_off_to("0.17.0", 0));
        let outcome = one(waiting(&system.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-130")));
        // A newer release than the one put off.
        system.plant(later::FILE, &put_off_to("0.16.9", 86_400));
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
        // No reminder, or a file that is not one.
        std::fs::remove_file(system.root().join(later::FILE.trim_start_matches('/'))).unwrap();
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
        system.plant(later::FILE, "not a record");
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
    }

    #[test]
    fn an_update_that_fixes_an_advisory_or_that_could_not_be_checked_is_not_put_off() {
        let system = looked(3600, "0.16.0", Found::Update);
        system.plant(later::FILE, &put_off_to("0.17.0", 86_400));
        // No check of root's kept: nothing says it fixes none.
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
        // An advisory the update fixes, which came after the reminder.
        system.plant(last::FILE, &record(&["0.17.0"], false));
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
        // One that another release fixes does not end it.
        system.plant(last::FILE, &record(&["0.18.0"], false));
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Passed);
        // Advisories that could not be read.
        system.plant(last::FILE, &record(&[], true));
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
        // A record of another format.
        system.plant(last::FILE, &record(&[], false).replacen(r#""format":1"#, r#""format":2"#, 1));
        assert_eq!(one(waiting(&system.context)).verdict, Verdict::Warning);
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
