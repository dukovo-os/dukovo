// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The tasks' routes (ADR 0516): the list, one task with its log, and
//! Stop.
//!
//! A signed-in session reads every task, as it reads the journal. An API
//! token reads the tasks of the actions whose area its scopes let it read,
//! which is how a script that asked for a long action finds out how it
//! went. Stop is for the task's own person or an administrator, from a
//! session, locked or not (a stop only has the command put back what it
//! did), or from a token whose scopes let it change the action's area and
//! whose owner is such a person.
//!
//! The answers are the task as the store keeps it, with the action's
//! title; the log of at most 2,000 lines only in one task's.

use serde_json::{Value, json};

use super::changes::{number, store, stored};
use super::{Context, Live, Outcome, Reply};
use crate::admind::actions::{self, Registered, show};
use crate::admind::caller::TokenCall;
use crate::admind::errors::{Failure, Kind};
use crate::admind::shared::{Shared, unix_now};
use crate::admind::store::tasks::Task;
use crate::admind::who::{self, Account};

/// How many tasks a list gives when it is not told.
const LIMIT_DEFAULT: usize = 50;
/// The most tasks a list gives.
const LIMIT_MAX: usize = 200;

/// How far back a list reaches for tasks that ended, when it is not told.
const SINCE_DEFAULT: i64 = 24 * 3600;

/// The failure for a task no one gave.
fn not_kept() -> Failure {
    Failure::new(Kind::NotKept, "the console has no task with that number")
}

/// The area of the action `task` is of, if the console still has it.
fn area_of(shared: &Shared, task: &Task) -> Option<&'static str> {
    show::lookup(shared.actions.get(), &task.action).ok().map(Registered::area)
}

/// Whether the token reads `task`: its scopes name the area of its
/// action.
fn token_reads(shared: &Shared, token: &TokenCall, task: &Task) -> bool {
    area_of(shared, task)
        .is_some_and(|area| actions::token_may(&token.name, &token.scopes, area, false).is_ok())
}

/// `task` as the answers give it: its summary, and the action's title.
fn task_json(shared: &Shared, task: &Task, with_log: bool) -> Value {
    let mut value = if with_log { task.detail() } else { task.summary() };
    let title = show::lookup(shared.actions.get(), &task.action).ok().map(Registered::title);
    value["title"] = json!(title);
    value
}

/// Answers `GET /api/1/tasks?since=&limit=`: the tasks that wait or run,
/// and those that ended at Unix time `since` or later (the last 24 hours
/// when it is not given), newest first, at most `limit` (50, and at most
/// 200).
pub fn list(cx: &Context<'_>, token: Option<&TokenCall>) -> Outcome {
    let since = number(cx, "since", 0, i64::MAX)?.unwrap_or_else(|| unix_now() - SINCE_DEFAULT);
    let limit = number(cx, "limit", 1, LIMIT_MAX as i64)?.map_or(LIMIT_DEFAULT, |n| n as usize);
    let tasks = cx.shared.tasks.list(store(cx.shared)?, since, limit).map_err(stored)?;
    let shown: Vec<Value> = tasks
        .iter()
        .filter(|task| token.is_none_or(|token| token_reads(cx.shared, token, task)))
        .map(|task| task_json(cx.shared, task, false))
        .collect();
    Ok(Reply::json(json!({ "tasks": shown })))
}

/// Answers `GET /api/1/tasks/{id}`: the task with its log.
pub fn one(cx: &Context<'_>, token: Option<&TokenCall>) -> Outcome {
    let task = found(cx)?;
    if token.is_some_and(|token| !token_reads(cx.shared, token, &task)) {
        // A task the token may not read is not one it knows of.
        return Err(not_kept());
    }
    Ok(Reply::json(task_json(cx.shared, &task, true)))
}

/// The task the address names.
fn found(cx: &Context<'_>) -> Result<Task, Failure> {
    let id = cx.id()?;
    cx.shared.tasks.get(store(cx.shared)?, id).map_err(stored)?.ok_or_else(not_kept)
}

/// Answers `POST /api/1/tasks/{id}/stop` for the session `live` or the
/// token `token`: the task is asked to stop, by SIGTERM, and the answer
/// says so; the task's end comes after, as its state says and its event
/// tells.
///
/// # Errors
///
/// ADM-127 for a task there is not, ADM-122 for a person who is neither
/// its own nor an administrator, ADM-124 for a token that may not change
/// the action's area, or for a task of an action the console no longer has,
/// whose area there is none to check, ADM-128 for a task that has ended.
pub fn stop(cx: &Context<'_>, live: Option<&Live>, token: Option<&TokenCall>) -> Outcome {
    let task = found(cx)?;
    let person = match (live, token) {
        (Some(live), None) => live.name.as_str(),
        (None, Some(token)) => {
            // A task of an action the console no longer has has no area
            // for the token's scopes to be checked against, so a token
            // may not stop it.
            let Some(area) = area_of(cx.shared, &task) else {
                return Err(Failure::new(
                    Kind::OutOfScope,
                    format!(
                        "the API token \"{}\" may not stop task {}: the console no longer has its \
                         action, {}, and so no scope to check the token against",
                        token.name, task.id, task.action
                    ),
                ));
            };
            actions::token_may(&token.name, &token.scopes, area, true)?;
            token.owner.as_str()
        }
        _ => return Err(super::ended()),
    };
    if person != task.who {
        let who = who::read(&cx.shared.accounts, Account::Name(person))
            .map_err(|why| Failure::new(Kind::Internal, why))?;
        if !who.admin {
            return Err(Failure::new(
                Kind::NotAdmin,
                "a task is stopped by the person who asked for it, or by an administrator",
            ));
        }
    }
    cx.shared.tasks.stop(task.id, person)?;
    let how = token.map_or_else(String::new, |token| format!(" (API token {})", token.name));
    crate::admind::log(
        6,
        &format!("{person}{how} from {}: asked task {} to stop", cx.peer, task.id),
    );
    let now = cx.shared.tasks.get(store(cx.shared)?, task.id).map_err(stored)?.unwrap_or(task);
    Ok(Reply::json(json!({ "stopping": true, "task": task_json(cx.shared, &now, false) })))
}
