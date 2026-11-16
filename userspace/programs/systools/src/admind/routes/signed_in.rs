// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an account has signed in now, for the Accounts page's Signed in
//! now (ADR 0562, which closes ADR 0517's deferral): its console sessions,
//! from admind's own table, and its console terminals, with a way to end
//! a session. A terminal ends by `POST /api/1/terminals/{id}/end`, which
//! an unlocked administrator may now ask for another account's terminal
//! too.
//!
//! They are routes, not actions: they act on admind's own state, which no
//! command reads or changes, as the log's follow and the trials' `fresh`
//! do, so they have no command to show and no Ansible task, and an API
//! token is refused, as on every route of the table.
//!
//! An account sees its own and ends its other sessions; an administrator
//! whose session is unlocked sees and ends another account's. Each look
//! at another's and each end is an audit line, and authd tells a person
//! whose session an administrator ended at their next sign-in. The session
//! in use is never ended here (ADM-136): that is Sign out. An account's
//! sessions at the machine, over SSH or at a screen, are authd's, read by
//! `accounts.show` and ended by `accounts.end`.

use std::time::Instant;

use serde_json::{Value, json};

use super::{Context, Live, Outcome, Reply, shown, unlocked};
use crate::admind::audit;
use crate::admind::errors::{Failure, Kind};
use crate::admind::sessions::{ms_at, unix_ms};
use crate::admind::who::{self, Account};

/// The value of `{part}` in the address.
fn part<'a>(cx: &'a Context<'_>, part: &str) -> Result<&'a str, Failure> {
    cx.params
        .get(part)
        .and_then(Value::as_str)
        .ok_or_else(|| Failure::new(Kind::BadRequest, format!("give the {part}")))
}

/// Whether `live` may see and end account `name`'s sessions: its own, or
/// another's as an administrator whose session is unlocked now. True for
/// another's.
///
/// # Errors
///
/// ADM-122 for another account's, asked by one who is not an
/// administrator; ADM-106 while the session is locked.
fn may(cx: &Context<'_>, live: &Live, name: &str) -> Result<bool, Failure> {
    if name == live.name {
        return Ok(false);
    }
    let who = who::read(&cx.shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    if !who.admin {
        let why =
            format!("{} is not an administrator, and sees only their own sessions", live.name);
        return Err(Failure::new(Kind::NotAdmin, why));
    }
    if !unlocked(cx.shared, &live.cookie) {
        let why = "the session is locked: unlock it to see or end another account's sessions";
        return Err(Failure::new(Kind::Locked, why));
    }
    Ok(true)
}

/// `GET /api/1/accounts/{name}/sessions`: the account's console sessions,
/// the oldest first, the one in use marked `this`, and its terminals. An
/// account that has none, or that is not there, has empty lists.
pub fn list(cx: &Context<'_>, live: &Live) -> Outcome {
    let name = part(cx, "name")?;
    if may(cx, live, name)? {
        audit(&format!("{} looked at the console sessions of {}", live.name, shown(name)));
    }
    let (now, now_ms) = (Instant::now(), unix_ms());
    let mut sessions: Vec<(i64, Value)> = cx
        .shared
        .sessions
        .borrow()
        .of_account(name)
        .map(|session| {
            let shown = json!({
                "id": session.id,
                "browser": session.browser,
                "from": session.from.to_string(),
                "since_ms": session.since_ms,
                "last_ms": ms_at(session.seen, now, now_ms),
                "this": session.authd == live.authd,
            });
            (session.since_ms, shown)
        })
        .collect();
    sessions.sort_by_key(|(since, _)| *since);
    let sessions: Vec<Value> = sessions.into_iter().map(|(_, shown)| shown).collect();
    let terminals = cx.shared.terminals.borrow().of_account(name);
    Ok(Reply::json(json!({ "sessions": sessions, "terminals": terminals })))
}

/// `POST /api/1/accounts/{name}/sessions/{id}/end`: ends that console
/// session as a sign-out does. Its tabs are told `by-administrator`, or
/// `by-you` when it is the account's own other session.
///
/// # Errors
///
/// As [`may`]; ADM-136 for the session in use; ADM-139 for one that has
/// ended already.
pub fn end(cx: &Context<'_>, live: &Live) -> Outcome {
    let name = part(cx, "name")?;
    let id = part(cx, "id")?;
    let another = may(cx, live, name)?;
    let in_use = cx.shared.with_session(&live.cookie, |session| session.id == id);
    if in_use == Some(true) {
        let why = "this is the session in use: Sign out ends it";
        return Err(Failure::new(Kind::NotSelf, why));
    }
    let Some(session) = cx.shared.sessions.borrow_mut().take_id(name, id) else {
        let why = format!("that session of {} has ended already", shown(name));
        return Err(Failure::new(Kind::SessionGone, why));
    };
    let from = session.from;
    let reason = if another {
        audit(&format!("{} ended the console session of {} from {from}", live.name, shown(name)));
        "by-administrator"
    } else {
        audit(&format!("{} ended their own console session from {from}", live.name));
        "by-you"
    };
    drop(cx.shared.finish_by(session, reason, Some(&live.name)));
    Ok(Reply::json(json!({ "done": true })))
}
