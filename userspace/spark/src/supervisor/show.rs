// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `show` request's answer (spark(8), ADR 0470): what Spark knows of
//! every service, or of one, as [`spark_api::show`]'s document, which
//! service(8), sysupdate, bundled(8) and the web console read.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use spark_api::show::{self, Health, HealthState, Holds, Show};

use super::{
    MS, SECOND, Service, Shutdown, State, Supervisor, cards_for, ending, now, pci_address,
};
use crate::services::Spec;

impl Supervisor {
    /// The answer to `show`, or to `show NAME` with `name`.
    pub(super) fn answer_show(&self, name: Option<&str>) -> String {
        let mut services: Vec<&Service> = match name {
            Some(name) => match self.services.iter().find(|s| s.spec.name == name) {
                Some(service) => alloc::vec![service],
                None => return alloc::format!("error: no service is called {name}\n"),
            },
            None => self.services.iter().collect(),
        };
        services.sort_by(|a, b| (a.spec.order, &a.spec.name).cmp(&(b.spec.order, &b.spec.name)));
        let time = now();
        let shutdown = match self.shutdown {
            Some(Shutdown::Pending { end, at }) => Some(show::Shutdown {
                end: end.words().to_string(),
                in_seconds: at.saturating_sub(time) / SECOND,
            }),
            _ => None,
        };
        let document = Show {
            show_version: show::VERSION,
            now_ms: time / MS,
            services: services.into_iter().map(|s| described(s, self.devices)).collect(),
            shutdown,
            trials: self.shown_trials(time),
            undone: self.undone.clone(),
        };
        match show::to_json(&document) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(err) => alloc::format!("error: {err}\n"),
        }
    }
}

/// `service` as the document describes it, with `devices` the root
/// device capability, for the cards it drives.
fn described(service: &Service, devices: Option<u32>) -> show::Service {
    let spec = &service.spec;
    let state = match service.state {
        State::Starting(_) | State::Running(_) if service.stop_by.is_some() => {
            show::State::Stopping
        }
        State::Stopped if !spec.enabled && service.wanted_by.is_none() => show::State::Disabled,
        State::Stopped => show::State::Stopped,
        State::Pending => show::State::Pending,
        State::Starting(_) => show::State::Starting,
        State::Running(_) => show::State::Running,
        State::Done => show::State::Done,
        State::Waiting(_) => show::State::Waiting,
        State::Draining(..) => show::State::Stopping,
        State::Failed => show::State::Failed,
    };
    let health = spec.health.as_ref().map(|check| {
        let checks = &service.checks;
        Health {
            state: match checks.passing {
                None => HealthState::Unknown,
                Some(true) => HealthState::Passing,
                Some(false) => HealthState::Failing,
            },
            failures: checks.failures,
            restart_after: check.failures,
            last_failure: checks.last_failure.clone(),
            checked_ms: checks.checked_ns.map(|ns| ns / MS),
        }
    });
    show::Service {
        name: spec.name.clone(),
        description: spec.description.clone(),
        enabled: spec.enabled,
        order: spec.order,
        restart: spec.restart.name().to_string(),
        give_up_after: spec.give_up_after,
        requires: spec.requires.clone(),
        wants: spec.wants.clone(),
        after: spec.after.clone(),
        ready_when: spec.ready.when.name().to_string(),
        state,
        pid: service.state.pid(),
        since_ms: service.since_ns / MS,
        started_ms: (service.started_ns > 0).then_some(service.started_ns / MS),
        ready_ms: service.ready_ns.map(|ns| ns / MS),
        ready_by_ms: (state == show::State::Starting).then_some(service.ready_by / MS),
        retry_ms: match service.state {
            State::Waiting(at) => Some(at / MS),
            _ => None,
        },
        restarts: service.restarts,
        failures: service.failures,
        last_end: service.last.map(ending),
        last_status: service.last,
        why: service.why.clone(),
        off_as_meant: service.off_as_meant,
        waiting_for: service.waiting_for.clone(),
        started_by: service.wanted_by.clone(),
        health,
        complaint: service.complaint.clone(),
        cause: service.cause.clone(),
        last_line: service.last_line.clone(),
        last_request: service.last_request.clone(),
        holds: holds(spec, devices),
    }
}

/// What `spec` gives its service, beyond what every service's view holds.
fn holds(spec: &Spec, devices: Option<u32>) -> Holds {
    let (changes, reads): (Vec<_>, Vec<_>) = spec.places.iter().partition(|place| place.write);
    let admin = [
        (spec.configure, "network"),
        (spec.hardware, "hardware"),
        (spec.time, "time"),
        (spec.firewall, "firewall"),
    ];
    Holds {
        whole_tree: spec.whole_tree,
        account: spec.account.clone(),
        reads: reads.into_iter().map(|place| place.path.clone()).collect(),
        changes: changes.into_iter().map(|place| place.path.clone()).collect(),
        mounts: spec.mounts.clone(),
        secrets: spec.secrets.clone(),
        listen: spec
            .listen
            .iter()
            .map(|&(first, count)| match count {
                1 => first.to_string(),
                _ => format!("{first}-{}", first + count - 1),
            })
            .collect(),
        outbound: spec.outbound,
        admin: admin.iter().filter(|(has, _)| *has).map(|(_, word)| word.to_string()).collect(),
        names: spec.names,
        cards: cards_for(devices, spec).iter().map(|card| pci_address(card.address)).collect(),
        memory_bytes: spec.memory_pages.map(|pages| pages * 4096),
        cpu_weight: spec.weight,
    }
}
