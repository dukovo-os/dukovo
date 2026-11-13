// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Why a service is in its state, and what to do about it (ADR 0471): the
//! sentence `service NAME explain` prints and the web console shows,
//! from Spark's `show` document alone. [`explain`] says what the service
//! is doing, then follows the service it waits for, or the one that
//! stopped it, and says what that is doing, down to the one at the root.
//! The words after "which" and "because" are Spark's own (`why` in
//! [`crate::show::Service`]), so that a person reads the same reason in
//! `service NAME status`, in the log and here.
//!
//! What only a client knows, the clock and the names of users, comes in
//! through [`Context`]. When a sentence reads wrong, look at the `state`,
//! `why` and `cause` of each service in `service list --json` first: the
//! rules here read those fields and nothing else. The `service-graph`
//! boot test and the lab's `services/explain` test check the words
//! against a running Spark.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::Serialize;

use crate::show::{HealthState, Request, Service, Show, State};
use crate::words::{and_list, ready_once, span};

/// The most services an explanation follows.
const DEPTH_MAX: usize = 16;

/// The `why` texts of a service Spark leaves off at boot, as it is meant
/// to be: in essentials only the system's own services start, and in
/// Rescue only Rescue's.
const LEFT_OFF: [&str; 2] = [
    "only the essentials start: it stays off until it is started",
    "Rescue leaves it off until it is started",
];

/// What a `why` starts with when it says how a service's process ended.
const ENDS: [&str; 3] =
    ["it exited with code ", "it was ended by signal ", "it did not get ready in "];

/// What the explanation needs besides the document: the clock and the
/// names of users, which only the client knows.
pub struct Context<'a> {
    /// Says when a time, in milliseconds since boot, was: "10:42:03, 2 min ago".
    pub when: &'a dyn Fn(u64) -> String,
    /// Names a user by number: "chase".
    pub user_name: &'a dyn Fn(u32) -> String,
}

/// Why a service is in its state, and what to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Explanation {
    /// One sentence: the service, its state, and the services followed
    /// to the root cause, each with its own state.
    pub sentence: String,
    /// The services followed, from the one asked about to the root
    /// cause, each once.
    pub chain: Vec<String>,
    /// The service at the root cause, the last of `chain`.
    pub root: String,
    /// What to do, as one sentence; None when nothing needs doing.
    pub fix: Option<String>,
}

impl Explanation {
    /// What `service NAME explain` prints: the sentence and the fix, one
    /// after the other, on one line.
    pub fn text(&self) -> String {
        match &self.fix {
            Some(fix) => format!("{} {fix}", self.sentence),
            None => self.sentence.clone(),
        }
    }
}

/// The explanation of service `name` in `document`, or None when the
/// document has no such service.
pub fn explain(document: &Show, name: &str, context: &Context) -> Option<Explanation> {
    let service = find(document, name)?;
    let mut chain = Vec::new();
    let mut sentence = format!("{name} {}", clause(document, service, context, &mut chain));
    if let Some(complaint) = &service.complaint {
        sentence.push_str(&format!(
            "; its file does not read ({complaint}), so Spark runs its last good copy"
        ));
    }
    sentence.push('.');
    let root = chain.last().cloned()?;
    let fix = fix_for(document, name, &chain, &root);
    Some(Explanation { sentence, chain, root, fix })
}

/// Service `name` in `document`.
fn find<'a>(document: &'a Show, name: &str) -> Option<&'a Service> {
    document.services.iter().find(|service| service.name == name)
}

/// Service `name`, if an explanation that has followed `chain` follows it
/// next: it is in the document, is not in the chain already (a circle
/// ends where it closes), and the chain is not at DEPTH_MAX.
fn followable<'a>(document: &'a Show, name: &str, chain: &[String]) -> Option<&'a Service> {
    if chain.len() >= DEPTH_MAX || chain.iter().any(|seen| seen == name) {
        return None;
    }
    find(document, name)
}

/// What `service` is doing, as words that follow its name or "which":
/// "is running as process 57, ready since 10:42:03". It adds its name to
/// `chain`, and the names of the services it follows.
fn clause(
    document: &Show,
    service: &Service,
    context: &Context,
    chain: &mut Vec<String>,
) -> String {
    chain.push(service.name.clone());
    match service.state {
        State::Running => running(service, context),
        State::Done => {
            format!("ran as a setup step and is done, since {}", (context.when)(service.since_ms))
        }
        State::Starting => starting(service, document.now_ms, context),
        State::Pending => pending(document, service, context, chain),
        State::Waiting => waiting(service, document.now_ms),
        State::Failed => failed(service),
        State::Stopping => format!("is stopping{}", process(service.pid)),
        State::Stopped | State::Disabled => stopped(document, service, context, chain),
    }
}

/// " as process 57", or nothing for a service with no process.
fn process(pid: Option<u64>) -> String {
    pid.map_or_else(String::new, |pid| format!(" as process {pid}"))
}

/// The last line the service's output wrote, as ` ("line")`, or nothing
/// when it wrote none since it started.
fn quote(service: &Service) -> String {
    service.last_line.as_ref().map_or_else(String::new, |line| format!(" (\"{line}\")"))
}

/// A running service, and how its health check goes.
fn running(service: &Service, context: &Context) -> String {
    let since = match service.ready_ms {
        Some(ready) => format!("ready since {}", (context.when)(ready)),
        None => format!("since {}", (context.when)(service.since_ms)),
    };
    let mut text = format!("is running{}, {since}", process(service.pid));
    let Some(health) = &service.health else { return text };
    match health.state {
        HealthState::Passing => text.push_str(", and its health check passes"),
        HealthState::Failing => {
            let last =
                health.last_failure.as_ref().map_or_else(String::new, |last| format!(" ({last})"));
            let times = if health.failures == 1 { "time" } else { "times" };
            text.push_str(&format!(
                ", but its health check failed {} {times} in a row{last}; Spark restarts it after \
                 {} failures in a row",
                health.failures, health.restart_after
            ));
        }
        HealthState::Unknown => {}
    }
    text
}

/// A starting service, and how long it has to be ready.
fn starting(service: &Service, now_ms: u64, context: &Context) -> String {
    let mut text = format!(
        "is starting{}, since {}, and is ready once {}",
        process(service.pid),
        (context.when)(service.since_ms),
        ready_once(&service.ready_when)
    );
    if let Some(ready_by) = service.ready_by_ms {
        text.push_str(&format!(" within {}", span(ready_by.saturating_sub(now_ms))));
    }
    text
}

/// A pending service, and the first service it waits for that an
/// explanation follows.
fn pending(
    document: &Show,
    service: &Service,
    context: &Context,
    chain: &mut Vec<String>,
) -> String {
    if service.waiting_for.is_empty() {
        // Spark fills the list in the turn after it makes a service pending.
        return String::from("is waiting for nothing yet");
    }
    let mut text = format!("is waiting for {}", and_list(&service.waiting_for));
    let next = service.waiting_for.iter().find_map(|name| followable(document, name, chain));
    if let Some(next) = next {
        text.push_str(&format!(", which {}", clause(document, next, context, chain)));
    }
    text
}

/// A service waiting to start again, and how many more failed starts
/// Spark allows it.
fn waiting(service: &Service, now_ms: u64) -> String {
    let mut text = match service.retry_ms {
        Some(retry) => format!("is to start again in {}", span(retry.saturating_sub(now_ms))),
        None => String::from("is to start again"),
    };
    if let Some(why) = &service.why {
        text.push_str(&format!(" after {why}"));
    }
    text.push_str(&quote(service));
    if service.give_up_after > 0 {
        let more = service.give_up_after.saturating_sub(service.failures);
        let starts = if more == 1 { "start" } else { "starts" };
        text.push_str(&format!("; Spark gives up after {more} more failed {starts} in a row"));
    }
    text
}

/// A service Spark gave up on.
fn failed(service: &Service) -> String {
    match &service.why {
        Some(why) => format!("was given up on: {why}{}", quote(service)),
        None => format!("was given up on after {} failed starts in a row", service.failures),
    }
}

/// Why a stopped or disabled service is off: the first of these that
/// holds.
enum Stop<'a> {
    /// Its `why` says it requires service `cause`, which `words` describe.
    Requirement { cause: &'a str, words: &'a str },
    /// Its `why`, which names no requirement.
    Reason(&'a str),
    /// A request stopped it.
    Request(&'a Request),
    /// Its process ended, in these words, and its restart setting does
    /// not start it again.
    End(&'a str),
    /// Nothing has started it since boot.
    Never,
}

/// The first rule that says why `service`, stopped or disabled, is off.
fn stop_reason(service: &Service) -> Stop<'_> {
    if let Some((cause, words)) = requirement(service) {
        Stop::Requirement { cause, words }
    } else if let Some(why) = &service.why {
        Stop::Reason(why)
    } else if let Some(request) = service.last_request.as_ref().filter(|r| r.verb == "stop") {
        Stop::Request(request)
    } else if let Some(end) = &service.last_end {
        Stop::End(end)
    } else {
        Stop::Never
    }
}

/// The service `service` requires, which Spark's `why` of it says is off,
/// and the words after "which": "it requires db, which is stopped" gives
/// ("db", "is stopped").
fn requirement(service: &Service) -> Option<(&str, &str)> {
    let cause = service.cause.as_deref()?;
    let words = service
        .why
        .as_deref()?
        .strip_prefix("it requires ")?
        .strip_prefix(cause)?
        .strip_prefix(", which ")?;
    Some((cause, words))
}

/// A stopped or disabled service, and the service that stopped it if
/// that is one an explanation follows.
fn stopped(
    document: &Show,
    service: &Service,
    context: &Context,
    chain: &mut Vec<String>,
) -> String {
    let words = match stop_reason(service) {
        Stop::Requirement { cause, words } => {
            let which = match followable(document, cause, chain) {
                Some(next) => clause(document, next, context, chain),
                None => unfollowed(document, cause, words, chain),
            };
            format!("is stopped because it requires {cause}, which {which}")
        }
        Stop::Reason(why) => format!("is stopped because {why}{}", quote(service)),
        Stop::Request(request) => format!(
            "was stopped by {} (process {}) at {}",
            (context.user_name)(request.user),
            request.process,
            (context.when)(request.ms)
        ),
        Stop::End(end) => format!(
            "ended at {} ({end}) and its restart setting, {}, does not start it again{}",
            (context.when)(service.since_ms),
            service.restart,
            quote(service)
        ),
        Stop::Never if service.state == State::Disabled => {
            return String::from("is off at boot, and nothing has started it since boot");
        }
        Stop::Never => String::from("is stopped; nothing has started it since boot"),
    };
    if service.state == State::Disabled {
        format!("is off at boot and {}", words.strip_prefix("is ").unwrap_or(&words))
    } else {
        words
    }
}

/// What follows "which" for the required service `name`, which an
/// explanation does not follow: Spark's own `words` for one the document
/// does not have, which is then the root, and its state for one it has,
/// which the chain holds already or is too long to extend.
fn unfollowed(document: &Show, name: &str, words: &str, chain: &mut Vec<String>) -> String {
    match find(document, name) {
        Some(service) => format!("is {}", service.state.word()),
        None => {
            chain.push(name.to_string());
            words.to_string()
        }
    }
}

/// What to do about the root cause `root` of why service `asked` is as it
/// is: None when nothing needs doing, or nothing is known to help.
fn fix_for(document: &Show, asked: &str, chain: &[String], root: &str) -> Option<String> {
    let Some(last) = find(document, root) else {
        return missing_requirement(document, asked, chain, root);
    };
    if let Some(names) =
        last.why.as_deref().and_then(|why| why.strip_prefix("it waits for itself, through "))
    {
        return Some(format!(
            "Take one of {names} out of another's requires, wants or after (`service edit NAME`)."
        ));
    }
    let alone = root == asked;
    match last.state {
        State::Stopped | State::Disabled => stopped_fix(last, asked, alone),
        State::Failed if alone => Some(format!(
            "Fix what stops {root} (`service {root} logs` shows what it wrote), then `service \
             {asked} start` tries again."
        )),
        State::Failed => Some(format!(
            "Fix what stops {root} (`service {root} logs` shows what it wrote), then `service \
             {asked} start` starts both."
        )),
        State::Waiting => Some(format!(
            "`service {root} logs` shows why it ended; Spark starts it again by itself."
        )),
        State::Running if last.health.as_ref().is_some_and(|h| h.state == HealthState::Failing) => {
            Some(format!("`service {root} logs` shows what it wrote before its check failed."))
        }
        State::Running | State::Done | State::Starting | State::Pending | State::Stopping => None,
    }
}

/// The fix for a root cause that is `last`, stopped or disabled, of why
/// service `asked` is as it is; `alone` when `last` is `asked`.
fn stopped_fix(last: &Service, asked: &str, alone: bool) -> Option<String> {
    let root = &last.name;
    match stop_reason(last) {
        Stop::Reason(why) if last.off_as_meant && LEFT_OFF.contains(&why) => Some(if alone {
            format!("`service {asked} start` starts it in this boot.")
        } else {
            format!("`service {asked} start` starts it and {root} in this boot.")
        }),
        Stop::Reason(why) if ENDS.iter().any(|end| why.starts_with(end)) => {
            Some(restart_after_end(asked, root, alone))
        }
        Stop::End(_) => Some(restart_after_end(asked, root, alone)),
        Stop::Request(_) if alone => Some(format!("`service {asked} start` starts it.")),
        Stop::Request(_) => Some(format!("`service {asked} start` starts it and {root}.")),
        Stop::Never if last.state == State::Disabled => Some(format!(
            "`service {asked} start` starts it now; `service {asked} enable` starts it at each boot \
             too."
        )),
        _ => None,
    }
}

/// The fix for a root cause whose process ended and is not started again.
fn restart_after_end(asked: &str, root: &str, alone: bool) -> String {
    let both = if alone { "it" } else { "both" };
    format!(
        "`service {root} logs` shows why it ended; `service {asked} start` starts {both} again."
    )
}

/// The fix for a root cause `root` that the document has no service for:
/// the service before it in `chain`, which requires it, says in its `why`
/// that `root` has no file, or has one that does not read.
fn missing_requirement(
    document: &Show,
    asked: &str,
    chain: &[String],
    root: &str,
) -> Option<String> {
    let parent = chain.iter().rev().nth(1).and_then(|name| find(document, name))?;
    let why = parent.why.as_deref()?;
    let parent = &parent.name;
    if why.ends_with("has no service file") {
        Some(format!(
            "Add {root}'s service file (`service new {root}`), or take {root} out of {parent}'s \
             requires (`service edit {parent}`)."
        ))
    } else if why.ends_with("has a service file that does not read") {
        Some(format!(
            "`service check {root}` says what is wrong in its file; once it reads, `service \
             {asked} start` starts both."
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::show::{Health, VERSION};

    fn service(name: &str, state: State) -> Service {
        Service { name: name.into(), state, ..Service::default() }
    }

    fn show_with(services: Vec<Service>) -> Show {
        Show {
            show_version: VERSION,
            now_ms: 600_000,
            services,
            shutdown: None,
            trials: Vec::new(),
            undone: Vec::new(),
        }
    }

    fn explained(document: &Show, name: &str) -> Explanation {
        let when = |ms: u64| format!("T{ms}");
        let user_name = |uid: u32| String::from(if uid == 0 { "root" } else { "chase" });
        let context = Context { when: &when, user_name: &user_name };
        explain(document, name, &context).unwrap()
    }

    /// Checks the whole explanation of `name`: its sentence, chain, root
    /// (the last of the chain) and fix.
    fn check(document: &Show, name: &str, sentence: &str, chain: &[&str], fix: Option<&str>) {
        let explanation = explained(document, name);
        assert_eq!(explanation.sentence, sentence);
        assert_eq!(explanation.chain, chain);
        assert_eq!(explanation.root, chain[chain.len() - 1]);
        assert_eq!(explanation.fix.as_deref(), fix);
    }

    fn running_service(name: &str) -> Service {
        Service {
            pid: Some(57),
            since_ms: 470_000,
            ready_ms: Some(480_000),
            ..service(name, State::Running)
        }
    }

    fn stopped_by_chase(name: &str) -> Service {
        Service {
            last_request: Some(Request {
                verb: "stop".into(),
                user: 1000,
                process: 812,
                ms: 420_000,
            }),
            ..service(name, State::Stopped)
        }
    }

    /// A service stopped because it requires `cause`, which `words` say.
    fn requiring(name: &str, cause: &str, words: &str) -> Service {
        Service {
            why: Some(format!("it requires {cause}, which {words}")),
            cause: Some(cause.into()),
            ..service(name, State::Stopped)
        }
    }

    fn failing_health(failures: u32, last_failure: Option<&str>) -> Health {
        Health {
            state: HealthState::Failing,
            failures,
            restart_after: 3,
            last_failure: last_failure.map(Into::into),
            ..Health::default()
        }
    }

    const GAVE_UP: &str = "it failed to start 2 times in a row (the last: it exited with code 0x1)";

    #[test]
    fn a_running_service_with_a_passing_check_says_so() {
        let web = Service {
            health: Some(Health { state: HealthState::Passing, ..Health::default() }),
            ..running_service("web")
        };
        check(
            &show_with(vec![web]),
            "web",
            "web is running as process 57, ready since T480000, and its health check passes.",
            &["web"],
            None,
        );
    }

    #[test]
    fn a_running_service_with_a_failing_check_says_how_often_and_when_it_restarts() {
        let web = Service {
            health: Some(failing_health(2, Some("no /tmp/ok"))),
            ..running_service("web")
        };
        check(
            &show_with(vec![web]),
            "web",
            "web is running as process 57, ready since T480000, but its health check failed 2 \
             times in a row (no /tmp/ok); Spark restarts it after 3 failures in a row.",
            &["web"],
            Some("`service web logs` shows what it wrote before its check failed."),
        );
        let silent = Service { health: Some(failing_health(1, None)), ..running_service("web") };
        let text = explained(&show_with(vec![silent]), "web").sentence;
        assert!(
            text.ends_with("failed 1 time in a row; Spark restarts it after 3 failures in a row."),
            "{text}"
        );
    }

    #[test]
    fn a_check_not_run_yet_says_nothing() {
        let web = Service { health: Some(Health::default()), ..running_service("web") };
        check(
            &show_with(vec![web]),
            "web",
            "web is running as process 57, ready since T480000.",
            &["web"],
            None,
        );
    }

    #[test]
    fn a_running_service_not_ready_yet_and_with_no_process_says_since_when() {
        let web = Service { since_ms: 480_000, ..service("web", State::Running) };
        check(&show_with(vec![web]), "web", "web is running, since T480000.", &["web"], None);
    }

    #[test]
    fn a_setup_step_says_it_is_done() {
        let setup = Service { since_ms: 450_000, ..service("setup", State::Done) };
        check(
            &show_with(vec![setup]),
            "setup",
            "setup ran as a setup step and is done, since T450000.",
            &["setup"],
            None,
        );
    }

    #[test]
    fn a_starting_service_says_what_it_waits_for_and_how_long() {
        let web = Service {
            pid: Some(90),
            since_ms: 596_000,
            ready_when: "listening".into(),
            ready_by_ms: Some(604_000),
            ..service("web", State::Starting)
        };
        check(
            &show_with(vec![web]),
            "web",
            "web is starting as process 90, since T596000, and is ready once it listens within 4 s.",
            &["web"],
            None,
        );
    }

    #[test]
    fn a_starting_service_with_no_process_and_no_time_left_says_so() {
        let web = Service {
            since_ms: 560_000,
            ready_when: "started".into(),
            ready_by_ms: Some(590_000),
            ..service("web", State::Starting)
        };
        check(
            &show_with(vec![web]),
            "web",
            "web is starting, since T560000, and is ready once started within 0 s.",
            &["web"],
            None,
        );
    }

    #[test]
    fn a_pending_service_follows_what_it_waits_for() {
        let web = Service { waiting_for: vec!["db".into()], ..service("web", State::Pending) };
        let db = Service {
            pid: Some(90),
            since_ms: 597_000,
            ready_when: "listening".into(),
            ready_by_ms: Some(607_000),
            ..service("db", State::Starting)
        };
        check(
            &show_with(vec![web, db]),
            "web",
            "web is waiting for db, which is starting as process 90, since T597000, and is ready \
             once it listens within 7 s.",
            &["web", "db"],
            None,
        );
    }

    #[test]
    fn a_pending_service_follows_the_first_it_waits_for_that_the_document_has() {
        let web = Service {
            waiting_for: vec!["ghost".into(), "db".into(), "cache".into()],
            ..service("web", State::Pending)
        };
        let db = Service { since_ms: 590_000, ..service("db", State::Done) };
        let cache = Service { since_ms: 591_000, ..service("cache", State::Done) };
        check(
            &show_with(vec![web, db, cache]),
            "web",
            "web is waiting for ghost, db and cache, which ran as a setup step and is done, since \
             T590000.",
            &["web", "db"],
            None,
        );
    }

    #[test]
    fn a_pending_service_with_nothing_to_wait_for_says_so() {
        let web = service("web", State::Pending);
        check(&show_with(vec![web]), "web", "web is waiting for nothing yet.", &["web"], None);
    }

    #[test]
    fn a_service_waiting_to_start_again_says_when_why_and_how_many_starts_are_left() {
        let web = Service {
            retry_ms: Some(603_000),
            why: Some("it exited with code 0x1".into()),
            last_line: Some("web: cannot reach db".into()),
            give_up_after: 5,
            failures: 2,
            ..service("web", State::Waiting)
        };
        check(
            &show_with(vec![web]),
            "web",
            "web is to start again in 3 s after it exited with code 0x1 (\"web: cannot reach \
             db\"); Spark gives up after 3 more failed starts in a row.",
            &["web"],
            Some("`service web logs` shows why it ended; Spark starts it again by itself."),
        );
    }

    #[test]
    fn a_wait_says_less_when_the_document_says_less() {
        let one_left = Service {
            retry_ms: Some(603_000),
            give_up_after: 3,
            failures: 2,
            ..service("web", State::Waiting)
        };
        let text = explained(&show_with(vec![one_left]), "web").sentence;
        assert_eq!(
            text,
            "web is to start again in 3 s; Spark gives up after 1 more failed start in a row."
        );
        let never_gives_up = service("web", State::Waiting);
        let text = explained(&show_with(vec![never_gives_up]), "web").sentence;
        assert_eq!(text, "web is to start again.");
    }

    #[test]
    fn a_service_given_up_on_says_why_and_what_it_wrote() {
        let db = Service {
            why: Some(GAVE_UP.into()),
            last_line: Some("db: cannot open /shared/db".into()),
            ..service("db", State::Failed)
        };
        check(
            &show_with(vec![db]),
            "db",
            "db was given up on: it failed to start 2 times in a row (the last: it exited with \
             code 0x1) (\"db: cannot open /shared/db\").",
            &["db"],
            Some(
                "Fix what stops db (`service db logs` shows what it wrote), then `service db \
                 start` tries again.",
            ),
        );
    }

    #[test]
    fn a_service_given_up_on_without_a_why_says_how_many_starts_failed() {
        let db = Service { failures: 3, ..service("db", State::Failed) };
        let text = explained(&show_with(vec![db]), "db").sentence;
        assert_eq!(text, "db was given up on after 3 failed starts in a row.");
    }

    #[test]
    fn a_stopping_service_names_its_process_if_it_has_one() {
        let db = Service { pid: Some(90), ..service("db", State::Stopping) };
        let drained = service("cache", State::Stopping);
        let document = show_with(vec![db, drained]);
        check(&document, "db", "db is stopping as process 90.", &["db"], None);
        check(&document, "cache", "cache is stopping.", &["cache"], None);
    }

    #[test]
    fn a_service_stopped_by_a_request_says_who_asked() {
        let document = show_with(vec![stopped_by_chase("db")]);
        check(
            &document,
            "db",
            "db was stopped by chase (process 812) at T420000.",
            &["db"],
            Some("`service db start` starts it."),
        );
    }

    #[test]
    fn a_service_stopped_with_what_it_requires_follows_the_request() {
        let web = requiring("web", "db", "was stopped");
        let document = show_with(vec![web, stopped_by_chase("db")]);
        check(
            &document,
            "web",
            "web is stopped because it requires db, which was stopped by chase (process 812) at \
             T420000.",
            &["web", "db"],
            Some("`service web start` starts it and db."),
        );
    }

    #[test]
    fn a_request_to_start_is_not_what_stopped_a_service() {
        let db = Service {
            last_request: Some(Request { verb: "start".into(), user: 0, process: 7, ms: 400_000 }),
            ..service("db", State::Stopped)
        };
        let text = explained(&show_with(vec![db]), "db").sentence;
        assert_eq!(text, "db is stopped; nothing has started it since boot.");
    }

    #[test]
    fn a_service_stopped_because_it_ended_says_how() {
        let db = Service {
            why: Some("it exited with code 0x1".into()),
            ..service("db", State::Stopped)
        };
        check(
            &show_with(vec![db]),
            "db",
            "db is stopped because it exited with code 0x1.",
            &["db"],
            Some("`service db logs` shows why it ended; `service db start` starts it again."),
        );
    }

    #[test]
    fn what_requires_a_service_that_ended_is_started_with_it() {
        let web = requiring("web", "db", "is stopped");
        for why in ["it was ended by signal 9", "it did not get ready in 10 s: nothing listened"] {
            let db = Service { why: Some(why.into()), ..service("db", State::Stopped) };
            let sentence =
                format!("web is stopped because it requires db, which is stopped because {why}.");
            check(
                &show_with(vec![web.clone(), db]),
                "web",
                &sentence,
                &["web", "db"],
                Some(
                    "`service db logs` shows why it ended; `service web start` starts both \
                     again.",
                ),
            );
        }
    }

    #[test]
    fn a_service_that_ended_and_is_not_restarted_says_what_its_file_says() {
        let db = Service {
            restart: "never".into(),
            since_ms: 500_000,
            last_end: Some("exited with code 0x0".into()),
            last_line: Some("db: done".into()),
            last_request: Some(Request { verb: "start".into(), user: 0, process: 7, ms: 400_000 }),
            ..service("db", State::Stopped)
        };
        check(
            &show_with(vec![db]),
            "db",
            "db ended at T500000 (exited with code 0x0) and its restart setting, never, does not \
             start it again (\"db: done\").",
            &["db"],
            Some("`service db logs` shows why it ended; `service db start` starts it again."),
        );
    }

    #[test]
    fn a_service_nothing_has_started_says_so() {
        let db = service("db", State::Stopped);
        check(
            &show_with(vec![db]),
            "db",
            "db is stopped; nothing has started it since boot.",
            &["db"],
            None,
        );
    }

    #[test]
    fn a_disabled_service_nothing_has_started_says_how_to_start_it_now_and_at_boot() {
        let db = service("db", State::Disabled);
        check(
            &show_with(vec![db]),
            "db",
            "db is off at boot, and nothing has started it since boot.",
            &["db"],
            Some(
                "`service db start` starts it now; `service db enable` starts it at each boot too.",
            ),
        );
    }

    #[test]
    fn a_disabled_service_stopped_by_a_request_says_who_asked() {
        let db = Service { state: State::Disabled, ..stopped_by_chase("db") };
        check(
            &show_with(vec![db]),
            "db",
            "db is off at boot and was stopped by chase (process 812) at T420000.",
            &["db"],
            Some("`service db start` starts it."),
        );
    }

    #[test]
    fn a_disabled_service_says_the_other_reasons_it_is_off_after_that() {
        let ended = Service {
            why: Some("it exited with code 0x1".into()),
            ..service("db", State::Disabled)
        };
        let never = Service {
            restart: "never".into(),
            since_ms: 500_000,
            last_end: Some("exited with code 0x0".into()),
            ..service("cache", State::Disabled)
        };
        let web = Service { state: State::Disabled, ..requiring("web", "db", "is stopped") };
        let document = show_with(vec![ended, never, web]);
        let say = |name| explained(&document, name).sentence;
        assert_eq!(say("db"), "db is off at boot and stopped because it exited with code 0x1.");
        assert_eq!(
            say("cache"),
            "cache is off at boot and ended at T500000 (exited with code 0x0) and its restart \
             setting, never, does not start it again."
        );
        assert_eq!(
            say("web"),
            "web is off at boot and stopped because it requires db, which is off at boot and \
             stopped because it exited with code 0x1."
        );
    }

    #[test]
    fn a_requirement_with_no_service_file_is_the_root_and_the_fix_adds_it() {
        let web = requiring("web", "db", "has no service file");
        check(
            &show_with(vec![web]),
            "web",
            "web is stopped because it requires db, which has no service file.",
            &["web", "db"],
            Some(
                "Add db's service file (`service new db`), or take db out of web's requires \
                 (`service edit web`).",
            ),
        );
    }

    #[test]
    fn a_requirement_whose_file_does_not_read_is_checked() {
        let web = requiring("web", "db", "has a service file that does not read");
        check(
            &show_with(vec![web]),
            "web",
            "web is stopped because it requires db, which has a service file that does not read.",
            &["web", "db"],
            Some(
                "`service check db` says what is wrong in its file; once it reads, `service web \
                 start` starts both.",
            ),
        );
    }

    #[test]
    fn a_missing_service_deeper_in_the_chain_is_taken_out_of_the_service_that_requires_it() {
        let web = requiring("web", "api", "was stopped");
        let api = requiring("api", "db", "has no service file");
        check(
            &show_with(vec![web, api]),
            "web",
            "web is stopped because it requires api, which is stopped because it requires db, \
             which has no service file.",
            &["web", "api", "db"],
            Some(
                "Add db's service file (`service new db`), or take db out of api's requires \
                 (`service edit api`).",
            ),
        );
    }

    #[test]
    fn a_circle_says_which_services_to_take_out_of_each_other() {
        let a = Service {
            why: Some("it waits for itself, through a and b".into()),
            ..service("a", State::Stopped)
        };
        let web = requiring("web", "a", "waits for itself");
        let document = show_with(vec![a, web]);
        let fix = "Take one of a and b out of another's requires, wants or after (`service edit \
                   NAME`).";
        check(
            &document,
            "a",
            "a is stopped because it waits for itself, through a and b.",
            &["a"],
            Some(fix),
        );
        check(
            &document,
            "web",
            "web is stopped because it requires a, which is stopped because it waits for itself, \
             through a and b.",
            &["web", "a"],
            Some(fix),
        );
    }

    #[test]
    fn a_service_left_off_in_rescue_or_essentials_starts_in_this_boot() {
        for why in LEFT_OFF {
            let db = Service {
                why: Some(why.into()),
                off_as_meant: true,
                ..service("db", State::Stopped)
            };
            let web = requiring("web", "db", "is stopped");
            let document = show_with(vec![db, web]);
            check(
                &document,
                "db",
                &format!("db is stopped because {why}."),
                &["db"],
                Some("`service db start` starts it in this boot."),
            );
            check(
                &document,
                "web",
                &format!("web is stopped because it requires db, which is stopped because {why}."),
                &["web", "db"],
                Some("`service web start` starts it and db in this boot."),
            );
        }
    }

    #[test]
    fn a_driver_the_kernel_left_no_cards_needs_nothing_done() {
        let e1000d = Service {
            why: Some("the kernel left it none of the cards it drives".into()),
            off_as_meant: true,
            ..service("e1000d", State::Stopped)
        };
        check(
            &show_with(vec![e1000d]),
            "e1000d",
            "e1000d is stopped because the kernel left it none of the cards it drives.",
            &["e1000d"],
            None,
        );
    }

    #[test]
    fn a_left_off_text_without_the_flag_is_no_service_left_off() {
        let db = Service { why: Some(LEFT_OFF[1].into()), ..service("db", State::Stopped) };
        check(
            &show_with(vec![db]),
            "db",
            "db is stopped because Rescue leaves it off until it is started.",
            &["db"],
            None,
        );
    }

    #[test]
    fn a_chain_three_deep_ends_at_the_service_given_up_on() {
        let web = requiring("web", "api", "was stopped");
        let api = requiring("api", "db", "Spark gave up on");
        let db = Service { why: Some(GAVE_UP.into()), ..service("db", State::Failed) };
        check(
            &show_with(vec![web, api, db]),
            "web",
            "web is stopped because it requires api, which is stopped because it requires db, \
             which was given up on: it failed to start 2 times in a row (the last: it exited with \
             code 0x1).",
            &["web", "api", "db"],
            Some(
                "Fix what stops db (`service db logs` shows what it wrote), then `service web \
                 start` starts both.",
            ),
        );
    }

    #[test]
    fn a_chain_that_loops_stops_where_it_closes_and_names_each_service_once() {
        let a = requiring("a", "b", "was stopped");
        let b = requiring("b", "a", "was stopped");
        check(
            &show_with(vec![a, b]),
            "a",
            "a is stopped because it requires b, which is stopped because it requires a, which is \
             stopped.",
            &["a", "b"],
            None,
        );
    }

    #[test]
    fn a_chain_is_followed_for_no_more_than_sixteen_services() {
        let mut services: Vec<Service> = (0..19)
            .map(|at| requiring(&format!("s{at}"), &format!("s{}", at + 1), "is stopped"))
            .collect();
        services.push(service("s19", State::Stopped));
        let explanation = explained(&show_with(services), "s0");
        let names: Vec<String> = (0..16).map(|at| format!("s{at}")).collect();
        assert_eq!(explanation.chain, names);
        assert_eq!(explanation.root, "s15");
        let mut sentence = String::from("s0");
        for next in 1..=16 {
            sentence.push_str(&format!(" is stopped because it requires s{next}, which"));
        }
        sentence.push_str(" is stopped.");
        assert_eq!(explanation.sentence, sentence);
        assert_eq!(explanation.fix, None);
    }

    #[test]
    fn a_requirement_with_no_cause_is_said_in_sparks_words() {
        let web = Service {
            why: Some("it requires db, which is stopped".into()),
            ..service("web", State::Stopped)
        };
        let db = service("db", State::Stopped);
        check(
            &show_with(vec![web, db]),
            "web",
            "web is stopped because it requires db, which is stopped.",
            &["web"],
            None,
        );
    }

    #[test]
    fn a_file_that_does_not_read_is_said_after_the_sentence() {
        let web =
            Service { complaint: Some("line 3: unknown key".into()), ..running_service("web") };
        check(
            &show_with(vec![web]),
            "web",
            "web is running as process 57, ready since T480000; its file does not read (line 3: \
             unknown key), so Spark runs its last good copy.",
            &["web"],
            None,
        );
    }

    #[test]
    fn a_service_the_document_does_not_have_has_no_explanation() {
        let when = |ms: u64| format!("T{ms}");
        let user_name = |_| String::new();
        let context = Context { when: &when, user_name: &user_name };
        assert_eq!(explain(&show_with(vec![running_service("web")]), "db", &context), None);
    }

    #[test]
    fn the_text_is_the_sentence_and_then_the_fix_if_there_is_one() {
        let explanation = Explanation {
            sentence: "web is stopped.".into(),
            chain: vec!["web".into()],
            root: "web".into(),
            fix: Some("`service web start` starts it.".into()),
        };
        assert_eq!(explanation.text(), "web is stopped. `service web start` starts it.");
        let nothing_to_do = Explanation { fix: None, ..explanation };
        assert_eq!(nothing_to_do.text(), "web is stopped.");
    }

    #[test]
    fn the_json_has_the_sentence_the_chain_the_root_and_the_fix() {
        let web = requiring("web", "db", "was stopped");
        let document = show_with(vec![web, stopped_by_chase("db")]);
        let json = serde_json::to_string(&explained(&document, "web")).unwrap();
        assert_eq!(
            json,
            r#"{"sentence":"web is stopped because it requires db, which was stopped by chase (process 812) at T420000.","chain":["web","db"],"root":"db","fix":"`service web start` starts it and db."}"#
        );
        let running =
            serde_json::to_string(&explained(&show_with(vec![running_service("web")]), "web"));
        assert!(running.unwrap().ends_with(r#""chain":["web"],"root":"web","fix":null}"#));
    }
}
