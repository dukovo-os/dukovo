// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The services check of dukovo-check(8): which services Spark gave up
//! on. Whether one is on is also what the checks of SSH and snapshots ask.

use spark_api::show::{Service, State};
use spark_api::words::and_list;

use super::context::Context;
use super::line::{Outcome, Problem};

/// Whether `service` runs or will: it is running, starting, waiting to
/// start again or for what it requires, or waiting for a connection at
/// its sockets. One that is disabled, stopped, ended or failed does not.
pub(super) fn is_on(service: &Service) -> bool {
    matches!(
        service.state,
        State::Running | State::Starting | State::Waiting | State::Pending | State::Listening
    )
}

/// Warns of each service that Spark gave up on after failed starts in a
/// row, by name: a service that is off as it was asked to be is no fault.
pub(super) fn failed(context: &Context) -> Vec<Outcome> {
    let services = match context.services() {
        Ok(services) => services,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let gave_up: Vec<String> = services
        .iter()
        .filter(|service| service.state == State::Failed)
        .map(|service| service.name.clone())
        .collect();
    if gave_up.is_empty() {
        return vec![Outcome::passed("none gave up")];
    }
    vec![Outcome::warning(
        Problem::ServiceGaveUp,
        format!("{} gave up after failed starts in a row", and_list(&gave_up)),
    )]
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::{System, service};
    use super::super::line::Verdict;
    use super::*;

    #[test]
    fn a_service_is_on_while_it_runs_or_is_to_start() {
        let on = |state| is_on(&service("x", state));
        for state in
            [State::Running, State::Starting, State::Waiting, State::Pending, State::Listening]
        {
            assert!(on(state), "{state:?}");
        }
        for state in [State::Disabled, State::Stopped, State::Stopping, State::Done, State::Failed]
        {
            assert!(!on(state), "{state:?}");
        }
    }

    #[test]
    fn services_that_gave_up_are_named_and_the_rest_are_not_a_fault() {
        let system = System::new(vec![
            service("logd", State::Running),
            service("web", State::Failed),
            service("sshd", State::Disabled),
            service("backup", State::Failed),
            service("firewall", State::Done),
        ]);
        let outcomes = failed(&system.context);
        assert_eq!(outcomes.len(), 1);
        let outcome = &outcomes[0];
        assert_eq!(outcome.verdict, Verdict::Warning);
        assert_eq!(outcome.id.as_deref(), Some("SEC-150"));
        assert_eq!(outcome.detail, "web and backup gave up after failed starts in a row");
    }

    #[test]
    fn one_service_that_gave_up_is_named_in_the_singular() {
        let system = System::new(vec![service("web", State::Failed)]);
        assert_eq!(failed(&system.context)[0].detail, "web gave up after failed starts in a row");
    }

    #[test]
    fn no_service_that_gave_up_passes() {
        let system =
            System::new(vec![service("logd", State::Running), service("sshd", State::Stopped)]);
        let outcomes = failed(&system.context);
        assert_eq!(
            (outcomes[0].verdict, outcomes[0].detail.as_str()),
            (Verdict::Passed, "none gave up")
        );
    }

    #[test]
    fn spark_that_cannot_say_skips_the_check() {
        let mut system = System::new(Vec::new());
        system.context.services = Err("not running".into());
        assert_eq!(failed(&system.context)[0].verdict, Verdict::Skipped);
    }
}
