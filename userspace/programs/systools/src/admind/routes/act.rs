// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An action's route (ADR 0515): its arguments from the address and the
//! body, checked; whether the caller may ask for it; its command, run as
//! the person or as an administrator; and the answer.
//!
//! The showing of an action, which runs nothing, is here too: [`show`].
//! So is the checking of one (ADR 0545), which runs only its probe as the
//! person and reads its files, and says whether the change would change
//! anything: [`check`].
//!
//! A change says whether it changed anything ([`changed`]): an action
//! with a probe has it run as the person before the command and after it,
//! and the journal reads the files the action names before and after. It
//! changed nothing only when each of those reads the same.
//!
//! A change that restarts the machine (ADR 0565) is refused before it runs
//! while a trial of settings runs, and while the other kind of restart is
//! set or runs: [`refuse_restarting`].
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
use std::time::Instant;

use accounts::console::Elevator;
use http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::{Context, Live, Outcome, Reply, Text, shown};
use crate::admind::actions::show;
use crate::admind::actions::{
    self, Browser, Caller, CutsOffRead, Params, Prepared, Registered, Restarts, Runs, Through,
    Trial,
};
use crate::admind::caller::{self, TokenCall};
use crate::admind::errors::{Failure, Kind};
use crate::admind::journal::{self, Ended};
use crate::admind::link::Link;
use crate::admind::log;
use crate::admind::reach;
use crate::admind::run::{self, Ran};
use crate::admind::serve::look_at_trials;
use crate::admind::shared::{Shared, unix_now};
use crate::admind::tasks;
use crate::admind::trials;
use crate::admind::who::{self, Account};

/// Who asks for a change, as the journal names them.
pub(super) struct Asker {
    /// The person: the session's, or the token's owner.
    pub who: String,
    /// `console`, `token NAME`, or `client NAME of OWNER` for a change an
    /// administrator approved for an MCP client.
    pub how: String,
}

/// Answers `action` at the address that gave `params`, for the session
/// `live` or the token `token`, one of which the request has.
pub(super) async fn action(
    cx: &Context<'_>,
    action: &'static dyn Registered,
    params: Params,
    live: Option<Live>,
    token: Option<TokenCall>,
) -> Outcome {
    // A change's arguments, and those of a read that takes or answers a
    // secret, are in the body of a POST (ADR 0561).
    let args = if action.posts() {
        actions::arguments(actions::typed_params(params, action.fields())?, &cx.body)?
    } else {
        actions::query_arguments(params, &cx.query, action.fields())?
    };
    let prepared = action.prepare(args)?;
    if live.is_some() && action.runs() == Runs::AsAdministrator && !action.changes() {
        from_the_console(cx)?;
    }
    match start(cx, prepared, live, token, None).await? {
        Started::Task(id) => {
            Ok(Reply { status: StatusCode::ACCEPTED, ..Reply::json(json!({ "task": id })) })
        }
        Started::Job(job) => match job.await {
            Ok(answer) => Ok(Reply::json(answer?)),
            Err(_) => Err(Failure::new(Kind::Internal, "the action's task did not end")),
        },
    }
}

/// An action that was started.
pub(super) enum Started {
    /// It is long, and runs as the task with this number.
    Task(i64),
    /// Its command runs in a task of its own, which gives its answer.
    Job(JoinHandle<Result<Value, Failure>>),
}

/// Starts `prepared` for the session `live` or the token `token`, one of
/// which the request has, once every guard has let it through. `for_client`
/// names the MCP client an administrator approved it for (ADR 0690),
/// as the journal says it: the change is the session's all the same.
///
/// The command runs in a task of its own, which the answer waits for: a
/// browser that goes away while it runs does not end it half way, with its
/// command hung up and no row in the journal.
///
/// # Errors
///
/// A guard's refusal, before anything ran: ADM-136, ADM-142, UPD-106, or
/// for a long action what [`start_task`] refuses with.
pub(super) async fn start(
    cx: &Context<'_>,
    prepared: Prepared,
    live: Option<Live>,
    token: Option<TokenCall>,
    for_client: Option<String>,
) -> Result<Started, Failure> {
    let (asker, whose) = match (&live, &token) {
        (Some(live), None) => match for_client {
            Some(client) => {
                let whose = format!("{}, for AI {client}", shown(&live.name));
                (Asker { who: live.name.clone(), how: client }, whose)
            }
            None => (Asker { who: live.name.clone(), how: "console".into() }, shown(&live.name)),
        },
        (None, Some(token)) => (
            Asker { who: token.owner.clone(), how: format!("token {}", token.name) },
            format!("{} (API token {})", shown(&token.owner), shown(&token.name)),
        ),
        // `check` lets an action through with exactly one of them.
        _ => return Err(super::ended()),
    };
    // What could lock the caller out is not done to their own account from
    // the console or with their token (ADR 0562).
    if prepared.not_self.as_deref() == Some(asker.who.as_str()) {
        return Err(Failure::new(
            Kind::NotSelf,
            "the console does not do this to your own account, so that it cannot lock you out: \
             another administrator does it, or `elevate user` at a terminal",
        ));
    }
    // Where the request came from and by which card, for a change that
    // could cut off the browser that asks for it (ADR 0562).
    let came = prepared.cuts_off.as_ref().map(|_| Came {
        from: reach::plain(cx.peer),
        card: cx.connection.to.and_then(|to| reach::card_of(to.ip())),
        host: cx.host.clone(),
    });
    let (shared, peer) = (Rc::clone(cx.shared), cx.peer);
    // Before a task too: a confirming try of the search is long.
    if let Some(restarts) = prepared.action.restarts() {
        let refused =
            refuse_restarting(&shared, &prepared, restarts, live.as_ref(), token.as_ref()).await;
        if let Err(failure) = refused {
            let id = failure.own_id.as_deref().unwrap_or(failure.kind.id());
            log(6, &format!("{whose} from {peer}: {} was not run: {id}", prepared.action.id()));
            return Err(failure);
        }
    }
    if prepared.action.long() {
        return long(&shared, peer, prepared, &asker, &whose, live.as_ref(), token.as_ref())
            .map(Started::Task);
    }
    let job = tokio::task::spawn_local(async move {
        if let (Some(read), Some(came)) = (&prepared.cuts_off, &came) {
            let refused =
                refuse_cutting_off(&shared, &prepared, read, came, live.as_ref(), token.as_ref())
                    .await;
            // A refusal here, of the caller's rights or of the cut, is logged
            // as one of the run's own is.
            if let Err(failure) = refused {
                let done = Err(failure);
                said(peer, prepared.action, &whose, &done);
                return done.and_then(|done: Done| run::answer(&prepared, &done.ran, done.changed));
            }
        }
        let begun = prepared
            .action
            .changes()
            .then(|| shared.journal.begin(shared.store.as_ref(), &prepared.files, unix_now()));
        let probed = match (&live, &token) {
            (Some(live), _) => for_session(&shared, &prepared, live).await,
            (None, Some(token)) => for_token(&shared, &prepared, token).await,
            (None, None) => Err(super::ended()),
        };
        // The time a trial began is kept as soon as its change answers well,
        // before anything else is awaited: the page asks for it next. The
        // open tabs are told at once of one that ends, and of one that begins
        // when Spark has been read, which has the time it has left.
        if let (Ok(probed), Some(trial)) = (&probed, prepared.trial)
            && probed.ran.status == 0
        {
            match trial {
                Trial::Begins(id) => {
                    shared.trials.borrow_mut().begun(id, Instant::now());
                    let reading = Rc::clone(&shared);
                    tokio::task::spawn_local(async move { look_at_trials(&reading).await });
                }
                Trial::Ends(id) => {
                    shared.trials.borrow_mut().ended(id);
                    shared.tell_trials(Instant::now());
                }
            }
        }
        // A restart or a shut down set or cancelled is told to the open
        // tabs as soon as Spark says so.
        let sets_power =
            prepared.action.area() == actions::POWER || prepared.action.restarts().is_some();
        if let Ok(probed) = &probed
            && probed.ran.status == 0
            && sets_power
            && prepared.action.changes()
        {
            let reading = Rc::clone(&shared);
            tokio::task::spawn_local(async move { look_at_trials(&reading).await });
        }
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
    Ok(Started::Job(job))
}

/// Checks that a session's read that runs as an administrator comes from
/// the console's own page (ADR 0518). A browser says where a request
/// comes from in `Sec-Fetch-Site`; one from another site is refused,
/// though the cookie's `SameSite=Strict` already keeps it from being sent
/// with one. A client that does not say is let through, as a script with
/// the cookie is.
///
/// # Errors
///
/// ADM-109 for a request from another site, or from the same site at
/// another origin.
pub(super) fn from_the_console(cx: &Context<'_>) -> Result<(), Failure> {
    match cx.headers.get("sec-fetch-site").map(http::HeaderValue::as_bytes) {
        None | Some(b"same-origin") => Ok(()),
        Some(_) => Err(Failure::new(
            Kind::ForeignRequest,
            "a read that runs as an administrator is asked for by the console's own page",
        )),
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
    let probes = Probes::Worker(&live.link);
    match prepared.action.runs() {
        Runs::AsAdministrator => {
            let by = session_elevator(live);
            let input = prepared.input.as_ref();
            let command = run::elevated(shared, by, &prepared.argv, input, run::RUN_TIME);
            probed(shared, prepared, &probes, command).await
        }
        Runs::AsThePerson => {
            let command = run::as_the_person(&live.link, &prepared.argv, prepared.input.as_ref());
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
            let input = prepared.input.as_ref();
            let command = run::elevated(shared, by, &prepared.argv, input, run::RUN_TIME);
            probed(shared, prepared, &probes, command).await
        }
        Runs::AsThePerson => {
            let link = caller::worker(shared, token).await?;
            let command = run::as_the_person(&link, &prepared.argv, prepared.input.as_ref());
            probed(shared, prepared, &probes, command).await
        }
    }
}

/// Where a request came from, for [`refuse_cutting_off`].
struct Came {
    /// The browser's address.
    from: IpAddr,
    /// The card of the address it came to; None when admind does not know.
    card: Option<String>,
    /// The name or address it asked for, in small letters, without the
    /// port.
    host: String,
}

/// Refuses a change that would cut off the browser that asks for it
/// (ADR 0562, ADM-137), or leave it at a name the console's certificate
/// does not hold (ADR 0567, ADM-141). A request that came by another card
/// than the one the change can cut off goes on, with nothing read; a
/// guard by name reads for every request. Otherwise the action's read runs
/// as the caller's administrator, as the change would, and the action says
/// from what it printed whether the browser still reaches the console
/// after the change. A read that fails refuses the change: the guard fails
/// closed.
///
/// # Errors
///
/// As [`actions::authorize`] for the caller; ADM-137 or ADM-141 for a
/// change that would cut the browser off, or whose read failed.
async fn refuse_cutting_off(
    shared: &Rc<Shared>,
    prepared: &Prepared,
    read: &CutsOffRead,
    came: &Came,
    live: Option<&Live>,
    token: Option<&TokenCall>,
) -> Result<(), Failure> {
    if let Through::Card(card) = read.on
        && came.card.as_deref() != Some(card)
    {
        return Ok(());
    }
    let kind = match read.on {
        Through::Card(_) => Kind::CutsOff,
        Through::Name => Kind::NotNamed,
    };
    let by = match (live, token) {
        (Some(live), _) => {
            permit_session(shared, prepared, live)?;
            session_elevator(live)
        }
        (None, Some(token)) => {
            permit_token(prepared, token)?;
            token_elevator(prepared, token)
        }
        (None, None) => return Err(super::ended()),
    };
    let shown = match run::elevated(shared, by, &read.argv, None, run::RUN_TIME).await {
        Ok(ran) if ran.status == 0 => ran.output,
        _ => {
            let why = match read.on {
                Through::Card(card) => format!(
                    "this request came through {card}, and what the change would take away from \
                     it could not be read: make the change from another connection"
                ),
                Through::Name => format!(
                    "the names the change would leave the console answering by could not be \
                     read, so the console cannot tell whether this browser's address, {}, is \
                     one of them: nothing was done",
                    came.host
                ),
            };
            return Err(Failure::new(kind, why));
        }
    };
    let browser = Browser { from: came.from, host: &came.host };
    prepared
        .action
        .cuts_off(&prepared.args, &browser, &shown)
        .map_err(|why| Failure::new(kind, why))
}

/// Refuses a change that restarts the machine (ADR 0565) while a trial of
/// settings runs, which the restart would undo half way (ADM-142), and
/// while the other kind of restart is set or runs (UPD-106): for a
/// restart or a shut down, the search for a bad change, whose tries
/// restart the machine on their own and would be spoiled; for the search,
/// a restart or a shut down that is set. When admind cannot tell, because
/// Spark does not answer or the search's state does not read, the change
/// is refused all the same: the guard fails closed. The caller is checked
/// first, so that only one who may make the change learns what runs.
///
/// # Errors
///
/// As [`actions::authorize`] for the caller; ADM-142 and UPD-106 as above.
async fn refuse_restarting(
    shared: &Rc<Shared>,
    prepared: &Prepared,
    restarts: Restarts,
    live: Option<&Live>,
    token: Option<&TokenCall>,
) -> Result<(), Failure> {
    let by = match (live, token) {
        (Some(live), _) => {
            permit_session(shared, prepared, live)?;
            session_elevator(live)
        }
        (None, Some(token)) => {
            permit_token(prepared, token)?;
            token_elevator(prepared, token)
        }
        (None, None) => return Err(super::ended()),
    };
    let document = trials::document(shared.spark.get()).await.map_err(|why| {
        let why = format!(
            "{why}, so the console cannot tell whether a trial runs, which a restart would undo: \
             nothing was done"
        );
        Failure::new(Kind::TrialRuns, why)
    })?;
    if let Some(trial) = document.trials.first() {
        let (page, _) = trials::owner(&trial.id);
        let why = format!(
            "the trial `{}` runs, and a restart would undo it half way: keep or undo it at {page} \
             first; nothing was done",
            trial.id
        );
        return Err(Failure::new(Kind::TrialRuns, why));
    }
    match restarts {
        Restarts::Search => match &document.shutdown {
            None => Ok(()),
            Some(set) => Err(other_restart(format!(
                "a {} is set, in {} seconds, and the search restarts the machine on its own: \
                 cancel it first; nothing was done",
                set.end, set.in_seconds
            ))),
        },
        Restarts::Machine => {
            let argv: Vec<String> = shared.search.get().iter().map(ToString::to_string).collect();
            let running = match run::elevated(shared, by, &argv, None, run::RUN_TIME).await {
                Ok(ran) if ran.status == 0 => searching(&ran.output),
                _ => None,
            };
            match running {
                Some(false) => Ok(()),
                Some(true) => Err(other_restart(
                    "the search for a bad change runs, and restarts the machine for each try: \
                     stop it on the Updates page first; nothing was done"
                        .into(),
                )),
                None => Err(other_restart(
                    "the console cannot tell whether the search for a bad change runs, whose \
                     tries a restart would spoil: nothing was done"
                        .into(),
                )),
            }
        }
    }
}

/// [`actions::OTHER_RESTART`], with `detail`: a restart or a shut down is
/// set, or the search for a bad change runs.
fn other_restart(detail: String) -> Failure {
    let id = actions::OTHER_RESTART.0.to_string();
    Failure { own_id: Some(id), ..Failure::new(Kind::Cannot, detail) }
}

/// Whether `changes bisect --status --json` printed that the search runs;
/// None when what it printed does not say.
fn searching(output: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(output.trim()).ok()?;
    value.get("running")?.as_bool()
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
pub(in crate::admind) enum Probes<'a> {
    /// A worker that runs: a session's, or the one an MCP client's owner
    /// reads through.
    Worker(&'a Link),
    /// The owner's worker is started when a probe needs it.
    Token(&'a TokenCall),
}

impl Probes<'_> {
    /// What the probe `argv` prints, as the person; None when it cannot be
    /// run or does not end well. A probe that fails is no reason to fail
    /// the change.
    async fn read(&self, shared: &Shared, argv: &[String]) -> Option<String> {
        let link = match self {
            Probes::Worker(link) => (*link).clone(),
            Probes::Token(token) => caller::worker(shared, token).await.ok()?,
        };
        let ran = run::as_the_person(&link, argv, None).await.ok()?;
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
/// the token `token`, and gives the task's number, which is answered 202. The checks
/// are the short action's; the command is not run in the request, so a
/// long action runs as an administrator only: a person's own worker does
/// not stream what a command writes (a unit test keeps the actions to
/// that).
///
/// A refusal is in the log as a short action's is ([`said`]).
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
) -> Result<i64, Failure> {
    let action = prepared.action.id();
    let started = start_task(shared, peer, prepared, asker, whose, live, token);
    if let Err(failure) = &started {
        log(6, &format!("{whose} from {peer}: {action} was not run: {}", failure.kind.id()));
    }
    started
}

/// [`long`], but for the log's line of a refusal.
fn start_task(
    shared: &Rc<Shared>,
    peer: IpAddr,
    prepared: Prepared,
    asker: &Asker,
    whose: &str,
    live: Option<&Live>,
    token: Option<&TokenCall>,
) -> Result<i64, Failure> {
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
    Ok(id)
}

/// What the log says an action asked for came to, which is the end of its
/// line: for a change always, with "changed nothing" when it ended well and
/// moved nothing; for a read only when it was refused or failed, which a
/// status it answers with ([`Action::ANSWERS`](crate::admind::actions::Action::ANSWERS)) is not.
fn told(action: &dyn Registered, done: &Result<Done, Failure>) -> Option<String> {
    match done {
        Ok(Done { ran, .. }) if !action.changes() && action.answers().contains(&ran.status) => None,
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
/// let it read the action's area. Nothing is run, and a secret is neither
/// needed nor kept: it is shown as what is typed when asked
/// ([`show::prepare`]).
pub(super) fn show(cx: &Context<'_>, token: Option<&TokenCall>) -> Outcome {
    let showing: Showing = cx.json()?;
    let action = show::lookup(cx.shared.actions.get(), &showing.action)?;
    let prepared = show::prepare(action, showing.args)?;
    if let Some(token) = token {
        actions::token_may(&token.name, &token.scopes, action.area(), false)?;
    }
    Ok(Reply::json(show::shown(&prepared, &cx.origin)))
}

/// Answers `POST /api/1/check` (ADR 0545), which takes what
/// `/api/1/show` takes: whether the action with the arguments would change
/// anything, for any session that is signed in, locked or not, or for the
/// API token `token` when its scopes let it read the action's area. The
/// change is not made: its probe runs as the person, as the change's own
/// does, and the files it names are read as the journal reads them. The
/// answer is `changed`, true, false or null when the action cannot tell,
/// and `now`, what the probe reads of what the change moves.
pub(super) async fn check(
    cx: &Context<'_>,
    live: Option<Live>,
    token: Option<TokenCall>,
) -> Outcome {
    let showing: Showing = cx.json()?;
    let action = show::lookup(cx.shared.actions.get(), &showing.action)?;
    let prepared = action.prepare(showing.args)?;
    let probes = match (&live, &token) {
        (Some(live), None) => Probes::Worker(&live.link),
        (None, Some(token)) => {
            actions::token_may(&token.name, &token.scopes, action.area(), false)?;
            Probes::Token(token)
        }
        _ => return Err(super::ended()),
    };
    let (changed, now) = would(cx.shared, &prepared, &probes).await;
    Ok(Reply::json(json!({ "changed": changed, "now": now })))
}

/// Whether the action `prepared` would change anything, and what its
/// probe reads now of what it moves, as `/api/1/check` answers them: the
/// probe runs through `probes`, and the files the action names are read
/// as the journal reads them. A read changes nothing, and has nothing
/// that is there now.
pub(in crate::admind) async fn would(
    shared: &Shared,
    prepared: &Prepared,
    probes: &Probes<'_>,
) -> (Option<bool>, Option<String>) {
    let action = prepared.action;
    if !action.changes() {
        return (Some(false), None);
    }
    let now = match prepared.probe.as_deref() {
        Some(probe) => probes
            .read(shared, probe)
            .await
            .and_then(|output| action.probed(&prepared.args, &output)),
        None => None,
    };
    let kept: Vec<Option<bool>> = prepared
        .files
        .iter()
        .map(|path| {
            let text = journal::current_text(path)?;
            action.keeps_file(&prepared.args, path, text.as_deref())
        })
        .collect();
    (would_change(prepared, now.as_deref(), &kept), now)
}

/// Whether the change `prepared` would change anything (ADR 0545), from
/// what its probe picks now (`now`, None when it has no probe or the probe
/// did not tell) and whether it would keep each file it names (`kept`,
/// None for one it cannot tell of).
///
/// It would when any part it compares would move, and would not only when
/// every part is settled; otherwise it cannot tell, None. A change that
/// compares nothing always does something, as its `changed` says once it
/// runs, and a read changes nothing.
fn would_change(prepared: &Prepared, now: Option<&str>, kept: &[Option<bool>]) -> Option<bool> {
    if !prepared.action.changes() {
        return Some(false);
    }
    if prepared.probe.is_none() && prepared.files.is_empty() {
        return Some(true);
    }
    let mut settled = kept.to_vec();
    if prepared.probe.is_some() {
        settled.push(now.and_then(|now| prepared.action.settled(&prepared.args, now)));
    }
    if settled.contains(&Some(false)) {
        Some(true)
    } else if settled.iter().all(|part| *part == Some(true)) {
        Some(false)
    } else {
        None
    }
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
        // service explain answers after 3 as after 0, and fails after 4.
        let explain = crate::admind::actions::REGISTRY
            .iter()
            .copied()
            .find(|action| action.id() == "service.explain")
            .unwrap();
        assert_eq!(told(explain, &done(3, false)), None);
        assert_eq!(told(explain, &done(4, false)).as_deref(), Some("ended with status 4"));
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

    #[test]
    fn a_check_says_would_change_when_a_part_moves_and_not_only_when_every_part_is_settled() {
        let name = serde_json::json!({ "name": "lab-web" });
        // A probe alone: what `settled` says of what it reads.
        let set = prepared("lab.set", name.clone(), &[]);
        assert_eq!(would_change(&set, Some("on"), &[]), Some(false));
        assert_eq!(would_change(&set, Some("off"), &[]), Some(true));
        assert_eq!(would_change(&set, Some("?"), &[]), None);
        assert_eq!(would_change(&set, None, &[]), None);
        // A probe and a file: one part that moves is enough, and both must
        // be settled for no change.
        let set_file = prepared("lab.set", name.clone(), &["/s/lab-web.toml"]);
        assert_eq!(would_change(&set_file, Some("on"), &[Some(true)]), Some(false));
        assert_eq!(would_change(&set_file, Some("on"), &[Some(false)]), Some(true));
        assert_eq!(would_change(&set_file, None, &[Some(false)]), Some(true));
        assert_eq!(would_change(&set_file, Some("off"), &[None]), Some(true));
        assert_eq!(would_change(&set_file, Some("on"), &[None]), None);
        // No probe and a file: the file alone says.
        let restart_file = prepared("lab.restart", name.clone(), &["/s/lab-web.toml"]);
        assert_eq!(would_change(&restart_file, None, &[Some(true)]), Some(false));
        assert_eq!(would_change(&restart_file, None, &[None]), None);
        // Nothing to compare: it always does something. A read never does.
        let restart = prepared("lab.restart", name, &[]);
        assert_eq!(would_change(&restart, None, &[]), Some(true));
        let list = prepared("lab.list", serde_json::json!({}), &[]);
        assert_eq!(would_change(&list, None, &[]), Some(false));
    }
}
