// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An action's route (ADR 0515): its arguments from the address and the
//! body, checked; whether the caller may ask for it; its command, run as
//! the person or as an administrator; and the answer.
//!
//! The showing of an action, which runs nothing, is here too: [`show`].
//!
//! The log has a line for each change asked for, and for each read that
//! was refused or failed: who asked, from where, the action and how it
//! ended. Arguments that do not read are refused before anyone is asked,
//! and have no line.

use std::net::IpAddr;
use std::rc::Rc;

use accounts::console::Elevator;
use serde::Deserialize;
use serde_json::Value;

use super::{Context, Live, Outcome, Reply, Text, shown};
use crate::admind::actions::show;
use crate::admind::actions::{self, Caller, Params, Prepared, Registered, Runs};
use crate::admind::caller::{self, TokenCall};
use crate::admind::errors::{Failure, Kind};
use crate::admind::journal::Ended;
use crate::admind::log;
use crate::admind::run::{self, Ran};
use crate::admind::shared::{Shared, unix_now};
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

/// Runs `prepared` for the session `live`: as the person, or as an
/// administrator when the person is one and the session is unlocked.
async fn for_session(
    shared: &Rc<Shared>,
    prepared: &Prepared,
    live: &Live,
) -> Result<Ran, Failure> {
    let who = who::read(&shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let unlocked = super::unlocked(shared, &live.cookie);
    let caller = Caller::Session { name: &live.name, admin: who.admin, unlocked };
    actions::authorize(&caller, prepared.action)?;
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            let by = Elevator::Session(live.authd.clone());
            run::elevated(shared, by, &prepared.argv, run::RUN_TIME).await
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
    let action = prepared.action;
    let caller = Caller::Token { name: &token.name, scopes: &token.scopes };
    actions::authorize(&caller, action)?;
    match action.runs() {
        Runs::AsAdministrator => {
            let by = Elevator::Token {
                token: token.token.bytes().to_vec(),
                area: action.area().to_string(),
            };
            run::elevated(shared, by, &prepared.argv, run::RUN_TIME).await
        }
        Runs::AsThePerson => {
            let link = caller::worker(shared, token).await?;
            run::as_the_person(&link, &prepared.argv).await
        }
    }
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
