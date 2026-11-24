// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the console's AI clients page asks admind itself (ADR 0690): the
//! areas a client can be given, the requests that wait for a yes, the
//! yes and the no, and what the clients did.
//!
//! They are routes, not actions: the requests and the history are
//! admind's own state, which no command reads or changes, so they have
//! no command to show and no Ansible task, and an API token is refused,
//! as on every route of the table. A person's own clients are authd's to
//! keep, and are listed, added and removed through the person's worker
//! ([`account`](super::account)); every administrator's are the actions
//! `console.clients` and `console.client-remove`.
//!
//! All of them are an administrator's. To approve, the session must be
//! unlocked as well, as for any change that runs as an administrator:
//! the change then runs as the administrator who approved it, by the
//! path a click on a page takes ([`act::start`]), with every guard of
//! that path, and the journal says whose client asked. To refuse, and to
//! look, a locked session is enough.
//!
//! A yes is checked against authd first: the client may have been
//! removed since it asked, or made again with less, and its requests
//! then lapse and nothing runs.

use std::rc::Rc;
use std::time::Instant;

use http::StatusCode;
use serde_json::{Value, json};

use super::act::{self, Started};
use super::{Context, Live, Outcome, Reply, shown, unlocked};
use crate::admind::actions::show;
use crate::admind::audit;
use crate::admind::errors::{Failure, Kind};
use crate::admind::mcp::offer::Offer;
use crate::admind::mcp::requests::{self, Request};
use crate::admind::mcp::{self, areas, client};
use crate::admind::shared::Shared;
use crate::admind::who::{self, Account};

/// The most entries of the history a page is sent, the newest.
const HISTORY_SHOWN: usize = 200;

/// Checks that `live` is an administrator's session.
///
/// # Errors
///
/// ADM-122 for one that is not.
fn administrator(cx: &Context<'_>, live: &Live) -> Result<(), Failure> {
    let who = who::read(&cx.shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    if who.admin {
        return Ok(());
    }
    let why = format!(
        "{} is not an administrator, and AI clients are an administrator's to let in",
        shown(&live.name)
    );
    Err(Failure::new(Kind::NotAdmin, why))
}

/// The number the address gives as `{id}`.
///
/// # Errors
///
/// ADM-145 for what is not a number a request could have.
fn number(cx: &Context<'_>) -> Result<u64, Failure> {
    cx.params
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<u64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| Failure::new(Kind::NoRequest, "that is not a request's number"))
}

/// `GET /api/1/clients/areas`: every area with what a client can be
/// given in it, for the dialog that adds a client.
pub fn areas(cx: &Context<'_>, live: &Live) -> Outcome {
    administrator(cx, live)?;
    Ok(Reply::json(areas::listed(cx.shared.actions.get())))
}

/// What the page shows of a request that waits: who asked, for what and
/// why, and the command a yes would run.
fn card(request: &Request, origin: &str) -> Value {
    // The arguments were checked when the request was filed, so the
    // command is there; without one, a yes is refused and says why.
    let shown = request
        .action
        .prepare(request.args.clone())
        .map_or_else(|_| Value::default(), |prepared| show::shown(&prepared, origin));
    json!({
        "request": request.id,
        "owner": request.owner,
        "client": request.client,
        "action": request.action.id(),
        "title": request.name,
        "area": request.action.area(),
        "args": request.args,
        "reason": request.reason,
        "asked": request.asked,
        "lapses": request.lapses(),
        "command": shown["command"],
        "files": shown["files"],
    })
}

/// Whether the client of `request` may still ask for what it asked for:
/// not when it was removed, or ended, or was made again with less.
///
/// # Errors
///
/// ADM-111 when authd does not answer.
async fn still_asks(shared: &Shared, request: &Request) -> Result<bool, Failure> {
    match client::reach_now(shared, &request.owner, &request.client).await {
        Ok(reach) => Ok(reach.allows(request.action).is_ok_and(|offer| offer == Offer::Ask)),
        Err(failure) if failure.kind == Kind::ClientRefused => Ok(false),
        Err(failure) => Err(failure),
    }
}

/// Lapses the waiting requests of each client that may no longer ask
/// for one of them. A client removed from this console lapses with the
/// removal; one removed at a terminal, or by another administrator's
/// `console.client-remove`, is a command's doing that admind hears
/// nothing of, so the page finds it out here, before it shows a card
/// for a client that is gone. When authd does not answer, the requests
/// stay as they are, and a yes asks it again.
async fn lapse_gone(shared: &Shared) {
    let waiting: Vec<Request> = shared.client_requests.borrow().waiting().cloned().collect();
    let mut gone: Vec<(&str, &str)> = Vec::new();
    for request in &waiting {
        let of = (request.owner.as_str(), request.client.as_str());
        if !gone.contains(&of) && matches!(still_asks(shared, request).await, Ok(false)) {
            gone.push(of);
        }
    }
    let now = Instant::now();
    for (owner, name) in gone {
        shared.client_requests.borrow_mut().lapse_of(owner, name, now);
    }
}

/// `GET /api/1/clients/requests`: the requests that wait, the oldest
/// first.
pub async fn requests(cx: &Context<'_>, live: &Live) -> Outcome {
    administrator(cx, live)?;
    lapse_gone(cx.shared).await;
    mcp::lapse(cx.shared, Instant::now());
    let held = cx.shared.client_requests.borrow();
    let waiting: Vec<Value> = held.waiting().map(|request| card(request, &cx.origin)).collect();
    Ok(Reply::json(json!({ "requests": waiting })))
}

/// `GET /api/1/clients/history`: what the clients did, the newest first:
/// each change asked for with what became of it, and each client's reads
/// joined into runs.
pub fn history(cx: &Context<'_>, live: &Live) -> Outcome {
    administrator(cx, live)?;
    mcp::lapse(cx.shared, Instant::now());
    let history = cx.shared.client_history.borrow();
    Ok(Reply::json(json!({ "entries": history.newest(HISTORY_SHOWN) })))
}

/// Notes what became of the approved change of the request `id`.
fn settle(shared: &Shared, id: u64, outcome: requests::Outcome) {
    shared.client_requests.borrow_mut().finish(id, outcome, Instant::now());
    mcp::moved(shared, id);
}

/// `POST /api/1/clients/requests/{id}/approve`: runs the change the
/// request asks for, as the administrator who approves it, and answers
/// as the change's own route would: what it did, or 202 with the task of
/// a long one.
///
/// # Errors
///
/// ADM-122 for one who is not an administrator; ADM-106 while the
/// session is locked; ADM-145 for a request that does not wait, or whose
/// client is gone or no longer reaches the change; and whatever the
/// change itself is refused or fails with, which the client is told too.
pub async fn approve(cx: &Context<'_>, live: Live) -> Outcome {
    let id = number(cx)?;
    administrator(cx, &live)?;
    if !unlocked(cx.shared, &live.cookie) {
        let why = "the session is locked: unlock it to approve a request";
        return Err(Failure::new(Kind::Locked, why));
    }
    let shared = cx.shared;
    mcp::lapse(shared, Instant::now());
    let request = shared.client_requests.borrow_mut().waits(id, Instant::now())?;
    let (owner, name) = (request.owner.as_str(), request.client.as_str());
    if !still_asks(shared, &request).await? {
        shared.client_requests.borrow_mut().lapse_of(owner, name, Instant::now());
        mcp::lapse(shared, Instant::now());
        let why = format!(
            "the AI client {} of {} was removed, or no longer reaches {}: its requests have \
             lapsed, and nothing was changed",
            shown(name),
            shown(owner),
            request.action.id()
        );
        return Err(Failure::new(Kind::NoRequest, why));
    }
    let prepared = request.action.prepare(request.args.clone())?;
    // authd was asked in between: another administrator may have
    // answered meanwhile, and only one yes runs the change.
    shared.client_requests.borrow_mut().approve(id, &live.name, Instant::now())?;
    mcp::moved(shared, id);
    audit(&format!(
        "{} approved request {id} of the AI client \"{}\" of {}: {}",
        shown(&live.name),
        shown(name),
        shown(owner),
        request.action.id()
    ));
    let how = format!("client {name} of {owner}");
    match act::start(cx, prepared, Some(live), None, Some(how)).await {
        Err(failure) => {
            settle(shared, id, requests::Outcome::Failed(failure.body()));
            Err(failure)
        }
        Ok(Started::Task(task)) => {
            settle(shared, id, requests::Outcome::Task(task));
            Ok(Reply { status: StatusCode::ACCEPTED, ..Reply::json(json!({ "task": task })) })
        }
        Ok(Started::Job(job)) => {
            // Noted by a task of its own: the client hears what became
            // of its request even when the browser that said yes has
            // gone before the command ended.
            let shared = Rc::clone(shared);
            let noted = tokio::task::spawn_local(async move {
                let answer = job.await.unwrap_or_else(|_| Err(not_ended()));
                let outcome = match &answer {
                    Ok(answer) => requests::Outcome::Done(answer.clone()),
                    Err(failure) => requests::Outcome::Failed(failure.body()),
                };
                settle(&shared, id, outcome);
                answer
            });
            Ok(Reply::json(noted.await.unwrap_or_else(|_| Err(not_ended()))?))
        }
    }
}

/// The failure of a change whose task went without an answer.
fn not_ended() -> Failure {
    Failure::new(Kind::Internal, "the action's task did not end")
}

/// `POST /api/1/clients/requests/{id}/refuse`: says no to the request.
/// Nothing runs, and the client is told who refused.
///
/// # Errors
///
/// ADM-122 for one who is not an administrator; ADM-145 for a request
/// that does not wait.
pub fn refuse(cx: &Context<'_>, live: &Live) -> Outcome {
    let id = number(cx)?;
    administrator(cx, live)?;
    let refused = cx.shared.client_requests.borrow_mut().refuse(id, &live.name, Instant::now())?;
    mcp::moved(cx.shared, id);
    audit(&format!(
        "{} refused request {id} of the AI client \"{}\" of {}: {}",
        shown(&live.name),
        shown(&refused.client),
        shown(&refused.owner),
        refused.action.id()
    ));
    Ok(Reply::json(json!({ "done": true })))
}
