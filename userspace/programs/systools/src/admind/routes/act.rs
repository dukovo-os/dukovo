// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An action's route (ADR 0515): its arguments from the address and the
//! body, checked; whether the caller may ask for it; its command, run as
//! the person or as an administrator; and the answer.
//!
//! The showing of an action, which runs nothing, is here too: [`show`].
//!
//! A long action (ADR 0516) is not run in the request: it starts a task,
//! which [`tasks`](crate::admind::tasks) runs, and the answer is 202 with
//! the task's number.
//!
//! The log has a line for each change asked for, and for each read that
//! was refused or failed: who asked, from where, the action and how it
//! ended. Arguments that do not read are refused before anyone is asked,
//! and have no line. A change that runs also has a row in the journal when
//! it ends (a long one, when its task does).

use std::net::IpAddr;
use std::rc::Rc;

use accounts::console::Elevator;
use http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Context, Live, Outcome, Reply, Text, shown};
use crate::admind::actions::show;
use crate::admind::actions::{self, Caller, Params, Prepared, Registered, Runs};
use crate::admind::caller::{self, TokenCall};
use crate::admind::errors::{Failure, Kind};
use crate::admind::journal::Ended;
use crate::admind::log;
use crate::admind::run::{self, Ran};
use crate::admind::shared::{Shared, unix_now};
use crate::admind::tasks;
use crate::admind::who::{self, Account};

/// Who asks for a change, as the journal names them.
pub(super) struct Asker {
    /// The person: the session's, or the token's owner.
    pub who: String,
    /// `console`, or `token NAME`.
    pub how: String,
}

/// Answers `action` at the address that gave `params`, for the session
/// `live` or the token `token`, one of which the request has.
///
/// The change runs in a task of its own, which the answer waits for: a
/// browser that goes away while it runs does not end it half way, with its
/// command hung up and no row in the journal.
pub async fn action(
    cx: &Context<'_>,
    action: &'static dyn Registered,
    params: Params,
    live: Option<Live>,
    token: Option<TokenCall>,
) -> Outcome {
    let args = actions::arguments(params, &cx.body)?;
    let prepared = action.prepare(args)?;
    let (asker, whose) = match (&live, &token) {
        (Some(live), None) => {
            (Asker { who: live.name.clone(), how: "console".into() }, shown(&live.name))
        }
        (None, Some(token)) => (
            Asker { who: token.owner.clone(), how: format!("token {}", token.name) },
            format!("{} (API token {})", shown(&token.owner), shown(&token.name)),
        ),
        // `check` lets an action through with exactly one of them.
        _ => return Err(super::ended()),
    };
    let (shared, peer) = (Rc::clone(cx.shared), cx.peer);
    if prepared.action.long() {
        return long(&shared, peer, prepared, &asker, &whose, live.as_ref(), token.as_ref());
    }
    let job = tokio::task::spawn_local(async move {
        let begun = prepared
            .action
            .changes()
            .then(|| shared.journal.begin(shared.store.as_ref(), &prepared.files, unix_now()));
        let ran = match (&live, &token) {
            (Some(live), _) => for_session(&shared, &prepared, live).await,
            (None, Some(token)) => for_token(&shared, &prepared, token).await,
            (None, None) => Err(super::ended()),
        };
        said(peer, prepared.action, &whose, &ran);
        if let Some(begun) = begun {
            match run::ending(&ran) {
                Some((status, note)) => {
                    let ended = Ended {
                        who: &asker.who,
                        how: &asker.how,
                        action: prepared.action.id(),
                        argv: &prepared.argv,
                        status,
                        task: None,
                        note,
                    };
                    begun.end(shared.store.as_ref(), &ended, unix_now());
                }
                None => begun.cancel(),
            }
        }
        run::answer(&prepared, &ran?)
    });
    match job.await {
        Ok(answer) => Ok(Reply::json(answer?)),
        Err(_) => Err(Failure::new(Kind::Internal, "the action's task did not end")),
    }
}

/// Checks that the session `live` may ask for `prepared`: its person is
/// an administrator and the session unlocked when the action runs as an
/// administrator. The person's own rights decide the rest.
///
/// # Errors
///
/// As [`actions::authorize`].
fn permit_session(shared: &Rc<Shared>, prepared: &Prepared, live: &Live) -> Result<(), Failure> {
    let who = who::read(&shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let unlocked = super::unlocked(shared, &live.cookie);
    let caller = Caller::Session { name: &live.name, admin: who.admin, unlocked };
    actions::authorize(&caller, prepared.action)
}

/// Checks that the API token `token` may ask for `prepared`.
///
/// # Errors
///
/// As [`actions::authorize`].
fn permit_token(prepared: &Prepared, token: &TokenCall) -> Result<(), Failure> {
    let caller = Caller::Token { name: &token.name, scopes: &token.scopes };
    actions::authorize(&caller, prepared.action)
}

/// The right an administrator's command runs by for the session `live`.
fn session_elevator(live: &Live) -> Elevator {
    Elevator::Session(live.authd.clone())
}

/// The right an administrator's command runs by for the API token
/// `token`, which authd checks the scopes of for the action's area again.
fn token_elevator(prepared: &Prepared, token: &TokenCall) -> Elevator {
    Elevator::Token {
        token: token.token.bytes().to_vec(),
        area: prepared.action.area().to_string(),
    }
}

/// Runs `prepared` for the session `live`: as the person, or as an
/// administrator when the person is one and the session is unlocked.
async fn for_session(
    shared: &Rc<Shared>,
    prepared: &Prepared,
    live: &Live,
) -> Result<Ran, Failure> {
    permit_session(shared, prepared, live)?;
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            run::elevated(shared, session_elevator(live), &prepared.argv, run::RUN_TIME).await
        }
        Runs::AsThePerson => run::as_the_person(&live.link, &prepared.argv).await,
    }
}

/// Runs `prepared` for the API token `token`: in its owner's worker, or as
/// an administrator by authd's leave, which checks the token's scopes for
/// the action's area again.
async fn for_token(
    shared: &Rc<Shared>,
    prepared: &Prepared,
    token: &TokenCall,
) -> Result<Ran, Failure> {
    permit_token(prepared, token)?;
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            run::elevated(shared, token_elevator(prepared, token), &prepared.argv, run::RUN_TIME)
                .await
        }
        Runs::AsThePerson => {
            let link = caller::worker(shared, token).await?;
            run::as_the_person(&link, &prepared.argv).await
        }
    }
}

/// Starts the long action `prepared` as a task for the session `live` or
/// the token `token`, and answers 202 with the task's number. The checks
/// are the short action's; the command is not run in the request, so a
/// long action runs as an administrator only: a person's own worker does
/// not stream what a command writes (a unit test keeps the actions to
/// that).
///
/// # Errors
///
/// As [`actions::authorize`], ADM-126 when there is no store, ADM-105
/// when 64 tasks wait already.
fn long(
    shared: &Rc<Shared>,
    peer: IpAddr,
    prepared: Prepared,
    asker: &Asker,
    whose: &str,
    live: Option<&Live>,
    token: Option<&TokenCall>,
) -> Outcome {
    if prepared.action.runs() != Runs::AsAdministrator {
        return Err(Failure::new(
            Kind::Internal,
            format!(
                "{} is long, and only an administrator's action can run as a task",
                prepared.action.id()
            ),
        ));
    }
    let by = match (live, token) {
        (Some(live), None) => {
            permit_session(shared, &prepared, live)?;
            session_elevator(live)
        }
        (None, Some(token)) => {
            permit_token(&prepared, token)?;
            token_elevator(&prepared, token)
        }
        _ => return Err(super::ended()),
    };
    let action = prepared.action.id();
    let id = tasks::start(shared, prepared, &asker.who, &asker.how, by)?;
    log(6, &format!("{whose} from {peer}: {action} is task {id}"));
    Ok(Reply { status: StatusCode::ACCEPTED, ..Reply::json(json!({ "task": id })) })
}

/// The log's line for an action asked for: a change always, a read only
/// when it was refused or failed.
fn said(peer: IpAddr, action: &dyn Registered, whose: &str, ran: &Result<Ran, Failure>) {
    let ended = match ran {
        Ok(ran) if ran.status == 0 && !action.changes() => return,
        Ok(ran) => format!("ended with status {}", ran.status),
        Err(failure) => format!("was not run: {}", failure.kind.id()),
    };
    log(6, &format!("{whose} from {peer}: {} {ended}", action.id()));
}

/// What `POST /api/1/show` takes: the action, and the arguments its route
/// would take, in one object.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Showing {
    action: Text,
    #[serde(default = "no_arguments")]
    args: Value,
}

/// The arguments of a showing that gives none.
fn no_arguments() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Answers `POST /api/1/show`: the command, the API call, the Ansible task
/// and the files of the action with the arguments, for any session that is
/// signed in, locked or not, or for the API token `token` when its scopes
/// let it read the action's area. Nothing is run.
pub fn show(cx: &Context<'_>, token: Option<&TokenCall>) -> Outcome {
    let showing: Showing = cx.json()?;
    let action = show::lookup(cx.shared.actions.get(), &showing.action)?;
    let prepared = action.prepare(showing.args)?;
    if let Some(token) = token {
        actions::token_may(&token.name, &token.scopes, action.area(), false)?;
    }
    Ok(Reply::json(show::shown(&prepared, &cx.origin)))
}
