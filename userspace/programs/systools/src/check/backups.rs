// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The backups check of dukovo-check(8): whether snapshots are made of the
//! people's files and the shared ones, which a snapshot that the same
//! mistake would not reach brings back (snap(1)).

use super::context::Context;
use super::line::{Outcome, Problem};
use super::services::is_on;
use crate::snap::Settings;

/// Where the places of snapshots are named.
const SETTINGS: &str = "/system/settings/snap.toml";

/// The places a snapshot schedule must cover: the folders that hold the
/// people's files, `@users` and `@shared`.
const COVERED: [&str; 2] = ["/users", "/shared"];

/// Warns when the `snap` service is off, or its places leave out `/users`
/// or `/shared`.
pub(super) fn snapshots(context: &Context) -> Vec<Outcome> {
    let service = match context.service("snap") {
        Ok(service) => service,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let settings = match context.read(SETTINGS) {
        Ok(None) => Ok(Settings::default()),
        Ok(Some(text)) => Settings::parse(&text).map_err(|why| format!("{SETTINGS}: {why}")),
        Err(why) => Err(why),
    };
    let settings = match settings {
        Ok(settings) => settings,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    if !service.is_some_and(is_on) {
        return vec![Outcome::warning(
            Problem::NoSnapshots,
            "the snap service is off, so no snapshot is made",
        )];
    }
    let left_out: Vec<&str> = COVERED
        .into_iter()
        .filter(|place| !settings.places.iter().any(|named| named.trim_end_matches('/') == *place))
        .collect();
    if !left_out.is_empty() {
        return vec![Outcome::warning(
            Problem::NoSnapshots,
            format!(
                "snap.toml leaves out {}, so no snapshot is made of it",
                left_out.join(" and ")
            ),
        )];
    }
    vec![Outcome::passed("a snapshot of /users and /shared is made each day")]
}

#[cfg(test)]
mod tests {
    use spark_api::show::State;

    use super::super::context::planted::{System, service};
    use super::super::line::Verdict;
    use super::*;

    fn one(outcomes: Vec<Outcome>) -> Outcome {
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        outcomes.into_iter().next().unwrap()
    }

    #[test]
    fn the_default_schedule_of_a_running_snap_service_passes() {
        let system = System::new(vec![service("snap", State::Running)]);
        let outcome = one(snapshots(&system.context));
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(outcome.detail, "a snapshot of /users and /shared is made each day");
    }

    #[test]
    fn a_snap_service_that_is_off_or_missing_warns() {
        for services in [
            vec![service("snap", State::Disabled)],
            vec![service("snap", State::Failed)],
            vec![service("logd", State::Running)],
        ] {
            let system = System::new(services);
            let outcome = one(snapshots(&system.context));
            assert_eq!(
                (outcome.verdict, outcome.id.as_deref()),
                (Verdict::Warning, Some("SEC-160"))
            );
            assert_eq!(outcome.detail, "the snap service is off, so no snapshot is made");
        }
    }

    #[test]
    fn places_that_leave_out_users_or_shared_warn_by_name() {
        let system = System::new(vec![service("snap", State::Running)]);
        system.plant(SETTINGS, "places = [\"/users\", \"/system/settings\"]\n");
        let outcome = one(snapshots(&system.context));
        assert_eq!(outcome.verdict, Verdict::Warning);
        assert_eq!(outcome.detail, "snap.toml leaves out /shared, so no snapshot is made of it");
        system.plant(SETTINGS, "places = []\n");
        assert!(one(snapshots(&system.context)).detail.contains("/users and /shared"));
    }

    #[test]
    fn a_place_written_with_a_slash_at_its_end_is_the_same_place() {
        let system = System::new(vec![service("snap", State::Running)]);
        system.plant(SETTINGS, "places = [\"/users/\", \"/shared\", \"/volumes/sdb1/@data\"]\n");
        assert_eq!(one(snapshots(&system.context)).verdict, Verdict::Passed);
    }

    #[test]
    fn a_schedule_file_that_does_not_read_or_spark_that_cannot_say_skips_the_check() {
        let mut system = System::new(vec![service("snap", State::Running)]);
        system.plant(SETTINGS, "places = 3\n");
        let outcome = one(snapshots(&system.context));
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert!(outcome.detail.starts_with("/system/settings/snap.toml: "), "{}", outcome.detail);
        system.context.services = Err("not running".into());
        assert_eq!(one(snapshots(&system.context)).verdict, Verdict::Skipped);
    }
}
