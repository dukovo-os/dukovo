// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What admind answers (ADR 0514): the order in which a request is
//! checked, the table of routes, and what each answer carries.
//!
//! A request meets these checks, and the first that fails answers it:
//!
//! 1. its method is `GET`, `HEAD` or `POST` and its address is at most 2
//!    KiB, before anything looks at what it asks for;
//! 2. its `Host` names this machine, or it is not answered (421: no other
//!    name's DNS may point a browser here);
//! 3. it brings an API token or a session cookie, not both, and a token
//!    only as one `Authorization: Bearer` header ([`caller`]);
//! 4. its address is a route's, an action's ([`actions`]), or a file's,
//!    or there is none;
//! 5. a token is only for the actions, the action list and the showing of
//!    an action, and authd must take it (401 for a token it does not);
//! 6. the route's access: a route for the signed in wants a session (401
//!    with the cookie cleared, once it has ended), and one that is not for a
//!    session that may only add a second step wants one that may do more;
//! 7. a request that changes anything passes the checks of [`guard`]:
//!    the session's three for the signed in, `Origin` and `Sec-Fetch-Site`
//!    for the sign-in routes, which also take their share of the ten an
//!    address may make in a minute; a token's request needs none, since a
//!    browser sends no token by itself;
//! 8. its body, which only a request that passed the checks above is
//!    allowed to send, is JSON and at most 64 KiB before sign-in and 1 MiB
//!    after;
//! 9. for an action, its arguments are checked, then whether the caller may
//!    ask for it ([`actions::authorize`]), before its command runs; for the
//!    showing of one, which runs nothing, the same arguments are checked,
//!    and a token needs the scope that reads the action's area.
//!
//! Every answer carries [`guard::headers`] and no `Server` header; an answer
//! of the API is `no-store`, a file `no-cache`. [`sign_in`] holds the
//! routes that sign a browser in, [`session`] those of a signed-in session,
//! [`events`] a tab's stream, and [`account`] the person's own second steps
//! and tokens, which their worker asks authd for.
//!
//! When a page gets an error it does not expect, the answer's `id` is in
//! [`errors`](super::errors), and `log admind` has the refusal with the
//! browser's address.

mod account;
mod act;
mod events;
#[cfg(feature = "lab")]
mod lab;
mod session;
mod sign_in;

use std::convert::Infallible;
use std::net::IpAddr;
use std::ops::Deref;
use std::rc::Rc;
use std::time::{Duration, Instant};

use accounts::console::Console;
use accounts::request::Answer;
use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::request::Parts;
use http::{Method, Request, Response, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, Limited};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::actions::{self, Found, Params, Registered};
use super::authd;
use super::caller::{self, TokenCall};
use super::errors::{self, Attempt, Failure, Kind};
use super::files::File;
use super::guard::{self, SESSION_COOKIE};
use super::link::Link;
use super::log;
use super::reach;
use super::shared::Shared;

/// The longest address a request may ask for, with its query.
const ADDRESS_MAX: usize = 2048;

/// The most a body may hold before sign-in and after.
const BODY_MAX_ANONYMOUS: usize = 64 << 10;
const BODY_MAX_SIGNED_IN: usize = 1 << 20;

/// How long a body has to arrive.
const BODY_TIME: Duration = Duration::from_secs(10);

/// What an answer's body is: a few bytes, or a tab's stream of events.
pub type Body = UnsyncBoxBody<Bytes, Infallible>;

/// What a handler gives: an answer, or why there is none.
type Outcome = Result<Reply, Failure>;

/// A string from a browser that may go to authd or a worker: no NUL, which
/// would split a field of the request, and at most 4096 bytes.
#[derive(Default, Deserialize)]
#[serde(try_from = "String")]
struct Text(String);

impl TryFrom<String> for Text {
    type Error = &'static str;

    fn try_from(text: String) -> Result<Text, &'static str> {
        if text.len() > 4096 {
            Err("a value is longer than 4096 bytes")
        } else if text.contains('\0') {
            Err("a value holds a NUL")
        } else {
            Ok(Text(text))
        }
    }
}

impl Deref for Text {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// What an answer says.
enum Content {
    Json(Value),
    File(File),
    Events(events::Events),
}

/// An answer, before its headers.
struct Reply {
    status: StatusCode,
    content: Content,
    cookies: Vec<String>,
    retry_after: Option<u64>,
}

impl Reply {
    /// A 200 with `value`.
    fn json(value: Value) -> Reply {
        Reply {
            status: StatusCode::OK,
            content: Content::Json(value),
            cookies: Vec::new(),
            retry_after: None,
        }
    }

    /// The answer to a request that failed.
    fn failure(failure: &Failure) -> Reply {
        let status = StatusCode::from_u16(failure.kind.status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Reply {
            status,
            content: Content::Json(failure.body()),
            cookies: Vec::new(),
            retry_after: failure.retry_after,
        }
    }

    /// The same, setting the cookie `cookie`.
    fn with_cookie(mut self, cookie: String) -> Reply {
        self.cookies.push(cookie);
        self
    }
}

/// How a request is used: what it takes from the table of routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    Get,
    Post,
    Head,
}

impl Verb {
    /// The verb of `method`, if admind answers it.
    fn of(method: &Method) -> Option<Verb> {
        match *method {
            Method::GET => Some(Verb::Get),
            Method::POST => Some(Verb::Post),
            Method::HEAD => Some(Verb::Head),
            _ => None,
        }
    }
}

/// Whom a route is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    /// Anyone, with no session: the files, and what the page asks first.
    Anyone,
    /// Anyone who is signing in: with the checks of a request that
    /// changes something, and at most ten a minute from an address.
    SignIn,
    /// A signed-in session, even one that may only add a second step.
    Limited,
    /// A signed-in session that may do all that is a person's.
    Full,
    /// What [`Access::Full`] allows, or an API token: the actions, their
    /// list and the showing of one.
    Api,
}

/// The routes of the API, and the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    File,
    Health,
    Fingerprint,
    SignIn,
    SignInCode,
    SignInPasskeyOptions,
    SignInPasskey,
    Session,
    SignOut,
    Unlock,
    UnlockPasskeyOptions,
    UnlockExtend,
    Lock,
    Events,
    Steps,
    AppBegin,
    AppConfirm,
    AppRemove,
    PasskeyBegin,
    PasskeyAdd,
    PasskeyRemove,
    PasskeyRename,
    Codes,
    TrustedForget,
    TokenAdd,
    TokenRemove,
    Actions,
    Show,
    #[cfg(feature = "lab")]
    TestElevate,
}

/// A route in the table.
#[derive(Debug)]
struct Entry {
    verb: Verb,
    path: &'static str,
    route: Route,
    access: Access,
}

const fn entry(verb: Verb, path: &'static str, route: Route, access: Access) -> Entry {
    Entry { verb, path, route, access }
}

/// What a file is answered with, whoever asks.
const FILE: Entry = entry(Verb::Get, "", Route::File, Access::Anyone);

/// Every route of the API. The second-step routes, `session`, `sign-out`
/// and `events` are those a session that may only add a second step uses.
const TABLE: &[Entry] = &[
    entry(Verb::Get, "/api/1/health", Route::Health, Access::Anyone),
    entry(Verb::Get, "/api/1/fingerprint", Route::Fingerprint, Access::Anyone),
    entry(Verb::Post, "/api/1/sign-in", Route::SignIn, Access::SignIn),
    entry(Verb::Post, "/api/1/sign-in/code", Route::SignInCode, Access::SignIn),
    entry(
        Verb::Post,
        "/api/1/sign-in/passkey/options",
        Route::SignInPasskeyOptions,
        Access::SignIn,
    ),
    entry(Verb::Post, "/api/1/sign-in/passkey", Route::SignInPasskey, Access::SignIn),
    entry(Verb::Get, "/api/1/session", Route::Session, Access::Limited),
    entry(Verb::Post, "/api/1/sign-out", Route::SignOut, Access::Limited),
    entry(Verb::Get, "/api/1/events", Route::Events, Access::Limited),
    entry(Verb::Post, "/api/1/unlock", Route::Unlock, Access::Full),
    entry(Verb::Post, "/api/1/unlock/passkey/options", Route::UnlockPasskeyOptions, Access::Full),
    entry(Verb::Post, "/api/1/unlock/extend", Route::UnlockExtend, Access::Full),
    entry(Verb::Post, "/api/1/lock", Route::Lock, Access::Full),
    entry(Verb::Get, "/api/1/account/second-step", Route::Steps, Access::Limited),
    entry(Verb::Post, "/api/1/account/second-step/app/begin", Route::AppBegin, Access::Limited),
    entry(Verb::Post, "/api/1/account/second-step/app/confirm", Route::AppConfirm, Access::Limited),
    entry(Verb::Post, "/api/1/account/second-step/app/remove", Route::AppRemove, Access::Limited),
    entry(
        Verb::Post,
        "/api/1/account/second-step/passkey/begin",
        Route::PasskeyBegin,
        Access::Limited,
    ),
    entry(Verb::Post, "/api/1/account/second-step/passkey/add", Route::PasskeyAdd, Access::Limited),
    entry(
        Verb::Post,
        "/api/1/account/second-step/passkey/remove",
        Route::PasskeyRemove,
        Access::Limited,
    ),
    entry(
        Verb::Post,
        "/api/1/account/second-step/passkey/rename",
        Route::PasskeyRename,
        Access::Limited,
    ),
    entry(Verb::Post, "/api/1/account/second-step/codes", Route::Codes, Access::Limited),
    entry(
        Verb::Post,
        "/api/1/account/second-step/trusted/forget",
        Route::TrustedForget,
        Access::Limited,
    ),
    entry(Verb::Post, "/api/1/account/tokens/add", Route::TokenAdd, Access::Full),
    entry(Verb::Post, "/api/1/account/tokens/remove", Route::TokenRemove, Access::Full),
    entry(Verb::Get, "/api/1/actions", Route::Actions, Access::Api),
    entry(Verb::Post, "/api/1/show", Route::Show, Access::Api),
    #[cfg(feature = "lab")]
    entry(Verb::Post, "/api/1/test/elevate", Route::TestElevate, Access::Full),
];

/// What a request asks for: a route of the table, or an action with the
/// arguments its address gives.
enum Target {
    Route(&'static Entry),
    Action(&'static dyn Registered, Params),
}

impl Target {
    fn access(&self) -> Access {
        match self {
            Target::Route(entry) => entry.access,
            Target::Action(..) => Access::Api,
        }
    }
}

/// What a request with `verb` for `path` asks for: a route of the table,
/// or else an action's address. An action is a `GET` or a `POST`, never a
/// `HEAD`, which would run a read for nothing.
fn target(
    actions: &'static [&'static dyn Registered],
    verb: Verb,
    path: &str,
) -> Result<Target, Failure> {
    let method = || no_method_of(verb);
    match lookup(verb, path) {
        Lookup::Found(entry) => return Ok(Target::Route(entry)),
        Lookup::WrongVerb => return Err(method()),
        Lookup::Missing => {}
    }
    match actions::find(actions, verb == Verb::Post, path) {
        Found::Action(_, _) if verb == Verb::Head => Err(method()),
        Found::Action(action, params) => Ok(Target::Action(action, params)),
        Found::WrongMethod => Err(method()),
        Found::None => Err(Failure::new(Kind::NoRoute, "the console has no such route")),
    }
}

/// The failure for a verb that the address does not take.
fn no_method_of(verb: Verb) -> Failure {
    let name = match verb {
        Verb::Get => "GET",
        Verb::Post => "POST",
        Verb::Head => "HEAD",
    };
    Failure::new(Kind::NoMethod, format!("{name} is not used here"))
}

/// What the table makes of a request.
#[derive(Debug)]
enum Lookup {
    Found(&'static Entry),
    /// The address is a route's, which takes another verb.
    WrongVerb,
    /// No route has the address.
    Missing,
}

/// The route for `verb` and `path`: an address of the API must be in the
/// table; any other is a file's, which `GET` and `HEAD` ask for.
fn lookup(verb: Verb, path: &str) -> Lookup {
    if path != "/api" && !path.starts_with("/api/") {
        return if verb == Verb::Post { Lookup::WrongVerb } else { Lookup::Found(&FILE) };
    }
    match TABLE.iter().find(|entry| entry.path == path) {
        Some(entry) if entry.verb == verb => Lookup::Found(entry),
        Some(_) => Lookup::WrongVerb,
        None => Lookup::Missing,
    }
}

/// Whether `content_type` is JSON, as `application/json` with parameters
/// or without.
fn is_json(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|text| text.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

/// The most a body may hold, for a request that is signed in or not.
fn body_limit(signed_in: bool) -> usize {
    if signed_in { BODY_MAX_SIGNED_IN } else { BODY_MAX_ANONYMOUS }
}

/// `text` as the log may show it: control characters as `?`, and at most
/// 64 characters, since a browser chooses it.
fn shown(text: &str) -> String {
    text.chars().take(64).map(|c| if c.is_control() { '?' } else { c }).collect()
}

/// A session, as a request sees it.
struct Live {
    cookie: String,
    authd: String,
    name: String,
    needs_step: bool,
    csrf: [u8; 32],
    link: Link,
}

/// What a handler knows of its request.
struct Context<'a> {
    shared: &'a Rc<Shared>,
    /// The browser's address, an IPv4 address as itself.
    peer: IpAddr,
    /// The address the request asks for, without its query.
    path: String,
    /// The name or address the browser asked for, in lower case: the
    /// relying party of a passkey.
    host: String,
    /// The origin the browser is at.
    origin: String,
    body: Bytes,
    /// The trust cookie, if the browser sent one.
    trust: Option<String>,
    user_agent: String,
}

impl Context<'_> {
    /// The body as the JSON `T`, or `T`'s default for a request with no
    /// body.
    fn optional_json<T: DeserializeOwned + Default>(&self) -> Result<T, Failure> {
        if self.body.is_empty() { Ok(T::default()) } else { self.json() }
    }

    /// The body as the JSON `T`.
    fn json<T: DeserializeOwned>(&self) -> Result<T, Failure> {
        serde_json::from_slice(&self.body).map_err(|err| {
            // The error's own words would repeat what the browser sent: a
            // password in a field of the wrong type.
            let what = match err.classify() {
                serde_json::error::Category::Syntax | serde_json::error::Category::Eof => {
                    "the body is not JSON"
                }
                _ => "the body is JSON, but not what this route takes",
            };
            Failure::new(Kind::BadRequest, what)
        })
    }
}

/// Asks authd `console` for a request, and gives the text of its answer,
/// or why it refused, as `attempt` was. `what` is how the log says what was
/// asked.
async fn ask(
    cx: &Context<'_>,
    console: Console,
    attempt: Attempt,
    what: &str,
) -> Result<String, Failure> {
    let asked = authd::ask(&cx.shared.authd, console, &[]).await;
    match asked.map_err(|why| Failure::new(Kind::AuthdSilent, why))?.0 {
        Answer::Done(text) => Ok(text),
        Answer::Refused(why) => Err(refused(cx, what, errors::refusal(attempt, &why))),
    }
}

/// The log's line for a refusal of `what`, which gives its failure back.
fn refused(cx: &Context<'_>, what: &str, failure: Failure) -> Failure {
    log(6, &format!("refused {what} from {}: {}", cx.peer, failure.kind.id()));
    failure
}

/// The failure for a session that is no more.
fn ended() -> Failure {
    Failure::new(Kind::SessionEnded, "this browser has no session, or it has ended")
}

/// The session of `cookie`, as a request sees it; its last use is now.
fn live(shared: &Shared, cookie: &str) -> Option<Live> {
    shared.with_session(cookie, |session| {
        session.seen = Instant::now();
        Live {
            cookie: cookie.to_string(),
            authd: session.authd.clone(),
            name: session.name.clone(),
            needs_step: session.needs_step,
            csrf: session.csrf,
            link: session.worker.clone(),
        }
    })
}

/// The value of the only header `name`, if there is exactly one and it
/// reads as text.
fn single<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

/// Answers `request` from `peer`.
pub async fn handle<B>(
    shared: Rc<Shared>,
    peer: IpAddr,
    request: Request<B>,
) -> Result<Response<Body>, Infallible>
where
    B: http_body::Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let (head, body) = request.into_parts();
    let reply = match answer(&shared, peer, &head, body).await {
        Ok(reply) => reply,
        Err(failure) => {
            let said = [Kind::ForeignRequest, Kind::WrongHost, Kind::Wait, Kind::TokenRefused];
            if said.contains(&failure.kind) {
                shared.refused_request(peer, &format!("{}: {}", failure.kind.id(), failure.detail));
            }
            let reply = Reply::failure(&failure);
            // A browser told its session has ended forgets its cookie, if
            // it sent one. One that sent none may still hold one that is
            // live: its SameSite kept it from a request another site's
            // page made, and the answer must not sign it out.
            let sent = brings_cookie(&head.headers, SESSION_COOKIE);
            if failure.kind == Kind::SessionEnded && sent {
                reply.with_cookie(guard::clear_cookie(SESSION_COOKIE))
            } else {
                reply
            }
        }
    };
    Ok(render(&shared, reply))
}

/// What a request is answered with, or why it is not.
async fn answer<B>(shared: &Rc<Shared>, peer: IpAddr, head: &Parts, body: B) -> Outcome
where
    B: http_body::Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let verb = verb_of(head)?;
    let (host, origin) = host_of(shared, &head.headers)?;
    let cookie = cookie_of(&head.headers, SESSION_COOKIE);
    let token = caller::bearer(&head.headers)?;
    if token.is_some() && cookie.is_some() {
        let why = "a request brings an API token or a session, not both";
        return Err(Failure::new(Kind::BadRequest, why));
    }
    let target = target(shared.actions.get(), verb, head.uri.path())?;
    let access = target.access();
    let token = match token {
        Some(_) if access != Access::Api => {
            let why = "an API token is for the actions and their list, not for this route";
            return Err(Failure::new(Kind::OutOfScope, why));
        }
        Some(token) => Some(caller::check(shared, peer, token).await?),
        None => None,
    };
    let live = cookie.and_then(|cookie| live(shared, cookie));
    check(shared, peer, head, access, live.as_ref(), token.is_some(), &origin)?;
    let signed_in = matches!(access, Access::Limited | Access::Full | Access::Api);
    let body = if verb == Verb::Post {
        read_body(&head.headers, body, signed_in).await?
    } else {
        Bytes::new()
    };
    let cx = Context {
        shared,
        peer,
        path: head.uri.path().to_string(),
        host,
        origin,
        body,
        trust: cookie_of(&head.headers, guard::TRUST_COOKIE).map(str::to_string),
        user_agent: single(&head.headers, &header::USER_AGENT).unwrap_or_default().to_string(),
    };
    match target {
        Target::Route(entry) => dispatch(&cx, entry.route, live, token).await,
        Target::Action(action, params) => act::action(&cx, action, params, live, token).await,
    }
}

/// The failure for a method that the route does not take.
fn no_method(method: &Method) -> Failure {
    Failure::new(Kind::NoMethod, format!("{method} is not used here"))
}

/// The verb of a request whose method admind answers and whose address
/// is not too long.
fn verb_of(head: &Parts) -> Result<Verb, Failure> {
    let verb = Verb::of(&head.method).ok_or_else(|| no_method(&head.method))?;
    let address = head.uri.path_and_query().map_or(head.uri.path(), |parts| parts.as_str());
    if address.len() > ADDRESS_MAX {
        let why = format!("an address is at most {ADDRESS_MAX} bytes");
        return Err(Failure::new(Kind::BadRequest, why));
    }
    Ok(verb)
}

/// The value of the cookie `name`, if the request has it once.
fn cookie_of<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    guard::cookie(
        headers.get_all(header::COOKIE).iter().filter_map(|value| value.to_str().ok()),
        name,
    )
}

/// Whether the request brings the cookie `name` at all, once or more, and
/// whatever it holds.
fn brings_cookie(headers: &HeaderMap, name: &str) -> bool {
    headers.get_all(header::COOKIE).iter().filter_map(|value| value.to_str().ok()).any(|text| {
        text.split(';')
            .any(|pair| pair.trim_start().split_once('=').is_some_and(|(key, _)| key == name))
    })
}

/// The checks of a request for `entry` that come before its body is read,
/// as the module says; `live` is its session, if it has one.
fn check(
    shared: &Shared,
    peer: IpAddr,
    head: &Parts,
    access: Access,
    live: Option<&Live>,
    token: bool,
    origin: &str,
) -> Result<(), Failure> {
    let fetch_site = single(&head.headers, &HeaderName::from_static("sec-fetch-site"));
    let given_origin = single(&head.headers, &header::ORIGIN);
    match (access, live) {
        (Access::Anyone, _) => Ok(()),
        // authd took the token; a browser never sends one by itself, so a
        // request that brings one was made on purpose.
        (Access::Api, None) if token => Ok(()),
        (Access::SignIn, _) => {
            if let Some(why) = guard::sign_in_refusal(given_origin, fetch_site, origin) {
                return Err(Failure::new(Kind::ForeignRequest, why));
            }
            let tries = shared.tries.borrow_mut().take(reach::counted(peer), Instant::now());
            tries.map_err(|seconds| {
                Failure::wait("too many sign-in requests from this address", seconds)
            })
        }
        (Access::Limited | Access::Full | Access::Api, None) => Err(ended()),
        (Access::Full | Access::Api, Some(live)) if live.needs_step => Err(Failure::new(
            Kind::SecondStepOnly,
            "this session may add a second step and sign out, and nothing else yet",
        )),
        (Access::Limited | Access::Full | Access::Api, Some(live)) => {
            let token = single(&head.headers, &HeaderName::from_static(guard::CSRF_HEADER));
            let method = head.method.as_str();
            match guard::csrf_refusal(method, given_origin, fetch_site, token, origin, &live.csrf) {
                Some(why) => Err(Failure::new(Kind::ForeignRequest, why)),
                None => Ok(()),
            }
        }
    }
}

/// Has the handler of `route` answer, for the session `live` if it has one,
/// or the API token `token`.
async fn dispatch(
    cx: &Context<'_>,
    route: Route,
    live: Option<Live>,
    token: Option<TokenCall>,
) -> Outcome {
    match (route, live) {
        (Route::File, _) => file(cx),
        (Route::Health, _) => Ok(Reply::json(json!({ "ok": true }))),
        (Route::Fingerprint, _) => Ok(fingerprint(cx)),
        (Route::SignIn, _) => sign_in::password(cx).await,
        (Route::SignInCode, _) => sign_in::code(cx).await,
        (Route::SignInPasskeyOptions, _) => sign_in::passkey_options(cx).await,
        (Route::SignInPasskey, _) => sign_in::passkey(cx).await,
        (Route::Session, Some(live)) => session::show(cx, &live).await,
        (Route::SignOut, Some(live)) => session::sign_out(cx, &live),
        (Route::Events, Some(live)) => events::open(cx, &live),
        (Route::Unlock, Some(live)) => session::unlock(cx, &live).await,
        (Route::UnlockPasskeyOptions, Some(live)) => session::unlock_options(cx, &live).await,
        (Route::UnlockExtend, Some(live)) => session::extend(cx, &live).await,
        (Route::Lock, Some(live)) => session::lock(cx, &live).await,
        (Route::Steps, Some(live)) => account::steps(&live).await,
        (Route::AppBegin, Some(live)) => account::app_begin(cx, &live).await,
        (Route::AppConfirm, Some(live)) => account::app_confirm(cx, &live).await,
        (Route::AppRemove, Some(live)) => account::app_remove(cx, &live).await,
        (Route::PasskeyBegin, Some(live)) => account::passkey_begin(cx, &live).await,
        (Route::PasskeyAdd, Some(live)) => account::passkey_add(cx, &live).await,
        (Route::PasskeyRemove, Some(live)) => account::passkey_remove(cx, &live).await,
        (Route::PasskeyRename, Some(live)) => account::passkey_rename(cx, &live).await,
        (Route::Codes, Some(live)) => account::codes(cx, &live).await,
        (Route::TrustedForget, Some(live)) => account::trusted_forget(&live).await,
        (Route::TokenAdd, Some(live)) => account::token_add(cx, &live).await,
        (Route::TokenRemove, Some(live)) => account::token_remove(cx, &live).await,
        // For a session or a token, which `check` has let through.
        (Route::Actions, _) => Ok(Reply::json(actions::listed(cx.shared.actions.get()))),
        (Route::Show, _) => act::show(cx, token.as_ref()),
        #[cfg(feature = "lab")]
        (Route::TestElevate, Some(live)) => lab::elevate(cx, &live).await,
        // A route of the signed in, for a session that has ended, which
        // `check` has refused already.
        (_, None) => Err(ended()),
    }
}

/// The host a request asks for and the origin that makes, if the host is
/// this machine's.
fn host_of(shared: &Shared, headers: &HeaderMap) -> Result<(String, String), Failure> {
    let wrong = || Failure::new(Kind::WrongHost, "this is not the name of this machine");
    let header = single(headers, &header::HOST).ok_or_else(wrong)?;
    let identity = shared.identity.borrow();
    let (host, port) =
        guard::host(header, &identity.names, &identity.addresses).ok_or_else(wrong)?;
    let host = host.to_ascii_lowercase();
    let origin = guard::origin(&host, port);
    Ok((host, origin))
}

/// The body of a request that changes something, if it is JSON and not
/// too long, or not there.
async fn read_body<B>(headers: &HeaderMap, body: B, signed_in: bool) -> Result<Bytes, Failure>
where
    B: http_body::Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    if body.is_end_stream() {
        return Ok(Bytes::new());
    }
    if !is_json(single(headers, &header::CONTENT_TYPE)) {
        return Err(Failure::new(Kind::NotJson, "a body is JSON, with the type application/json"));
    }
    let limit = body_limit(signed_in);
    let too_big = || Failure::new(Kind::TooBig, format!("a body is at most {limit} bytes here"));
    let declared =
        single(headers, &header::CONTENT_LENGTH).and_then(|text| text.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return Err(too_big());
    }
    match tokio::time::timeout(BODY_TIME, Limited::new(body, limit).collect()).await {
        Ok(Ok(collected)) => Ok(collected.to_bytes()),
        Ok(Err(err)) if err.is::<http_body_util::LengthLimitError>() => Err(too_big()),
        Ok(Err(_)) => Err(Failure::new(Kind::BadRequest, "the body did not arrive whole")),
        Err(_) => Err(Failure::new(Kind::BadRequest, "the body did not arrive in time")),
    }
}

/// The console's file for the address asked for.
fn file(cx: &Context<'_>) -> Outcome {
    match cx.shared.files.get(&cx.path) {
        Some(file) => Ok(Reply {
            status: StatusCode::OK,
            content: Content::File(file.clone()),
            cookies: Vec::new(),
            retry_after: None,
        }),
        None => Err(Failure::new(Kind::NoRoute, "the console has no such page")),
    }
}

/// What a page shows the first time it is opened: the machine's name and
/// its certificate's fingerprint, to compare with the browser's warning.
fn fingerprint(cx: &Context<'_>) -> Reply {
    let host = cx.shared.identity.borrow().host.clone();
    let fingerprint = cx.shared.certificate.borrow().fingerprint.clone();
    Reply::json(json!({ "host": host, "fingerprint": fingerprint }))
}

/// `reply` as the response that goes out, with the headers every answer
/// carries.
fn render(shared: &Shared, reply: Reply) -> Response<Body> {
    let (kind, caching, body) = match reply.content {
        Content::Json(value) => (
            "application/json",
            "no-store",
            Full::new(Bytes::from(value.to_string())).boxed_unsync(),
        ),
        Content::File(file) => {
            (file.content_type, "no-cache", Full::new(file.bytes).boxed_unsync())
        }
        Content::Events(events) => ("text/event-stream", "no-store", events.boxed_unsync()),
    };
    let mut response = Response::new(body);
    *response.status_mut() = reply.status;
    let headers = response.headers_mut();
    for (key, value) in guard::headers(shared.authority()) {
        headers.insert(HeaderName::from_static(key), HeaderValue::from_static(value));
    }
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(caching));
    for cookie in reply.cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            headers.append(header::SET_COOKIE, value);
        }
    }
    if let Some(seconds) = reply.retry_after {
        headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    }
    response
}

#[cfg(test)]
mod tests;
