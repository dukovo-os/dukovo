// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An action's route (ADR 0515): its arguments from the address and the
//! body, checked; whether the caller may ask for it; its command, run as
//! the person or as an administrator; and the answer.
//!
//! The showing of an action, which runs nothing, is here too: [`show`].
//!
//! A change says whether it changed anything ([`changed`]): an action
//! with a probe has it run as the person before the command and after it,
//! and the journal reads the files the action names before and after. It
//! changed nothing only when each of those reads the same.
//!
//! A long action (ADR 0516) is not run in the request: it starts a task,
//! which [`tasks`](crate::admind::tasks) runs, and the answer is 202 with
//! the task's number.
//!
//! The log has a line for each change asked for, and for each read that
//! was refused or failed: who asked, from where, the action and how it
//! ended, with "changed nothing" for a change that ended well and moved
//! nothing. Arguments that do not read are refused before anyone is asked,
//! and have no line. A change that runs also has a row in the journal when
//! it ends (a long one, when its task does).

use std::future::Future;
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
use crate::admind::link::Link;
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
        let probed = match (&live, &token) {
            (Some(live), _) => for_session(&shared, &prepared, live).await,
            (None, Some(token)) => for_token(&shared, &prepared, token).await,
            (None, None) => Err(super::ended()),
        };
        // A change that names no files has none that moved.
        let mut files_same = true;
        if let Some(begun) = begun {
            let ending = match &probed {
                Ok(probed) => Some((Some(probed.ran.status), None)),
                Err(failure) => run::ending_failure(failure),
            };
            match ending {
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
                    files_same = begun.end(shared.store.as_ref(), &ended, unix_now()).same;
                }
                None => begun.cancel(),
            }
        }
        let done = probed.map(|Probed { ran, same }| {
            let changed = changed(&prepared, same, files_same);
            Done { ran, changed }
        });
        said(peer, prepared.action, &whose, &done);
        let done = done?;
        run::answer(&prepared, &done.ran, done.changed)
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
) -> Result<Probed, Failure> {
    permit_session(shared, prepared, live)?;
    let probes = Probes::Session(&live.link);
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            let by = session_elevator(live);
            let command = run::elevated(shared, by, &prepared.argv, run::RUN_TIME);
            probed(shared, prepared, &probes, command).await
        }
        Runs::AsThePerson => {
            let command = run::as_the_person(&live.link, &prepared.argv);
            probed(shared, prepared, &probes, command).await
        }
    }
}

/// Runs `prepared` for the API token `token`: in its owner's worker, or as
/// an administrator by authd's leave, which checks the token's scopes for
/// the action's area again.
async fn for_token(
    shared: &Rc<Shared>,
    prepared: &Prepared,
    token: &TokenCall,
) -> Result<Probed, Failure> {
    permit_token(prepared, token)?;
    let probes = Probes::Token(token);
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            let by = token_elevator(prepared, token);
            let command = run::elevated(shared, by, &prepared.argv, run::RUN_TIME);
            probed(shared, prepared, &probes, command).await
        }
        Runs::AsThePerson => {
            let link = caller::worker(shared, token).await?;
            let command = run::as_the_person(&link, &prepared.argv);
            probed(shared, prepared, &probes, command).await
        }
    }
}

/// A change's command that ran, and what its probe found.
struct Probed {
    /// What the command printed and how it ended.
    ran: Ran,
    /// Whether the probe read the same before the command and after it:
    /// None for an action with no probe, false when it could not tell.
    same: Option<bool>,
}

/// A command that ran, and whether it changed anything.
struct Done {
    /// What the command printed and how it ended.
    ran: Ran,
    /// Whether a change moved anything ([`changed`]); false for a read.
    changed: bool,
}

/// Where a change's probes run: the worker of the person who asked, the
/// session's or the token owner's, so that a probe reads only what that
/// person can (ADR 0515).
enum Probes<'a> {
    Session(&'a Link),
    /// The owner's worker is started when a probe needs it.
    Token(&'a TokenCall),
}

impl Probes<'_> {
    /// What the probe `argv` prints, as the person; None when it cannot be
    /// run or does not end well. A probe that fails is no reason to fail
    /// the change.
    async fn read(&self, shared: &Shared, argv: &[String]) -> Option<String> {
        let link = match self {
            Probes::Session(link) => (*link).clone(),
            Probes::Token(token) => caller::worker(shared, token).await.ok()?,
        };
        let ran = run::as_the_person(&link, argv).await.ok()?;
        (ran.status == 0).then_some(ran.output)
    }
}

/// Runs `command`, which makes the change `prepared` is, between two
/// reads of its probe, if it has one (ADR 0515). The probe is read again
/// only when the command ended well; the two are the same when the part
/// of each that the action picks is there and equal.
async fn probed(
    shared: &Shared,
    prepared: &Prepared,
    probes: &Probes<'_>,
    command: impl Future<Output = Result<Ran, Failure>>,
) -> Result<Probed, Failure> {
    let Some(probe) = prepared.probe.as_deref() else {
        return Ok(Probed { ran: command.await?, same: None });
    };
    let before = probes.read(shared, probe).await;
    let ran = command.await?;
    if ran.status != 0 {
        return Ok(Probed { ran, same: Some(false) });
    }
    let after = probes.read(shared, probe).await;
    let picked = |output: Option<String>| prepared.action.probed(&prepared.args, &output?);
    let same =
        matches!((picked(before), picked(after)), (Some(before), Some(after)) if before == after);
    Ok(Probed { ran, same: Some(same) })
}

/// Whether the change `prepared` changed anything, given whether its probe
/// read the same (`probe_same`, None when it has none) and whether the
/// files it names read the same (`files_same`, true when it names none).
///
/// It changed nothing only when what it can compare says so: its probe,
/// its files, or both. One that compares neither always did something,
/// and so does one whose probe or files could not tell, as Ansible says
/// of a task it cannot compare. A read changes nothing.
fn changed(prepared: &Prepared, probe_same: Option<bool>, files_same: bool) -> bool {
    if !prepared.action.changes() {
        return false;
    }
    if probe_same.is_none() && prepared.files.is_empty() {
        return true;
    }
    !(probe_same.unwrap_or(true) && files_same)
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

/// What the log says an action asked for came to, which is the end of its
/// line: for a change always, with "changed nothing" when it ended well and
/// moved nothing; for a read only when it was refused or failed.
fn told(action: &dyn Registered, done: &Result<Done, Failure>) -> Option<String> {
    match done {
        Ok(Done { ran, .. }) if ran.status == 0 && !action.changes() => None,
        Ok(Done { ran, changed }) if ran.status == 0 && !changed => {
            Some(String::from("ended with status 0, changed nothing"))
        }
        Ok(Done { ran, .. }) => Some(format!("ended with status {}", ran.status)),
        Err(failure) => Some(format!("was not run: {}", failure.kind.id())),
    }
}

/// The log's line for an action asked for, if it has one: who asked, from
/// where, the action and [`told`].
fn said(peer: IpAddr, action: &dyn Registered, whose: &str, done: &Result<Done, Failure>) {
    if let Some(ended) = told(action, done) {
        log(6, &format!("{whose} from {peer}: {} {ended}", action.id()));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::actions::tests::LAB;

    /// A command that ended with `status`.
    fn done(status: i32, changed: bool) -> Result<Done, Failure> {
        Ok(Done { ran: Ran { status, ..Ran::default() }, changed })
    }

    #[test]
    fn a_change_is_logged_with_how_it_ended_and_that_it_changed_nothing() {
        let (restart, list) = (LAB[0], LAB[1]);
        assert!(restart.changes() && !list.changes());
        assert_eq!(told(restart, &done(0, true)).as_deref(), Some("ended with status 0"));
        assert_eq!(
            told(restart, &done(0, false)).as_deref(),
            Some("ended with status 0, changed nothing")
        );
        // A change that failed has no `changed` to tell of.
        assert_eq!(told(restart, &done(3, true)).as_deref(), Some("ended with status 3"));
        let refused = Err(Failure::new(Kind::Locked, "locked"));
        assert_eq!(told(restart, &refused).as_deref(), Some("was not run: ADM-106"));
    }

    #[test]
    fn a_read_is_logged_only_when_it_was_refused_or_failed() {
        let list = LAB[1];
        assert_eq!(told(list, &done(0, false)), None);
        assert_eq!(told(list, &done(1, false)).as_deref(), Some("ended with status 1"));
        let refused = Err(Failure::new(Kind::NotAdmin, "no"));
        assert_eq!(told(list, &refused).as_deref(), Some("was not run: ADM-122"));
    }

    /// `id` of the lab's actions, prepared with `args`, naming `files`.
    fn prepared(id: &str, args: Value, files: &[&str]) -> Prepared {
        let action = LAB.iter().copied().find(|action| action.id() == id).unwrap();
        let mut prepared = action.prepare(args).unwrap();
        prepared.files = files.iter().map(ToString::to_string).collect();
        prepared
    }

    #[test]
    fn a_change_changed_nothing_only_when_what_it_compares_reads_the_same() {
        let name = serde_json::json!({ "name": "lab-web" });
        // A probe and no files: the probe alone says.
        let set = prepared("lab.set", name.clone(), &[]);
        assert!(!changed(&set, Some(true), true));
        assert!(changed(&set, Some(false), true));
        // A probe and a file: both must read the same.
        let set_file = prepared("lab.set", name.clone(), &["/s/lab-web.toml"]);
        assert!(!changed(&set_file, Some(true), true));
        assert!(changed(&set_file, Some(true), false));
        assert!(changed(&set_file, Some(false), true));
        // No probe and a file: the file alone says.
        let restart_file = prepared("lab.restart", name.clone(), &["/s/lab-web.toml"]);
        assert!(!changed(&restart_file, None, true));
        assert!(changed(&restart_file, None, false));
        // Neither: it always did something.
        let restart = prepared("lab.restart", name, &[]);
        assert!(changed(&restart, None, true));
        // A read changed nothing.
        let list = prepared("lab.list", serde_json::json!({}), &[]);
        assert!(!changed(&list, None, true));
    }
}
