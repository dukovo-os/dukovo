// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Services that are not running as asked (ADR 0485): alertd asks Spark
//! what its services are doing every 10 seconds, as sysupdate's health
//! check does, and raises `service:<name>` for each one that has not
//! been as asked for 60 seconds in a row. The alert's source is `spark`.
//!
//! Spark is PID 1 and never waits on a socket, so nothing is added to
//! it: alertd looks. A service that fails and is started again shows
//! as starting part of the time; the 60 seconds count from when it was
//! first seen wrong, and start over only when it is seen running or done
//! as asked, so a crash loop is not missed between its runs. The alert is critical
//! once Spark has given up on the service, and a warning before. It
//! clears as soon as the service is as asked.
//!
//! What "as asked" means is decided by [`not_as_asked`], the rule
//! `service list` marks services by.

use std::collections::HashMap;

use spark_api::show::{HealthState, Service, Show, State};

use crate::alerts::protocol::{Alert, Severity, TITLE_MAX};

/// How long a service must stay wrong before it is an alert, in
/// milliseconds of Spark's clock.
pub const GRACE_MS: u64 = 60_000;

/// How often Spark is asked.
pub const EVERY_SECONDS: u64 = 10;

/// The source of these alerts.
pub const SOURCE: &str = "spark";

/// Whether `service` is not doing what Spark was asked of it: the rule
/// `service list` marks services by, so that the two never disagree.
pub(crate) use crate::service::not_as_asked;

/// When each service was first seen wrong, in Spark's milliseconds.
#[derive(Debug, Default)]
pub struct Watch {
    wrong_since: HashMap<String, u64>,
}

impl Watch {
    /// The alerts for what `show` says: one for each service that has
    /// been wrong for [`GRACE_MS`].
    pub fn observe(&mut self, show: &Show) -> Vec<Alert> {
        let now = show.now_ms;
        // A service on its way, starting or stopping, neither begins a
        // count nor ends one, and keeps its alert: one that crashes again
        // and again is seen starting part of the time.
        let on_its_way = |service: &Service| {
            matches!(service.state, State::Pending | State::Starting | State::Stopping)
                && self.wrong_since.contains_key(&service.name)
        };
        let wrong: Vec<&Service> = show
            .services
            .iter()
            .filter(|service| not_as_asked(service) || on_its_way(service))
            .collect();
        self.wrong_since.retain(|name, _| wrong.iter().any(|service| service.name == *name));
        let mut alerts = Vec::new();
        for service in wrong {
            let since = *self.wrong_since.entry(service.name.clone()).or_insert(now);
            if now.saturating_sub(since) >= GRACE_MS {
                alerts.push(alert(service));
            }
        }
        alerts
    }
}

/// The alert for `service`.
fn alert(service: &Service) -> Alert {
    let name = &service.name;
    let (severity, title) = match service.state {
        State::Failed => {
            (Severity::Critical, format!("Service {name} failed; Spark gave up on it"))
        }
        State::Stopped => (Severity::Warning, format!("Service {name} is stopped and should run")),
        State::Pending => (Severity::Warning, format!("Service {name} has not started")),
        State::Starting => (Severity::Warning, format!("Service {name} is not ready")),
        State::Waiting => {
            (Severity::Warning, format!("Service {name} stops and waits to start again"))
        }
        State::Stopping => (Severity::Warning, format!("Service {name} is not done stopping")),
        State::Running | State::Done | State::Disabled => {
            (Severity::Warning, format!("Service {name} fails its health check"))
        }
    };
    Alert {
        key: format!("service:{}", key_part(name)),
        severity,
        title: title.chars().take(TITLE_MAX).collect(),
        detail: detail(service),
    }
}

/// `text` as a part of a key: lower case, and `-` for what a key may not
/// hold.
fn key_part(text: &str) -> String {
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c);
    text.to_ascii_lowercase().chars().map(|c| if allowed(c) { c } else { '-' }).collect()
}

/// What Spark says is wrong, and how to look further.
fn detail(service: &Service) -> String {
    let mut lines = Vec::new();
    if let Some(why) = &service.why {
        lines.push(format!("Spark says: {why}."));
    }
    if let Some(health) = &service.health
        && health.state == HealthState::Failing
    {
        let last = health.last_failure.as_deref().unwrap_or("no reason given");
        lines.push(format!("Its last health check failed: {last}."));
    }
    if let Some(end) = &service.last_end {
        lines.push(format!("Its process last ended: {end}."));
    }
    lines.push(format!("Run \"service {} status\" to see more.", service.name));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_api::show::Health;

    fn service(name: &str, state: State) -> Service {
        Service { name: name.into(), enabled: true, state, ..Service::default() }
    }

    fn show(now_ms: u64, services: Vec<Service>) -> Show {
        Show { now_ms, services, ..Show::default() }
    }

    #[test]
    fn a_service_is_as_asked_when_it_runs_is_done_is_disabled_or_was_stopped_by_a_request() {
        assert!(!not_as_asked(&service("a", State::Running)));
        assert!(!not_as_asked(&service("a", State::Done)));
        assert!(!not_as_asked(&Service { enabled: false, ..service("a", State::Disabled) }));
        assert!(!not_as_asked(&service("a", State::Stopped)));
        assert!(!not_as_asked(&Service { enabled: false, ..service("a", State::Pending) }));
    }

    #[test]
    fn a_service_is_not_as_asked_when_failed_refused_waiting_or_unhealthy() {
        assert!(not_as_asked(&service("a", State::Failed)));
        assert!(not_as_asked(&service("a", State::Waiting)));
        let refused =
            Service { why: Some("it requires db".into()), ..service("a", State::Stopped) };
        assert!(not_as_asked(&refused));
        // A driver whose device is not there is off as it is meant to be.
        let no_card = Service { off_as_meant: true, ..refused };
        assert!(!not_as_asked(&no_card));
        for state in [State::Pending, State::Starting, State::Stopping] {
            assert!(!not_as_asked(&service("a", state)), "{state:?}");
        }
        let unhealthy = Service {
            health: Some(Health { state: HealthState::Failing, ..Health::default() }),
            ..service("a", State::Running)
        };
        assert!(not_as_asked(&unhealthy));
    }

    #[test]
    fn a_service_wrong_for_60_seconds_raises_and_not_before() {
        let mut watch = Watch::default();
        let wrong = vec![service("web", State::Waiting)];
        assert_eq!(watch.observe(&show(1_000, wrong.clone())), []);
        assert_eq!(watch.observe(&show(60_999, wrong.clone())), []);
        let alerts = watch.observe(&show(61_000, wrong));
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "service:web");
        assert_eq!(alerts[0].severity, Severity::Warning);
    }

    #[test]
    fn a_failed_service_is_critical_and_the_detail_says_why_and_how_to_look() {
        let mut watch = Watch::default();
        let failed = Service {
            why: Some("it requires db, which is stopped".into()),
            last_end: Some("exited with code 1".into()),
            health: Some(Health {
                state: HealthState::Failing,
                last_failure: Some("no answer on port 80".into()),
                ..Health::default()
            }),
            ..service("web", State::Failed)
        };
        watch.observe(&show(0, vec![failed.clone()]));
        let alert = watch.observe(&show(GRACE_MS, vec![failed])).remove(0);
        assert_eq!(alert.severity, Severity::Critical);
        assert_eq!(alert.title, "Service web failed; Spark gave up on it");
        assert_eq!(
            alert.detail,
            "Spark says: it requires db, which is stopped.\nIts last health \
             check failed: no answer on port 80.\nIts process last ended: exited with code 1.\nRun \
             \"service web status\" to see more."
        );
        assert_eq!(alert.invalid(), None);
    }

    #[test]
    fn a_service_seen_as_asked_clears_and_starts_its_60_seconds_again() {
        let mut watch = Watch::default();
        watch.observe(&show(0, vec![service("web", State::Waiting)]));
        assert_eq!(watch.observe(&show(70_000, vec![service("web", State::Waiting)])).len(), 1);
        // Running again: no alert, and the count is forgotten.
        assert_eq!(watch.observe(&show(75_000, vec![service("web", State::Running)])), []);
        assert_eq!(watch.observe(&show(80_000, vec![service("web", State::Waiting)])), []);
        assert_eq!(watch.observe(&show(139_000, vec![service("web", State::Waiting)])), []);
        assert_eq!(watch.observe(&show(140_000, vec![service("web", State::Waiting)])).len(), 1);
    }

    #[test]
    fn a_service_that_goes_from_waiting_to_starting_keeps_counting() {
        let mut watch = Watch::default();
        watch.observe(&show(0, vec![service("web", State::Waiting)]));
        watch.observe(&show(30_000, vec![service("web", State::Starting)]));
        assert_eq!(watch.observe(&show(60_000, vec![service("web", State::Waiting)])).len(), 1);
    }

    #[test]
    fn an_alert_stays_while_the_service_starts_again() {
        let mut watch = Watch::default();
        watch.observe(&show(0, vec![service("web", State::Waiting)]));
        assert_eq!(watch.observe(&show(60_000, vec![service("web", State::Waiting)])).len(), 1);
        assert_eq!(watch.observe(&show(70_000, vec![service("web", State::Starting)])).len(), 1);
        // One that starts and was never seen wrong is no alert.
        assert_eq!(watch.observe(&show(80_000, vec![service("db", State::Starting)])), []);
    }

    #[test]
    fn a_name_that_a_key_may_not_hold_is_made_to_fit() {
        assert_eq!(key_part("My Service_1.x"), "my-service_1.x");
        let mut watch = Watch::default();
        watch.observe(&show(0, vec![service("Odd Name", State::Failed)]));
        let alert =
            watch.observe(&show(GRACE_MS, vec![service("Odd Name", State::Failed)])).remove(0);
        assert_eq!(alert.key, "service:odd-name");
        assert_eq!(alert.invalid(), None);
    }

    #[test]
    fn several_services_are_counted_each_for_itself() {
        let mut watch = Watch::default();
        watch.observe(&show(0, vec![service("a", State::Failed)]));
        let both = vec![service("a", State::Failed), service("b", State::Failed)];
        let alerts = watch.observe(&show(60_000, both));
        assert_eq!(alerts.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), ["service:a"]);
    }
}
