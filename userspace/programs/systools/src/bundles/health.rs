// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bundled makes of Spark's report on a service of a bundle that was
//! just updated (ADR 0386, 0522): the check after an update, for a service
//! whose manifest has a health command and no health socket. Spark runs
//! that command as it runs a service file's `[health]` (ADR 0470) and
//! `show` says whether the last check passed.

use spark_api::show;

/// What a service's report says of the new version of its bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It runs, and its last health check passed.
    Works,
    /// Not yet, and it may be: why, as a clause that follows "it".
    Waits(String),
    /// Never, as it ended for good: why, in the same words.
    Fails(String),
}

/// The verdict on a service from `report`, Spark's entry for it, which is
/// none when Spark does not report the service. A service that is
/// starting, waiting to start again or failing its checks may still come
/// right, as Spark restarts it after `health.failures` failed checks; one
/// that ended for good will not. One that listens for the connection that
/// starts it (`start: "on-connection"`, ADR 0477) works: nothing of it
/// runs to be checked until a connection comes, which an update cannot
/// wait for. A service Spark reports with no health
/// check, which an administrator's own service file took away, is judged
/// by whether it runs: there is no check to wait for.
pub fn verdict(report: Option<&show::Service>) -> Verdict {
    let Some(report) = report else {
        return Verdict::Waits("is not one Spark reports".into());
    };
    let ended = |what: &str| match &report.last_end {
        Some(end) => Verdict::Fails(format!("{what} ({end})")),
        None => Verdict::Fails(what.into()),
    };
    match report.state {
        show::State::Running => match &report.health {
            None => Verdict::Works,
            Some(health) => match health.state {
                show::HealthState::Passing => Verdict::Works,
                show::HealthState::Unknown => Verdict::Waits("has not been checked yet".into()),
                show::HealthState::Failing => Verdict::Waits(match &health.last_failure {
                    Some(line) => format!("fails its check: {line}"),
                    None => "fails its check".into(),
                }),
            },
        },
        show::State::Listening => Verdict::Works,
        show::State::Starting => Verdict::Waits("is still starting".into()),
        show::State::Pending if report.waiting_for.is_empty() => {
            Verdict::Waits("has not started".into())
        }
        show::State::Pending => {
            Verdict::Waits(format!("waits for {}", report.waiting_for.join(", ")))
        }
        show::State::Waiting => Verdict::Waits("waits to start again".into()),
        show::State::Stopping => Verdict::Waits("is stopping".into()),
        show::State::Stopped => ended("has stopped"),
        show::State::Done => ended("has ended"),
        show::State::Disabled => ended("is not enabled"),
        show::State::Failed => ended("failed to start, and Spark gave up on it"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: show::State) -> show::Service {
        show::Service { name: "db".into(), state, ..show::Service::default() }
    }

    fn checked(state: show::State, health: show::HealthState) -> show::Service {
        show::Service {
            health: Some(show::Health { state: health, ..show::Health::default() }),
            ..report(state)
        }
    }

    #[test]
    fn a_running_service_that_passed_its_check_works() {
        let service = checked(show::State::Running, show::HealthState::Passing);
        assert_eq!(verdict(Some(&service)), Verdict::Works);
    }

    #[test]
    fn a_running_service_not_yet_checked_waits() {
        let service = checked(show::State::Running, show::HealthState::Unknown);
        assert_eq!(verdict(Some(&service)), Verdict::Waits("has not been checked yet".into()));
    }

    #[test]
    fn a_running_service_failing_its_check_waits_and_says_why() {
        let mut service = checked(show::State::Running, show::HealthState::Failing);
        assert_eq!(verdict(Some(&service)), Verdict::Waits("fails its check".into()));
        service.health.as_mut().unwrap().last_failure = Some("pg_isready: no response".into());
        assert_eq!(
            verdict(Some(&service)),
            Verdict::Waits("fails its check: pg_isready: no response".into())
        );
    }

    #[test]
    fn a_running_service_with_no_check_in_the_report_works() {
        assert_eq!(verdict(Some(&report(show::State::Running))), Verdict::Works);
    }

    // Started only by a connection, it has nothing running to check.
    #[test]
    fn a_service_listening_for_its_first_connection_works() {
        let service = checked(show::State::Listening, show::HealthState::Unknown);
        assert_eq!(verdict(Some(&service)), Verdict::Works);
    }

    #[test]
    fn a_service_on_its_way_waits() {
        for (state, why) in [
            (show::State::Starting, "is still starting"),
            (show::State::Pending, "has not started"),
            (show::State::Waiting, "waits to start again"),
            (show::State::Stopping, "is stopping"),
        ] {
            assert_eq!(verdict(Some(&report(state))), Verdict::Waits(why.into()), "{state:?}");
        }
        let mut pending = report(show::State::Pending);
        pending.waiting_for = vec!["netd".into(), "resolved".into()];
        assert_eq!(verdict(Some(&pending)), Verdict::Waits("waits for netd, resolved".into()));
    }

    #[test]
    fn a_service_that_ended_for_good_fails_and_says_how() {
        for (state, why) in [
            (show::State::Stopped, "has stopped"),
            (show::State::Done, "has ended"),
            (show::State::Disabled, "is not enabled"),
            (show::State::Failed, "failed to start, and Spark gave up on it"),
        ] {
            assert_eq!(verdict(Some(&report(state))), Verdict::Fails(why.into()), "{state:?}");
        }
        let mut stopped = report(show::State::Stopped);
        stopped.last_end = Some("exited with code 1".into());
        assert_eq!(
            verdict(Some(&stopped)),
            Verdict::Fails("has stopped (exited with code 1)".into())
        );
    }

    #[test]
    fn a_service_spark_does_not_report_waits() {
        assert_eq!(verdict(None), Verdict::Waits("is not one Spark reports".into()));
    }

    // A health check only counts while the service runs: one that passed
    // before it stopped says nothing of the service that is not running.
    #[test]
    fn a_passing_check_does_not_save_a_service_that_stopped() {
        let service = checked(show::State::Stopped, show::HealthState::Passing);
        assert_eq!(verdict(Some(&service)), Verdict::Fails("has stopped".into()));
    }
}
