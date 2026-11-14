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

use accounts::console::Elevator;
use serde::Deserialize;
use serde_json::Value;

use super::{Context, Live, Outcome, Reply, Text, shown};
use crate::admind::actions::show;
use crate::admind::actions::{self, Caller, Params, Prepared, Registered, Runs};
use crate::admind::caller::{self, TokenCall};
use crate::admind::errors::{Failure, Kind};
use crate::admind::log;
use crate::admind::run::{self, Ran};
use crate::admind::shared::unix_now;
use crate::admind::who::{self, Account};

/// Answers `action` at the address that gave `params`, for the session
/// `live` or the token `token`, one of which the request has.
pub async fn action(
    cx: &Context<'_>,
    action: &'static dyn Registered,
    params: Params,
    live: Option<Live>,
    token: Option<TokenCall>,
) -> Outcome {
    let args = actions::arguments(params, &cx.body)?;
    let prepared = action.prepare(args)?;
    let (whose, ran) = match (&live, &token) {
        (Some(live), None) => (shown(&live.name), for_session(cx, &prepared, live).await),
        (None, Some(token)) => {
            let whose = format!("{} (API token {})", shown(&token.owner), shown(&token.name));
            (whose, for_token(cx, &prepared, token).await)
        }
        // `check` lets an action through with exactly one of them.
        _ => return Err(super::ended()),
    };
    said(cx, action, &whose, &ran);
    Ok(Reply::json(run::answer(&prepared, &ran?)?))
}

/// Runs `prepared` for the session `live`: as the person, or as an
/// administrator when the person is one and the session is unlocked.
async fn for_session(cx: &Context<'_>, prepared: &Prepared, live: &Live) -> Result<Ran, Failure> {
    let who = who::read(&cx.shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let until = cx.shared.with_session(&live.cookie, |session| session.unlocked_until);
    let unlocked = until.flatten().is_some_and(|until| until > unix_now());
    let caller = Caller::Session { name: &live.name, admin: who.admin, unlocked };
    actions::authorize(&caller, prepared.action)?;
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            let by = Elevator::Session(live.authd.clone());
            run::elevated(cx.shared, by, &prepared.argv, run::RUN_TIME).await
        }
        Runs::AsThePerson => run::as_the_person(&live.link, &prepared.argv).await,
    }
}

/// Runs `prepared` for the API token `token`: in its owner's worker, or as
/// an administrator by authd's leave, which checks the token's scopes for
/// the action's area again.
async fn for_token(
    cx: &Context<'_>,
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
            run::elevated(cx.shared, by, &prepared.argv, run::RUN_TIME).await
        }
        Runs::AsThePerson => {
            let link = caller::worker(cx.shared, token).await?;
            run::as_the_person(&link, &prepared.argv).await
        }
    }
}

/// The log's line for an action asked for: a change always, a read only
/// when it was refused or failed.
fn said(cx: &Context<'_>, action: &dyn Registered, whose: &str, ran: &Result<Ran, Failure>) {
    let ended = match ran {
        Ok(ran) if ran.status == 0 && !action.changes() => return,
        Ok(ran) => format!("ended with status {}", ran.status),
        Err(failure) => format!("was not run: {}", failure.kind.id()),
    };
    log(6, &format!("{whose} from {}: {} {ended}", cx.peer, action.id()));
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
