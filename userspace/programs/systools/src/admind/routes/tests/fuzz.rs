// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Mutation fuzzing of admind's routes with callers (ADR 0545). The fuzz
//! test of `crate::fuzz` sends random requests that nobody signed in
//! makes; this one sends the requests of an administrator whose session
//! is unlocked and of one whose session is locked, of a person who is no
//! administrator, of one who must add a second step, of each API token,
//! of each AI client's token at the MCP server and elsewhere, and of
//! strangers with a cookie or a token that no one was given. Each is a
//! request the console's page, a script or an MCP client makes, bent from
//! a fixed seed:
//! headers taken out, doubled, emptied, made huge or given to another
//! caller; the JSON of its body with a member taken out, doubled, of
//! another type, at its limits or clashing with the address; a message
//! to the MCP server bent where a tool reads, or its headers set against
//! it; the bytes of its body bent; a part of its address made `..`, a NUL, very long or
//! not UTF-8. The requests reach admind as hyper hands them over, so what
//! hyper refuses before then (a header with a control character, chunks
//! that do not add up) is hyper's and is not sent.
//!
//! For every answer: nothing panics; it is an error with an ID, a title and
//! a detail, or JSON where the API answers; a refused request ran nothing
//! as an administrator, wrote no row of the journal and changed nothing in
//! the worker; an answer to a stranger holds no account's name and no
//! session's secrets; every sign-in request that reaches authd, and every
//! token that authd does not know, is counted against the address; a
//! check runs only probes, and a read nothing as an administrator; and a
//! message to the MCP server, whatever it is and whoever sends it, runs
//! nothing as an administrator, writes no row of the journal, changes
//! nothing of its owner's, and has the worker run a read's command or a
//! probe and nothing else.
//!
//! `ADMIND_FUZZ=N` runs N requests instead of the default, and
//! `ADMIND_FUZZ_SEED=S` starts from another seed. A failure names the
//! request's number, its caller and the request.

use std::net::IpAddr;
use std::rc::Rc;
use std::time::Duration;

use http::header::{HeaderName, HeaderValue};
use http::{Method, Uri};

use super::*;
use crate::admind::actions::tests::LAB;
use crate::admind::actions::{REGISTRY, Registered, Runs};
use crate::admind::guard::CSRF_HEADER;
use crate::admind::limits::Tries;
use crate::admind::reach;

/// xorshift64*: enough to bend requests, and the same every run.
struct Rng(u64);

impl Rng {
    fn new() -> Rng {
        let seed = std::env::var("ADMIND_FUZZ_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0x0545_ad31_5eed_u64);
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n`, or 0.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { usize::try_from(self.next() % n as u64).unwrap_or(0) }
    }

    /// True `percent` times in a hundred.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// The requests to send, which keep the test under 20 seconds.
fn rounds() -> usize {
    std::env::var("ADMIND_FUZZ").ok().and_then(|n| n.parse().ok()).unwrap_or(4000)
}

/// The requests between two sign-ins of the callers that have sessions,
/// whose sessions the requests before may have ended or locked.
const ROUNDS_PER_SESSION: usize = 60;

/// The requests in a hundred that go to the MCP server or to the routes
/// of the page for its clients.
const CLIENTS_SHARE: usize = 15;

/// Who sends a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    /// No cookie and no token.
    Nobody,
    /// A session cookie that no session has.
    Stale,
    /// chase, an administrator, signed in and locked.
    Locked,
    /// chase, signed in and unlocked.
    Unlocked,
    /// dave, who is no administrator.
    NotAdmin,
    /// eve, who must add a second step first.
    AddsStep,
    /// chase's token for everything.
    TokenAll,
    /// chase's token that reads services.
    TokenRead,
    /// chase's token that reads the firewall.
    TokenOther,
    /// A token that authd does not know.
    TokenWrong,
    /// chase's AI client that reaches everything.
    ClientAll,
    /// chase's AI client that reads what chase reads before unlocking.
    ClientRead,
    /// An AI client's token that authd does not know.
    ClientWrong,
}

impl Caller {
    /// The bearer token the caller sends, if it sends one.
    fn bearer(self) -> Option<&'static str> {
        match self {
            Caller::TokenAll => Some(TOKEN_ALL),
            Caller::TokenRead => Some(TOKEN_READ),
            Caller::TokenOther => Some(TOKEN_OTHER),
            Caller::TokenWrong => Some(WRONG_TOKEN),
            Caller::ClientAll => Some(CLIENT_ALL),
            Caller::ClientRead => Some(CLIENT_READ),
            Caller::ClientWrong => Some(WRONG_CLIENT),
            _ => None,
        }
    }
}

const CALLERS: [Caller; 13] = [
    Caller::Nobody,
    Caller::Stale,
    Caller::Locked,
    Caller::Unlocked,
    Caller::NotAdmin,
    Caller::AddsStep,
    Caller::TokenAll,
    Caller::TokenRead,
    Caller::TokenOther,
    Caller::TokenWrong,
    Caller::ClientAll,
    Caller::ClientRead,
    Caller::ClientWrong,
];

/// The callers whom the console lets in, for most of the requests.
const ADMITTED: [Caller; 5] =
    [Caller::Locked, Caller::Unlocked, Caller::NotAdmin, Caller::TokenAll, Caller::TokenRead];

/// The callers whom the MCP server lets in, for most of the messages.
const CLIENTS: [Caller; 2] = [Caller::ClientAll, Caller::ClientRead];

const WRONG_TOKEN: &str =
    "dkv_chase_0000000000000000000000000000000000000000000000000000000000000000";
const WRONG_CLIENT: &str =
    "dkc_chase_0000000000000000000000000000000000000000000000000000000000000000";

/// Where the MCP server is.
const MCP: &str = "/mcp";

/// The accounts of the test authd, which an answer to a stranger never
/// names: by their names, and by chase's full name.
const ACCOUNTS: [&str; 6] = ["chase", "alice", "eve", "dave", "bob", "Chase Y"];

/// The sessions of one round of sign-ins.
struct Sessions {
    locked: Session,
    unlocked: Session,
    not_admin: Session,
    adds_step: Session,
}

impl Sessions {
    /// Signs the callers in afresh, and forgets the tries it took.
    async fn new(world: &World) -> Sessions {
        let locked = sign_in(world, "chase").await;
        let unlocked = sign_in(world, "chase").await;
        let body = json!({ "kind": "password", "password": "right" });
        let done = send(world, post("/api/1/unlock", &body).by(&unlocked)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        let not_admin = sign_in(world, "dave").await;
        let adds_step = sign_in_to_add_a_step(world, "eve").await;
        forget_tries(&world.console.shared);
        Sessions { locked, unlocked, not_admin, adds_step }
    }

    /// Signs them all out, so that the console does not fill with them.
    async fn end(self, world: &World) {
        for session in [self.locked, self.unlocked, self.not_admin, self.adds_step] {
            let _ = send(world, post("/api/1/sign-out", &json!({})).by(&session)).await;
        }
    }

    /// The session `caller` uses, if it has one.
    fn of(&self, caller: Caller) -> Option<&Session> {
        match caller {
            Caller::Locked => Some(&self.locked),
            Caller::Unlocked => Some(&self.unlocked),
            Caller::NotAdmin => Some(&self.not_admin),
            Caller::AddsStep => Some(&self.adds_step),
            _ => None,
        }
    }

    /// Every cookie and CSRF token of these sessions, which no answer to a
    /// stranger holds.
    fn secrets(&self) -> Vec<String> {
        [&self.locked, &self.unlocked, &self.not_admin, &self.adds_step]
            .iter()
            .flat_map(|session| [session.cookie.clone(), session.csrf.clone()])
            .collect()
    }
}

/// Takes back the tries the test's own sign-ins, and the request before,
/// counted, so that the limits never stop the run.
fn forget_tries(shared: &Shared) {
    *shared.tries.borrow_mut() = Tries::default();
    *shared.wrong_tokens.borrow_mut() = Tries::default();
}

/// A request before it is bent: an address template with the values of
/// its `{name}` parts, and a JSON body.
#[derive(Clone)]
struct Seed {
    /// What it is, for a failure's words.
    what: String,
    method: &'static str,
    template: &'static str,
    params: Vec<(String, String)>,
    /// A read's other arguments, as the address's query with its `?`, or
    /// nothing.
    query: String,
    body: Option<Value>,
    /// The headers that are the request's own, as those of a message to
    /// the MCP server that names its revision.
    headers: Vec<(&'static str, String)>,
}

impl Seed {
    fn new(what: &str, method: &'static str, template: &'static str, body: Option<Value>) -> Seed {
        Seed {
            what: what.to_string(),
            method,
            template,
            params: Vec::new(),
            query: String::new(),
            body,
            headers: Vec::new(),
        }
    }
}

/// `value` percent-encoded but for the characters RFC 3986 leaves as they
/// are.
fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The address `template` with each `{name}` part filled in from `params`,
/// percent-encoded but for the characters RFC 3986 leaves as they are; a
/// part with no value gets `nginx`.
fn fill(template: &str, params: &[(String, String)]) -> String {
    template
        .split('/')
        .map(|part| match part.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
            Some(name) => {
                let value = params
                    .iter()
                    .find(|(have, _)| have == name)
                    .map_or("nginx", |(_, value)| value.as_str());
                encoded(value)
            }
            None => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `path` is the address of a read of `actions` that runs as an
/// administrator, the one kind of read that elevates: the whole log, the
/// firewall's state and the like. Whether the caller may is the refusals'
/// to say.
fn read_as_administrator(actions: &[&'static dyn Registered], path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    actions.iter().any(|action| {
        let template: Vec<&str> = action.path().split('/').collect();
        !action.changes()
            && action.runs() == Runs::AsAdministrator
            && template.len() == parts.len()
            && template
                .iter()
                .zip(&parts)
                .all(|(want, have)| (want.starts_with('{') && !have.is_empty()) || want == have)
    })
}

/// The seeds of the action `action` with the arguments `args`: on its own
/// route, with the address's parts taken out of them and, for a read, the
/// rest in its query; and shown and checked.
fn action_seeds(action: &'static dyn Registered, what: &str, args: &Value, seeds: &mut Vec<Seed>) {
    let mut rest = args.as_object().cloned().unwrap_or_default();
    let mut params = Vec::new();
    for part in action.path().split('/') {
        if let Some(name) = part.strip_prefix('{').and_then(|part| part.strip_suffix('}'))
            && let Some(value) = rest.remove(name)
        {
            let value = value.as_str().map_or_else(|| value.to_string(), ToString::to_string);
            params.push((name.to_string(), value));
        }
    }
    let (method, query, body) = if action.posts() {
        ("POST", String::new(), Some(Value::Object(rest)))
    } else {
        let pairs: Vec<String> = rest
            .iter()
            .map(|(name, value)| {
                let value = value.as_str().map_or_else(|| value.to_string(), ToString::to_string);
                format!("{}={}", encoded(name), encoded(&value))
            })
            .collect();
        let query = if pairs.is_empty() { String::new() } else { format!("?{}", pairs.join("&")) };
        ("GET", query, None)
    };
    seeds.push(Seed {
        what: format!("{} {what}", action.id()),
        method,
        template: action.path(),
        params,
        query,
        body,
        headers: Vec::new(),
    });
    for route in ["/api/1/show", "/api/1/check"] {
        let body = json!({ "action": action.id(), "args": args });
        seeds.push(Seed::new(
            &format!("{route} {} {what}", action.id()),
            "POST",
            route,
            Some(body),
        ));
    }
}

/// The messages an MCP client sends: those of the revisions that begin
/// with `initialize`, a batch of them, and those of the revision that
/// names itself in each request and in its headers. The actions are the
/// tests' own.
fn mcp_seeds(seeds: &mut Vec<Seed>) {
    let asks = |id: u64, method: &str, params: Value| json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let calls = |tool: &str, arguments: Value| {
        asks(7, "tools/call", json!({ "name": tool, "arguments": arguments }))
    };
    let hello = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "fuzz", "version": "1" },
    });
    let restart = json!({
        "action": "lab.restart",
        "arguments": { "name": "nginx" },
        "reason": "nginx stopped answering",
    });
    let set = json!({
        "action": "lab.set",
        "arguments": { "name": "moving" },
        "reason": "the setting is off and the service needs it on",
    });
    let messages = [
        ("ping", asks(1, "ping", json!({}))),
        ("initialize", asks(1, "initialize", hello)),
        ("initialized", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
        ("tools", asks(2, "tools/list", json!({}))),
        ("no method", asks(3, "resources/list", json!({}))),
        ("areas", calls("areas", json!({}))),
        ("actions", calls("actions", json!({ "area": "services" }))),
        ("read", calls("read", json!({ "action": "lab.list" }))),
        ("read log", calls("read", json!({ "action": "lab.log", "arguments": { "lines": 5 } }))),
        (
            "check",
            calls("check", json!({ "action": "lab.set", "arguments": { "name": "moving" } })),
        ),
        ("ask", calls("ask", restart)),
        ("ask set", calls("ask", set)),
        ("request", calls("request", json!({ "id": 1 }))),
        ("batch", json!([asks(1, "ping", json!({})), asks(2, "tools/list", json!({}))])),
    ];
    for (what, message) in messages {
        seeds.push(Seed::new(&format!("mcp {what}"), "POST", MCP, Some(message)));
    }
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let read = json!({ "name": "read", "arguments": { "action": "lab.list" } });
    let named = [
        ("discover", "server/discover", json!({}), None),
        ("named tools", "tools/list", json!({}), None),
        ("named read", "tools/call", read, Some("read")),
        ("named no method", "resources/list", json!({}), None),
    ];
    for (what, method, mut params, tool) in named {
        params["_meta"] = meta.clone();
        let mut headers =
            vec![("mcp-protocol-version", "2026-07-28".to_string()), ("mcp-method", method.into())];
        if let Some(tool) = tool {
            headers.push(("mcp-name", tool.into()));
        }
        let message = asks(4, method, params);
        seeds.push(Seed {
            headers,
            ..Seed::new(&format!("mcp {what}"), "POST", MCP, Some(message))
        });
    }
}

/// Every request the fuzzing starts from: each route of the table with a
/// body it takes, each action's good and bad examples on its route, shown
/// and checked, the messages of an MCP client, and a few pages. The events and a follow of the log, whose
/// answers are streams that do not end, are left out.
fn seeds(actions: &[&'static dyn Registered]) -> Vec<Seed> {
    let mut seeds = vec![
        Seed::new("page", "GET", "/", None),
        Seed::new("script", "GET", "/app.js", None),
        Seed::new(
            "sign-in",
            "POST",
            "/api/1/sign-in",
            Some(json!({ "name": "chase", "password": "right" })),
        ),
        Seed::new(
            "sign-in wrong",
            "POST",
            "/api/1/sign-in",
            Some(json!({ "name": "chase", "password": "wrong" })),
        ),
        Seed::new(
            "sign-in step",
            "POST",
            "/api/1/sign-in",
            Some(json!({ "name": "alice", "password": "right" })),
        ),
        Seed::new(
            "sign-in refused",
            "POST",
            "/api/1/sign-in",
            Some(json!({ "name": "bob", "password": "right" })),
        ),
        Seed::new(
            "sign-in code",
            "POST",
            "/api/1/sign-in/code",
            Some(json!({ "ticket": "0a0b02", "code": "123456", "trust": true })),
        ),
        Seed::new(
            "passkey options",
            "POST",
            "/api/1/sign-in/passkey/options",
            Some(json!({ "ticket": "0a0b02" })),
        ),
        Seed::new(
            "passkey",
            "POST",
            "/api/1/sign-in/passkey",
            Some(json!({
                "ticket": "0a0b02",
                "trust": false,
                "credential": { "id": "AQI", "response": {
                    "clientDataJSON": "e30", "authenticatorData": "AA", "signature": "AA",
                } },
            })),
        ),
        Seed::new(
            "unlock",
            "POST",
            "/api/1/unlock",
            Some(json!({ "kind": "password", "password": "right" })),
        ),
        Seed::new(
            "unlock code",
            "POST",
            "/api/1/unlock",
            Some(json!({ "kind": "code", "code": "123456" })),
        ),
        Seed::new("unlock options", "POST", "/api/1/unlock/passkey/options", Some(json!({}))),
        Seed::new("extend", "POST", "/api/1/unlock/extend", Some(json!({}))),
        Seed::new(
            "app begin",
            "POST",
            "/api/1/account/second-step/app/begin",
            Some(json!({ "proof": "right" })),
        ),
        Seed::new(
            "app confirm",
            "POST",
            "/api/1/account/second-step/app/confirm",
            Some(json!({ "code": "123456" })),
        ),
        Seed::new(
            "app remove",
            "POST",
            "/api/1/account/second-step/app/remove",
            Some(json!({ "proof": "right" })),
        ),
        Seed::new(
            "passkey begin",
            "POST",
            "/api/1/account/second-step/passkey/begin",
            Some(json!({ "proof": "right" })),
        ),
        Seed::new(
            "passkey add",
            "POST",
            "/api/1/account/second-step/passkey/add",
            Some(json!({ "name": "key", "credential": { "response": {
                "clientDataJSON": "e30", "attestationObject": "oA",
            } } })),
        ),
        Seed::new(
            "passkey remove",
            "POST",
            "/api/1/account/second-step/passkey/remove",
            Some(json!({ "proof": "right", "id": "BQY" })),
        ),
        Seed::new(
            "passkey rename",
            "POST",
            "/api/1/account/second-step/passkey/rename",
            Some(json!({ "id": "BQY", "name": "work key" })),
        ),
        Seed::new(
            "codes",
            "POST",
            "/api/1/account/second-step/codes",
            Some(json!({ "proof": "right" })),
        ),
        Seed::new(
            "forget browsers",
            "POST",
            "/api/1/account/second-step/trusted/forget",
            Some(json!({})),
        ),
        Seed::new("steps", "GET", "/api/1/account/second-step", None),
        Seed::new(
            "token add",
            "POST",
            "/api/1/account/tokens/add",
            Some(
                json!({ "proof": "right", "name": "ci2", "scopes": ["services:read"], "days": 30 }),
            ),
        ),
        Seed::new(
            "token remove",
            "POST",
            "/api/1/account/tokens/remove",
            Some(json!({ "name": "ci" })),
        ),
        Seed::new("clients", "GET", "/api/1/account/clients", None),
        Seed::new(
            "client add",
            "POST",
            "/api/1/account/clients/add",
            Some(json!({
                "proof": "right", "name": "desk", "way": "https", "scopes": ["services"],
                "days": 30,
            })),
        ),
        Seed::new(
            "client add ssh",
            "POST",
            "/api/1/account/clients/add",
            Some(json!({
                "proof": "right", "name": "shell", "way": "ssh", "scopes": ["*:read"], "days": 0,
            })),
        ),
        Seed::new(
            "client remove",
            "POST",
            "/api/1/account/clients/remove",
            Some(json!({ "name": "helper" })),
        ),
        Seed::new("client areas", "GET", "/api/1/clients/areas", None),
        Seed::new("client requests", "GET", "/api/1/clients/requests", None),
        Seed::new("client history", "GET", "/api/1/clients/history", None),
        Seed::new(
            "client approve",
            "POST",
            "/api/1/clients/requests/{id}/approve",
            Some(json!({})),
        ),
        Seed::new("client refuse", "POST", "/api/1/clients/requests/{id}/refuse", Some(json!({}))),
        Seed::new("preferences", "GET", "/api/1/account/preferences", None),
        Seed::new(
            "preferences change",
            "POST",
            "/api/1/account/preferences",
            Some(json!({ "theme": "light", "text": 112 })),
        ),
        Seed::new("session", "GET", "/api/1/session", None),
        Seed::new("health", "GET", "/api/1/health", None),
        Seed::new("fingerprint", "GET", "/api/1/fingerprint", None),
        Seed::new("description", "GET", "/api/1/openapi.json", None),
        Seed::new("actions", "GET", "/api/1/actions", None),
        Seed {
            params: vec![("trial".into(), "network".into())],
            ..Seed::new("fresh", "GET", "/api/1/console/fresh/{trial}", None)
        },
        Seed::new("changes", "GET", "/api/1/changes", None),
        Seed::new("change", "GET", "/api/1/changes/{id}", None),
        Seed::new("keep", "POST", "/api/1/changes/{id}/keep", Some(json!({}))),
        Seed::new("put back", "POST", "/api/1/changes/{id}/put-back", Some(json!({}))),
        Seed::new("tasks", "GET", "/api/1/tasks", None),
        Seed::new("task", "GET", "/api/1/tasks/{id}", None),
        Seed::new("stop task", "POST", "/api/1/tasks/{id}/stop", Some(json!({}))),
        Seed::new("terminals", "GET", "/api/1/terminals", None),
        Seed::new(
            "terminal add",
            "POST",
            "/api/1/terminals",
            Some(json!({ "cols": 80, "rows": 24, "name": "logs", "keep_days": 2 })),
        ),
        Seed::new(
            "terminal add root",
            "POST",
            "/api/1/terminals",
            Some(json!({ "cols": 80, "rows": 24, "admin": true })),
        ),
        Seed::new(
            "terminal pane",
            "POST",
            "/api/1/terminals/{id}/panes",
            Some(json!({ "cols": 40, "rows": 12, "layout": "main-left" })),
        ),
        Seed::new(
            "terminal layout",
            "POST",
            "/api/1/terminals/{id}/layout",
            Some(json!({ "layout": "grid", "sizes": [0.5, 0.25] })),
        ),
        Seed::new(
            "terminal rename",
            "POST",
            "/api/1/terminals/{id}/rename",
            Some(json!({ "name": "build box" })),
        ),
        Seed::new("terminal end", "POST", "/api/1/terminals/{id}/end", Some(json!({}))),
        Seed::new("pane end", "POST", "/api/1/terminals/panes/{pane}/end", Some(json!({}))),
        Seed::new("terminal socket", "GET", "/api/1/terminal", None),
        Seed {
            params: vec![("name".into(), "chase".into())],
            ..Seed::new("signed in", "GET", "/api/1/accounts/{name}/sessions", None)
        },
        Seed {
            params: vec![("name".into(), "dave".into())],
            ..Seed::new(
                "signed in end",
                "POST",
                "/api/1/accounts/{name}/sessions/{id}/end",
                Some(json!({})),
            )
        },
        Seed {
            params: vec![("draft".into(), super::once::DRAFT.into())],
            ..Seed::new(
                "phone link",
                "POST",
                "/api/1/reports/drafts/{draft}/phone",
                Some(json!({ "sha256": super::once::SHA256 })),
            )
        },
        Seed {
            params: vec![("token".into(), "5".repeat(32))],
            ..Seed::new("one-time link", "GET", "/once/{token}/report.txt", None)
        },
        Seed {
            params: vec![("token".into(), "5".repeat(32))],
            ..Seed::new("one-time link head", "HEAD", "/once/{token}/report.txt", None)
        },
        Seed::new("lock", "POST", "/api/1/lock", Some(json!({}))),
        Seed::new("sign-out", "POST", "/api/1/sign-out", Some(json!({}))),
    ];
    mcp_seeds(&mut seeds);
    for seed in &mut seeds {
        for name in ["id", "pane"] {
            if seed.template.contains(&format!("{{{name}}}")) {
                seed.params.push((name.into(), "1".into()));
            }
        }
    }
    for &action in actions {
        let (good, bad, undo) = action.example_values();
        for (at, args) in good.iter().enumerate() {
            action_seeds(action, &format!("good {at}"), args, &mut seeds);
        }
        for (at, args) in bad.iter().enumerate() {
            action_seeds(action, &format!("bad {at}"), args, &mut seeds);
        }
        if let Some(undo) = undo {
            let undoes = actions.iter().copied().find(|action| action.id() == undo.action);
            if let Some(undoes) = undoes {
                action_seeds(undoes, "undo", &undo.args, &mut seeds);
            }
        }
    }
    seeds
}

/// A request as bytes, which a bend may make anything of.
#[derive(Clone)]
struct Sent {
    method: Vec<u8>,
    target: Vec<u8>,
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
}

impl Sent {
    /// The request `seed` is, as `caller` sends it with `sessions`.
    fn of(seed: &Seed, caller: Caller, sessions: &Sessions) -> Sent {
        let mut headers = vec![("host".to_string(), HOST.as_bytes().to_vec())];
        let post = seed.method == "POST";
        match (caller, sessions.of(caller)) {
            (_, Some(session)) => {
                let cookie = format!("{SESSION_COOKIE}={}", session.cookie);
                headers.push(("cookie".into(), cookie.into_bytes()));
                headers.push((CSRF_HEADER.into(), session.csrf.clone().into_bytes()));
            }
            (Caller::Stale, None) => {
                let cookie = format!("{SESSION_COOKIE}={}", "5a".repeat(32));
                headers.push(("cookie".into(), cookie.into_bytes()));
                headers.push((CSRF_HEADER.into(), "6b".repeat(32).into_bytes()));
            }
            _ => {
                if let Some(token) = caller.bearer() {
                    let bearer = format!("Bearer {token}").into_bytes();
                    headers.push(("authorization".into(), bearer));
                }
            }
        }
        // A script and an MCP client are no page: they name no origin.
        if post && caller.bearer().is_none() {
            headers.push(("origin".into(), ORIGIN.as_bytes().to_vec()));
        }
        if post {
            headers.push(("content-type".into(), b"application/json".to_vec()));
        }
        for (name, value) in &seed.headers {
            headers.push(((*name).into(), value.clone().into_bytes()));
        }
        // The Terminal's socket is opened as the page opens it: the
        // session's csrf token goes in the protocols, not a header.
        if seed.template == "/api/1/terminal" {
            let csrf = sessions.of(caller).map_or("6b", |session| session.csrf.as_str());
            headers.retain(|(name, _)| name != CSRF_HEADER);
            for (name, value) in [
                ("origin", ORIGIN.to_string()),
                ("connection", "Upgrade".into()),
                ("upgrade", "websocket".into()),
                ("sec-websocket-version", "13".into()),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==".into()),
                ("sec-websocket-protocol", format!("dukovo-terminal-1, csrf.{csrf}")),
                ("sec-fetch-site", "same-origin".into()),
            ] {
                headers.push((name.into(), value.into_bytes()));
            }
        }
        Sent {
            method: seed.method.as_bytes().to_vec(),
            target: format!("{}{}", fill(seed.template, &seed.params), seed.query).into_bytes(),
            headers,
            body: seed.body.as_ref().map_or_else(Vec::new, |body| body.to_string().into_bytes()),
        }
    }

    /// The path the request asks for, without its query.
    fn path(&self) -> String {
        let target = String::from_utf8_lossy(&self.target);
        target.split('?').next().unwrap_or_default().to_string()
    }

    /// The request as hyper would hand it to admind; None when hyper
    /// would refuse it first.
    fn request(&self) -> Option<Request<Full<Bytes>>> {
        let mut builder = Request::builder()
            .method(Method::from_bytes(&self.method).ok()?)
            .uri(Uri::try_from(self.target.as_slice()).ok()?);
        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
            builder = builder.header(name, HeaderValue::from_bytes(value).ok()?);
        }
        builder.body(Full::new(Bytes::from(self.body.clone()))).ok()
    }

    /// The request in words, for a failure.
    fn describe(&self) -> String {
        let shown = |bytes: &[u8]| {
            let text = String::from_utf8_lossy(bytes);
            if text.chars().count() > 300 {
                let start: String = text.chars().take(300).collect();
                format!("{start}... ({} bytes)", bytes.len())
            } else {
                text.into_owned()
            }
        };
        let mut words = format!("{} {}\n", shown(&self.method), shown(&self.target));
        for (name, value) in &self.headers {
            words.push_str(&format!("{name}: {}\n", shown(value)));
        }
        words.push('\n');
        words.push_str(&shown(&self.body));
        words
    }
}

/// Values that find the edges of what a route reads.
const ODD_VALUES: [&str; 12] = [
    "",
    "..",
    "../../etc/passwd",
    "a\u{0}b",
    "-x",
    "~",
    "a/b",
    "ä",
    "*",
    " nginx",
    "nginx\n",
    "%2e%2e",
];

/// A value of another type, or at an edge.
fn odd_value(rng: &mut Rng) -> Value {
    match rng.below(10) {
        0 => Value::Null,
        1 => json!(true),
        2 => json!(-1),
        3 => json!(1e308),
        4 => json!(u64::MAX),
        5 => json!([]),
        6 => json!({}),
        7 => json!("a".repeat(4096)),
        8 => json!("x".repeat(300)),
        _ => json!(rng.pick(&ODD_VALUES)),
    }
}

/// The JSON `body`, bent once.
fn bend_json(rng: &mut Rng, body: &Value, params: &[(String, String)]) -> Vec<u8> {
    let mut object = body.as_object().cloned().unwrap_or_default();
    let keys: Vec<String> = object.keys().cloned().collect();
    match rng.below(8) {
        0 if !keys.is_empty() => {
            object.remove(rng.pick(&keys));
        }
        // A member twice, which serde refuses where a route reads its body
        // as a struct.
        1 if !keys.is_empty() => {
            let key = rng.pick(&keys).clone();
            let text = Value::Object(object).to_string();
            let twice = format!("{{{}:{},{}", json!(key), odd_value(rng), &text[1..]);
            return twice.into_bytes();
        }
        2 | 3 if !keys.is_empty() => {
            let key = rng.pick(&keys).clone();
            object.insert(key, odd_value(rng));
        }
        4 => {
            object.insert(format!("x{}", rng.below(100)), odd_value(rng));
        }
        // The address's part again in the body, with another value.
        5 if !params.is_empty() => {
            let (name, _) = rng.pick(params);
            object.insert(name.clone(), json!(rng.pick(&ODD_VALUES)));
        }
        6 => return rng.pick(&[&b"[]"[..], b"null", b"\"x\"", b"7", b"{", b""]).to_vec(),
        // Bytes that are not UTF-8 inside a string.
        _ => {
            let mut text = Value::Object(object).to_string().into_bytes();
            if let Some(at) = text.iter().position(|&byte| byte == b'"') {
                text.insert(at + 1, 0xff);
            }
            return text;
        }
    }
    Value::Object(object).to_string().into_bytes()
}

/// `seed`'s bytes, bent one to four times, as smbd's fuzzing bends them.
fn bend_bytes(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut data = seed.to_vec();
    for _ in 0..=rng.below(4) {
        let at = rng.below(data.len());
        match rng.below(5) {
            0 if at < data.len() => data[at] ^= 1 << rng.below(8),
            1 => data.truncate(at),
            2 => {
                let more: Vec<u8> = (0..=rng.below(16))
                    .map(|_| u8::try_from(rng.next() >> 56).unwrap_or(0))
                    .collect();
                data.splice(at..at, more);
            }
            3 if at < data.len() => {
                let end = (at + 1 + rng.below(16)).min(data.len());
                data.drain(at..end);
            }
            _ => data.push(*rng.pick(b"{}[]\",:\\\0")),
        }
    }
    data
}

/// The message `body` to the MCP server, bent once where a tool reads:
/// in its parameters, or in the arguments they give the tool. A batch is
/// bent in one of its messages.
fn bend_message(rng: &mut Rng, body: &Value) -> Vec<u8> {
    const PLACE: &str = "bent-here";
    let mut message = body.clone();
    let one = match &mut message {
        Value::Array(batch) if !batch.is_empty() => {
            let at = rng.below(batch.len());
            &mut batch[at]
        }
        whole => whole,
    };
    // A message that is no object has no parameters: all of it is bent.
    if !one.is_object() {
        return bend_json(rng, one, &[]);
    }
    let deep = rng.chance(60) && one["params"]["arguments"].is_object();
    let place = if deep { &mut one["params"]["arguments"] } else { &mut one["params"] };
    let bent = bend_json(rng, place, &[]);
    *place = json!(PLACE);
    // What the bend made may be no JSON: it goes in as the bytes it is.
    let text = message.to_string().into_bytes();
    let marked = format!("\"{PLACE}\"").into_bytes();
    match text.windows(marked.len()).position(|window| window == marked) {
        Some(at) => [&text[..at], &bent, &text[at + marked.len()..]].concat(),
        None => bent,
    }
}

/// Sets header `name` to `value`, in the place it has, or last.
fn set_header(sent: &mut Sent, name: &str, value: Vec<u8>) {
    match sent.headers.iter_mut().find(|(have, _)| have == name) {
        Some((_, have)) => *have = value,
        None => sent.headers.push((name.to_string(), value)),
    }
}

/// `sent`, bent once.
fn bend(rng: &mut Rng, sent: &mut Sent, seed: &Seed, sessions: &Sessions) {
    let names: Vec<String> = sent.headers.iter().map(|(name, _)| name.clone()).collect();
    match rng.below(18) {
        0 if !names.is_empty() => {
            let name = rng.pick(&names).clone();
            sent.headers.retain(|(have, _)| *have != name);
        }
        1 if !sent.headers.is_empty() => {
            let twice = rng.pick(&sent.headers).clone();
            sent.headers.push(twice);
        }
        2 if !sent.headers.is_empty() => {
            let at = rng.below(sent.headers.len());
            sent.headers[at].1 = match rng.below(3) {
                0 => Vec::new(),
                1 => vec![b'a'; 64 << 10],
                _ => {
                    let mut value = sent.headers[at].1.clone();
                    value.push(0xff);
                    value
                }
            };
        }
        // Another caller's cookie, or another token.
        3 => {
            let other = Sent::of(seed, *rng.pick(&CALLERS), sessions);
            sent.headers.retain(|(name, _)| name != "cookie" && name != "authorization");
            sent.headers.extend(
                other
                    .headers
                    .into_iter()
                    .filter(|(name, _)| name == "cookie" || name == "authorization"),
            );
        }
        4 => {
            let host = rng.pick(&[
                "evil.example",
                "nas.example:9091",
                "NAS.EXAMPLE:9090",
                "nas.example",
                "",
            ]);
            set_header(sent, "host", host.as_bytes().to_vec());
        }
        5 => {
            let origin = rng.pick(&[
                "https://evil.example",
                "null",
                "http://nas.example:9090",
                "https://nas.example",
            ]);
            set_header(sent, "origin", origin.as_bytes().to_vec());
        }
        6 => {
            let site = rng.pick(&["cross-site", "same-site", "none", "same-origin", "x"]);
            set_header(sent, "sec-fetch-site", site.as_bytes().to_vec());
        }
        7 if !sent.body.is_empty() => sent.body = bend_bytes(rng, &sent.body),
        8 | 9 => {
            let body = seed.body.clone().unwrap_or_else(|| json!({}));
            sent.body = bend_json(rng, &body, &seed.params);
        }
        // A part of the address that is an argument, made strange.
        10 if !seed.params.is_empty() => {
            let mut params = seed.params.clone();
            let at = rng.below(params.len());
            let part = match rng.below(6) {
                0 => "..".to_string(),
                1 => "%2e%2e".to_string(),
                2 => "a%00b".to_string(),
                3 => "%ff%fe".to_string(),
                4 => "a".repeat(300),
                _ => "a%2Fb".to_string(),
            };
            params[at].1 = String::new();
            let path = fill(seed.template, &params);
            let mut parts: Vec<String> = path.split('/').map(String::from).collect();
            // The first part is the empty one before the address's first
            // slash.
            if let Some(empty) = parts.iter().skip(1).position(String::is_empty) {
                parts[empty + 1] = part;
            }
            sent.target = format!("{}{}", parts.join("/"), seed.query).into_bytes();
        }
        11 => {
            sent.method = rng.pick(&[&b"GET"[..], b"POST", b"HEAD", b"PUT", b"OPTIONS"]).to_vec();
        }
        12 => {
            let query = rng.pick(&[
                "?since=-1",
                "?since=99999999999999999999",
                "?limit=0",
                "?x",
                "?&&=",
                "?since=1&since=2",
                "?lines=%ff%fe",
                "?service=a%00b",
                "?all=yes",
            ]);
            sent.target.extend_from_slice(query.as_bytes());
        }
        13 => {
            let kind = rng.pick(&[
                "text/plain",
                "application/json; charset=latin1",
                "application/jsonx",
                "",
            ]);
            set_header(sent, "content-type", kind.as_bytes().to_vec());
        }
        14 => sent.body = vec![b' '; (1 << 20) + 1],
        // A header of the MCP server's set against the message, on any
        // request: a revision, a method or a tool that the body does not
        // name.
        15 => {
            let (name, values): (&str, &[&str]) = match rng.below(3) {
                0 => (
                    "mcp-protocol-version",
                    &["2026-07-28", "2025-06-18", "2024-11-05", "1999-01-01", ""],
                ),
                1 => ("mcp-method", &["tools/call", "tools/list", "ping", "x", ""]),
                _ => ("mcp-name", &["ask", "read", "shell", ""]),
            };
            let value = rng.pick(values).as_bytes().to_vec();
            if rng.chance(70) {
                set_header(sent, name, value);
            } else {
                sent.headers.push((name.into(), value));
            }
        }
        16 if seed.template == MCP => {
            if let Some(body) = &seed.body {
                sent.body = bend_message(rng, body);
            }
        }
        _ => {
            let value: &[u8] = rng.pick(&[
                &b"Basic Y2hhc2U6cmlnaHQ="[..],
                b"Bearer",
                b"Bearer ",
                b"bearer dkv_chase_aa",
                b"Bearer dkv_",
                b"Bearer dkc_",
                b"Bearer dkc_chase_aa",
                b"Bearer dkp_chase_aa",
            ]);
            if rng.chance(50) {
                set_header(sent, "authorization", value.to_vec());
            } else {
                sent.headers.push(("authorization".into(), value.to_vec()));
            }
        }
    }
}

/// What a request left behind, to compare before and after.
struct Traces {
    worker: usize,
    authd: usize,
    /// The number of the journal's newest row.
    newest: i64,
    tries: u32,
    wrong_tokens: u32,
}

impl Traces {
    fn now(world: &World, address: IpAddr) -> Traces {
        let shared = &world.console.shared;
        Traces {
            worker: world.calls.lock().unwrap().len(),
            authd: world.console.authd.seen.lock().unwrap().len(),
            newest: shared
                .store
                .as_ref()
                .and_then(|store| store.changes(0, 1).unwrap().first().map(|row| row.id))
                .unwrap_or(0),
            tries: shared.tries.borrow().count(address),
            wrong_tokens: shared.wrong_tokens.borrow().count(address),
        }
    }
}

/// The worker's methods that change something of the person's.
const CHANGING: [&str; 14] = [
    "app-begin",
    "app-confirm",
    "app-remove",
    "passkey-begin",
    "passkey-add",
    "passkey-remove",
    "passkey-rename",
    "codes",
    "trusted-forget",
    "token-add",
    "token-remove",
    "client-add",
    "client-remove",
    "preferences-change",
];

/// What admind asks authd as a person signs in.
const SIGN_IN_VERBS: [&str; 6] = ["begin", "trusted", "code", "challenge", "passkey", "start"];

/// The probe of the action of `actions` that the body of a check names,
/// with its arguments, as admind prepares it: the one command the check may
/// run.
fn probe_of(actions: &[&'static dyn Registered], body: &[u8]) -> Option<Value> {
    let asked: Value = serde_json::from_slice(body).ok()?;
    let action = actions.iter().find(|action| asked["action"] == action.id())?;
    let args = asked.get("args").cloned().unwrap_or_else(|| json!({}));
    action.prepare(args).ok()?.probe.map(|probe| json!(probe))
}

/// Whether `argv` is a probe of the actions this test has.
fn is_probe(argv: &Value) -> bool {
    argv[0] == "probe"
        || *argv == json!(["service", "list", "--json"])
        || *argv == json!(["admin", "reach"])
}

/// Whether `text` holds `word` as a word of its own: with no letter, digit
/// or underscore next to it.
fn holds_word(text: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// An answer, whole.
struct Got {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// Waits, for at most 10 seconds, until no task waits or runs.
async fn tasks_end(shared: &Shared) {
    for _ in 0..1000 {
        if shared.tasks.unfinished() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// What admind answers `request` with, as a task of its own so that a
/// panic is caught and told; an answer that does not end in ten seconds
/// is a failure too.
async fn answer(shared: &Rc<Shared>, request: Request<Full<Bytes>>) -> Result<Got, String> {
    let shared = Rc::clone(shared);
    let job = tokio::task::spawn_local(async move {
        let Ok(response) = handle(shared, PEER.parse().unwrap(), request).await;
        let (head, body) = response.into_parts();
        let bytes = tokio::time::timeout(Duration::from_secs(10), body.collect()).await;
        (head, bytes)
    });
    match job.await {
        Ok((head, Ok(Ok(collected)))) => Ok(Got {
            status: head.status.as_u16(),
            headers: head.headers,
            body: collected.to_bytes().to_vec(),
        }),
        Ok((_, Ok(Err(err)))) => Err(format!("the answer's body failed: {err}")),
        Ok((_, Err(_))) => Err("the answer did not end in 10 seconds".into()),
        Err(err) if err.is_panic() => {
            let panic = err.into_panic();
            let why = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
            Err(format!("admind panicked: {why}"))
        }
        Err(err) => Err(format!("the request's task ended: {err}")),
    }
}

/// The error IDs an answer may have: the console's own, and those the
/// actions' commands give.
fn known_id(id: &str, actions: &[&'static dyn Registered]) -> bool {
    let own = id
        .strip_prefix("ADM-1")
        .is_some_and(|rest| rest.len() == 2 && rest.bytes().all(|byte| byte.is_ascii_digit()));
    own || actions.iter().any(|action| action.errors().contains(&id))
}

/// Whether `answer` is a response of JSON-RPC 2.0: the request's ID or
/// null, and a result or an error with its code and its words, never both.
fn is_rpc_answer(answer: &Value) -> bool {
    let error = &answer["error"];
    let said = error["code"].is_i64() && error["message"].as_str().is_some_and(|m| !m.is_empty());
    answer["jsonrpc"] == "2.0"
        && answer.get("id").is_some()
        && answer["result"].is_object() != error.is_object()
        && (said || !error.is_object())
}

/// The commands the messages of `body` may have the worker run for an MCP
/// client: for each call of a tool that names an action, the action's
/// command if it is a read, and its probe.
fn mcp_may_run(actions: &[&'static dyn Registered], body: &[u8]) -> Vec<Value> {
    let messages = match serde_json::from_slice::<Value>(body) {
        Ok(Value::Array(batch)) => batch,
        Ok(one) => vec![one],
        Err(_) => Vec::new(),
    };
    let mut may = Vec::new();
    for message in &messages {
        let given = &message["params"]["arguments"];
        let Some(action) = actions.iter().find(|action| given["action"] == action.id()) else {
            continue;
        };
        let args = match given.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(args) => args.clone(),
        };
        let Ok(prepared) = action.prepare(args) else { continue };
        if !action.changes() {
            may.push(json!(prepared.argv));
        }
        may.extend(prepared.probe.map(|probe| json!(probe)));
    }
    may
}

/// What a request left behind, as [`broken`] read it from the traces.
struct Left<'a> {
    /// What the worker was asked, and what the test authd noted of a read
    /// it ran as an administrator.
    calls: &'a [(String, Value)],
    /// Whether a command ran as an administrator.
    elevated: bool,
    /// Whether the journal got a row.
    wrote: bool,
    /// Whether the worker changed something of the person's.
    changed: bool,
}

/// Checks what must hold of the answer `got` to `sent` when it went to
/// the MCP server, whoever sent it and whatever it answered: no message
/// makes a change, and the worker ran a read's command or a probe that
/// the message names, and nothing else.
fn mcp_broken(
    actions: &[&'static dyn Registered],
    sent: &Sent,
    got: &Got,
    json: Option<&Value>,
    left: &Left<'_>,
) -> Option<String> {
    if left.elevated || left.wrote || left.changed {
        return Some(format!(
            "a request to the MCP server ran a command as an administrator, wrote the journal \
             or changed its owner's things: {:?}",
            left.calls
        ));
    }
    let may = mcp_may_run(actions, &sent.body);
    for (method, params) in left.calls {
        let allowed = match method.as_str() {
            "run" => is_probe(&params["argv"]) || may.contains(&params["argv"]),
            "client-read" => {
                actions.iter().any(|action| params[0] == action.id() && !action.changes())
            }
            _ => false,
        };
        if !allowed {
            return Some(format!("a message to the MCP server had {method} {params} run"));
        }
    }
    if sent.method != b"POST" {
        return None;
    }
    match (got.status, json) {
        (200, Some(Value::Array(batch)))
            if !batch.is_empty() && batch.iter().all(is_rpc_answer) =>
        {
            None
        }
        (200, Some(one)) if is_rpc_answer(one) => None,
        (200, _) => Some("200 from the MCP server is no answer of JSON-RPC".into()),
        (202, _) if !got.body.is_empty() => Some("202 from the MCP server has a body".into()),
        _ => None,
    }
}

/// Checks what must hold of the answer `got` to `sent` from `caller`, given
/// what was seen before it and after it; the reason it does not, if any.
fn broken(
    world: &World,
    actions: &[&'static dyn Registered],
    sessions: &Sessions,
    sent: &Sent,
    caller: Caller,
    got: &Got,
    (before, after): (&Traces, &Traces),
) -> Option<String> {
    let path = sent.path();
    let api = path.starts_with("/api/");
    let head = sent.method == b"HEAD";
    // A one-time link is a phone's: its answers are plain words, which
    // name their ID, and each fetch counts against the address's tries.
    let once = matches!(sent.method.as_slice(), b"GET" | b"HEAD")
        && matches!(lookup(Verb::Get, &path), Lookup::Found(entry, _) if entry.route == Route::Once);
    let body = String::from_utf8_lossy(&got.body);
    let json: Option<Value> = serde_json::from_slice(&got.body).ok();
    let mcp = path == MCP;
    // An error is JSON with an ID, a title and a detail, or what the
    // protocol of the MCP server answers a message it does not take with.
    let error = if got.status >= 400 {
        if head {
            None
        } else if mcp && json.as_ref().is_some_and(|json| json.get("jsonrpc").is_some()) {
            let refused =
                json.as_ref().is_some_and(|json| is_rpc_answer(json) && json["error"].is_object());
            if !refused {
                return Some(format!("{} is no error of JSON-RPC: {body}", got.status));
            }
            None
        } else if once && json.is_none() {
            // Refused before the route, as for a wrong host, it is JSON.
            let id = if got.status == 429 { "ADM-105" } else { "ADM-140" };
            if !body.contains(&format!("({id})")) {
                return Some(format!(
                    "{} from a one-time link does not name {id}: {body}",
                    got.status
                ));
            }
            Some(id.to_string())
        } else {
            let Some(error) = json.as_ref().map(|json| &json["error"]) else {
                return Some(format!("{} is not JSON: {body}", got.status));
            };
            let id = error["id"].as_str().unwrap_or_default();
            if !known_id(id, actions) {
                return Some(format!("{} has no known error ID: {body}", got.status));
            }
            let words = |key: &str| error[key].as_str().is_some_and(|text| !text.is_empty());
            if !words("title") || !words("detail") {
                return Some(format!("{id} has no title or no detail: {body}"));
            }
            Some(id.to_string())
        }
    } else {
        if api && !head && json.is_none() {
            return Some(format!("{} from the API is not JSON: {body}", got.status));
        }
        None
    };
    // What happened between the two looks.
    let calls: Vec<(String, Value)> = world.calls.lock().unwrap()[before.worker..].to_vec();
    let asked: Vec<String> = world.console.authd.seen.lock().unwrap()[before.authd..].to_vec();
    let elevated = asked.iter().any(|line| line.starts_with("elevate "));
    let changed_worker = calls.iter().any(|(method, params)| {
        CHANGING.contains(&method.as_str())
            && worker_answers(method, params, &Calls::default()).is_ok()
    });
    if mcp {
        let left = Left {
            calls: &calls,
            elevated,
            wrote: after.newest != before.newest,
            changed: changed_worker,
        };
        if let Some(why) = mcp_broken(actions, sent, got, json.as_ref(), &left) {
            return Some(why);
        }
    }
    // ADM-123 and an action's own IDs are a command that ran and failed.
    let refused = error.as_deref().is_some_and(|id| id.starts_with("ADM-") && id != "ADM-123");
    if refused {
        if elevated {
            return Some("a refused request ran a command as an administrator".into());
        }
        if after.newest != before.newest {
            return Some("a refused request wrote a row of the journal".into());
        }
        if changed_worker {
            return Some(format!("a refused request changed the person's things: {calls:?}"));
        }
        // authd refuses a shell with ADM-132 itself, after it was asked.
        let shell = calls.iter().any(|(method, _)| method == "terminal");
        if shell && error.as_deref() != Some("ADM-132") {
            return Some(format!("a refused request started a shell: {calls:?}"));
        }
    }
    if matches!(sent.method.as_slice(), b"GET" | b"HEAD")
        && ((elevated && !read_as_administrator(actions, &path)) || after.newest != before.newest)
    {
        return Some("a read ran a command as an administrator or wrote the journal".into());
    }
    if path == "/api/1/check" {
        let own = probe_of(actions, &sent.body);
        let only_probes = calls.iter().all(|(method, params)| {
            method == "run" && (is_probe(&params["argv"]) || own.as_ref() == Some(&params["argv"]))
        });
        if elevated || !only_probes || after.newest != before.newest {
            return Some(format!("a check ran more than probes: {calls:?} {asked:?}"));
        }
    }
    // The tries: a sign-in request that reached authd was counted, and
    // nothing else was.
    let sign_in = matches!(
        lookup(Verb::Post, &path),
        Lookup::Found(entry, _) if entry.access == Access::SignIn
    ) && sent.method == b"POST";
    let counted = after.tries - before.tries.min(after.tries);
    if counted > 1 {
        return Some(format!("one request counted {counted} sign-in tries"));
    }
    if !sign_in && !once && counted != 0 {
        return Some(format!("{counted} sign-in tries were counted for a request that is not one"));
    }
    // Whether authd was asked about the sign-in itself: a token the
    // request also brings is asked about first, whatever the route, and a
    // session that ended before is told to authd when it can be.
    let signing = asked
        .iter()
        .any(|line| SIGN_IN_VERBS.contains(&line.split(' ').next().unwrap_or_default()));
    if sign_in && signing && counted != 1 {
        return Some(format!(
            "a sign-in request that reached authd counted {counted} tries: {asked:?}"
        ));
    }
    let wrong = asked
        .iter()
        .filter(|line| {
            let shown = |verb: &str| {
                line.strip_prefix(verb).map(|rest| rest.split(' ').next().unwrap_or_default())
            };
            shown("token ").is_some_and(|hex| token_answer(hex).is_none())
                || shown("client ").is_some_and(|hex| client_answer(hex).is_none())
        })
        .count();
    let counted_tokens = after.wrong_tokens - before.wrong_tokens.min(after.wrong_tokens);
    if usize::try_from(counted_tokens).unwrap_or(usize::MAX) != wrong {
        return Some(format!("{wrong} tokens authd refused, and {counted_tokens} counted"));
    }
    // A stranger is told nothing of an account.
    let request_text = sent.describe();
    let secrets = sessions.secrets();
    let tokens = [TOKEN_ALL, TOKEN_READ, TOKEN_OTHER, CLIENT_ALL, CLIENT_READ];
    let known = secrets.iter().any(|secret| request_text.contains(secret.as_str()))
        || tokens.iter().any(|token| request_text.contains(token));
    let signs_in = sign_in && got.status < 400;
    if !known && !signs_in && caller != Caller::AddsStep {
        let mut answer_text = body.into_owned();
        for (name, value) in &got.headers {
            answer_text
                .push_str(&format!("\n{name}: {}", String::from_utf8_lossy(value.as_bytes())));
        }
        for name in ACCOUNTS {
            if holds_word(&answer_text, name) && !holds_word(&request_text, name) {
                return Some(format!("an answer to a stranger names {name}: {answer_text}"));
            }
        }
        if let Some(secret) = secrets.iter().find(|secret| answer_text.contains(secret.as_str())) {
            return Some(format!("an answer to a stranger holds a session's secret {secret}"));
        }
    }
    None
}

#[test]
fn every_route_of_the_table_has_a_request_to_start_from() {
    let actions: Vec<&'static dyn Registered> = REGISTRY.to_vec();
    let seeds = seeds(&actions);
    for entry in TABLE {
        // The event stream and a follow of the log do not end; the lab's
        // own route is no installed console's.
        if matches!(entry.path, "/api/1/events" | "/api/1/logs/follow")
            || entry.path.starts_with("/api/1/test/")
        {
            continue;
        }
        let method = match entry.verb {
            Verb::Get => "GET",
            Verb::Head => "HEAD",
            Verb::Post => "POST",
        };
        assert!(
            seeds.iter().any(|seed| seed.method == method && seed.template == entry.path),
            "no request starts from {method} {}",
            entry.path
        );
    }
}

#[test]
fn requests_of_every_caller_bent_at_random_keep_what_the_console_promises() {
    run(async {
        let world = world("fuzz-callers");
        let mut offered: Vec<&'static dyn Registered> = REGISTRY.to_vec();
        // The lab's long action starts tasks that run for seconds: the
        // tasks' own tests drive it.
        offered.extend(LAB.iter().copied().filter(|action| !action.long()));
        let actions: &'static [&'static dyn Registered] = Box::leak(offered.into_boxed_slice());
        world.console.shared.actions.set(actions);
        // An `ask` waits for an administrator's answer, which comes in
        // another round or never.
        world.console.shared.ask_wait.set(Duration::from_millis(2));
        let seeds = seeds(actions);
        // Mostly the requests that can work: a bad example is refused at
        // the first look at its arguments.
        let good: Vec<&Seed> = seeds.iter().filter(|seed| !seed.what.contains(" bad ")).collect();
        // The MCP server and the page for its clients, which are few
        // among the actions' hundreds, get a share of their own.
        let answers = |seed: &Seed| seed.template.starts_with("/api/1/clients/requests/{id}/");
        let of_clients: Vec<&Seed> = seeds
            .iter()
            .filter(|seed| seed.template == MCP || seed.template.contains("/clients"))
            .collect();
        let mut approved = 0;
        let address = reach::counted(PEER.parse().unwrap());
        let mut rng = Rng::new();
        let mut sessions = Sessions::new(&world).await;
        let (mut sent_count, mut statuses) = (0, std::collections::BTreeMap::new());
        for round in 0..rounds() {
            if round > 0 && round % ROUNDS_PER_SESSION == 0 {
                sessions.end(&world).await;
                sessions = Sessions::new(&world).await;
            }
            let seed = if rng.chance(CLIENTS_SHARE) {
                *rng.pick(&of_clients)
            } else if rng.chance(75) {
                *rng.pick(&good)
            } else {
                rng.pick(&seeds)
            };
            // Mostly the callers whose requests get past the first checks:
            // an AI client at the MCP server, and an administrator who
            // may answer what a client asked.
            let caller = if rng.chance(30) {
                *rng.pick(&CALLERS)
            } else if seed.template == MCP {
                *rng.pick(&CLIENTS)
            } else if answers(seed) && rng.chance(60) {
                Caller::Unlocked
            } else {
                *rng.pick(&ADMITTED)
            };
            // A yes or a no is to a request that waits, when one does.
            let waits = {
                let requests = world.console.shared.client_requests.borrow();
                requests.waiting().next().map(|request| request.id)
            };
            let answering;
            let seed = match waits {
                Some(id) if answers(seed) => {
                    answering =
                        Seed { params: vec![("id".into(), id.to_string())], ..seed.clone() };
                    &answering
                }
                _ => seed,
            };
            let mut sent = Sent::of(seed, caller, &sessions);
            let bends = if rng.chance(35) { 0 } else { 1 + rng.below(3) };
            for _ in 0..bends {
                bend(&mut rng, &mut sent, seed, &sessions);
            }
            let Some(request) = sent.request() else { continue };
            let before = Traces::now(&world, address);
            let got = answer(&world.console.shared, request).await;
            // A task runs its command after the answer: what it does is the
            // request's that began it, and not the next one's.
            if got.as_ref().is_ok_and(|got| got.status == 202) {
                tasks_end(&world.console.shared).await;
            }
            let after = Traces::now(&world, address);
            let failure = |why: &str| {
                format!("round {round}, {caller:?}, from {}: {why}\n{}", seed.what, sent.describe())
            };
            let got = got.unwrap_or_else(|why| panic!("{}", failure(&why)));
            if let Some(why) =
                broken(&world, actions, &sessions, &sent, caller, &got, (&before, &after))
            {
                panic!("{}", failure(&why));
            }
            forget_tries(&world.console.shared);
            sent_count += 1;
            *statuses.entry(got.status).or_insert(0) += 1;
            if seed.what == "client approve" && got.status == 200 {
                approved += 1;
            }
        }
        sessions.end(&world).await;
        // The requests reached the routes and the actions, not only the
        // first checks: some ran, some were refused for each reason.
        if rounds() >= 4000 {
            assert!(sent_count * 10 > rounds() * 9, "{sent_count} of {} sent", rounds());
            for status in [200, 400, 401, 403, 404, 405, 413, 415, 421] {
                assert!(statuses.get(&status).copied().unwrap_or(0) > 20, "{statuses:?}");
            }
            assert!(statuses[&200] > rounds() / 8, "{statuses:?}");
            // Changes ran, and the journal kept them.
            assert!(Traces::now(&world, address).newest > 20);
            // Clients asked, were answered 202 for what they only told,
            // and an administrator's yes ran what some of them asked for.
            assert!(statuses.get(&202).copied().unwrap_or(0) > 5, "{statuses:?}");
            assert!(approved > 0, "no request of a client's was approved");
        }
    });
}
