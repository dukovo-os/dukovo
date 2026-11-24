// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The six tools of the console's MCP server (ADR 0690).
//!
//! The console has some two hundred actions, each with its own
//! arguments. A client is not sent them all as tools: it is sent six,
//! and finds the actions through them, so what it must read first stays
//! small and is the same on every machine.
//!
//! | Tool | Does |
//! |---|---|
//! | `areas` | lists the areas the client reaches, and how far |
//! | `actions` | lists an area's actions the client may use, with their arguments |
//! | `read` | runs an action that reads |
//! | `check` | says whether a change would change anything, without making it |
//! | `ask` | files a request for a change, which an administrator answers |
//! | `request` | says where a request stands |
//!
//! Each call is counted against its client ([`client::begin`]), checked
//! against what the client's scopes reach ([`Reach::allows`]), and has
//! a line in admind's log. A read that runs as the client's owner runs
//! in the owner's worker; one that runs as an administrator goes
//! through authd and `admind --read` ([`run::checked_read`]). No tool
//! makes a change: `ask` only files a request
//! ([`requests`](super::requests)).
//!
//! A tool's arguments come to at most 16 KiB and its answer to 256 KiB;
//! an answer that is longer is refused with words that say to narrow
//! the read.

use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::super::actions::{self, Registered, show};
use super::super::errors::{Failure, Kind};
use super::super::log;
use super::super::routes::act::{self, Probes};
use super::super::run;
use super::super::shared::{Shared, unix_now};
use super::areas;
use super::client::{self, Client};
use super::history::{Entry, What};
use super::offer::{Level, Offer, offer};
use super::protocol::Tool;
use super::requests::Asking;

/// The most bytes of a call's arguments, as JSON.
pub const ARGUMENTS_MAX: usize = 16 * 1024;
/// The most bytes of a call's answer, as JSON.
pub const ANSWER_MAX: usize = 256 * 1024;
/// The most characters of the reason a client gives for a change.
pub const REASON_MAX: usize = 200;
/// How long `ask` waits for an administrator before it answers that the
/// request waits.
pub const ASK_WAIT: Duration = Duration::from_secs(45);

/// The tools, in the order a client is told of them.
pub const TOOLS: [Tool; 6] = [
    Tool {
        name: "areas",
        title: "List the areas",
        description: "Lists the areas of this server's console that you reach, and how far in \
            each: `read` is what your owner's account sees before it unlocks the console, \
            `read all` is every read, and `read all and ask` adds asking for changes. Begin \
            here, then call `actions` for an area.",
        input: no_arguments,
        reads: true,
    },
    Tool {
        name: "actions",
        title: "List an area's actions",
        description: "Lists the actions of one area that you may use: each action's ID, whether \
            it is a read or a change, its title, and the JSON Schema of its arguments. Give an \
            action's ID to `read` when it is a read, and to `check` or `ask` when it is a change.",
        input: area_argument,
        reads: true,
    },
    Tool {
        name: "read",
        title: "Read",
        description: "Runs an action that reads, and answers what it found as JSON. It changes \
            nothing on the server.",
        input: action_arguments,
        reads: true,
    },
    Tool {
        name: "check",
        title: "Check a change",
        description: "Says whether a change would change anything, and what is there now, \
            without making the change. `changes` is true, false, or null when the server \
            cannot tell.",
        input: action_arguments,
        reads: true,
    },
    Tool {
        name: "ask",
        title: "Ask for a change",
        description: "Asks an administrator to approve a change. Nothing changes until a person \
            approves the request in the console, and the change then runs as that person. Give \
            the reason in one sentence that the person can judge. Answers what came of it when \
            that is known within 45 seconds, and otherwise the request's number, to give to \
            `request` later. Asking again for the same change while it waits gives the same \
            request.",
        input: ask_arguments,
        reads: false,
    },
    Tool {
        name: "request",
        title: "Look at a request",
        description: "Says where a request made with `ask` stands: waiting, approved, done, \
            failed, a task that runs, refused or lapsed, with what the change answered.",
        input: request_argument,
        reads: true,
    },
];

/// The schema of a tool that takes nothing.
fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// The schema of `actions`.
fn area_argument() -> Value {
    json!({
        "type": "object",
        "properties": {
            "area": { "type": "string", "description": "An area's ID, as `areas` lists them." },
        },
        "required": ["area"],
        "additionalProperties": false,
    })
}

/// The properties `read`, `check` and `ask` share.
fn action_properties() -> Value {
    json!({
        "action": {
            "type": "string",
            "description": "An action's ID, as `actions` lists them, such as service.status.",
        },
        "arguments": {
            "type": "object",
            "description": "The action's arguments, by the schema `actions` gives for it. Leave \
                it out for an action that takes none.",
        },
    })
}

/// The schema of `read` and `check`.
fn action_arguments() -> Value {
    json!({
        "type": "object",
        "properties": action_properties(),
        "required": ["action"],
        "additionalProperties": false,
    })
}

/// The schema of `ask`.
fn ask_arguments() -> Value {
    let mut properties = action_properties();
    properties["reason"] = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": REASON_MAX,
        "description": "Why the change is needed, in one sentence, for the administrator who \
            decides.",
    });
    json!({
        "type": "object",
        "properties": properties,
        "required": ["action", "reason"],
        "additionalProperties": false,
    })
}

/// The schema of `request`.
fn request_argument() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "integer",
                "minimum": 1,
                "description": "The request's number, as `ask` gave it.",
            },
        },
        "required": ["id"],
        "additionalProperties": false,
    })
}

/// Runs `client`'s call of the tool `name` with `arguments`, and gives
/// its answer, an object.
///
/// # Errors
///
/// ADM-105 when the client has too much running or made too many calls;
/// ADM-113 for arguments or an answer that is too long; ADM-125 for
/// arguments the tool or the action does not take; ADM-144 for what the
/// client may not reach; and whatever the action's command failed with.
pub async fn call(
    shared: &Rc<Shared>,
    client: &Client,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<Value, Failure> {
    let _running = client::begin(shared, client, Instant::now()).inspect_err(|failure| {
        log(6, &format!("{} was refused {name}: {}", client.told(), failure.kind.id()));
    })?;
    let (did, outcome) = match within(&arguments) {
        Err(failure) => (format!("called {name}"), Err(failure)),
        Ok(()) => match name {
            "areas" => ("listed its areas".into(), areas_reached(shared, client, &arguments)),
            "actions" => actions_of(shared, client, &arguments),
            "read" => read(shared, client, &arguments).await,
            "check" => check(shared, client, &arguments).await,
            "ask" => ask(shared, client, &arguments).await,
            "request" => request(shared, client, &arguments),
            _ => (
                format!("called {name}"),
                Err(Failure::new(Kind::Internal, "the server has no such tool")),
            ),
        },
    };
    let outcome = outcome.and_then(fitting);
    match &outcome {
        Ok(_) => log(6, &format!("{} {did}", client.told())),
        Err(failure) => {
            let id = failure.own_id.as_deref().unwrap_or(failure.kind.id());
            log(6, &format!("{} {did}, and was refused: {id}", client.told()));
        }
    }
    outcome
}

/// Refuses arguments that come to more than [`ARGUMENTS_MAX`].
fn within(arguments: &Map<String, Value>) -> Result<(), Failure> {
    let bytes = serde_json::to_vec(arguments).map_or(usize::MAX, |json| json.len());
    if bytes > ARGUMENTS_MAX {
        let why = format!("a tool's arguments come to at most {ARGUMENTS_MAX} bytes");
        return Err(Failure::new(Kind::TooBig, why));
    }
    Ok(())
}

/// Refuses an answer that comes to more than [`ANSWER_MAX`].
fn fitting(answer: Value) -> Result<Value, Failure> {
    let bytes = serde_json::to_vec(&answer).map_or(usize::MAX, |json| json.len());
    if bytes > ANSWER_MAX {
        let why = format!(
            "the answer comes to {} KiB, and an AI client is sent at most {}: narrow the read \
             with its arguments",
            bytes / 1024,
            ANSWER_MAX / 1024
        );
        return Err(Failure::new(Kind::TooBig, why));
    }
    Ok(answer)
}

/// Refuses arguments with a name that is none of `known`.
fn only(arguments: &Map<String, Value>, known: &[&str]) -> Result<(), Failure> {
    match arguments.keys().find(|name| !known.contains(&name.as_str())) {
        None => Ok(()),
        Some(_) => {
            let why = format!("the tool takes {} and nothing else", list(known));
            Err(Failure::new(Kind::BadRequest, why))
        }
    }
}

/// `words` as a sentence lists them, or `no arguments` for none.
fn list(words: &[&str]) -> String {
    match words {
        [] => "no arguments".into(),
        [one] => (*one).into(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// The text the argument `name` gives.
fn text<'a>(arguments: &'a Map<String, Value>, name: &'static str) -> Result<&'a str, Failure> {
    match arguments.get(name) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text),
        Some(Value::String(_)) => Err(Failure::bad_value(name, "it is empty")),
        Some(_) => Err(Failure::bad_value(name, "it is not text")),
        None => Err(Failure::bad_value(name, "it is not given")),
    }
}

/// The action that `arguments` name, and the arguments they give it:
/// `action`, and `arguments`, an object or nothing.
fn named(
    shared: &Shared,
    arguments: &Map<String, Value>,
) -> Result<(&'static dyn Registered, Value), Failure> {
    let action = show::lookup(shared.actions.get(), text(arguments, "action")?)?;
    let given = match arguments.get("arguments") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(Value::Object(given)) => Value::Object(given.clone()),
        Some(_) => return Err(Failure::bad_value("arguments", "it is not an object")),
    };
    Ok((action, given))
}

/// A refusal of what `client` may not do.
fn may_not(why: String) -> Failure {
    Failure::new(Kind::ClientMayNot, why)
}

/// `areas`: each area `client` reaches and has something to do in.
fn areas_reached(
    shared: &Shared,
    client: &Client,
    arguments: &Map<String, Value>,
) -> Result<Value, Failure> {
    only(arguments, &[])?;
    let actions = shared.actions.get();
    let reached: Vec<Value> = areas::AREAS
        .iter()
        .filter_map(|(area, title)| {
            let counts = areas::counts(actions, area);
            let (reach, reads, changes) = match client.reach.level(area) {
                Level::Off => return None,
                Level::Read => ("read", counts.reads, 0),
                Level::All if counts.asks > 0 => {
                    ("read all and ask", counts.reads + counts.reads_all, counts.asks)
                }
                Level::All => ("read all", counts.reads + counts.reads_all, 0),
            };
            (reads + changes > 0).then(|| {
                json!({
                    "area": area,
                    "title": title,
                    "reach": reach,
                    "reads": reads,
                    "changes": changes,
                })
            })
        })
        .collect();
    Ok(json!({ "areas": reached }))
}

/// `actions`: the actions of one area that `client` may use.
fn actions_of(
    shared: &Shared,
    client: &Client,
    arguments: &Map<String, Value>,
) -> (String, Result<Value, Failure>) {
    let area = match only(arguments, &["area"]).and_then(|()| text(arguments, "area")) {
        Ok(area) => area,
        Err(failure) => return ("called actions".into(), Err(failure)),
    };
    let Some((area, title)) = areas::AREAS.iter().find(|(id, _)| *id == area) else {
        let why = "there is no such area: the tool areas lists them";
        return ("called actions".into(), Err(Failure::bad_value("area", why)));
    };
    let did = format!("listed the actions of {area}");
    let level = client.reach.level(area);
    if level == Level::Off {
        return (did, Err(may_not(format!("this AI client may not read {area}"))));
    }
    let listed: Vec<Value> = shared
        .actions
        .get()
        .iter()
        .filter(|action| action.area() == *area)
        .filter_map(|action| {
            let offered = offer(*action).ok()?;
            let (kind, runs_as) = match offered {
                Offer::Read => ("read", "owner"),
                Offer::ReadAll if level == Level::All => ("read", "administrator"),
                Offer::Ask if level == Level::All => ("change", "administrator"),
                Offer::ReadAll | Offer::Ask => return None,
            };
            Some(json!({
                "id": action.id(),
                "kind": kind,
                "title": action.title(),
                "arguments": actions::schema(action.fields()),
                "as": runs_as,
            }))
        })
        .collect();
    (did, Ok(json!({ "area": area, "title": title, "actions": listed })))
}

/// `read`: runs a read `client` reaches, and answers what it found.
async fn read(
    shared: &Rc<Shared>,
    client: &Client,
    arguments: &Map<String, Value>,
) -> (String, Result<Value, Failure>) {
    let (action, given) =
        match only(arguments, &["action", "arguments"]).and_then(|()| named(shared, arguments)) {
            Ok(named) => named,
            Err(failure) => return ("called read".into(), Err(failure)),
        };
    let did = format!("read {}", action.id());
    let answered = async {
        let offered = client.reach.allows(action)?;
        if offered == Offer::Ask {
            let why = format!("{} is a change: ask for it with the tool ask", action.id());
            return Err(may_not(why));
        }
        let prepared = action.prepare(given.clone())?;
        let ran = if offered == Offer::Read {
            let link = client::worker(shared, client).await?;
            run::as_the_person(&link, &prepared.argv, None).await?
        } else {
            let credential = client.credential();
            run::checked_read(shared, credential, action.id(), &given, run::RUN_TIME).await?
        };
        let answer = run::answer(&prepared, &ran, false)?;
        Ok(json!({ "action": action.id(), "answer": answer }))
    }
    .await;
    if answered.is_ok() {
        let mut history = shared.client_history.borrow_mut();
        history.read(&client.owner, &client.name, action.area(), unix_now());
    }
    (did, answered)
}

/// `check`: whether a change `client` may ask for would change anything,
/// and what is there now. Its probe runs as the client's owner.
async fn check(
    shared: &Rc<Shared>,
    client: &Client,
    arguments: &Map<String, Value>,
) -> (String, Result<Value, Failure>) {
    let (action, given) =
        match only(arguments, &["action", "arguments"]).and_then(|()| named(shared, arguments)) {
            Ok(named) => named,
            Err(failure) => return ("called check".into(), Err(failure)),
        };
    let did = format!("checked {}", action.id());
    let answered = async {
        client.reach.allows(action)?;
        let prepared = action.prepare(given)?;
        let (changes, now) = if action.changes() {
            let link = client::worker(shared, client).await?;
            act::would(shared, &prepared, &Probes::Worker(&link)).await
        } else {
            (Some(false), None)
        };
        Ok(json!({ "action": action.id(), "changes": changes, "now": now }))
    }
    .await;
    (did, answered)
}

/// The reason the arguments give for a change: 1 to [`REASON_MAX`]
/// characters, on one line.
fn reason(arguments: &Map<String, Value>) -> Result<&str, Failure> {
    let reason = text(arguments, "reason")?.trim();
    if reason.is_empty() {
        return Err(Failure::bad_value("reason", "it is empty"));
    }
    if reason.chars().count() > REASON_MAX {
        let why = format!("it is longer than {REASON_MAX} characters");
        return Err(Failure::bad_value("reason", why));
    }
    if reason.chars().any(char::is_control) {
        return Err(Failure::bad_value("reason", "it is one line of text"));
    }
    Ok(reason)
}

/// `ask`: files `client`'s request for a change, and waits a while for
/// an administrator's answer.
async fn ask(
    shared: &Rc<Shared>,
    client: &Client,
    arguments: &Map<String, Value>,
) -> (String, Result<Value, Failure>) {
    let (action, given) = match only(arguments, &["action", "arguments", "reason"])
        .and_then(|()| named(shared, arguments))
    {
        Ok(named) => named,
        Err(failure) => return ("called ask".into(), Err(failure)),
    };
    let filed = (|| {
        let reason = reason(arguments)?;
        if client.reach.allows(action)? != Offer::Ask {
            let why = format!("{} is a read: run it with the tool read", action.id());
            return Err(may_not(why));
        }
        // Checked as the API checks them, so that what waits can run.
        let name = action.prepare(given.clone())?.ansible.name;
        super::lapse(shared, Instant::now());
        let asking = Asking {
            owner: &client.owner,
            client: &client.name,
            action,
            name,
            args: given,
            reason,
        };
        let (id, new) =
            shared.client_requests.borrow_mut().ask(asking, Instant::now(), unix_now())?;
        if new {
            super::moved(shared, id);
        }
        Ok(id)
    })();
    let id = match filed {
        Ok(id) => id,
        Err(failure) => return (format!("asked for {}", action.id()), Err(failure)),
    };
    let did = format!("asked for {} (request {id})", action.id());
    let mut news = shared.client_requests.borrow().subscribe();
    let until = tokio::time::Instant::now() + shared.ask_wait.get();
    loop {
        let settled = {
            let requests = shared.client_requests.borrow();
            requests.get(id).is_none_or(|request| request.state.settled())
        };
        if settled || tokio::time::timeout_at(until, news.changed()).await.is_err() {
            break;
        }
    }
    (did, told(shared, client, id))
}

/// `request`: where `client`'s request stands.
fn request(
    shared: &Rc<Shared>,
    client: &Client,
    arguments: &Map<String, Value>,
) -> (String, Result<Value, Failure>) {
    let id = only(arguments, &["id"]).and_then(|()| {
        let number = arguments.get("id").and_then(Value::as_u64).filter(|id| *id > 0);
        number.ok_or_else(|| Failure::bad_value("id", "it is the request's number, as ask gave it"))
    });
    let id = match id {
        Ok(id) => id,
        Err(failure) => return ("called request".into(), Err(failure)),
    };
    super::lapse(shared, Instant::now());
    (format!("looked at request {id}"), told(shared, client, id))
}

/// What `client` is told of its request `id`: as admind holds it, with
/// its task's state when it runs as one, or as the history has it when
/// admind holds it no more.
fn told(shared: &Shared, client: &Client, id: u64) -> Result<Value, Failure> {
    let held = {
        let requests = shared.client_requests.borrow();
        requests
            .get(id)
            .filter(|request| request.is_of(&client.owner, &client.name))
            .map(super::requests::Request::told)
    };
    let mut told = match held {
        Some(told) => told,
        None => {
            let history = shared.client_history.borrow();
            let old = history.request(id, &client.owner, &client.name).and_then(told_of_old);
            old.ok_or_else(|| {
                Failure::new(Kind::NoRequest, format!("this AI client has no request {id}"))
            })?
        }
    };
    if let (Some(task), Some(store)) = (told["task"].as_i64(), shared.store.as_ref())
        && let Ok(Some(task)) = shared.tasks.get(store, task)
    {
        told["task"] = task.summary();
    }
    Ok(told)
}

/// What a client is told of a request that only the history has: where
/// it ended, without what the change answered, which is not kept.
fn told_of_old(entry: &Entry) -> Option<Value> {
    let What::Ask { request, action, state, by, task, ended, .. } = &entry.what else {
        return None;
    };
    let mut told = json!({
        "request": request,
        "action": action,
        "state": state,
        "asked": entry.at,
        "note": "This request was settled a while ago: what the change answered is not kept.",
    });
    if !by.is_empty() {
        told["by"] = by.as_str().into();
    }
    if let Some(task) = task {
        told["task"] = (*task).into();
    }
    if let Some(ended) = ended {
        told["ended"] = (*ended).into();
    }
    Some(told)
}

#[cfg(test)]
mod tests;
