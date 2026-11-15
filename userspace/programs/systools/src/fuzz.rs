// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a fuzz target of the web console links to (ADR 0515). The module
//! is there only when systools is built with its `fuzz` feature, which a
//! build for people leaves out; the target is a crate of its own that
//! depends on systools with that feature and calls the items below. Every
//! item takes bytes or text and gives something simple, so that admind's
//! own types stay private.
//!
//! [`Console`] is made once and answers many [`Console::request`] calls:
//! admind's request handling on a console with a certificate and pages of
//! its own, whose authd refuses every request, with no socket and no TLS.
//! A request that has no session or token for a route that asks for one is
//! answered 401, and one that asks for a token's or a sign-in's check
//! reaches that authd and is refused. [`HOST`] is the name the console
//! answers to, and has to be the request's `Host`. A target makes its
//! console once, in a thread-local, as a console is not `Send`, and splits
//! each input into the method, the address, the headers and the body of a
//! request.
//!
//! The parsers of what a client sends, one function each:
//!
//! - [`find`] and [`decode`]: the action an address names and its `%XX`
//!   escapes; [`arguments`] puts an address's arguments and a body
//!   together; [`action_ids`] and [`prepare`] read a JSON body as the
//!   arguments of one of the actions, and make what the action runs and
//!   shows;
//! - [`bearer`], [`cookie`] and [`scopes`]: the `Authorization` header's
//!   API token, the `Cookie` headers' one cookie, and the scopes of a
//!   token as authd writes them;
//! - [`preferences_body`] and [`settings_text`]: the body that changes the
//!   console's preferences, and the settings file text that change is made
//!   to;
//! - [`webauthn_client_data`], [`webauthn_authenticator_data`],
//!   [`webauthn_key`], [`webauthn_attestation`], [`webauthn_assertion`],
//!   [`webauthn_challenge`] and [`webauthn_json`]: what a browser's passkey
//!   sends, as admind passes it on and as authd checks it;
//! - [`websocket_opening`] and [`websocket_frames`]: the request that opens
//!   the Terminal's socket, and the frames a browser sends on it.
//!
//! Each parser that checks is given the console's own name, origin and
//! challenge ([`ORIGIN`], [`CHALLENGE`]), so that a check that passes can
//! be written. None of them reads a file, except the check of a time zone
//! that [`preferences_body`] and [`settings_text`] make, which looks for the
//! name in the system's time zone database.

use std::net::IpAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request, Uri};
use http_body_util::{BodyExt, Full};
use serde_json::{Map, Value};
use tokio::runtime::{Builder, Runtime};
use tokio::task::LocalSet;

use crate::admind::actions::{self, Found, REGISTRY, Registered, show};
use crate::admind::fixture;
use crate::admind::preferences::{Change, Preferences};
use crate::admind::scopes::Scopes;
use crate::admind::webauthn::{self, Credential, Made};
use crate::admind::websocket::{self, Frames, MESSAGE_MAX};
use crate::admind::{QUIET, caller, routes};
use crate::authd::proofs::{self, Ceremony, Expected};
use crate::{settings_edit, timedatectl};

/// The name and port the console answers to: a request's `Host` header
/// has to be this one, as a browser's is.
pub const HOST: &str = "nas.example:9090";

/// The console's origin, which the passkey checks take as the browser's.
pub const ORIGIN: &str = "https://nas.example:9090";

/// The relying party of the passkey checks: the host of [`ORIGIN`].
const RELYING_PARTY: &str = "nas.example";

/// The challenge that the passkey checks expect an answer to.
pub const CHALLENGE: &[u8] = b"fuzz-challenge";

/// The address every request is from: this machine's, which the console's
/// reach lets in.
const PEER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// What a console's authd says to every request.
const REFUSED: &str = "no the console of a fuzz target has no accounts";

/// admind's request handling on a console made for fuzzing, with its own
/// runtime. One console answers any number of requests, one after the
/// other; the limits that count a minute of requests are cleared before
/// each, so that the millionth is answered as the first.
///
/// A console is not [`Send`]: a target that fuzzes on several threads makes
/// one for each.
pub struct Console {
    // Dropped in this order: the tasks that are left, the runtime that
    // runs them, and the console they use.
    local: LocalSet,
    runtime: Runtime,
    console: fixture::Console,
}

impl Console {
    /// Makes a console, which takes a few tens of milliseconds, and keeps
    /// its log quiet: it answers millions of requests, and no one reads it.
    ///
    /// # Panics
    ///
    /// If the runtime cannot be built or the console's files cannot be
    /// made in the system's temporary folder.
    pub fn new() -> Console {
        /// Names each console's files, so that two in one process do not
        /// share them.
        static MADE: AtomicUsize = AtomicUsize::new(0);
        QUIET.store(true, Ordering::Relaxed);
        let runtime = Builder::new_current_thread().enable_all().build().expect("a runtime");
        let local = LocalSet::new();
        let script: Arc<fixture::Script> = Arc::new(|_, _| vec![REFUSED.to_string()]);
        let what = format!("fuzz-{}", MADE.fetch_add(1, Ordering::Relaxed));
        let console = local.block_on(&runtime, async { fixture::Console::new(&what, script) });
        Console { local, runtime, console }
    }

    /// Answers a request made of these bytes as the server would, and
    /// gives the status and the body of the answer. Bytes that make no
    /// request, a method, an address, a header name or a header value that
    /// HTTP does not allow, are answered 400 with no body, as hyper does
    /// before admind hears of them. The headers are taken as given, and
    /// are not made to agree with the body as a connection's are.
    pub fn request(
        &self,
        method: &[u8],
        target: &[u8],
        headers: &[(&[u8], &[u8])],
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let Some(request) = build(method, target, headers, body) else {
            return (400, Vec::new());
        };
        // An address may make so many sign-in requests, and a token be
        // wrong so many times, in a minute. A target would use them up in
        // a moment and from then on find only the refusal.
        let shared = &self.console.shared;
        drop(shared.tries.take());
        drop(shared.wrong_tokens.take());
        let shared = Rc::clone(shared);
        self.local.block_on(&self.runtime, async move {
            let Ok(response) = routes::handle(shared, PEER, request).await;
            let (head, body) = response.into_parts();
            let Ok(collected) = body.collect().await;
            (head.status.as_u16(), collected.to_bytes().to_vec())
        })
    }
}

impl Default for Console {
    fn default() -> Console {
        Console::new()
    }
}

/// The request these bytes make, if they make one.
fn build(
    method: &[u8],
    target: &[u8],
    headers: &[(&[u8], &[u8])],
    body: &[u8],
) -> Option<Request<Full<Bytes>>> {
    let mut request = Request::builder()
        .method(Method::from_bytes(method).ok()?)
        .uri(Uri::try_from(target).ok()?)
        .body(Full::new(Bytes::copy_from_slice(body)))
        .ok()?;
    for &(name, value) in headers {
        let (name, value) = (HeaderName::from_bytes(name).ok()?, HeaderValue::from_bytes(value));
        request.headers_mut().append(name, value.ok()?);
    }
    Some(request)
}

/// The words of a failure, with its ID.
fn said(failure: &crate::admind::errors::Failure) -> String {
    format!("{}: {}", failure.kind.id(), failure.detail)
}

/// The action that a request for `path` asks for, as a `POST` when `post`
/// and a `GET` when not, and the arguments its address gives, as JSON.
/// None when no action has the address, or has it for the other method.
pub fn find(post: bool, path: &str) -> Option<(&'static str, String)> {
    match actions::find(REGISTRY, post, path) {
        Found::Action(action, params) => Some((action.id(), Value::Object(params).to_string())),
        Found::WrongMethod | Found::None => None,
    }
}

/// A part of an address with its `%XX` escapes decoded, if it is one
/// argument.
pub fn decode(part: &str) -> Option<String> {
    actions::decode(part)
}

/// The arguments of a request whose address gave `params`, names and
/// values, and whose body is `body`, as JSON; or why it cannot be read.
///
/// # Errors
///
/// The error's ID and words, for a body that is not a JSON object or that
/// gives an argument the address gives.
pub fn arguments(params: &[(&str, &str)], body: &[u8]) -> Result<String, String> {
    let params: Map<String, Value> =
        params.iter().map(|&(name, value)| (name.into(), value.into())).collect();
    actions::arguments(params, body).map(|args| args.to_string()).map_err(|failure| said(&failure))
}

/// The IDs of the actions, in the order of the list.
pub fn action_ids() -> Vec<&'static str> {
    REGISTRY.iter().map(|action| action.id()).collect()
}

/// What the action `id` takes of the JSON `args`: the command it would
/// run, as its words. It also makes what `POST /api/1/show` answers with.
///
/// # Errors
///
/// The error's ID and words, for an ID that no action has, text that is not
/// JSON, and arguments that the action does not take.
pub fn prepare(id: &str, args: &[u8]) -> Result<Vec<String>, String> {
    let action: &'static dyn Registered = REGISTRY
        .iter()
        .copied()
        .find(|action| action.id() == id)
        .ok_or_else(|| format!("there is no action {id}"))?;
    let args: Value = serde_json::from_slice(args).map_err(|err| err.to_string())?;
    let prepared = action.prepare(args).map_err(|failure| said(&failure))?;
    let _ = show::shown(&prepared, ORIGIN);
    Ok(prepared.argv)
}

/// Whether the `Authorization` headers `values`, one for each, bring an API
/// token.
///
/// # Errors
///
/// The error's words, for more than one bearer, a bearer that is not a
/// token of the console's form, and a value that is no header's.
pub fn bearer(values: &[&[u8]]) -> Result<bool, String> {
    let mut headers = HeaderMap::new();
    for value in values {
        let value = HeaderValue::from_bytes(value).map_err(|_| "not a header's value")?;
        headers.append(header::AUTHORIZATION, value);
    }
    caller::bearer(&headers).map(|token| token.is_some()).map_err(|failure| said(&failure))
}

/// The value of the cookie `name` in the `Cookie` headers `values`, one
/// for each, if there is exactly one such cookie.
pub fn cookie(values: &[&[u8]], name: &str) -> Option<String> {
    let mut headers = HeaderMap::new();
    for value in values {
        headers.append(header::COOKIE, HeaderValue::from_bytes(value).ok()?);
    }
    routes::cookie_of(&headers, name).map(str::to_string)
}

/// Whether `list`, a comma-separated list, is the scopes of an API token
/// as authd writes them.
pub fn scopes(list: &str) -> bool {
    Scopes::parse(list).is_some()
}

/// Whether `body` is a change of the console's preferences that a request
/// may make: JSON of the keys that a preference has, with values that can
/// work.
///
/// # Errors
///
/// The error's words, for a body that is not that.
pub fn preferences_body(body: &[u8]) -> Result<(), String> {
    let change: Change = serde_json::from_slice(body).map_err(|err| err.to_string())?;
    change.check(timedatectl::is_zone).map_err(|failure| said(&failure))
}

/// The settings file `text` after the preferences change `body` is made to
/// it, as the person's worker makes it, or None when it changes nothing.
///
/// # Errors
///
/// The error's words, for a body that is not a change and a text that is
/// not TOML.
pub fn settings_text(text: &str, body: &[u8]) -> Result<Option<String>, String> {
    let change: Change = serde_json::from_slice(body).map_err(|err| err.to_string())?;
    settings_edit::edited(text, |document| {
        change.apply(document);
        // What the worker answers with, which reads the text again.
        let _ = Preferences::of(document, timedatectl::is_zone);
        Ok::<(), std::convert::Infallible>(())
    })
    .map_err(|unedited| match unedited {
        settings_edit::Unedited::Parse { message, .. } => message,
        settings_edit::Unedited::Edit(never) => match never {},
    })
}

/// What the passkey checks expect of the ceremony `create`, or of the use
/// of a passkey when not.
fn expected(create: bool, verified: bool) -> Expected<'static> {
    let ceremony = if create { Ceremony::Create } else { Ceremony::Get };
    Expected {
        ceremony,
        challenge: CHALLENGE,
        origin: ORIGIN,
        relying_party: RELYING_PARTY,
        verified,
    }
}

/// The client data of a browser that ran the ceremony `create` for
/// [`CHALLENGE`] at [`ORIGIN`].
fn client_data(create: bool) -> Vec<u8> {
    let kind = if create { "webauthn.create" } else { "webauthn.get" };
    let challenge = URL_SAFE_NO_PAD.encode(CHALLENGE);
    format!(r#"{{"type":"{kind}","challenge":"{challenge}","origin":"{ORIGIN}"}}"#).into_bytes()
}

/// Checks the client data a browser wrote for the ceremony `create`, or
/// for the use of a passkey when not, against [`CHALLENGE`] and
/// [`ORIGIN`].
///
/// # Errors
///
/// The words that say what is wrong with it.
pub fn webauthn_client_data(create: bool, client_data: &[u8]) -> Result<(), String> {
    proofs::check_client_data(&expected(create, false), client_data).map(drop)
}

/// Reads authenticator data for the ceremony `create`, or for the use of a
/// passkey when not, which is for the console's own name, and checks that
/// the person was verified when `verified`.
///
/// # Errors
///
/// The words that say what is wrong with it.
pub fn webauthn_authenticator_data(
    create: bool,
    verified: bool,
    data: &[u8],
) -> Result<(), String> {
    proofs::read_authenticator_data(&expected(create, verified), data).map(drop)
}

/// Reads a passkey's COSE key.
///
/// # Errors
///
/// The words that say what is wrong with it.
pub fn webauthn_key(cose: &[u8]) -> Result<(), String> {
    proofs::read_key(cose).map(drop)
}

/// Checks the attestation object of a new passkey, as authd checks it,
/// with the client data of a browser that made it right.
///
/// # Errors
///
/// The words that say what is wrong with it.
pub fn webauthn_attestation(attestation: &[u8]) -> Result<(), String> {
    proofs::check_registration(&expected(true, false), &client_data(true), attestation).map(drop)
}

/// Checks the use of a passkey as authd checks it, with the client data of
/// a browser that made it right, for the passkey `key`, a COSE key, whose
/// count is 0.
///
/// # Errors
///
/// The words that say what is wrong with it.
pub fn webauthn_assertion(
    authenticator_data: &[u8],
    signature: &[u8],
    key: &[u8],
) -> Result<(), String> {
    let checked = proofs::check_assertion(
        &expected(false, false),
        &client_data(false),
        authenticator_data,
        signature,
        key,
        0,
    );
    checked.map(drop)
}

/// The challenge that the client data names, as admind reads it.
///
/// # Errors
///
/// The words that say what is wrong with it.
pub fn webauthn_challenge(client_data: &[u8]) -> Result<Vec<u8>, String> {
    webauthn::challenge_of(client_data).map_err(|failure| said(&failure))
}

/// Reads the JSON a browser sends of a passkey, as admind reads it: of a
/// new one when `create`, and of the use of one when not.
///
/// # Errors
///
/// The error's words, for JSON that is not what a browser writes, and for
/// a value that is not base64url.
pub fn webauthn_json(create: bool, json: &[u8]) -> Result<(), String> {
    let not = |err: serde_json::Error| format!("not what a browser writes: {err}");
    if create {
        let made: Made = serde_json::from_slice(json).map_err(not)?;
        webauthn::registration(&made).map(drop).map_err(|failure| said(&failure))
    } else {
        let credential: Credential = serde_json::from_slice(json).map_err(not)?;
        webauthn::assertion(&credential, ORIGIN, RELYING_PARTY)
            .map(drop)
            .map_err(|failure| said(&failure))
    }
}

/// Whether the headers `pairs`, names and values, open the Terminal's
/// socket, and the session's token they bring.
///
/// # Errors
///
/// The refusal's words, for headers that do not.
pub fn websocket_opening(pairs: &[(&[u8], &[u8])]) -> Result<String, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        let name = HeaderName::from_bytes(name).map_err(|_| "not a header's name")?;
        let value = HeaderValue::from_bytes(value).map_err(|_| "not a header's value")?;
        headers.append(name, value);
    }
    websocket::opening(&headers).map(|opening| opening.csrf).map_err(str::to_string)
}

/// The messages a browser's bytes make on the Terminal's socket, read in
/// pieces of `piece` bytes as the socket reads them: how many, and the
/// close code of the rule they broke, if one.
///
/// # Panics
///
/// If what is held while the next message waits for its bytes is ever
/// more than one message and a frame's head: memory a browser could grow.
pub fn websocket_frames(bytes: &[u8], piece: usize) -> (usize, Option<u16>) {
    /// A frame's longest head, and a control frame's longest body, which
    /// may come between a message's fragments.
    const SLACK: usize = 14 + 125;
    let mut frames = Frames::new();
    let mut messages = 0;
    for chunk in bytes.chunks(piece.max(1)) {
        frames.push(chunk);
        loop {
            match frames.next() {
                Ok(Some(_)) => messages += 1,
                Ok(None) => break,
                Err(code) => return (messages, Some(code.number())),
            }
        }
        assert!(frames.held() <= MESSAGE_MAX + SLACK, "{} bytes held", frames.held());
    }
    (messages, None)
}

#[cfg(test)]
mod tests;
