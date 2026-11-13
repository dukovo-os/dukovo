// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What putting a service file to use would do (ADR 0471): the clause
//! `service check` and `service edit` print after "ok;", and the web
//! console shows beside a file it has just checked. [`dry_run`] reads
//! the file's settings that differ from the copy Spark has, and the
//! `show` document, and says whether the service restarts, what starts
//! with it, what keeps running, and whether it would start at all.
//!
//! What only a client can read, the files themselves, comes in through
//! [`Change`]; Spark is not asked to do anything. When a sentence reads
//! wrong, look at the `state`, `why`, `requires`, `wants` and `after` of
//! the services in `service list --json` first: the rules read those
//! fields and nothing else. The `service-graph` boot test checks the
//! words against a running Spark.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::graph::{self, Link, Node, Peer, Verdict};
use crate::show::{Service, Show, State};
use crate::words::{and_list, tenths};

/// The most services a sentence names before it counts the rest.
const NAMED_MAX: usize = 3;

/// A service file that reads, and what Spark has of it.
pub struct Change<'a> {
    /// The service's name.
    pub name: &'a str,
    /// The settings that differ between the copy Spark has and this file
    /// (with their overrides), as dotted keys, sorted; None when Spark
    /// has no copy, as for a file added since boot.
    pub differ: Option<&'a [String]>,
    /// This file's `enabled`.
    pub enabled: bool,
    /// This file's links: `requires`, `wants` and `after`, by name.
    pub links: &'a [(String, Link)],
}

/// What putting `change` to use would do, as a clause to follow "ok;", or
/// None when it changes nothing.
pub fn dry_run(document: &Show, change: &Change) -> Option<String> {
    let name = change.name;
    let Some(differ) = change.differ else {
        let boot = if change.enabled { " and at each boot" } else { "" };
        return Some(format!("Spark reads it when {name} first starts{boot}"));
    };
    if differ.is_empty() {
        return None;
    }
    let service = find(document, name);
    if let [only] = differ
        && only == "enabled"
    {
        return Some(enabled_only(name, change.enabled, service.map(|s| s.state)));
    }
    let keys = and_list(differ);
    let (verb, applies) =
        if differ.len() == 1 { ("differs", "it applies") } else { ("differ", "they apply") };
    let mut said = match service.filter(|service| lives(service.state)) {
        Some(service) => restarts(document, service, change, &format!("{keys} {verb}")),
        None => {
            let mut said = format!(
                "{keys} {verb} from the copy Spark read last; {applies} when {name} next starts"
            );
            if let Some(why) = refusal(document, change) {
                said.push_str(&format!(", and {name} would not start: {why}"));
            }
            said
        }
    };
    if differ.iter().any(|key| key == "enabled") {
        said.push_str(if change.enabled {
            "; it starts at each boot from the next"
        } else {
            "; it no longer starts at boot"
        });
    }
    Some(said)
}

/// Service `name` in `document`.
fn find<'a>(document: &'a Show, name: &str) -> Option<&'a Service> {
    document.services.iter().find(|service| service.name == name)
}

/// Whether a service in `state` has a process, or is about to: a restart
/// is what applies a change to it.
fn lives(state: State) -> bool {
    matches!(state, State::Running | State::Starting | State::Waiting | State::Stopping)
}

/// What `enabled` alone differing means for service `name`, which is in
/// `state` if the document has it.
fn enabled_only(name: &str, enabled: bool, state: Option<State>) -> String {
    if enabled {
        let mut said = format!("{name} starts at each boot from the next");
        if !state.is_some_and(lives) {
            said.push_str(&format!("; `service {name} start` starts it now"));
        }
        said
    } else {
        let mut said = format!("{name} no longer starts at boot");
        if matches!(state, Some(State::Running | State::Starting)) {
            said.push_str(&format!("; `service {name} stop` stops it now"));
        }
        said
    }
}

/// What a restart of `service`, which runs, does with the settings that
/// differ, which `differ` names: "description differs".
fn restarts(document: &Show, service: &Service, change: &Change, differ: &str) -> String {
    let name = change.name;
    let mut said = format!(
        "{name} runs an older copy: {differ}; `service {name} restart` applies the change: it \
         restarts {name}"
    );
    // A time that is from an earlier start than the last says nothing.
    if let Some(took) = service.ready_ms.zip(service.started_ms).and_then(|(r, s)| r.checked_sub(s))
    {
        said.push_str(&format!(" (its last start took {})", tenths(took)));
    }
    match refusal(document, change) {
        Some(why) => said.push_str(&format!(" but {name} would not start: {why}")),
        None => {
            if let Some(starts) = starts(document, change) {
                said.push_str(&format!(" and {starts}"));
            }
        }
    }
    let dependents: Vec<String> = document
        .services
        .iter()
        .filter(|other| other.state == State::Running && other.requires.iter().any(|r| r == name))
        .map(|other| other.name.clone())
        .collect();
    if !dependents.is_empty() {
        let (require, keep) =
            if dependents.len() == 1 { ("requires", "keeps") } else { ("require", "keep") };
        let mut named: Vec<String> = dependents.iter().take(NAMED_MAX).cloned().collect();
        if dependents.len() > NAMED_MAX {
            named.push(format!("{} more", dependents.len() - NAMED_MAX));
        }
        said.push_str(&format!("; {}, which {require} {name}, {keep} running", and_list(&named)));
    }
    said
}

/// What stops the service from starting with the links of `change`: the
/// words after "would not start:", or None when nothing does.
fn refusal(document: &Show, change: &Change) -> Option<String> {
    let name = change.name;
    let peer = |other: &str| -> Peer<'static> {
        match find(document, other) {
            None => Peer::Off("has no service file"),
            Some(other)
                if other
                    .why
                    .as_deref()
                    .is_some_and(|why| why.starts_with("it waits for itself")) =>
            {
                Peer::Off("waits for itself")
            }
            Some(other) if matches!(other.state, State::Running | State::Done) => Peer::Ready,
            // A start of the service starts what it requires and wants,
            // stopped or given up on.
            Some(_) => Peer::Coming,
        }
    };
    let links: Vec<(&str, Link)> =
        change.links.iter().map(|(other, link)| (other.as_str(), *link)).collect();
    if let Verdict::Refuse { why, .. } = graph::verdict(&links, peer) {
        return Some(why);
    }
    // The graph as the new file would make it: the service's links are
    // the file's, and the file may be one the document has not seen.
    let mut nodes: Vec<Node> = document
        .services
        .iter()
        .filter(|service| service.name != name)
        .map(|service| Node {
            name: &service.name,
            order: service.order,
            links: [
                (&service.requires, Link::Requires),
                (&service.wants, Link::Wants),
                (&service.after, Link::After),
            ]
            .into_iter()
            .flat_map(|(names, link)| names.iter().map(move |other| (other.as_str(), link)))
            .collect(),
        })
        .collect();
    nodes.push(Node { name, order: 0, links });
    let circle = graph::circles(&nodes)
        .into_iter()
        .find(|circle| circle.iter().any(|member| member == name))?;
    Some(format!("it waits for itself, through {}", and_list(&circle)))
}

/// The services a start of the service would start with it, as a clause:
/// "starts db, which it now requires". Those that run, or are coming
/// already, are no news.
fn starts(document: &Show, change: &Change) -> Option<String> {
    let known = find(document, change.name);
    let started: Vec<String> = change
        .links
        .iter()
        .filter(|(_, link)| *link != Link::After)
        .filter(|(other, _)| {
            find(document, other).is_some_and(|other| {
                matches!(other.state, State::Stopped | State::Disabled | State::Failed)
            })
        })
        .map(|(other, link)| {
            let old = known.is_some_and(|service| {
                service.requires.contains(other) || service.wants.contains(other)
            });
            format!("{other}, which it {}{}", if old { "" } else { "now " }, link.key())
        })
        .collect();
    if started.is_empty() {
        return None;
    }
    let named = started.get(..NAMED_MAX).unwrap_or(&started);
    let mut said = format!("starts {}", named.join(", and "));
    if started.len() > NAMED_MAX {
        said.push_str(&format!(", and {} more", started.len() - NAMED_MAX));
    }
    Some(said)
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    fn service(name: &str, state: State) -> Service {
        Service { name: name.into(), state, ..Service::default() }
    }

    fn running(name: &str) -> Service {
        service(name, State::Running)
    }

    /// `name`, which is in `state` and requires `requires`.
    fn requiring(name: &str, state: State, requires: &[&str]) -> Service {
        let requires = requires.iter().map(|other| (*other).to_string()).collect();
        Service { requires, ..service(name, state) }
    }

    fn show_with(services: Vec<Service>) -> Show {
        Show { services, ..Show::default() }
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn links(links: &[(&str, Link)]) -> Vec<(String, Link)> {
        links.iter().map(|(name, link)| ((*name).to_string(), *link)).collect()
    }

    /// The dry run of `name`, with `differ` settings differing, whether
    /// it is `enabled` and these `links`.
    fn run(
        document: &Show,
        name: &str,
        differ: &[&str],
        enabled: bool,
        links: &[(String, Link)],
    ) -> Option<String> {
        let differ = strings(differ);
        dry_run(document, &Change { name, differ: Some(&differ), enabled, links })
    }

    #[test]
    fn a_file_spark_has_no_copy_of_is_read_when_the_service_first_starts() {
        let document = show_with(Vec::new());
        let change = |enabled| Change { name: "web", differ: None, enabled, links: &[] };
        assert_eq!(
            dry_run(&document, &change(true)).as_deref(),
            Some("Spark reads it when web first starts and at each boot")
        );
        assert_eq!(
            dry_run(&document, &change(false)).as_deref(),
            Some("Spark reads it when web first starts")
        );
    }

    #[test]
    fn a_file_that_differs_in_nothing_changes_nothing() {
        let document = show_with(vec![running("web")]);
        assert_eq!(run(&document, "web", &[], true, &[]), None);
    }

    #[test]
    fn enabling_a_service_that_is_not_running_says_how_to_start_it_now() {
        for state in [State::Stopped, State::Disabled, State::Failed, State::Done, State::Pending] {
            let document = show_with(vec![service("web", state)]);
            assert_eq!(
                run(&document, "web", &["enabled"], true, &[]).as_deref(),
                Some("web starts at each boot from the next; `service web start` starts it now"),
                "{state:?}"
            );
        }
        // A service the document has not seen is not running either.
        assert_eq!(
            run(&show_with(Vec::new()), "web", &["enabled"], true, &[]).as_deref(),
            Some("web starts at each boot from the next; `service web start` starts it now")
        );
    }

    #[test]
    fn enabling_a_service_that_runs_leaves_it_running() {
        for state in [State::Running, State::Starting, State::Waiting, State::Stopping] {
            let document = show_with(vec![service("web", state)]);
            assert_eq!(
                run(&document, "web", &["enabled"], true, &[]).as_deref(),
                Some("web starts at each boot from the next"),
                "{state:?}"
            );
        }
    }

    #[test]
    fn disabling_a_service_that_runs_says_how_to_stop_it_now() {
        for state in [State::Running, State::Starting] {
            let document = show_with(vec![service("web", state)]);
            assert_eq!(
                run(&document, "web", &["enabled"], false, &[]).as_deref(),
                Some("web no longer starts at boot; `service web stop` stops it now"),
                "{state:?}"
            );
        }
    }

    #[test]
    fn disabling_a_service_that_is_not_running_says_only_that_it_no_longer_starts_at_boot() {
        for state in [State::Stopped, State::Disabled, State::Waiting, State::Stopping] {
            let document = show_with(vec![service("web", state)]);
            assert_eq!(
                run(&document, "web", &["enabled"], false, &[]).as_deref(),
                Some("web no longer starts at boot"),
                "{state:?}"
            );
        }
    }

    #[test]
    fn settings_that_differ_apply_when_a_service_that_is_not_running_next_starts() {
        let document = show_with(vec![service("web", State::Stopped)]);
        assert_eq!(
            run(&document, "web", &["order"], true, &[]).as_deref(),
            Some("order differs from the copy Spark read last; it applies when web next starts")
        );
        assert_eq!(
            run(&document, "web", &["order", "restart"], true, &[]).as_deref(),
            Some(
                "order and restart differ from the copy Spark read last; they apply when web \
                 next starts"
            )
        );
        assert_eq!(
            run(&document, "web", &["limits.memory", "order", "restart"], true, &[]).as_deref(),
            Some(
                "limits.memory, order and restart differ from the copy Spark read last; they \
                 apply when web next starts"
            )
        );
    }

    #[test]
    fn a_service_the_document_does_not_have_is_not_running() {
        assert_eq!(
            run(&show_with(Vec::new()), "web", &["order"], true, &[]).as_deref(),
            Some("order differs from the copy Spark read last; it applies when web next starts")
        );
    }

    #[test]
    fn a_service_that_runs_is_restarted_to_apply_what_differs() {
        let document = show_with(vec![running("web")]);
        assert_eq!(
            run(&document, "web", &["description"], true, &[]).as_deref(),
            Some(
                "web runs an older copy: description differs; `service web restart` applies \
                 the change: it restarts web"
            )
        );
        assert_eq!(
            run(&document, "web", &["ready.timeout", "requires"], true, &[]).as_deref(),
            Some(
                "web runs an older copy: ready.timeout and requires differ; `service web \
                 restart` applies the change: it restarts web"
            )
        );
        for state in [State::Starting, State::Waiting, State::Stopping] {
            let document = show_with(vec![service("web", state)]);
            let said = run(&document, "web", &["order"], true, &[]).unwrap();
            assert!(said.starts_with("web runs an older copy: order differs;"), "{state:?}");
        }
    }

    #[test]
    fn the_time_of_the_last_start_is_said_when_both_times_are_known() {
        let started = |started_ms, ready_ms| {
            show_with(vec![Service { started_ms, ready_ms, ..running("web") }])
        };
        let said = |document: &Show| run(document, "web", &["order"], true, &[]).unwrap();
        assert_eq!(
            said(&started(Some(1_000), Some(3_149))),
            "web runs an older copy: order differs; `service web restart` applies the change: it \
             restarts web (its last start took 2.1 s)"
        );
        for document in [
            started(None, Some(3_000)),
            started(Some(1_000), None),
            started(None, None),
            // It was ready before the start it is running.
            started(Some(5_000), Some(3_000)),
        ] {
            assert!(!said(&document).contains("its last start took"));
        }
    }

    #[test]
    fn a_requirement_that_is_stopped_starts_with_the_service() {
        let document = show_with(vec![running("web"), service("db", State::Stopped)]);
        let new = links(&[("db", Link::Requires)]);
        assert_eq!(
            run(&document, "web", &["requires"], true, &new).as_deref(),
            Some(
                "web runs an older copy: requires differs; `service web restart` applies the \
                 change: it restarts web and starts db, which it now requires"
            )
        );
        // One the document's copy names already is no news.
        let document = show_with(vec![
            requiring("web", State::Running, &["db"]),
            service("db", State::Failed),
        ]);
        assert_eq!(
            run(&document, "web", &["order"], true, &new).as_deref(),
            Some(
                "web runs an older copy: order differs; `service web restart` applies the change: \
                 it restarts web and starts db, which it requires"
            )
        );
    }

    #[test]
    fn a_service_that_is_wanted_starts_too_and_one_that_runs_or_is_after_does_not() {
        let document = show_with(vec![
            running("web"),
            service("cache", State::Disabled),
            running("db"),
            service("log", State::Stopped),
        ]);
        let new = links(&[
            ("cache", Link::Wants),
            ("db", Link::Requires),
            ("log", Link::After),
            ("away", Link::Wants),
        ]);
        assert_eq!(
            run(&document, "web", &["wants"], true, &new).as_deref(),
            Some(
                "web runs an older copy: wants differs; `service web restart` applies the change: \
                 it restarts web and starts cache, which it now wants"
            )
        );
        // A service the wants names that has no file starts nothing.
        let nothing = links(&[("away", Link::Wants)]);
        assert_eq!(
            run(&document, "web", &["wants"], true, &nothing).as_deref(),
            Some(
                "web runs an older copy: wants differs; `service web restart` applies the change: \
                 it restarts web"
            )
        );
    }

    #[test]
    fn a_wanted_service_the_copy_wanted_already_is_no_news() {
        let web = Service { wants: strings(&["cache"]), ..running("web") };
        let document = show_with(vec![web, service("cache", State::Stopped)]);
        let new = links(&[("cache", Link::Requires)]);
        let said = run(&document, "web", &["requires"], true, &new).unwrap();
        assert!(said.ends_with("and starts cache, which it requires"), "{said}");
    }

    #[test]
    fn a_requirement_that_has_no_file_refuses_the_start() {
        let document = show_with(vec![running("web")]);
        let new = links(&[("db", Link::Requires)]);
        assert_eq!(
            run(&document, "web", &["requires"], true, &new).as_deref(),
            Some(
                "web runs an older copy: requires differs; `service web restart` applies the \
                 change: it restarts web but web would not start: it requires db, which has no \
                 service file"
            )
        );
        // For a service that is not running, it says so after when it applies.
        let document = show_with(vec![service("web", State::Stopped)]);
        assert_eq!(
            run(&document, "web", &["requires"], true, &new).as_deref(),
            Some(
                "requires differs from the copy Spark read last; it applies when web next \
                 starts, and web would not start: it requires db, which has no service file"
            )
        );
    }

    #[test]
    fn a_requirement_that_waits_for_itself_refuses_the_start() {
        let a = Service {
            why: Some("it waits for itself, through a and b".into()),
            ..service("a", State::Stopped)
        };
        let document = show_with(vec![running("web"), a]);
        let new = links(&[("a", Link::Requires)]);
        let said = run(&document, "web", &["requires"], true, &new).unwrap();
        assert!(
            said.ends_with("but web would not start: it requires a, which waits for itself"),
            "{said}"
        );
    }

    #[test]
    fn links_that_close_a_circle_refuse_the_start() {
        let a = Service { wants: strings(&["web"]), ..running("a") };
        let b = Service { requires: strings(&["a"]), ..running("b") };
        let document = show_with(vec![running("web"), a, b]);
        let new = links(&[("b", Link::Wants)]);
        assert_eq!(
            run(&document, "web", &["wants"], true, &new).as_deref(),
            Some(
                "web runs an older copy: wants differs; `service web restart` applies the change: \
                 it restarts web but web would not start: it waits for itself, through a, b and web"
            )
        );
        // A circle that leaves the service out is not its mistake.
        let c = Service { requires: strings(&["d"]), ..running("c") };
        let d = Service { requires: strings(&["c"]), ..running("d") };
        let document = show_with(vec![running("web"), c, d]);
        let said = run(&document, "web", &["wants"], true, &links(&[("c", Link::Wants)])).unwrap();
        assert!(!said.contains("would not start"), "{said}");
        // A file the document has not seen takes part in the graph too.
        let a = Service { wants: strings(&["new"]), ..running("a") };
        let document = show_with(vec![a]);
        let new = links(&[("a", Link::Requires)]);
        assert_eq!(
            run(&document, "new", &["order"], true, &new).as_deref(),
            Some(
                "order differs from the copy Spark read last; it applies when new next starts, \
                 and new would not start: it waits for itself, through a and new"
            )
        );
    }

    #[test]
    fn at_most_three_services_that_start_are_named() {
        let mut services = vec![running("web")];
        let mut new = Vec::new();
        for name in ["a", "b", "c", "d"] {
            services.push(service(name, State::Stopped));
            new.push((name, if name == "c" { Link::Wants } else { Link::Requires }));
        }
        let document = show_with(services);
        let four = links(&new);
        assert_eq!(
            run(&document, "web", &["requires"], true, &four).as_deref(),
            Some(
                "web runs an older copy: requires differs; `service web restart` applies the \
                 change: it restarts web and starts a, which it now requires, and b, which it now \
                 requires, and c, which it now wants, and 1 more"
            )
        );
        let three = links(&new[..3]);
        let said = run(&document, "web", &["requires"], true, &three).unwrap();
        assert!(said.ends_with("and c, which it now wants"), "{said}");
    }

    #[test]
    fn the_services_that_require_it_and_run_keep_running() {
        let api = requiring("api", State::Running, &["web"]);
        let worker = requiring("worker", State::Running, &["web", "db"]);
        let idle = requiring("idle", State::Stopped, &["web"]);
        let other = requiring("other", State::Running, &["db"]);
        let document = show_with(vec![running("web"), api.clone(), idle.clone(), other.clone()]);
        assert_eq!(
            run(&document, "web", &["order"], true, &[]).as_deref(),
            Some(
                "web runs an older copy: order differs; `service web restart` applies the change: \
                 it restarts web; api, which requires web, keeps running"
            )
        );
        let document = show_with(vec![running("web"), api.clone(), idle, worker.clone(), other]);
        assert_eq!(
            run(&document, "web", &["order"], true, &[]).as_deref(),
            Some(
                "web runs an older copy: order differs; `service web restart` applies the change: \
                 it restarts web; api and worker, which require web, keep running"
            )
        );
        let mut services = vec![running("web"), api, worker];
        for name in ["c", "d", "e"] {
            services.push(requiring(name, State::Running, &["web"]));
        }
        let said = run(&show_with(services), "web", &["order"], true, &[]).unwrap();
        assert!(
            said.ends_with("; api, worker, c and 2 more, which require web, keep running"),
            "{said}"
        );
    }

    #[test]
    fn the_links_come_before_the_dependents() {
        let api = requiring("api", State::Running, &["web"]);
        let document = show_with(vec![running("web"), api, service("db", State::Stopped)]);
        let new = links(&[("db", Link::Requires)]);
        assert_eq!(
            run(&document, "web", &["requires", "ready.timeout"], true, &new).as_deref(),
            Some(
                "web runs an older copy: requires and ready.timeout differ; `service web \
                 restart` applies the change: it restarts web and starts db, which it now \
                 requires; api, which requires web, keeps running"
            )
        );
    }

    #[test]
    fn enabled_among_other_settings_is_said_at_the_end() {
        let document = show_with(vec![running("web"), requiring("api", State::Running, &["web"])]);
        assert_eq!(
            run(&document, "web", &["enabled", "order"], true, &[]).as_deref(),
            Some(
                "web runs an older copy: enabled and order differ; `service web restart` \
                 applies the change: it restarts web; api, which requires web, keeps running; it \
                 starts at each boot from the next"
            )
        );
        assert_eq!(
            run(&document, "web", &["enabled", "order"], false, &[]).as_deref(),
            Some(
                "web runs an older copy: enabled and order differ; `service web restart` \
                 applies the change: it restarts web; api, which requires web, keeps running; it \
                 no longer starts at boot"
            )
        );
        let document = show_with(vec![service("web", State::Stopped)]);
        assert_eq!(
            run(&document, "web", &["enabled", "order"], true, &[]).as_deref(),
            Some(
                "enabled and order differ from the copy Spark read last; they apply when web \
                 next starts; it starts at each boot from the next"
            )
        );
    }
}
