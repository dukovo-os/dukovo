// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What MCP clients have asked to change, and what became of each
//! request (ADR 0690).
//!
//! A client changes nothing by itself. Its tool `ask` files a request
//! here: the action, its arguments, which were checked as the API
//! checks them, and the client's reason. The request waits for an
//! administrator, who approves it from an unlocked session or refuses
//! it, on the console's page. An approved change runs as that
//! administrator, by the path a click on a page takes, and what it
//! answered is kept here for the client to read.
//!
//! The requests are in admind's memory alone. One that nobody answers
//! lapses after an hour; one that was answered is kept an hour for the
//! client to ask about; all of them go when admind restarts, and the
//! history ([`history`](super::history)) then shows the ones that
//! waited as lapsed. A client has at most 8 waiting and all clients 64,
//! and the same action with the same arguments, asked again while it
//! waits, is the same request.
//!
//! A request's number is unique for as long as the history is kept:
//! [`Requests::starting_after`] begins after the last number there, so
//! that a number a client held before a restart is never another
//! request's after it.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::watch;

use super::super::actions::Registered;
use super::super::errors::{Failure, Kind};

/// The most requests one client has waiting.
pub const WAITING_EACH: usize = 8;
/// The most that wait, of all clients.
pub const WAITING_MAX: usize = 64;
/// How long a request waits for an answer.
pub const LAPSES_AFTER: Duration = Duration::from_secs(3600);
/// How long one that was answered or lapsed is kept, for its client to
/// ask what came of it.
const KEPT_FOR: Duration = Duration::from_secs(3600);
/// The most requests kept that no longer wait; the oldest go first.
const SETTLED_MAX: usize = 256;

/// What became of an approved change.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Its command runs.
    Running,
    /// It ended well, and answered this.
    Done(Value),
    /// It was not run, or did not end well: the error's JSON.
    Failed(Value),
    /// It is long, and runs as the task with this number.
    Task(i64),
}

/// Where a request stands.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// No administrator has answered.
    Waiting,
    /// An administrator approved it.
    Approved {
        /// Who.
        by: String,
        /// What became of the change.
        outcome: Outcome,
    },
    /// An administrator refused it.
    Refused {
        /// Who.
        by: String,
    },
    /// Nobody answered in time, or its client is no more.
    Lapsed {
        /// Why, in words for the client.
        why: &'static str,
    },
}

impl State {
    /// The word the page and the tools say it with.
    pub fn word(&self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Approved { outcome: Outcome::Running, .. } => "approved",
            State::Approved { outcome: Outcome::Done(_), .. } => "done",
            State::Approved { outcome: Outcome::Failed(_), .. } => "failed",
            State::Approved { outcome: Outcome::Task(_), .. } => "task",
            State::Refused { .. } => "refused",
            State::Lapsed { .. } => "lapsed",
        }
    }

    /// Whether nothing more will come of it without the client asking
    /// again: every state but waiting and an approved change that still
    /// runs.
    pub fn settled(&self) -> bool {
        !matches!(self, State::Waiting | State::Approved { outcome: Outcome::Running, .. })
    }
}

/// One request.
#[derive(Clone, Debug)]
pub struct Request {
    /// Its number.
    pub id: u64,
    /// The administrator whose client asked.
    pub owner: String,
    /// The client's name.
    pub client: String,
    /// The change asked for.
    pub action: &'static dyn Registered,
    /// What the change is called with what it is done to, as its task is
    /// named: "Restart a service: nginx".
    pub name: String,
    /// Its arguments, as the client gave them and the action took them.
    pub args: Value,
    /// Why, in the client's own words.
    pub reason: String,
    /// The Unix time it was asked.
    pub asked: i64,
    /// Where it stands.
    pub state: State,
    /// When it was asked, and when its state last changed.
    since: Instant,
}

impl Request {
    /// The Unix time it lapses, while it waits.
    pub fn lapses(&self) -> i64 {
        self.asked.saturating_add(i64::try_from(LAPSES_AFTER.as_secs()).unwrap_or(i64::MAX))
    }

    /// Whether it is `client`'s of `owner`.
    pub fn is_of(&self, owner: &str, client: &str) -> bool {
        self.owner == owner && self.client == client
    }

    /// What the tools tell its client of it.
    pub fn told(&self) -> Value {
        let mut told = json!({
            "request": self.id,
            "action": self.action.id(),
            "state": self.state.word(),
            "asked": self.asked,
        });
        match &self.state {
            State::Waiting => {
                told["lapses"] = self.lapses().into();
                told["note"] = format!(
                    "Waiting for an administrator's yes in the console (request {}).",
                    self.id
                )
                .into();
            }
            State::Approved { by, outcome } => {
                told["by"] = by.as_str().into();
                match outcome {
                    Outcome::Running => {
                        told["note"] = "Approved: the change runs now.".into();
                    }
                    Outcome::Done(answer) => told["answer"] = answer.clone(),
                    Outcome::Failed(error) => told["error"] = error["error"].clone(),
                    Outcome::Task(task) => told["task"] = (*task).into(),
                }
            }
            State::Refused { by } => {
                told["by"] = by.as_str().into();
                told["note"] = "An administrator refused this request. Nothing was changed.".into();
            }
            State::Lapsed { why } => {
                told["note"] = format!("{why} Nothing was changed.").into();
            }
        }
        told
    }
}

/// A new request, before it has a number.
pub struct Asking<'a> {
    /// The administrator whose client asks.
    pub owner: &'a str,
    /// The client's name.
    pub client: &'a str,
    /// The change.
    pub action: &'static dyn Registered,
    /// What it is called with what it is done to.
    pub name: String,
    /// Its arguments.
    pub args: Value,
    /// Why.
    pub reason: &'a str,
}

/// Every request admind holds.
pub struct Requests {
    /// The number the next one takes.
    next: u64,
    /// The requests, oldest first.
    all: Vec<Request>,
    /// The requests that lapsed since [`Requests::take_lapsed`] was last
    /// asked, as they were then: the history has yet to hear of them.
    lapsed: Vec<Request>,
    /// Sent to whenever a request is filed or changes its state, with
    /// how often that has happened: a client's `ask` waits on it, and
    /// the tabs' event streams tell the pages when it has moved.
    news: watch::Sender<u64>,
}

impl Default for Requests {
    fn default() -> Requests {
        Requests::starting_after(0)
    }
}

impl Requests {
    /// No requests, the first of which takes the number after `last`.
    pub fn starting_after(last: u64) -> Requests {
        Requests {
            next: last.saturating_add(1),
            all: Vec::new(),
            lapsed: Vec::new(),
            news: watch::channel(0).0,
        }
    }

    /// Begins after `last` if no request was filed yet: for the history,
    /// which is read after this is made.
    pub fn start_after(&mut self, last: u64) {
        if self.all.is_empty() {
            self.next = self.next.max(last.saturating_add(1));
        }
    }

    /// What is told when a request is filed or changes.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.news.subscribe()
    }

    /// How often the requests have changed, and how many wait now.
    pub fn told(&self) -> (u64, usize) {
        (*self.news.borrow(), self.waiting().count())
    }

    /// Tells whoever waits that something changed.
    fn tell(&self) {
        self.news.send_modify(|times| *times = times.wrapping_add(1));
    }

    /// The requests that wait, oldest first.
    pub fn waiting(&self) -> impl Iterator<Item = &Request> {
        self.all.iter().filter(|request| request.state == State::Waiting)
    }

    /// The request `id`, if admind still holds it.
    pub fn get(&self, id: u64) -> Option<&Request> {
        self.all.iter().find(|request| request.id == id)
    }

    /// Files the request `asking` at `now`, the Unix time `unix`, and
    /// gives its number and whether it is new: the same client asking
    /// for the same action with the same arguments while its request
    /// waits is given that request again, with the reason it first
    /// gave.
    ///
    /// # Errors
    ///
    /// ADM-146 when the client has [`WAITING_EACH`] requests waiting, or
    /// all clients [`WAITING_MAX`].
    pub fn ask(
        &mut self,
        asking: Asking<'_>,
        now: Instant,
        unix: i64,
    ) -> Result<(u64, bool), Failure> {
        self.lapse(now);
        let Asking { owner, client, action, name, args, reason } = asking;
        let same = self.waiting().find(|request| {
            request.is_of(owner, client)
                && request.action.id() == action.id()
                && request.args == args
        });
        if let Some(same) = same {
            return Ok((same.id, false));
        }
        let own = self.waiting().filter(|request| request.is_of(owner, client)).count();
        if own >= WAITING_EACH {
            let why = format!(
                "{WAITING_EACH} requests of this AI client's wait for an administrator already"
            );
            return Err(Failure::new(Kind::TooManyRequests, why));
        }
        if self.waiting().count() >= WAITING_MAX {
            let why =
                format!("{WAITING_MAX} requests of AI clients wait for an administrator already");
            return Err(Failure::new(Kind::TooManyRequests, why));
        }
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.all.push(Request {
            id,
            owner: owner.into(),
            client: client.into(),
            action,
            name,
            args,
            reason: reason.into(),
            asked: unix,
            state: State::Waiting,
            since: now,
        });
        self.tell();
        Ok((id, true))
    }

    /// The request `id`, which must wait, for an administrator's answer.
    fn waiting_mut(&mut self, id: u64, now: Instant) -> Result<&mut Request, Failure> {
        self.lapse(now);
        let none = || Failure::new(Kind::NoRequest, format!("there is no request {id}"));
        let request = self.all.iter_mut().find(|request| request.id == id).ok_or_else(none)?;
        match &request.state {
            State::Waiting => Ok(request),
            State::Approved { by, .. } => Err(Failure::new(
                Kind::NoRequest,
                format!("request {id} was approved already, by {by}"),
            )),
            State::Refused { by } => Err(Failure::new(
                Kind::NoRequest,
                format!("request {id} was refused already, by {by}"),
            )),
            State::Lapsed { .. } => {
                Err(Failure::new(Kind::NoRequest, format!("request {id} has lapsed")))
            }
        }
    }

    /// The request `id` as it waits, for an administrator's session to
    /// check before it answers.
    ///
    /// # Errors
    ///
    /// ADM-145 when there is no such request, or it no longer waits.
    pub fn waits(&mut self, id: u64, now: Instant) -> Result<Request, Failure> {
        self.waiting_mut(id, now).map(|request| request.clone())
    }

    /// Notes that `by` approved the request `id`, whose change now
    /// runs, and gives the request as it stands.
    ///
    /// # Errors
    ///
    /// ADM-145 when there is no such request, or it no longer waits.
    pub fn approve(&mut self, id: u64, by: &str, now: Instant) -> Result<Request, Failure> {
        let request = self.waiting_mut(id, now)?;
        request.state = State::Approved { by: by.into(), outcome: Outcome::Running };
        request.since = now;
        let approved = request.clone();
        self.tell();
        Ok(approved)
    }

    /// Notes what became of the approved change of the request `id`.
    pub fn finish(&mut self, id: u64, outcome: Outcome, now: Instant) {
        let Some(request) = self.all.iter_mut().find(|request| request.id == id) else { return };
        if let State::Approved { outcome: was, .. } = &mut request.state {
            *was = outcome;
            request.since = now;
            self.tell();
        }
    }

    /// Notes that `by` refused the request `id`, and gives the request.
    ///
    /// # Errors
    ///
    /// ADM-145 when there is no such request, or it no longer waits.
    pub fn refuse(&mut self, id: u64, by: &str, now: Instant) -> Result<Request, Failure> {
        let request = self.waiting_mut(id, now)?;
        request.state = State::Refused { by: by.into() };
        request.since = now;
        let refused = request.clone();
        self.tell();
        Ok(refused)
    }

    /// Lapses every waiting request of `client` of `owner`, which is no
    /// more or no longer reaches what it asked for.
    pub fn lapse_of(&mut self, owner: &str, client: &str, now: Instant) {
        let why = "The AI client was removed, or no longer reaches this.";
        self.lapse_where(now, why, |request| request.is_of(owner, client));
    }

    /// Lapses the requests that have waited [`LAPSES_AFTER`], and forgets
    /// the ones settled for [`KEPT_FOR`] or past [`SETTLED_MAX`].
    pub fn lapse(&mut self, now: Instant) {
        let why = "No administrator answered within an hour.";
        self.lapse_where(now, why, |request| now.duration_since(request.since) >= LAPSES_AFTER);
        self.all.retain(|request| {
            !request.state.settled() || now.duration_since(request.since) < KEPT_FOR
        });
        let settled = self.all.iter().filter(|request| request.state.settled()).count();
        let mut over = settled.saturating_sub(SETTLED_MAX);
        self.all.retain(|request| {
            let goes = over > 0 && request.state.settled();
            over -= usize::from(goes);
            !goes
        });
    }

    /// Lapses, for `why`, the waiting requests that `which` picks.
    fn lapse_where(&mut self, now: Instant, why: &'static str, which: impl Fn(&Request) -> bool) {
        let before = self.lapsed.len();
        for request in &mut self.all {
            if request.state == State::Waiting && which(request) {
                request.state = State::Lapsed { why };
                request.since = now;
                self.lapsed.push(request.clone());
            }
        }
        if self.lapsed.len() > before {
            self.tell();
        }
    }

    /// The requests that lapsed since this was last asked, for the
    /// history.
    pub fn take_lapsed(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.lapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::actions::tests::LAB;

    const HOUR: Duration = Duration::from_secs(3600);

    fn asking<'a>(owner: &'a str, client: &'a str, name: &str) -> Asking<'a> {
        Asking {
            owner,
            client,
            action: LAB[0],
            name: format!("{}: {name}", LAB[0].title()),
            args: json!({ "name": name }),
            reason: "it stopped answering",
        }
    }

    #[test]
    fn a_request_gets_the_next_number_and_waits() {
        let mut requests = Requests::starting_after(41);
        let mut news = requests.subscribe();
        let now = Instant::now();
        let (id, new) =
            requests.ask(asking("chase", "helper", "nginx"), now, 1_800_000_000).unwrap();
        assert_eq!((id, new), (42, true));
        assert!(news.has_changed().unwrap());
        news.mark_unchanged();
        let request = requests.get(42).expect("the request");
        assert_eq!(request.state, State::Waiting);
        assert_eq!(request.lapses(), 1_800_003_600);
        assert_eq!(
            request.told(),
            json!({
                "request": 42,
                "action": "lab.restart",
                "state": "waiting",
                "asked": 1_800_000_000,
                "lapses": 1_800_003_600,
                "note": "Waiting for an administrator's yes in the console (request 42).",
            })
        );
        assert_eq!(requests.told(), (1, 1));
        // The same asked again is the same request, and no news.
        let again = requests.ask(asking("chase", "helper", "nginx"), now, 1_800_000_009).unwrap();
        assert_eq!(again, (42, false));
        assert!(!news.has_changed().unwrap());
        // Other arguments, another client and another owner's client of
        // the same name are other requests.
        assert_eq!(
            requests.ask(asking("chase", "helper", "postgres"), now, 0).unwrap(),
            (43, true)
        );
        assert_eq!(requests.ask(asking("chase", "other", "nginx"), now, 0).unwrap(), (44, true));
        assert_eq!(requests.ask(asking("alice", "helper", "nginx"), now, 0).unwrap(), (45, true));
    }

    #[test]
    fn the_first_number_follows_the_history_s_last() {
        let mut requests = Requests::default();
        requests.start_after(7);
        let now = Instant::now();
        assert_eq!(requests.ask(asking("chase", "helper", "a"), now, 0).unwrap().0, 8);
        // Once one is filed, the numbers only go on.
        requests.start_after(100);
        assert_eq!(requests.ask(asking("chase", "helper", "b"), now, 0).unwrap().0, 9);
    }

    #[test]
    fn a_client_and_all_clients_have_a_most_that_wait() {
        let mut requests = Requests::default();
        let now = Instant::now();
        for n in 0..WAITING_EACH {
            requests.ask(asking("chase", "helper", &format!("s{n}")), now, 0).unwrap();
        }
        let full = requests.ask(asking("chase", "helper", "one-more"), now, 0).unwrap_err();
        assert_eq!(full.kind, Kind::TooManyRequests);
        assert!(full.detail.starts_with("8 requests of this AI client's"), "{}", full.detail);
        // One that is answered makes room.
        requests.refuse(1, "alice", now).unwrap();
        assert!(requests.ask(asking("chase", "helper", "one-more"), now, 0).is_ok());

        let mut requests = Requests::default();
        for n in 0..WAITING_MAX {
            let client = format!("c{}", n / WAITING_EACH);
            requests.ask(asking("chase", &client, &format!("s{n}")), now, 0).unwrap();
        }
        let full = requests.ask(asking("alice", "new", "x"), now, 0).unwrap_err();
        assert_eq!(full.kind, Kind::TooManyRequests);
        assert!(full.detail.starts_with("64 requests of AI clients"), "{}", full.detail);
    }

    #[test]
    fn an_approved_request_tells_what_became_of_its_change() {
        let mut requests = Requests::default();
        let now = Instant::now();
        let (id, _) = requests.ask(asking("chase", "helper", "nginx"), now, 5).unwrap();
        let approved = requests.approve(id, "alice", now).unwrap();
        assert_eq!(approved.state.word(), "approved");
        assert!(!approved.state.settled());
        assert_eq!(requests.get(id).unwrap().told()["note"], "Approved: the change runs now.");

        let answer = json!({ "done": true, "changed": true, "output": ["restarted"] });
        requests.finish(id, Outcome::Done(answer.clone()), now);
        let told = requests.get(id).unwrap().told();
        assert_eq!(
            (&told["state"], &told["by"], &told["answer"]),
            (&json!("done"), &json!("alice"), &answer)
        );
        assert!(requests.get(id).unwrap().state.settled());

        let (failed, _) = requests.ask(asking("chase", "helper", "broken"), now, 5).unwrap();
        requests.approve(failed, "alice", now).unwrap();
        let error = Failure::new(Kind::CommandFailed, "broken.conf did not load").body();
        requests.finish(failed, Outcome::Failed(error), now);
        let told = requests.get(failed).unwrap().told();
        assert_eq!(told["state"], "failed");
        assert_eq!(told["error"]["id"], "ADM-123");

        let (long, _) = requests.ask(asking("chase", "helper", "long"), now, 5).unwrap();
        requests.approve(long, "alice", now).unwrap();
        requests.finish(long, Outcome::Task(12), now);
        let told = requests.get(long).unwrap().told();
        assert_eq!((&told["state"], &told["task"]), (&json!("task"), &json!(12)));
    }

    #[test]
    fn a_request_is_answered_once() {
        let mut requests = Requests::default();
        let now = Instant::now();
        let (first, _) = requests.ask(asking("chase", "helper", "a"), now, 0).unwrap();
        let (second, _) = requests.ask(asking("chase", "helper", "b"), now, 0).unwrap();
        requests.approve(first, "alice", now).unwrap();
        requests.refuse(second, "chase", now).unwrap();
        for (id, why) in
            [(first, "approved already, by alice"), (second, "refused already, by chase")]
        {
            for again in [requests.approve(id, "eve", now), requests.refuse(id, "eve", now)] {
                let failure = again.unwrap_err();
                assert_eq!(failure.kind, Kind::NoRequest);
                assert!(failure.detail.contains(why), "{}", failure.detail);
            }
        }
        assert_eq!(
            requests.approve(99, "alice", now).unwrap_err().detail,
            "there is no request 99"
        );
        // A refusal says so to the client, and that nothing changed.
        let told = requests.get(second).unwrap().told();
        assert_eq!((&told["state"], &told["by"]), (&json!("refused"), &json!("chase")));
        assert!(told["note"].as_str().unwrap().ends_with("Nothing was changed."));
        // A finish for a request that was not approved is nothing.
        requests.finish(second, Outcome::Task(1), now);
        assert_eq!(requests.get(second).unwrap().state.word(), "refused");
    }

    #[test]
    fn a_request_lapses_after_an_hour_and_is_forgotten_an_hour_later() {
        let mut requests = Requests::default();
        let start = Instant::now();
        let (old, _) = requests.ask(asking("chase", "helper", "a"), start, 0).unwrap();
        let later = start + HOUR - Duration::from_secs(1);
        let (new, _) = requests.ask(asking("chase", "helper", "b"), later, 0).unwrap();
        requests.lapse(later);
        assert!(requests.take_lapsed().is_empty());
        requests.lapse(start + HOUR);
        let lapsed: Vec<u64> = requests.take_lapsed().iter().map(|request| request.id).collect();
        assert_eq!(lapsed, [old]);
        // The history is told of each once.
        assert!(requests.take_lapsed().is_empty());
        let told = requests.get(old).unwrap().told();
        assert_eq!(told["state"], "lapsed");
        assert_eq!(told["note"], "No administrator answered within an hour. Nothing was changed.");
        assert_eq!(
            requests.approve(old, "alice", start + HOUR).unwrap_err().detail,
            "request 1 has lapsed"
        );
        assert_eq!(requests.get(new).unwrap().state, State::Waiting);
        // The same asked again after it lapsed is a new request.
        let (again, fresh) = requests.ask(asking("chase", "helper", "a"), start + HOUR, 0).unwrap();
        assert!(fresh && again != old);

        requests.lapse(start + HOUR * 2);
        assert!(requests.get(old).is_none());
        assert_eq!(requests.get(new).unwrap().state.word(), "lapsed");
    }

    #[test]
    fn a_client_that_is_no_more_has_its_waiting_requests_lapsed() {
        let mut requests = Requests::default();
        let now = Instant::now();
        let (first, _) = requests.ask(asking("chase", "helper", "a"), now, 0).unwrap();
        let (second, _) = requests.ask(asking("chase", "helper", "b"), now, 0).unwrap();
        let (kept, _) = requests.ask(asking("chase", "other", "a"), now, 0).unwrap();
        requests.approve(second, "alice", now).unwrap();
        let mut news = requests.subscribe();
        requests.lapse_of("chase", "helper", now);
        let lapsed: Vec<u64> = requests.take_lapsed().iter().map(|request| request.id).collect();
        assert_eq!(lapsed, [first]);
        assert!(news.has_changed().unwrap());
        news.mark_unchanged();
        assert_eq!(requests.get(first).unwrap().state.word(), "lapsed");
        // What was approved stays so, and another client's waits on.
        assert_eq!(requests.get(second).unwrap().state.word(), "approved");
        assert_eq!(requests.get(kept).unwrap().state, State::Waiting);
        requests.lapse_of("chase", "helper", now);
        assert!(requests.take_lapsed().is_empty());
        assert!(!news.has_changed().unwrap());
    }

    #[test]
    fn settled_requests_are_kept_up_to_a_most() {
        let mut requests = Requests::default();
        let now = Instant::now();
        for n in 0..SETTLED_MAX + 3 {
            let (id, _) =
                requests.ask(asking("chase", "helper", &format!("s{n}")), now, 0).unwrap();
            requests.refuse(id, "alice", now).unwrap();
        }
        let (waits, _) = requests.ask(asking("chase", "helper", "waits"), now, 0).unwrap();
        requests.lapse(now);
        // The oldest three went; the one that waits is not counted.
        assert!((1..=3).all(|id| requests.get(id).is_none()));
        assert!(requests.get(4).is_some());
        assert_eq!(requests.get(waits).unwrap().state, State::Waiting);
    }
}
