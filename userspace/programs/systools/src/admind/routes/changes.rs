// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The journal's routes (ADR 0516): "What changed", one change with the
//! diff of each file, Keep it, and Put back.
//!
//! Everyone who is signed in reads the journal, locked or not; an API
//! token does not, since the journal names people and files and a token is
//! for the actions. It never has the text of a secret, nor of a file that
//! not everyone may read (see [`journal`](crate::admind::journal)).
//!
//! **Keep it** marks a hand edit seen. It is the person's own say that
//! they have looked, so it needs no administrator, and an unlocked session:
//! nothing is changed on the machine, but a locked session only looks.
//!
//! **Put back** writes the earlier text of a change's files back, as an
//! administrator, through `console-elevate` of `admin put-back ID`: the
//! command the answer shows is the one that ran. It is a route and not an
//! action of the registry, since its argument is a row of the journal,
//! which no fixed example can give. It needs an administrator's unlocked
//! session and is refused as an action is (ADM-122, ADM-106). Like any
//! console change it has a row of its own, which says which change it
//! puts back, so that it can be put back itself.

use std::rc::Rc;

use accounts::console::Elevator;
use serde_json::{Value, json};

use super::{Context, Live, Outcome, Reply};
use crate::admind::actions::{self, show};
use crate::admind::diff;
use crate::admind::errors::{Failure, Kind};
use crate::admind::journal::{self, Ended};
use crate::admind::log;
use crate::admind::run;
use crate::admind::shared::{Shared, unix_now};
use crate::admind::store::Store;
use crate::admind::store::changes::{Change, FileChange, Seen};
use crate::admind::who::{self, Account};

/// How many changes a list gives when it is not told.
const LIMIT_DEFAULT: usize = 50;
/// The most changes a list gives.
const LIMIT_MAX: usize = 200;

/// The most lines of diff one change's answer holds, all its files'
/// together.
const DIFF_LINES_MAX: usize = 2000;

/// The ID a put back has in the journal, as an action has its own.
pub const PUT_BACK_ID: &str = "changes.put-back";
/// The title a put back has in the journal.
const PUT_BACK_TITLE: &str = "Put a change back";

/// The store, or ADM-126 when it did not open.
pub(super) fn store(shared: &Shared) -> Result<&Store, Failure> {
    shared.store.as_ref().ok_or_else(|| {
        Failure::new(Kind::NoStore, "the console's store did not open when it started")
    })
}

/// A failure of the store, as the console's own. It takes the error by
/// value to be a `map_err`'s function.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn stored(why: crate::admind::store::Error) -> Failure {
    Failure::new(Kind::Internal, why.to_string())
}

/// The failure for a change no one gave.
fn not_kept() -> Failure {
    Failure::new(Kind::NotKept, "the journal has no change with that number")
}

/// The title a person reads for the action `id`.
fn title_of(shared: &Shared, id: &str) -> Option<&'static str> {
    if id == PUT_BACK_ID {
        return Some(PUT_BACK_TITLE);
    }
    shared.actions.get().iter().find(|action| action.id() == id).map(|action| action.title())
}

/// A change as the list gives it.
fn change_json(shared: &Shared, change: &Change) -> Value {
    json!({
        "id": change.id,
        "at": change.at,
        "who": change.who,
        "how": change.how,
        "action": change.action,
        "title": change.action.as_deref().and_then(|id| title_of(shared, id)),
        "argv": change.argv,
        "status": change.status,
        "task": change.task,
        "note": change.note,
        "files": change.files,
        "seen": change.seen,
    })
}

/// A number the query gives, at least `least` and at most `most`.
pub(super) fn number(
    cx: &Context<'_>,
    name: &'static str,
    least: i64,
    most: i64,
) -> Result<Option<i64>, Failure> {
    let Some(text) = cx.query(name) else { return Ok(None) };
    let wrong = || {
        Failure::bad_value(
            name,
            format!("{name} is a number from {least} to {most}. Nothing was done."),
        )
    };
    let value: i64 = text.parse().map_err(|_| wrong())?;
    if (least..=most).contains(&value) { Ok(Some(value)) } else { Err(wrong()) }
}

/// Answers `GET /api/1/changes?since=&limit=`: the changes made at Unix
/// time `since` or later, newest first, at most `limit` (50 when it is not
/// given, and at most 200).
pub fn list(cx: &Context<'_>) -> Outcome {
    let since = number(cx, "since", 0, i64::MAX)?.unwrap_or(0);
    let limit = number(cx, "limit", 1, LIMIT_MAX as i64)?.map_or(LIMIT_DEFAULT, |n| n as usize);
    let changes = store(cx.shared)?.changes(since, limit).map_err(stored)?;
    let rows: Vec<Value> = changes.iter().map(|change| change_json(cx.shared, change)).collect();
    Ok(Reply::json(json!({ "changes": rows })))
}

/// What a file's side is, as its text: the version the store kept of
/// it, an empty text for a file that was not there, or why there is none.
fn text_of(store: &Store, seen: &Seen) -> Result<String, &'static str> {
    match seen {
        Seen::Absent => Ok(String::new()),
        Seen::Secret => {
            Err("it is a secret or a file that not everyone may read: only its name is kept")
        }
        Seen::Unreadable => Err("the console could not read it"),
        Seen::Large { .. } => Err("it is longer than 1 MiB, and no text of it is kept"),
        Seen::Text { sha256, .. } => {
            let bytes = store
                .version_text(sha256)
                .ok()
                .flatten()
                .ok_or("its text is not kept any more: the last 20 versions of a file are")?;
            match String::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => Ok(text),
                _ => Err("it is not text"),
            }
        }
    }
}

/// One file of a change, with its diff if there is one and there is room
/// for it in `budget`, which it takes its lines from.
fn file_json(store: &Store, file: &FileChange, budget: &mut usize) -> Value {
    let mut value = json!({ "path": file.path, "before": file.before, "after": file.after });
    if file.before.same_as(&file.after) {
        value["no_diff"] = "the file is as it was".into();
        return value;
    }
    match (text_of(store, &file.before), text_of(store, &file.after)) {
        (Ok(_), Ok(_)) if *budget == 0 => {
            value["no_diff"] =
                format!("the diffs are at most {DIFF_LINES_MAX} lines in all").into();
        }
        (Ok(before), Ok(after)) => {
            let shown = diff::unified(
                &before,
                &after,
                &format!("{} (before)", file.path),
                &format!("{} (after)", file.path),
                *budget,
            );
            *budget -= shown.text.lines().count();
            value["diff"] = shown.text.into();
            value["diff_truncated"] = shown.truncated.into();
        }
        (Err(why), _) | (_, Err(why)) => value["no_diff"] = why.into(),
    }
    value
}

/// The command a put back of change `id` runs.
fn put_back_argv(id: i64) -> Vec<String> {
    ["admin", "put-back"].iter().map(ToString::to_string).chain([id.to_string()]).collect()
}

/// `argv` as the line a person pastes at a terminal, as an administrator.
fn command_line(argv: &[String]) -> String {
    let words: Vec<String> = argv.iter().map(|word| show::shell_word(word)).collect();
    format!("elevate {}", words.join(" "))
}

/// Answers `GET /api/1/changes/{id}`: the change with a unified diff of
/// each file it changed (at most 2,000 lines of diff in all), and whether
/// it can be put back and the command that does.
pub fn one(cx: &Context<'_>) -> Outcome {
    let store = store(cx.shared)?;
    let change = store.change(cx.id()?).map_err(stored)?.ok_or_else(not_kept)?;
    let mut budget = DIFF_LINES_MAX;
    let files: Vec<Value> =
        change.files.iter().map(|file| file_json(store, file, &mut budget)).collect();
    let mut value = change_json(cx.shared, &change);
    value["files"] = Value::Array(files);
    let command = command_line(&put_back_argv(change.id));
    value["put_back"] = match journal::restore_plan(store, &change) {
        Ok(_) => json!({ "possible": true, "command": command }),
        Err(why) => json!({ "possible": false, "why": why, "command": command }),
    };
    Ok(Reply::json(value))
}

/// Answers `POST /api/1/changes/{id}/keep`: marks the hand edit seen by
/// the person, for a session that is unlocked, and gives the change.
///
/// # Errors
///
/// ADM-106 for a locked session, ADM-127 for no such change, ADM-128 for
/// one that was not made by hand.
pub fn keep(cx: &Context<'_>, live: &Live) -> Outcome {
    let store = store(cx.shared)?;
    let id = cx.id()?;
    if !super::unlocked(cx.shared, &live.cookie) {
        return Err(Failure::new(Kind::Locked, "unlock the console to keep a change"));
    }
    match store.mark_seen(id, &live.name, unix_now()).map_err(stored)? {
        crate::admind::store::changes::Marking::Missing => return Err(not_kept()),
        crate::admind::store::changes::Marking::NotByHand => {
            return Err(Failure::new(
                Kind::Cannot,
                "a change made through the console is kept already: only a change made by hand is marked seen",
            ));
        }
        crate::admind::store::changes::Marking::Done => {
            log(6, &format!("{} from {} kept change {id}", super::shown(&live.name), cx.peer));
        }
        crate::admind::store::changes::Marking::Already => {}
    }
    let change = store.change(id).map_err(stored)?.ok_or_else(not_kept)?;
    Ok(Reply::json(change_json(cx.shared, &change)))
}

/// Answers `POST /api/1/changes/{id}/put-back`: runs `admin put-back ID`
/// as an administrator for the person's unlocked session, and says what it
/// did and the command that did it.
///
/// The command runs in a task of its own, which the answer waits for, as an
/// action's does.
///
/// # Errors
///
/// ADM-122 and ADM-106 as an action is refused, ADM-127 for no such
/// change, ADM-128 when its earlier text is not all kept, ADM-123 when the
/// command did not work.
pub async fn put_back(cx: &Context<'_>, live: &Live) -> Outcome {
    let store = store(cx.shared)?;
    let change = store.change(cx.id()?).map_err(stored)?.ok_or_else(not_kept)?;
    let who = who::read(&cx.shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let unlocked = super::unlocked(cx.shared, &live.cookie);
    actions::session_may_elevate(&live.name, who.admin, unlocked, "what changed")?;
    let plan =
        journal::restore_plan(store, &change).map_err(|why| Failure::new(Kind::Cannot, why))?;
    let files: Vec<String> = plan.into_iter().map(|restore| restore.path).collect();
    let (shared, peer, name, authd) =
        (Rc::clone(cx.shared), cx.peer, live.name.clone(), live.authd.clone());
    let id = change.id;
    let job = tokio::task::spawn_local(async move {
        put_back_run(&shared, peer, &name, authd, id, &files).await
    });
    match job.await {
        Ok(answer) => Ok(Reply::json(answer?)),
        Err(_) => Err(Failure::new(Kind::Internal, "the put back's task did not end")),
    }
}

/// Runs the put back of change `id` for `name`, whose session in authd is
/// `authd`, and keeps its row. `files` are those it writes.
async fn put_back_run(
    shared: &Rc<Shared>,
    peer: std::net::IpAddr,
    name: &str,
    authd: String,
    id: i64,
    files: &[String],
) -> Result<Value, Failure> {
    let argv = put_back_argv(id);
    let begun = shared.journal.begin(shared.store.as_ref(), files, unix_now());
    let ran = run::elevated(shared, Elevator::Session(authd), &argv, run::RUN_TIME).await;
    let what = match &ran {
        Ok(ran) => format!("ended with status {}", ran.status),
        Err(failure) => format!("was not run: {}", failure.kind.id()),
    };
    log(6, &format!("{} from {peer}: {PUT_BACK_ID} {id} {what}", super::shown(name)));
    let mut row = None;
    match run::ending(&ran) {
        Some((status, note)) => {
            let puts = format!("puts back change {id}");
            let ended = Ended {
                who: name,
                how: "console",
                action: PUT_BACK_ID,
                argv: &argv,
                status,
                task: None,
                note: Some(note.unwrap_or(&puts)),
            };
            row = begun.end(shared.store.as_ref(), &ended, unix_now()).row;
        }
        None => begun.cancel(),
    }
    let ran = ran?;
    if ran.status != 0 {
        return Err(run::command_failed(&argv, &[], &ran));
    }
    let output: Vec<&str> = ran.output.lines().collect();
    Ok(json!({
        "done": true,
        "output": output,
        "command": command_line(&argv),
        "change": row,
    }))
}
