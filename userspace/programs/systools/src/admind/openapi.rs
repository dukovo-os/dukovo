// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The description of the console's API (ADR 0545): the OpenAPI 3.1
//! document that `GET /api/1/openapi.json` answers to anyone, made from the
//! table of routes ([`routes`](super::routes)) and the actions the console
//! offers ([`actions::offered`]). A route or an action that is added to
//! either is in the document, and what it says of an action's arguments is
//! what [`actions::schema`] says, so that the description cannot differ
//! from what answers.
//!
//! A route of the table is one operation, named by its route's name in the
//! table, with its summary, its path parameters, the security it needs and
//! the errors it can give. An action is one operation, named by its ID: the
//! arguments its address holds are path parameters with the checks of their
//! fields, and the others are its request body, since a read is a `GET` and
//! takes none. The first good example of an action is the example of its
//! parameters and its body, so a tester can send it as it is.
//!
//! Each status an operation answers is listed with the error IDs it can
//! come with, from the checks that come before a route
//! ([`routes`](super::routes) says in which order), from the route's own
//! handler and, for an action, from its command: `{"error": {"id", "title",
//! "detail", ...}}` ([`errors`](super::errors)). Where the table does not
//! know the shape of what a route answers or takes, the document says so by
//! leaving it out: only an action's arguments and the `{action, args}` that
//! `POST /api/1/show` and `POST /api/1/check` take are described as a body.
//!
//! The two ways to ask are the security schemes: `session`, the cookie of a
//! signed-in browser, with `csrf`, the header every change from a session
//! carries, and `token`, an API token as a bearer. A token is for the
//! operations tagged `api` alone; any other route refuses one.
//!
//! When the document says something the console does not do, the unit
//! tests below check what they can, and the lab test `web-console/fuzz`
//! sends requests made from the document to a machine and fails on any
//! answer the document does not describe.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use spark_api::trial;

use super::actions::{self, Field, Registered, Runs, logs};
use super::errors::Kind;
use super::guard::{CSRF_HEADER, SESSION_COOKIE};
use super::routes::{Access, Entry, Route, TABLE, Verb};
use super::scopes::Needed;

/// The description of [`TABLE`]'s routes and of `actions`.
pub fn document(actions: &[&dyn Registered]) -> Value {
    let mut paths = Map::new();
    for entry in TABLE {
        add(&mut paths, entry.path, method(entry.verb), route_operation(entry, actions));
    }
    for action in actions {
        let method = if action.changes() { "post" } else { "get" };
        add(&mut paths, action.path(), method, action_operation(*action));
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Dukovo Admin API",
            "version": "1",
            "description": "The API of the web console, admind(8). A request names this machine \
                in its Host header, or it is not answered (421). A request that changes \
                anything from a session must also come from the console's own origin, in its \
                Origin header, and carry the session's token in x-dukovo-csrf; one that brings \
                an API token needs neither, since a browser never sends a token by itself. A \
                body is JSON, at most 64 KiB before sign-in and 1 MiB after.",
        },
        "tags": [
            { "name": "open", "description": "Answered to anyone." },
            { "name": "sign-in", "description": "Signs a browser in: at most ten requests a \
                minute from an address." },
            { "name": "session", "description": "For a signed-in session; a token is refused." },
            { "name": "api", "description": "For a signed-in session or an API token." },
        ],
        "paths": paths,
        "components": {
            "securitySchemes": security_schemes(),
            "schemas": schemas(),
        },
    })
}

/// Puts `operation` in the item of `path` under `method`.
fn add(paths: &mut Map<String, Value>, path: &str, method: &str, operation: Value) {
    let item = paths.entry(path).or_insert_with(|| Value::Object(Map::new()));
    item[method] = operation;
}

/// The key of `verb` in a path item.
fn method(verb: Verb) -> &'static str {
    match verb {
        Verb::Get => "get",
        Verb::Post => "post",
        Verb::Head => "head",
    }
}

/// The parts of `path` that are arguments: `{name}` is `name`.
fn parameters_of(path: &str) -> impl Iterator<Item = &str> {
    path.split('/')
        .filter_map(|part| part.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')))
}

fn security_schemes() -> Value {
    json!({
        "session": {
            "type": "apiKey",
            "in": "cookie",
            "name": SESSION_COOKIE,
            "description": "The cookie a browser has once it is signed in. It is a __Host- \
                cookie, so it goes with requests to this machine's own name alone.",
        },
        "csrf": {
            "type": "apiKey",
            "in": "header",
            "name": CSRF_HEADER,
            "description": "The session's token, which a signed-in browser reads from \
                GET /api/1/session. With the cookie it makes a request that changes anything; \
                the request's Origin must be the console's own too, and Sec-Fetch-Site absent \
                or same-origin.",
        },
        "token": {
            "type": "http",
            "scheme": "bearer",
            "description": "An API token, from admin token add or the console's page for the \
                account. Its scopes name what it may do; it is never more than its owner.",
        },
    })
}

fn schemas() -> Value {
    json!({
        "Error": {
            "type": "object",
            "required": ["error"],
            "properties": {
                "error": {
                    "type": "object",
                    "required": ["id", "title", "detail"],
                    "properties": {
                        "id": {
                            "type": "string",
                            "pattern": "^[A-Z]{2,5}-[0-9]{3}$",
                            "description": "ADM-101 and on, or the ID the command gave when \
                                the action names it.",
                        },
                        "title": { "type": "string", "description": "What happened, in a few words." },
                        "detail": { "type": "string", "description": "What happened this time." },
                        "fix": { "type": "string", "description": "What a person can try." },
                        "field": {
                            "type": "string",
                            "description": "For ADM-125, the argument whose value cannot work.",
                        },
                        "command": {
                            "type": "object",
                            "description": "For a command that did not work, what ran.",
                            "required": ["argv", "status", "stderr"],
                            "properties": {
                                "argv": { "type": "array", "items": { "type": "string" } },
                                "status": { "type": "integer" },
                                "stderr": { "type": "string" },
                            },
                        },
                    },
                },
            },
        },
        "Done": {
            "type": "object",
            "required": ["done", "changed", "output"],
            "properties": {
                "done": { "const": true },
                "changed": {
                    "type": "boolean",
                    "description": "Whether the change moved anything: a change with nothing \
                        to compare always did.",
                },
                "output": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "The first lines the command printed.",
                },
            },
        },
        "Started": {
            "type": "object",
            "required": ["task"],
            "properties": {
                "task": { "type": "integer", "description": "The task's number, for GET /api/1/tasks/{id}." },
            },
        },
    })
}

/// The operation of the route `entry`; `actions` are what a route that
/// takes an action's ID names.
fn route_operation(entry: &Entry, actions: &[&dyn Registered]) -> Value {
    let post = entry.verb == Verb::Post;
    let mut operation = json!({
        "operationId": format!("{:?}", entry.route),
        "summary": summary(entry.route),
        "tags": [tag(entry.access)],
        "security": security(entry.access, post),
        "responses": responses(success_of(entry.route), &route_kinds(entry), &[]),
    });
    if let Some(id) = parameters_of(entry.path).next() {
        operation["parameters"] = json!([id_parameter(id, entry.path)]);
    }
    if entry.route == Route::Follow {
        let query: Vec<Value> =
            logs::Follow::FIELDS.iter().map(|field| parameter(field, None, "query")).collect();
        operation["parameters"] = query.into();
    }
    if matches!(entry.route, Route::Show | Route::Check) {
        operation["requestBody"] = action_and_arguments(actions);
    }
    operation
}

/// The body of a route that takes an action and the arguments its own route
/// would take, `{action, args}`: any of `actions`, and an object the action
/// itself checks.
fn action_and_arguments(actions: &[&dyn Registered]) -> Value {
    let ids: Vec<&str> = actions.iter().map(|action| action.id()).collect();
    let mut media = json!({
        "schema": {
            "type": "object",
            "required": ["action"],
            "additionalProperties": false,
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ids,
                    "description": "The action's ID, as GET /api/1/actions lists them.",
                },
                "args": {
                    "type": "object",
                    "description": "The arguments of the action's own route, those of its \
                        address and those of its body, in one object.",
                },
            },
        },
    });
    if let Some(action) = actions.first() {
        let (good, _, _) = action.example_values();
        media["example"] = json!({ "action": action.id(), "args": good.first() });
    }
    json!({ "required": true, "content": { "application/json": media } })
}

/// The operation of `action`: its arguments in the address are its
/// parameters, and the others its body, or for a read the parameters of
/// its query.
fn action_operation(action: &dyn Registered) -> Value {
    let (good, _, _) = action.example_values();
    let example = good.first().and_then(Value::as_object);
    let in_address: Vec<&str> = parameters_of(action.path()).collect();
    let (address, body): (Vec<Field>, Vec<Field>) =
        action.fields().iter().copied().partition(|field| in_address.contains(&field.name));
    let mut operation = json!({
        "operationId": action.id(),
        "summary": action.title(),
        "description": describe(action),
        "tags": [tag(Access::Api)],
        "security": security(Access::Api, action.changes()),
        "responses": responses(action_success(action), &action_kinds(action), action.errors()),
    });
    let given = |field: &Field| example.and_then(|given| given.get(field.name));
    let mut parameters: Vec<Value> =
        address.iter().map(|field| parameter(field, given(field), "path")).collect();
    if !action.changes() {
        parameters.extend(body.iter().map(|field| parameter(field, given(field), "query")));
    }
    if !parameters.is_empty() {
        operation["parameters"] = parameters.into();
    }
    if action.changes() {
        operation["requestBody"] = request_body(&body, example, &in_address);
    }
    operation
}

/// What an action does and who may ask for it, in words.
fn describe(action: &dyn Registered) -> String {
    let needed = Needed { area: action.area(), changes: action.changes() };
    match (action.changes(), action.runs()) {
        (false, _) => format!("Reads, as the person who asks. A token needs the scope {needed}."),
        (true, Runs::AsThePerson) => {
            format!("Changes something, as the person who asks. A token needs the scope {needed}.")
        }
        (true, Runs::AsAdministrator) => format!(
            "Changes something, as an administrator: for an unlocked session of an \
             administrator, or a token of one. A token needs the scope {needed}."
        ),
    }
}

/// The parameter that `field` describes, `place` being `path` for a part
/// of an action's address, which is always there, and `query` for one of a
/// read's query, which is there when the field is required.
fn parameter(field: &Field, example: Option<&Value>, place: &str) -> Value {
    let mut schema = actions::schema(std::slice::from_ref(field))["properties"][field.name].clone();
    if let Value::Object(keywords) = &mut schema {
        keywords.remove("description");
    }
    let mut parameter = json!({
        "name": field.name,
        "in": place,
        "required": place == "path" || field.required,
        "description": field.about,
        "schema": schema,
    });
    if let Some(example) = example {
        parameter["example"] = example.clone();
    }
    parameter
}

/// The parameter `name` of a route's address, the number the console gave
/// a change, a task, a terminal or a pane, or the name of a trial.
fn id_parameter(name: &str, path: &str) -> Value {
    if name == "trial" {
        let pattern = format!("^[A-Za-z0-9][A-Za-z0-9._-]{{0,{}}}$", trial::ID_MAX - 1);
        return json!({
            "name": name,
            "in": "path",
            "required": true,
            "description": "The trial's name, as Spark gives it: network, firewall, or service.NAME.",
            "schema": { "type": "string", "pattern": pattern },
        });
    }
    let what = if name == "pane" {
        "pane"
    } else if path.contains("/terminals/") {
        "terminal"
    } else if path.contains("/tasks/") {
        "task"
    } else {
        "change"
    };
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "description": format!("The {what}'s number, as the console gave it."),
        "schema": { "type": "integer", "minimum": 1 },
    })
}

/// The body of an action: the arguments `fields` its address does not
/// hold, and the example's, without those `in_address`.
fn request_body(
    fields: &[Field],
    example: Option<&Map<String, Value>>,
    in_address: &[&str],
) -> Value {
    let schema = actions::schema(fields);
    let mut media = json!({ "schema": schema });
    if let Some(example) = example {
        let mut rest = example.clone();
        for name in in_address {
            rest.remove(*name);
        }
        media["example"] = rest.into();
    }
    json!({
        "required": fields.iter().any(|field| field.required),
        "content": { "application/json": media },
    })
}

/// The tag of the operations of `access`.
fn tag(access: Access) -> &'static str {
    match access {
        Access::Anyone => "open",
        Access::SignIn => "sign-in",
        Access::Limited | Access::Full => "session",
        Access::Api => "api",
    }
}

/// What an operation of `access` needs to be asked with, for a request
/// that changes something when `post`.
fn security(access: Access, post: bool) -> Value {
    let session =
        if post { json!({ "session": [], "csrf": [] }) } else { json!({ "session": [] }) };
    match access {
        Access::Anyone | Access::SignIn => json!([]),
        Access::Limited | Access::Full => json!([session]),
        Access::Api => json!([{ "token": [] }, session]),
    }
}

/// The errors a request meets whatever its route, for a request of `access`
/// that changes something when `post`: [`routes`](super::routes)'s checks
/// before the handler.
fn access_kinds(access: Access, post: bool) -> Vec<Kind> {
    let mut kinds = vec![Kind::BadRequest, Kind::WrongHost];
    if post {
        kinds.extend([Kind::TooBig, Kind::NotJson]);
    }
    let session = [Kind::SessionEnded, Kind::OutOfScope];
    match access {
        // A token is for the actions alone, on the routes that are not
        // theirs it is refused before authd is asked.
        Access::Anyone => kinds.push(Kind::OutOfScope),
        Access::SignIn => kinds.extend([Kind::OutOfScope, Kind::ForeignRequest, Kind::Wait]),
        Access::Limited => kinds.extend(session),
        Access::Full => kinds.extend(session.into_iter().chain([Kind::SecondStepOnly])),
        Access::Api => kinds.extend([
            Kind::TokenRefused,
            Kind::Wait,
            Kind::AuthdSilent,
            Kind::SessionEnded,
            Kind::SecondStepOnly,
        ]),
    }
    if post && matches!(access, Access::Limited | Access::Full | Access::Api) {
        kinds.push(Kind::ForeignRequest);
    }
    kinds
}

/// What authd's or a worker's answer can make of a request for the
/// sign-in, the session or the account.
const PERSON: &[Kind] = &[
    Kind::WrongPassword,
    Kind::WrongCode,
    Kind::PasskeyRefused,
    Kind::NotAllowed,
    Kind::Locked,
    Kind::SessionEnded,
    Kind::Refused,
    Kind::NotAdmin,
    Kind::BadValue,
    Kind::Busy,
    Kind::AuthdSilent,
    Kind::WorkerSilent,
    Kind::Internal,
];

/// What the journal and the tasks can answer with, which a put back adds
/// its command's to.
const JOURNAL: &[Kind] = &[
    Kind::NoStore,
    Kind::NotKept,
    Kind::Cannot,
    Kind::Locked,
    Kind::NotAdmin,
    Kind::OutOfScope,
    Kind::BadValue,
    Kind::CommandFailed,
    Kind::AuthdSilent,
    Kind::Internal,
];

/// What the Terminal's routes can answer with: the account may not have a
/// terminal or a root one, holds as many as it may, names one it has not,
/// or authd did not start the shell.
const TERMINAL: &[Kind] = &[
    Kind::BadValue,
    Kind::BadRequest,
    Kind::TerminalClosed,
    Kind::TooManyTerminals,
    Kind::NoTerminal,
    Kind::NotAdmin,
    Kind::Locked,
    Kind::AuthdSilent,
    Kind::Internal,
];

/// What a follow of the log can answer with (ADR 0518): a filter that
/// does not read, the whole log for a session that may not read it, too
/// many follows, and authd's silence.
const FOLLOW: &[Kind] = &[
    Kind::BadRequest,
    Kind::BadValue,
    Kind::NotAdmin,
    Kind::Locked,
    Kind::TooManyFollows,
    Kind::AuthdSilent,
    Kind::Internal,
];

/// What the Terminal's socket can answer with besides: the opening request
/// did not come from the console's page with the session's token.
const SOCKET: &[Kind] = &[
    Kind::BadValue,
    Kind::BadRequest,
    Kind::ForeignRequest,
    Kind::TerminalClosed,
    Kind::TooManyTerminals,
    Kind::Internal,
];

/// The errors the handler of `route` can give besides those of its access.
fn own_kinds(route: Route) -> &'static [Kind] {
    match route {
        Route::File | Route::Health | Route::Fingerprint | Route::OpenApi | Route::Actions => &[],
        // A probe that does not answer makes check mode say it cannot
        // tell, not an error.
        Route::Show | Route::Check => &[Kind::BadValue, Kind::OutOfScope],
        Route::Follow => FOLLOW,
        // A trial's ID that is none, and a connection opened before the
        // change or a trial that has ended (ADR 0518).
        Route::Fresh => &[Kind::BadRequest, Kind::OldConnection, Kind::NoTrial, Kind::Internal],
        Route::Terminals
        | Route::TerminalAdd
        | Route::TerminalPanes
        | Route::TerminalLayout
        | Route::TerminalRename
        | Route::TerminalEnd
        | Route::PaneEnd => TERMINAL,
        Route::TerminalSocket => SOCKET,
        Route::Changes
        | Route::Change
        | Route::Keep
        | Route::PutBack
        | Route::Tasks
        | Route::Task
        | Route::StopTask => JOURNAL,
        Route::SignIn
        | Route::SignInCode
        | Route::SignInPasskeyOptions
        | Route::SignInPasskey
        | Route::Session
        | Route::SignOut
        | Route::Unlock
        | Route::UnlockPasskeyOptions
        | Route::UnlockExtend
        | Route::Lock
        | Route::Events
        | Route::Steps
        | Route::AppBegin
        | Route::AppConfirm
        | Route::AppRemove
        | Route::PasskeyBegin
        | Route::PasskeyAdd
        | Route::PasskeyRemove
        | Route::PasskeyRename
        | Route::Codes
        | Route::TrustedForget
        | Route::TokenAdd
        | Route::TokenRemove
        | Route::Preferences
        | Route::PreferencesChange => PERSON,
        #[cfg(feature = "lab")]
        Route::TestElevate => PERSON,
    }
}

/// The errors the route `entry` can answer with.
fn route_kinds(entry: &Entry) -> Vec<Kind> {
    let mut kinds = access_kinds(entry.access, entry.verb == Verb::Post);
    kinds.extend_from_slice(own_kinds(entry.route));
    if parameters_of(entry.path).next().is_some() {
        // An address part that is not one: a bad escape, or a `/`.
        kinds.push(Kind::NoRoute);
    }
    kinds
}

/// The errors the route of `action` can answer with.
fn action_kinds(action: &dyn Registered) -> Vec<Kind> {
    let mut kinds = access_kinds(Access::Api, action.changes());
    kinds.extend([
        Kind::BadValue,
        Kind::OutOfScope,
        Kind::CommandFailed,
        Kind::WorkerSilent,
        Kind::Busy,
        Kind::Internal,
        // The console has the action, and this machine not its program.
        Kind::NoProgram,
    ]);
    if parameters_of(action.path()).next().is_some() {
        kinds.push(Kind::NoRoute);
    }
    if action.changes() {
        kinds.extend([Kind::Locked, Kind::NotAdmin, Kind::Refused]);
    }
    if action.long() {
        kinds.push(Kind::NoStore);
    }
    kinds
}

/// The responses of an operation: its success, and one for each status
/// that `kinds` give, listing the error IDs that come with it. `own` are
/// the IDs the action's command can give instead of ADM-123.
fn responses(success: (&str, Value), kinds: &[Kind], own: &[&str]) -> Value {
    let mut all = Map::new();
    all.insert(success.0.into(), success.1);
    let mut by_status: BTreeMap<u16, BTreeMap<&str, &str>> = BTreeMap::new();
    for kind in kinds {
        by_status.entry(kind.status()).or_default().insert(kind.id(), kind.title());
    }
    if let Some(ids) = by_status.get_mut(&Kind::CommandFailed.status()) {
        for id in own {
            ids.insert(id, "The command gave this ID");
        }
    }
    for (status, ids) in &by_status {
        all.insert(status.to_string(), error_response(*status, ids));
    }
    Value::Object(all)
}

/// The response for `status`, which comes with one of the errors `ids`
/// (each with its title).
fn error_response(status: u16, ids: &BTreeMap<&str, &str>) -> Value {
    let words: Vec<String> = ids.iter().map(|(id, title)| format!("{id}: {title}")).collect();
    let mut response = json!({
        "description": words.join(". "),
        "content": {
            "application/json": {
                "schema": { "allOf": [
                    { "$ref": "#/components/schemas/Error" },
                    { "properties": { "error": { "properties": {
                        "id": { "enum": ids.keys().collect::<Vec<_>>() },
                    } } } },
                ] },
            },
        },
    });
    if status == Kind::Wait.status() {
        response["headers"] = json!({
            "Retry-After": {
                "description": "Seconds after which to try again.",
                "schema": { "type": "integer", "minimum": 0 },
            },
        });
    }
    response
}

/// A response whose body is JSON, with `schema` when the shape is known.
fn json_response(description: &str, schema: Option<Value>) -> Value {
    let mut media = Map::new();
    if let Some(schema) = schema {
        media.insert("schema".into(), schema);
    }
    json!({ "description": description, "content": { "application/json": media } })
}

/// The status and response of a request of `action` that works.
fn action_success(action: &dyn Registered) -> (&'static str, Value) {
    if !action.changes() {
        return ("200", json_response("What the command printed, which is JSON.", None));
    }
    if action.long() {
        let schema = json!({ "$ref": "#/components/schemas/Started" });
        return ("202", json_response("The task that runs the command has begun.", Some(schema)));
    }
    let schema = json!({ "$ref": "#/components/schemas/Done" });
    ("200", json_response("The command ended well.", Some(schema)))
}

/// The status and response of a request of `route` that works.
fn success_of(route: Route) -> (&'static str, Value) {
    let ok = |description, schema| ("200", json_response(description, schema));
    match route {
        Route::Health => ok(
            "The console answers.",
            Some(json!({
                "type": "object",
                "required": ["ok"],
                "properties": { "ok": { "const": true } },
            })),
        ),
        Route::Fingerprint => ok(
            "The machine's name and its certificate's SHA-256 fingerprint.",
            Some(json!({
                "type": "object",
                "required": ["host", "fingerprint"],
                "properties": { "host": { "type": "string" }, "fingerprint": { "type": "string" } },
            })),
        ),
        Route::OpenApi => ok(
            "This document.",
            Some(json!({ "type": "object", "required": ["openapi", "info", "paths"] })),
        ),
        Route::Actions => ok(
            "The actions, each with its route and the schema of its arguments.",
            Some(json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "title", "area", "changes", "method", "path", "input"],
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "area": { "type": "string" },
                        "changes": { "type": "boolean" },
                        "method": { "enum": ["GET", "POST"] },
                        "path": { "type": "string" },
                        "input": { "type": "object" },
                    },
                },
            })),
        ),
        Route::Show => ok(
            "What the action with these arguments runs, in the forms a person copies.",
            Some(json!({
                "type": "object",
                "required": ["command", "api", "ansible", "files"],
                "properties": {
                    "command": { "type": "string" },
                    "api": { "type": "string" },
                    "ansible": { "type": "string" },
                    "files": { "type": "array", "items": { "type": "string" } },
                },
            })),
        ),
        Route::Check => ok(
            "Whether the change would change anything, and what its probe reads now.",
            Some(json!({
                "type": "object",
                "required": ["changed", "now"],
                "properties": {
                    "changed": {
                        "type": ["boolean", "null"],
                        "description": "true when a part the change compares would move, \
                            false when every part is already as it leaves it, null when the \
                            console cannot tell.",
                    },
                    "now": {
                        "type": ["string", "null"],
                        "description": "The part of the probe's output the change compares.",
                    },
                },
            })),
        ),
        Route::TerminalSocket => (
            "101",
            json!({
                "description": "The connection is handed to the Terminal's WebSocket, with the \
                    protocol dukovo-terminal-1. The opening request names the session's csrf \
                    token among its protocols, as csrf.TOKEN.",
            }),
        ),
        Route::Events => (
            "200",
            json!({
                "description": "A stream of server-sent events, which goes on until the tab \
                    closes or the session ends.",
                "content": { "text/event-stream": {} },
            }),
        ),
        Route::Follow => (
            "200",
            json!({
                "description": "A stream of server-sent events, a record of the log each, which \
                    goes on until the tab closes, the session ends or, for the whole log, locks.",
                "content": { "text/event-stream": {} },
            }),
        ),
        Route::Fresh => ok(
            "The request came by a connection opened after the change took effect.",
            Some(json!({
                "type": "object",
                "required": ["fresh", "from", "via"],
                "properties": {
                    "fresh": { "const": true },
                    "from": {
                        "type": "string",
                        "description": "The address the request came from.",
                    },
                    "via": {
                        "type": ["object", "null"],
                        "description": "The address and port of this machine it came to, and \
                            the network card that holds the address.",
                    },
                },
            })),
        ),
        _ => ok("What the route answers, as JSON.", None),
    }
}

/// What `route` is, in a line.
fn summary(route: Route) -> &'static str {
    match route {
        // The table does not hold the files, so no operation has this.
        Route::File => "A file of the console's page.",
        Route::Health => "Says that the console answers.",
        Route::Fingerprint => {
            "The machine's name and the fingerprint of the certificate the console serves."
        }
        Route::OpenApi => "This description of the API.",
        Route::SignIn => "Signs in with a name and a password: the session, or the second step.",
        Route::SignInCode => {
            "Gives an app's code or a recovery code, the second step of a sign-in."
        }
        Route::SignInPasskeyOptions => {
            "Asks what a passkey is to answer, for a second step or a sign-in alone."
        }
        Route::SignInPasskey => "Gives a passkey's answer, a second step or the whole sign-in.",
        Route::Session => "Says who is signed in and what the page needs to go on.",
        Route::SignOut => "Ends the session.",
        Route::Unlock => "Unlocks the session with a password, a code or a passkey.",
        Route::UnlockPasskeyOptions => "Asks what a passkey is to answer, to unlock.",
        Route::UnlockExtend => "Keeps an unlocked session unlocked a while longer.",
        Route::Lock => "Locks the session again.",
        Route::Events => "Opens a tab's stream of events.",
        Route::Steps => "Lists the second steps the account has.",
        Route::AppBegin => "Makes a new secret for an authenticator app.",
        Route::AppConfirm => "Turns the app on with the first code of its new secret.",
        Route::AppRemove => "Removes the authenticator app.",
        Route::PasskeyBegin => "Asks what a passkey is to be made with.",
        Route::PasskeyAdd => "Adds the passkey the browser made.",
        Route::PasskeyRemove => "Removes a passkey.",
        Route::PasskeyRename => "Renames a passkey.",
        Route::Codes => "Makes new recovery codes, which replace the old.",
        Route::TrustedForget => "Forgets every browser the person trusted.",
        Route::TokenAdd => "Makes an API token, which is shown this once.",
        Route::TokenRemove => "Removes an API token.",
        Route::Preferences => "Says how the console looks to the person.",
        Route::PreferencesChange => "Changes the person's preferences, and answers them all.",
        Route::Actions => "Lists the actions the API offers.",
        Route::Show => "Shows what an action would run, and runs nothing.",
        Route::Check => "Says whether a change would change anything, and changes nothing.",
        Route::Terminals => "Lists the account's terminals.",
        Route::TerminalAdd => "Opens a new terminal, with its one pane.",
        Route::TerminalPanes => "Adds a pane to a terminal.",
        Route::TerminalLayout => "Lays a terminal's panes out.",
        Route::TerminalRename => "Renames a terminal.",
        Route::TerminalEnd => "Ends a terminal and its panes.",
        Route::PaneEnd => "Ends one pane of a terminal.",
        Route::TerminalSocket => "Opens the WebSocket that shows the account's terminals.",
        Route::Changes => "Lists what changed on the machine, newest first.",
        Route::Change => "One change, with a diff of each file it changed.",
        Route::Keep => "Marks a change made by hand as seen.",
        Route::PutBack => "Puts a change back.",
        Route::Tasks => "Lists the tasks that run or ran.",
        Route::Task => "One task, with its log.",
        Route::StopTask => "Asks a task to stop.",
        Route::Follow => "Follows the log as it is written, filtered, as the person may read it.",
        Route::Fresh => {
            "Says whether the browser reaches the console by a connection opened after a tried \
             change took effect."
        }
        #[cfg(feature = "lab")]
        Route::TestElevate => "Runs a command as an administrator in an unlocked session.",
    }
}

#[cfg(test)]
mod tests;
