// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of what admind answers: the table of routes and the small
//! decisions on their own, then each check in its place and each route's
//! answer, through [`handle`] with an authd and a worker that are threads
//! answering from scripts (see [`fixture`]). A request here is made as a
//! browser at `https://nas.example:9090` makes it.

use std::collections::HashSet;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use accounts::request::WRONG_PASSWORD;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::HeaderMap;

use super::*;
use crate::admind::fixture::{self, Asked, Console, run};
use crate::admind::guard::TRUST_COOKIE;
use crate::admind::link::Fault;
use crate::admind::sessions;
use crate::admind::shared::unix_now;

/// The browser's address, and what it asks for.
const PEER: &str = "192.0.2.7";
const HOST: &str = "nas.example:9090";
const ORIGIN: &str = "https://nas.example:9090";

/// What the test worker was called with, in order.
type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A console and what its worker was asked.
struct World {
    console: Console,
    calls: Calls,
}

/// What the test worker answers.
fn worker_answers(method: &str, params: &Value) -> Result<Value, Fault> {
    match method {
        "steps" => Ok(json!({
            "app": { "added": 1 }, "passkeys": [], "recovery": null, "trusted": 0,
            "tokens": [], "needed": false,
        })),
        "app-begin" => Ok(json!({ "uri": "otpauth://totp/nas:chase?secret=ABCD" })),
        "app-confirm" if params["code"] == "000000" => {
            Err(Fault::new(Kind::Refused, "that code is not right"))
        }
        "app-confirm" | "codes" => Ok(json!({ "codes": ["AAAA-BBBB"] })),
        "passkey-begin" => Ok(json!({
            "challenge": "AQI",
            "user": { "id": "AwQ", "name": "chase", "displayName": "Chase Y" },
            "exclude": ["BQY"],
        })),
        "passkey-add" => Ok(json!({ "codes": [] })),
        "trusted-forget" => Ok(json!({ "forgotten": 2 })),
        "token-add" => Ok(json!({ "token": "dkv_chase_aa" })),
        _ => Ok(json!({ "done": true })),
    }
}

/// What the test authd answers: three accounts that sign in, as
/// `chase` (an administrator), `alice` (who has a second step) and `eve`
/// (who needs to add one), with the password `right`; the code `123456`;
/// the trusted browser `aa55bb66`.
fn script(calls: Calls) -> Arc<fixture::Script> {
    Arc::new(move |asked: &Asked, mut fds: Vec<OwnedFd>| -> Vec<String> {
        let no = |why: &str| vec![format!("no {why}")];
        let ok = || vec![String::from("ok")];
        let in_15_minutes = || vec![format!("ok {}", unix_now() + 900)];
        match asked.verb.as_str() {
            "begin" => match (asked.text(1).as_str(), asked.text(2).as_str()) {
                ("chase", "right") => vec!["ok done 0a0b01".into()],
                ("alice", "right") => vec!["ok step 0a0b02 app,recovery".into()],
                ("eve", "right") => vec!["ok needs-step 0a0b03".into()],
                ("bob", "right") => no("bob may not use the web console"),
                _ => no(WRONG_PASSWORD),
            },
            "trusted" if asked.text(1) == "aa55bb66" => ok(),
            "trusted" => no("that browser is not trusted"),
            "code" if asked.text(1) == "123456" => ok(),
            "code" => no("that code is not right"),
            "start" => {
                let session = format!("{:0<32}", asked.text(0));
                let trust = if asked.text(1) == "trust" { " aa55bb66" } else { "" };
                fixture::worker(fds.remove(0), Arc::clone(&calls), Arc::new(worker_answers));
                vec![format!("ok 4242 {session}{trust}")]
            }
            "unlock" => match (asked.text(1).as_str(), asked.text(2).as_str()) {
                ("password", "right") | ("code", "123456") => in_15_minutes(),
                ("code", _) => no("that code is not right"),
                _ => no(WRONG_PASSWORD),
            },
            "extend" => in_15_minutes(),
            "lock" | "end" => ok(),
            _ => no("the test's authd does not answer that"),
        }
    })
}

fn world(what: &str) -> World {
    let calls = Calls::default();
    World { console: Console::new(what, script(Arc::clone(&calls))), calls }
}

/// A request, as a browser at the console's own address makes it.
struct Req {
    method: &'static str,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn get(path: &str) -> Req {
    Req { method: "GET", path: path.into(), headers: vec![("host".into(), HOST.into())], body: Vec::new() }
}

fn post(path: &str, body: &Value) -> Req {
    let headers = [("host", HOST), ("origin", ORIGIN), ("content-type", "application/json")];
    Req {
        method: "POST",
        path: path.into(),
        headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        body: body.to_string().into_bytes(),
    }
}

impl Req {
    /// The same with header `name` set to `value`, or left out.
    fn with(mut self, name: &str, value: Option<&str>) -> Req {
        self.headers.retain(|(have, _)| have != name);
        if let Some(value) = value {
            self.headers.push((name.into(), value.into()));
        }
        self
    }

    /// The same as the signed-in `session` makes it.
    fn by(self, session: &Session) -> Req {
        let cookie = format!("{SESSION_COOKIE}={}", session.cookie);
        let token = session.csrf.clone();
        self.with("cookie", Some(&cookie)).with("x-dukovo-csrf", Some(&token))
    }
}

/// An answer, whole.
struct Answered {
    status: u16,
    headers: HeaderMap,
    body: String,
}

impl Answered {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("JSON: {}", self.body))
    }

    /// The ID of the error it is.
    fn error(&self) -> String {
        self.json()["error"]["id"].as_str().unwrap_or_else(|| panic!("an error: {}", self.body)).into()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|value| value.to_str().unwrap())
    }

    /// The cookies it sets, each whole.
    fn cookies(&self) -> Vec<String> {
        self.headers.get_all("set-cookie").iter().map(|v| v.to_str().unwrap().to_string()).collect()
    }

    /// The value of the cookie `name` it sets.
    fn cookie(&self, name: &str) -> Option<String> {
        let start = format!("{name}=");
        let line = self.cookies().into_iter().find(|line| line.starts_with(&start))?;
        Some(line[start.len()..].split(';').next().unwrap().to_string())
    }
}

/// `req` as the request the server hands the routes.
fn build(req: Req) -> Request<Full<Bytes>> {
    let mut builder = Request::builder().method(req.method).uri(&req.path);
    for (name, value) in &req.headers {
        builder = builder.header(name, value);
    }
    builder.body(Full::new(Bytes::from(req.body))).unwrap()
}

async fn send_from(world: &World, peer: &str, req: Req) -> Answered {
    let request = build(req);
    let shared = Rc::clone(&world.console.shared);
    let Ok(response) = handle(shared, peer.parse().unwrap(), request).await;
    let (head, body) = response.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    Answered { status: head.status.as_u16(), headers: head.headers, body: String::from_utf8(bytes.to_vec()).unwrap() }
}

async fn send(world: &World, req: Req) -> Answered {
    send_from(world, PEER, req).await
}

/// A signed-in session, as its browser holds it.
struct Session {
    cookie: String,
    csrf: String,
}

/// Signs `name` in with the right password, which must be all it takes.
async fn sign_in(world: &World, name: &str) -> Session {
    let body = json!({ "name": name, "password": "right" });
    let answered = send(world, post("/api/1/sign-in", &body)).await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    let cookie = answered.cookie(SESSION_COOKIE).expect("a session cookie");
    let cookie_header = format!("{SESSION_COOKIE}={cookie}");
    let shown = send(world, get("/api/1/session").with("cookie", Some(&cookie_header))).await;
    Session { csrf: shown.json()["csrf"].as_str().unwrap().into(), cookie }
}

/// The calls the worker has had, as `method params`.
fn called(world: &World) -> Vec<(String, Value)> {
    world.calls.lock().unwrap().clone()
}

// The table and the small decisions.

#[test]
fn every_route_is_found_by_its_own_verb_and_no_other_and_each_address_is_one_route() {
    let mut paths = HashSet::new();
    for entry in TABLE {
        assert!(paths.insert(entry.path), "{} twice", entry.path);
        assert!(entry.path.starts_with("/api/1/"), "{}", entry.path);
        assert!(matches!(lookup(entry.verb, entry.path), Lookup::Found(found) if found.route == entry.route));
        let other = if entry.verb == Verb::Get { Verb::Post } else { Verb::Get };
        assert!(matches!(lookup(other, entry.path), Lookup::WrongVerb), "{}", entry.path);
        // What only reads is a GET, and what changes anything is a POST.
        let reads = [Route::Health, Route::Fingerprint, Route::Session, Route::Events, Route::Steps];
        assert_eq!(entry.verb == Verb::Get, reads.contains(&entry.route), "{}", entry.path);
    }
    assert_eq!(TABLE.len(), 26 + usize::from(cfg!(feature = "lab")));
    assert!(matches!(lookup(Verb::Get, "/api/1/nothing"), Lookup::Missing));
    assert!(matches!(lookup(Verb::Get, "/api/1/health/"), Lookup::Missing));
    assert!(matches!(lookup(Verb::Get, "/api"), Lookup::Missing));
    assert!(matches!(lookup(Verb::Get, "/api/2/health"), Lookup::Missing));
    // A path that only starts like the API's is a file's.
    assert!(matches!(lookup(Verb::Get, "/apiary"), Lookup::Found(entry) if entry.route == Route::File));
}

#[test]
fn files_are_for_get_and_head_alone_and_a_post_to_a_page_is_the_wrong_verb() {
    for path in ["/", "/app.js", "/accounts/users", "/x.png"] {
        for verb in [Verb::Get, Verb::Head] {
            assert!(matches!(lookup(verb, path), Lookup::Found(entry) if entry.route == Route::File), "{path}");
        }
        assert!(matches!(lookup(Verb::Post, path), Lookup::WrongVerb), "{path}");
    }
}

#[test]
fn before_sign_in_only_health_fingerprint_and_the_sign_in_routes_are_open() {
    let access = |route: Route| TABLE.iter().find(|entry| entry.route == route).unwrap().access;
    for open in [Route::Health, Route::Fingerprint] {
        assert_eq!(access(open), Access::Anyone);
    }
    for sign in [Route::SignIn, Route::SignInCode, Route::SignInPasskeyOptions, Route::SignInPasskey] {
        assert_eq!(access(sign), Access::SignIn);
    }
    // What a session that may only add a second step can use.
    let limited: Vec<Route> =
        TABLE.iter().filter(|entry| entry.access == Access::Limited).map(|entry| entry.route).collect();
    assert_eq!(
        limited,
        [
            Route::Session, Route::SignOut, Route::Events, Route::Steps, Route::AppBegin,
            Route::AppConfirm, Route::AppRemove, Route::PasskeyBegin, Route::PasskeyAdd,
            Route::PasskeyRemove, Route::PasskeyRename, Route::Codes, Route::TrustedForget,
        ]
    );
    for full in [Route::Unlock, Route::UnlockPasskeyOptions, Route::UnlockExtend, Route::Lock, Route::TokenAdd, Route::TokenRemove] {
        assert_eq!(access(full), Access::Full, "{full:?}");
    }
}

#[test]
fn an_api_token_is_a_bearer_of_dkv_and_nothing_else_is() {
    let with = |value: &str| {
        let mut headers = HeaderMap::new();
        headers.append(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        offers_token(&headers)
    };
    assert!(with("Bearer dkv_chase_00ff"));
    assert!(with("bearer   dkv_chase_00ff"));
    assert!(!with("Bearer abc"));
    assert!(!with("Basic dkv_chase_00ff"));
    assert!(!with("dkv_chase_00ff"));
    assert!(!offers_token(&HeaderMap::new()));
    // A second header cannot hide a token behind a first that is none.
    let mut two = HeaderMap::new();
    two.append(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
    two.append(header::AUTHORIZATION, HeaderValue::from_static("Bearer dkv_a_00"));
    assert!(offers_token(&two));
}

#[test]
fn a_body_is_json_with_or_without_parameters_and_the_limit_grows_after_sign_in() {
    for json in [Some("application/json"), Some("Application/JSON"), Some("application/json; charset=utf-8")] {
        assert!(is_json(json), "{json:?}");
    }
    for other in [None, Some("text/plain"), Some("application/x-www-form-urlencoded"), Some("application/jsonp"), Some("")] {
        assert!(!is_json(other), "{other:?}");
    }
    assert_eq!((body_limit(false), body_limit(true)), (64 << 10, 1 << 20));
}

#[test]
fn what_a_browser_chose_is_shown_in_the_log_short_and_without_control_characters() {
    assert_eq!(shown("chase"), "chase");
    assert_eq!(shown("a\nb\r\x1b[31mc"), "a?b??[31mc");
    assert_eq!(shown(&"x".repeat(100)).len(), 64);
    assert_eq!(shown(""), "");
}

#[test]
fn a_value_for_authd_has_no_nul_and_is_at_most_4096_bytes() {
    let text = |value: Value| serde_json::from_value::<Text>(value);
    assert_eq!(&*text(json!("a password")).unwrap(), "a password");
    assert!(text(json!("a\u{0}b")).is_err());
    assert!(text(json!("a".repeat(4096))).is_ok());
    assert!(text(json!("a".repeat(4097))).is_err());
    assert!(text(json!(5)).is_err());
}

// The order of the checks.

#[test]
fn every_answer_carries_the_security_headers_and_no_server_header() {
    run(async {
        let world = world("headers");
        for request in [get("/api/1/health"), get("/"), get("/api/1/session"), get("/api/1/nothing")] {
            let answered = send(&world, request).await;
            assert!(answered.header("content-security-policy").unwrap().contains("frame-ancestors 'none'"));
            assert_eq!(answered.header("x-content-type-options"), Some("nosniff"));
            assert_eq!(answered.header("referrer-policy"), Some("no-referrer"));
            assert_eq!(answered.header("cross-origin-opener-policy"), Some("same-origin"));
            assert!(answered.header("server").is_none());
            // The console's own certificate is never pinned by HSTS.
            assert!(answered.header("strict-transport-security").is_none());
        }
        let health = send(&world, get("/api/1/health")).await;
        assert_eq!((health.status, health.json()), (200, json!({ "ok": true })));
        assert_eq!(health.header("cache-control"), Some("no-store"));
        assert_eq!(health.header("content-type"), Some("application/json"));
    });
}

#[test]
fn the_fingerprint_is_the_certificates_with_the_machines_name() {
    run(async {
        let world = world("fingerprint");
        let answered = send(&world, get("/api/1/fingerprint")).await;
        let expected = world.console.shared.certificate.borrow().fingerprint.clone();
        assert_eq!(answered.json(), json!({ "host": "nas", "fingerprint": expected }));
    });
}

#[test]
fn a_method_or_address_that_is_not_answered_is_refused_before_anything_else_is_looked_at() {
    run(async {
        let world = world("methods");
        // Not this machine's name, and no method admind takes: the
        // method is first.
        for method in ["PUT", "DELETE", "PATCH", "OPTIONS", "TRACE"] {
            let mut request = get("/api/1/health").with("host", Some("evil.example"));
            request.method = method;
            assert_eq!(send(&world, request).await.error(), "ADM-117", "{method}");
        }
        // An address of more than 2 KiB, with a wrong host, is a bad request.
        let long = format!("/{}", "a".repeat(2100));
        let answered = send(&world, get(&long).with("host", Some("evil.example"))).await;
        assert_eq!((answered.status, answered.error().as_str()), (400, "ADM-108"));
        // The same with its query counted.
        let query = format!("/api/1/health?{}", "q".repeat(2048));
        assert_eq!(send(&world, get(&query)).await.status, 400);
        // 2048 bytes is as long as it may be: a page of the router's.
        let edge = format!("/{}", "a".repeat(2047));
        assert_eq!(send(&world, get(&edge)).await.status, 200);
    });
}

#[test]
fn a_host_that_is_not_this_machines_is_not_answered_whatever_it_asks_for() {
    run(async {
        let world = world("hosts");
        for host in ["evil.example", "evil.example:9090", "nas.example:9091", "nas.example:", ""] {
            for path in ["/api/1/health", "/", "/api/1/nothing"] {
                let answered = send(&world, get(path).with("host", Some(host))).await;
                assert_eq!((answered.status, answered.error().as_str()), (421, "ADM-114"), "{host} {path}");
            }
        }
        let none = send(&world, get("/api/1/health").with("host", None)).await;
        assert_eq!(none.status, 421);
        // The name, the machine's own, `.local`, localhost and an address
        // of its cards are its own; case does not matter.
        for host in ["nas.example:9090", "NAS.Example:9090", "nas:9090", "nas.local:9090", "localhost:9090", "127.0.0.1:9090", "[::1]:9090"] {
            let answered = send(&world, get("/api/1/health").with("host", Some(host))).await;
            assert_eq!(answered.status, 200, "{host}");
        }
    });
}

#[test]
fn an_api_token_is_refused_and_a_token_with_a_session_is_a_bad_request() {
    run(async {
        let world = world("tokens");
        let token = get("/api/1/health").with("authorization", Some("Bearer dkv_chase_00ff"));
        let answered = send(&world, token).await;
        assert_eq!((answered.status, answered.error().as_str()), (401, "ADM-115"));
        let both = get("/api/1/health")
            .with("authorization", Some("Bearer dkv_chase_00ff"))
            .with("cookie", Some("__Host-dukovo=abc"));
        let answered = send(&world, both).await;
        assert_eq!((answered.status, answered.error().as_str()), (400, "ADM-108"));
        // Another kind of authorization is no token.
        let basic = get("/api/1/health").with("authorization", Some("Basic YTpi"));
        assert_eq!(send(&world, basic).await.status, 200);
        // The host is checked before the token is.
        let elsewhere = get("/api/1/health")
            .with("host", Some("evil.example"))
            .with("authorization", Some("Bearer dkv_chase_00ff"));
        assert_eq!(send(&world, elsewhere).await.status, 421);
    });
}

#[test]
fn a_route_that_is_not_there_or_takes_another_method_says_which() {
    run(async {
        let world = world("routes");
        let missing = send(&world, get("/api/1/nothing")).await;
        assert_eq!((missing.status, missing.error().as_str()), (404, "ADM-116"));
        let wrong = send(&world, post("/api/1/health", &json!({}))).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (405, "ADM-117"));
        let wrong = send(&world, get("/api/1/sign-in")).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (405, "ADM-117"));
        let wrong = send(&world, post("/", &json!({}))).await;
        assert_eq!(wrong.status, 405);
    });
}

#[test]
fn the_consoles_files_are_served_whole_and_any_page_of_it_is_the_shell() {
    run(async {
        let world = world("files");
        let shell = send(&world, get("/")).await;
        assert_eq!((shell.status, shell.body.as_str()), (200, fixture::PAGE));
        assert_eq!(shell.header("content-type"), Some("text/html; charset=utf-8"));
        assert_eq!(shell.header("cache-control"), Some("no-cache"));
        let script = send(&world, get("/app.js")).await;
        assert_eq!((script.status, script.body.as_str()), (200, "export {};"));
        assert_eq!(script.header("content-type"), Some("text/javascript; charset=utf-8"));
        // A route of the page router is the shell's; a file that is not
        // there is a 404, never a listing.
        assert_eq!(send(&world, get("/accounts/users")).await.body, fixture::PAGE);
        let none = send(&world, get("/missing.png")).await;
        assert_eq!((none.status, none.error().as_str()), (404, "ADM-116"));
        let mut head = get("/");
        head.method = "HEAD";
        assert_eq!(send(&world, head).await.status, 200);
        // A query does not change which file it is.
        assert_eq!(send(&world, get("/app.js?v=3")).await.body, "export {};");
    });
}

#[test]
fn a_sign_in_that_does_not_come_from_the_consoles_own_page_is_refused_before_authd_hears_of_it() {
    run(async {
        let world = world("foreign");
        let body = json!({ "name": "chase", "password": "right" });
        let requests = [
            post("/api/1/sign-in", &body).with("origin", None),
            post("/api/1/sign-in", &body).with("origin", Some("https://evil.example")),
            post("/api/1/sign-in", &body).with("origin", Some("https://nas.example:9091")),
            post("/api/1/sign-in", &body).with("sec-fetch-site", Some("cross-site")),
            post("/api/1/sign-in", &body).with("sec-fetch-site", Some("same-site")),
        ];
        for request in requests {
            let answered = send(&world, request).await;
            assert_eq!((answered.status, answered.error().as_str()), (403, "ADM-109"));
            assert!(answered.cookies().is_empty());
        }
        // The page's own request, with the header a browser adds.
        let same = post("/api/1/sign-in", &body).with("sec-fetch-site", Some("same-origin"));
        assert_eq!(send(&world, same).await.status, 200);
        assert_eq!(world.console.authd.verbs().first().map(String::as_str), Some("begin"));
        assert_eq!(world.console.authd.verbs().iter().filter(|v| *v == "begin").count(), 1);
    });
}

#[test]
fn a_body_is_json_and_not_too_long_and_what_is_wrong_with_it_is_said_without_repeating_it() {
    run(async {
        let world = world("bodies");
        let body = json!({ "name": "chase", "password": "wrong" });
        let plain = post("/api/1/sign-in", &body).with("content-type", Some("text/plain"));
        let answered = send(&world, plain).await;
        assert_eq!((answered.status, answered.error().as_str()), (415, "ADM-119"));
        let none = post("/api/1/sign-in", &body).with("content-type", None);
        assert_eq!(send(&world, none).await.error(), "ADM-119");
        let charset = post("/api/1/sign-in", &body).with("content-type", Some("application/json; charset=utf-8"));
        assert_eq!(send(&world, charset).await.error(), "ADM-101");
        // Not JSON, and JSON that is not what the route takes: the words
        // never hold what was sent, which may be a password.
        let mut broken = post("/api/1/sign-in", &body);
        broken.body = b"{\"name\": \"chase\", \"password\": \"s3cret".to_vec();
        let answered = send(&world, broken).await;
        assert_eq!((answered.status, answered.error().as_str()), (400, "ADM-108"));
        assert!(!answered.body.contains("s3cret"));
        let wrong_type = post("/api/1/sign-in", &json!({ "name": 7, "password": "s3cret" }));
        let answered = send(&world, wrong_type).await;
        assert_eq!(answered.error(), "ADM-108");
        assert!(!answered.body.contains("s3cret"));
        // A NUL would split a field of authd's request.
        let nul = post("/api/1/sign-in", &json!({ "name": "chase", "password": "a\u{0}b" }));
        assert_eq!(send(&world, nul).await.error(), "ADM-108");
        // At most 64 KiB before sign-in, with the length said or not.
        let mut large = post("/api/1/sign-in", &body);
        large.body = vec![b' '; (64 << 10) + 1];
        let said = large.with("content-length", Some(&((64 << 10) + 1).to_string()));
        let answered = send(&world, said).await;
        assert_eq!((answered.status, answered.error().as_str()), (413, "ADM-113"));
        let mut unsaid = post("/api/1/sign-in", &body);
        unsaid.body = vec![b' '; (64 << 10) + 1];
        assert_eq!(send(&world, unsaid).await.status, 413);
        // No body at all, on a route that takes one.
        let mut empty = post("/api/1/sign-in", &body);
        empty.body = Vec::new();
        assert_eq!(send(&world, empty).await.error(), "ADM-108");
        // Only the one sign-in that was right in form got as far as authd.
        assert_eq!(world.console.authd.verbs(), ["begin"]);
    });
}

#[test]
fn ten_sign_in_requests_a_minute_from_an_address_and_the_eleventh_is_told_when_to_try_again() {
    run(async {
        let world = world("bucket");
        let wrong = json!({ "name": "chase", "password": "wrong" });
        for _ in 0..10 {
            assert_eq!(send(&world, post("/api/1/sign-in", &wrong)).await.error(), "ADM-101");
        }
        let eleventh = send(&world, post("/api/1/sign-in", &wrong)).await;
        assert_eq!((eleventh.status, eleventh.error().as_str()), (429, "ADM-105"));
        let seconds: u64 = eleventh.header("retry-after").unwrap().parse().unwrap();
        assert!((1..=60).contains(&seconds), "{seconds}");
        // Another address has its own ten, and so does a request that is
        // not a sign-in's.
        let other = send_from(&world, "192.0.2.8", post("/api/1/sign-in", &wrong)).await;
        assert_eq!(other.error(), "ADM-101");
        assert_eq!(send(&world, get("/api/1/health")).await.status, 200);
        // Refused with no word to authd.
        assert_eq!(world.console.authd.verbs().len(), 11);
    });
}

// Signing in.

#[test]
fn a_password_that_authd_accepts_signs_the_browser_in_with_a_cookie_that_only_this_site_sends() {
    run(async {
        let world = world("password");
        let body = json!({ "name": "chase", "password": "right" });
        let answered = send(&world, post("/api/1/sign-in", &body)).await;
        assert_eq!((answered.status, answered.json()), (200, json!({ "done": true })));
        let cookie = answered.cookie(SESSION_COOKIE).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(&cookie).unwrap().len(), 32);
        assert_eq!(
            answered.cookies(),
            [format!("{SESSION_COOKIE}={cookie}; Path=/; Secure; HttpOnly; SameSite=Strict")]
        );
        // The worker was started, and the session exists.
        assert_eq!(world.console.authd.verbs(), ["begin", "start"]);
        assert_eq!(world.console.shared.sessions.borrow().len(), 1);
        // A password nobody has: the same words for a name that is not
        // there and a password that is wrong.
        let wrong = send(&world, post("/api/1/sign-in", &json!({ "name": "nobody", "password": "x" }))).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-101"));
        assert!(wrong.cookies().is_empty());
        let empty = send(&world, post("/api/1/sign-in", &json!({ "name": "chase", "password": "" }))).await;
        assert_eq!(empty.error(), "ADM-101");
        // An account that may not use the console is told so, as authd
        // says it only to someone who knows the password.
        let bob = send(&world, post("/api/1/sign-in", &json!({ "name": "bob", "password": "right" }))).await;
        assert_eq!((bob.status, bob.error().as_str()), (403, "ADM-104"));
        assert!(bob.body.contains("bob may not use the web console"));
    });
}

#[test]
fn the_session_says_who_it_is_from_the_account_files_and_how_it_can_unlock() {
    run(async {
        let world = world("session");
        let session = sign_in(&world, "chase").await;
        let shown = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!(
            shown.json(),
            json!({
                "name": "chase", "full_name": "Chase Y", "admin": true, "host": "nas",
                "csrf": session.csrf, "unlock_seconds": null, "needs_step": false,
                "kinds": ["password", "app"],
            })
        );
        assert_eq!(URL_SAFE_NO_PAD.decode(&session.csrf).unwrap().len(), 32);
        // No cookie, a cookie that is no session's, and one sent twice.
        for cookie in [None, Some("__Host-dukovo=nothing"), Some("__Host-dukovo=a; __Host-dukovo=b")] {
            let answered = send(&world, get("/api/1/session").with("cookie", cookie)).await;
            assert_eq!((answered.status, answered.error().as_str()), (401, "ADM-107"));
            // The browser forgets the cookie it was told is no good.
            assert_eq!(answered.cookies(), [guard::clear_cookie(SESSION_COOKIE)]);
        }
    });
}

#[test]
fn a_second_step_is_asked_for_by_a_ticket_that_only_the_address_it_was_given_to_may_use() {
    run(async {
        let world = world("step");
        let body = json!({ "name": "alice", "password": "right" });
        let answered = send(&world, post("/api/1/sign-in", &body)).await;
        let step = &answered.json()["step"];
        assert_eq!(step["kinds"], json!(["app", "recovery"]));
        let ticket = step["ticket"].as_str().unwrap().to_string();
        assert!(answered.cookies().is_empty());
        let code = |code: &str| post("/api/1/sign-in/code", &json!({ "ticket": ticket, "code": code }));
        // Another address has no such sign-in.
        let other = send_from(&world, "192.0.2.8", code("123456")).await;
        assert_eq!((other.status, other.error().as_str()), (401, "ADM-107"));
        // A wrong code is refused and leaves the sign-in where it was.
        let wrong = send(&world, code("000000")).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-102"));
        let right = send(&world, code("123456")).await;
        assert_eq!((right.status, right.json()), (200, json!({ "done": true })));
        assert!(right.cookie(SESSION_COOKIE).is_some());
        assert!(right.cookie(TRUST_COOKIE).is_none());
        // The ticket is spent.
        assert_eq!(send(&world, code("123456")).await.error(), "ADM-107");
        // Not hexadecimal, or not a ticket at all.
        let bad = post("/api/1/sign-in/code", &json!({ "ticket": "zz", "code": "123456" }));
        assert_eq!(send(&world, bad).await.error(), "ADM-107");
    });
}

#[test]
fn a_browser_that_is_trusted_is_trusted_for_30_days_and_skips_the_step_until_it_is_not() {
    run(async {
        let world = world("trusted");
        let body = json!({ "name": "alice", "password": "right" });
        let first = send(&world, post("/api/1/sign-in", &body)).await;
        let ticket = first.json()["step"]["ticket"].as_str().unwrap().to_string();
        let asked = json!({ "ticket": ticket, "code": "123456", "trust": true });
        let trusted = send(&world, post("/api/1/sign-in/code", &asked)).await;
        assert_eq!(trusted.cookie(TRUST_COOKIE).as_deref(), Some("aa55bb66"));
        assert!(trusted.cookies().contains(&format!(
            "{TRUST_COOKIE}=aa55bb66; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=2592000"
        )));
        // With its cookie, the next sign-in needs no step.
        let known = post("/api/1/sign-in", &body).with("cookie", Some("__Host-dukovo-trust=aa55bb66"));
        let answered = send(&world, known).await;
        assert_eq!(answered.json(), json!({ "done": true }));
        // With a cookie authd does not know, the step is asked for and the
        // cookie is cleared.
        let stale = post("/api/1/sign-in", &body).with("cookie", Some("__Host-dukovo-trust=00ff"));
        let answered = send(&world, stale).await;
        assert!(answered.json()["step"]["ticket"].is_string());
        assert_eq!(answered.cookies(), [guard::clear_cookie(TRUST_COOKIE)]);
        // A trust cookie that is not hexadecimal is no cookie.
        let odd = post("/api/1/sign-in", &body).with("cookie", Some("__Host-dukovo-trust=zz"));
        assert!(send(&world, odd).await.json()["step"].is_object());
    });
}

#[test]
fn a_session_that_may_only_add_a_second_step_may_do_that_and_sign_out_and_nothing_more() {
    run(async {
        let world = world("needs-step");
        let body = json!({ "name": "eve", "password": "right" });
        let answered = send(&world, post("/api/1/sign-in", &body)).await;
        assert_eq!(answered.json(), json!({ "needs_step": true }));
        let cookie = answered.cookie(SESSION_COOKIE).unwrap();
        let header = format!("{SESSION_COOKIE}={cookie}");
        let shown = send(&world, get("/api/1/session").with("cookie", Some(&header))).await;
        let csrf = shown.json()["csrf"].as_str().unwrap().to_string();
        assert_eq!(shown.json()["needs_step"], json!(true));
        let session = Session { cookie, csrf };
        // What it may use.
        assert_eq!(send(&world, get("/api/1/account/second-step").by(&session)).await.status, 200);
        let begun = send(&world, post("/api/1/account/second-step/app/begin", &json!({ "proof": "right" })).by(&session)).await;
        assert_eq!(begun.status, 200, "{}", begun.body);
        // What it may not, which authd would refuse anyway.
        for (path, body) in [
            ("/api/1/unlock", json!({ "kind": "password", "password": "right" })),
            ("/api/1/unlock/extend", json!({})),
            ("/api/1/lock", json!({})),
            ("/api/1/account/tokens/add", json!({ "proof": "", "name": "a", "scopes": ["*"], "days": 1 })),
        ] {
            let refused = send(&world, post(path, &body).by(&session)).await;
            assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-118"), "{path}");
        }
        assert!(!world.console.authd.verbs().contains(&"unlock".to_string()));
        let out = send(&world, post("/api/1/sign-out", &json!({})).by(&session)).await;
        assert_eq!(out.json(), json!({ "done": true }));
    });
}

#[test]
fn a_request_that_changes_something_carries_the_origin_and_the_sessions_token() {
    run(async {
        let world = world("csrf");
        let session = sign_in(&world, "chase").await;
        let cookie = format!("{SESSION_COOKIE}={}", session.cookie);
        let bare = || post("/api/1/lock", &json!({})).with("cookie", Some(&cookie));
        let other = URL_SAFE_NO_PAD.encode([7u8; 32]);
        let requests = [
            // No token, a wrong one, one that is not base64url.
            bare(),
            bare().with("x-dukovo-csrf", Some(&other)),
            bare().with("x-dukovo-csrf", Some("not base64!")),
            // A token that is right, but another site's page sent it.
            bare().with("x-dukovo-csrf", Some(&session.csrf)).with("origin", None),
            bare().with("x-dukovo-csrf", Some(&session.csrf)).with("origin", Some("https://evil.example")),
            bare().with("x-dukovo-csrf", Some(&session.csrf)).with("sec-fetch-site", Some("cross-site")),
        ];
        for request in requests {
            let answered = send(&world, request).await;
            assert_eq!((answered.status, answered.error().as_str()), (403, "ADM-109"));
        }
        assert!(!world.console.authd.verbs().contains(&"lock".to_string()));
        let fine = send(&world, post("/api/1/lock", &json!({})).by(&session)).await;
        assert_eq!(fine.json(), json!({ "unlock_seconds": null }));
    });
}

#[test]
fn signing_out_ends_the_session_for_authd_and_the_browser_and_is_logged_once() {
    run(async {
        let world = world("sign-out");
        let session = sign_in(&world, "chase").await;
        let out = send(&world, post("/api/1/sign-out", &json!({})).by(&session)).await;
        assert_eq!(out.json(), json!({ "done": true }));
        assert_eq!(out.cookies(), [guard::clear_cookie(SESSION_COOKIE)]);
        let again = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!((again.status, again.error().as_str()), (401, "ADM-107"));
        // authd is told, on a task of its own.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(world.console.authd.verbs(), ["begin", "start", "end"]);
        assert!(world.console.shared.sessions.borrow().is_empty());
    });
}

// Unlocking.

#[test]
fn a_session_unlocks_with_a_password_or_a_code_for_what_authd_says_and_locks_again() {
    run(async {
        let world = world("unlock");
        let session = sign_in(&world, "chase").await;
        let unlock = |body: Value| post("/api/1/unlock", &body).by(&session);
        let wrong = send(&world, unlock(json!({ "kind": "password", "password": "nope" }))).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-101"));
        let wrong = send(&world, unlock(json!({ "kind": "code", "code": "000000" }))).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-102"));
        let shown = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!(shown.json()["unlock_seconds"], Value::Null);
        for body in [json!({ "kind": "password", "password": "right" }), json!({ "kind": "code", "code": "123456" })] {
            let done = send(&world, unlock(body)).await;
            let seconds = done.json()["unlock_seconds"].as_i64().unwrap();
            assert!((898..=900).contains(&seconds), "{seconds}");
        }
        let shown = send(&world, get("/api/1/session").by(&session)).await;
        assert!(shown.json()["unlock_seconds"].as_i64().unwrap() > 890);
        let extended = send(&world, post("/api/1/unlock/extend", &json!({})).by(&session)).await;
        assert!(extended.json()["unlock_seconds"].as_i64().unwrap() > 890);
        let locked = send(&world, post("/api/1/lock", &json!({})).by(&session)).await;
        assert_eq!(locked.json(), json!({ "unlock_seconds": null }));
        let shown = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!(shown.json()["unlock_seconds"], Value::Null);
    });
}

#[test]
fn an_unlock_that_does_not_say_what_it_needs_is_a_bad_request_and_authd_is_not_asked() {
    run(async {
        let world = world("unlock-bad");
        let session = sign_in(&world, "chase").await;
        let asked = world.console.authd.verbs().len();
        for body in [
            json!({}),
            json!({ "kind": "magic" }),
            json!({ "kind": "password" }),
            json!({ "kind": "code" }),
            json!({ "kind": "passkey" }),
            json!({ "kind": "password", "password": "a\u{0}b" }),
        ] {
            let answered = send(&world, post("/api/1/unlock", &body).by(&session)).await;
            assert_eq!((answered.status, answered.error().as_str()), (400, "ADM-108"), "{body}");
        }
        assert_eq!(world.console.authd.verbs().len(), asked);
    });
}

#[test]
fn authd_answering_a_time_that_does_not_read_is_not_an_unlock() {
    run(async {
        let shared_calls = Calls::default();
        let silly: Arc<fixture::Script> = Arc::new(move |asked: &Asked, mut fds: Vec<OwnedFd>| match asked.verb.as_str() {
            "begin" => vec!["ok done 0a0b01".into()],
            "start" => {
                fixture::worker(fds.remove(0), Arc::clone(&shared_calls), Arc::new(worker_answers));
                vec![format!("ok 1 {:0<32}", asked.text(0))]
            }
            "unlock" => vec!["ok soon".into()],
            _ => vec!["ok".into()],
        });
        let world = World { console: Console::new("silly", silly), calls: Calls::default() };
        let session = sign_in(&world, "chase").await;
        let body = json!({ "kind": "password", "password": "right" });
        let answered = send(&world, post("/api/1/unlock", &body).by(&session)).await;
        assert_eq!((answered.status, answered.error().as_str()), (502, "ADM-111"));
        let shown = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!(shown.json()["unlock_seconds"], Value::Null);
    });
}

// The account's second step, through the worker.

#[test]
fn each_second_step_route_is_one_call_to_the_worker_with_what_admind_adds_to_it() {
    run(async {
        let world = world("account");
        let session = sign_in(&world, "chase").await;
        let path = |rest: &str| format!("/api/1/account/{rest}");
        let call = |rest: &str, body: Value| post(&path(rest), &body).by(&session);
        let steps = send(&world, get(&path("second-step")).by(&session)).await;
        assert_eq!(steps.json()["app"], json!({ "added": 1 }));
        let begun = send(&world, call("second-step/app/begin", json!({ "proof": "pw" }))).await;
        assert_eq!(begun.json(), json!({ "uri": "otpauth://totp/nas:chase?secret=ABCD" }));
        let confirmed = send(&world, call("second-step/app/confirm", json!({ "code": "123456" }))).await;
        assert_eq!(confirmed.json(), json!({ "codes": ["AAAA-BBBB"] }));
        let removed = send(&world, call("second-step/app/remove", json!({ "proof": "" }))).await;
        assert_eq!(removed.json(), json!({ "done": true }));
        let renamed = send(&world, call("second-step/passkey/rename", json!({ "id": "0aff", "name": "Desk" }))).await;
        assert_eq!(renamed.json(), json!({ "done": true }));
        let gone = send(&world, call("second-step/passkey/remove", json!({ "proof": "pw", "id": "0aff" }))).await;
        assert_eq!(gone.json(), json!({ "done": true }));
        let codes = send(&world, call("second-step/codes", json!({ "proof": "pw" }))).await;
        assert_eq!(codes.json(), json!({ "codes": ["AAAA-BBBB"] }));
        let forgotten = send(&world, post(&path("second-step/trusted/forget"), &json!({})).by(&session)).await;
        assert_eq!(forgotten.json(), json!({ "forgotten": 2 }));
        let token = json!({ "proof": "pw", "name": "ansible", "scopes": ["services:read"], "days": 30 });
        let added = send(&world, call("tokens/add", token)).await;
        assert_eq!(added.json(), json!({ "token": "dkv_chase_aa" }));
        let removed = send(&world, call("tokens/remove", json!({ "name": "ansible" }))).await;
        assert_eq!(removed.json(), json!({ "done": true }));
        // What the worker was told: the host is the machine's own, from
        // admind, and nothing the browser sent is the worker's to decide.
        let seen = called(&world);
        let methods: Vec<&str> = seen.iter().map(|(method, _)| method.as_str()).collect();
        assert_eq!(
            methods,
            ["steps", "steps", "app-begin", "app-confirm", "app-remove", "passkey-rename", "passkey-remove", "codes", "trusted-forget", "token-add", "token-remove"]
        );
        assert_eq!(seen[2].1, json!({ "proof": "pw", "host": "nas" }));
        assert_eq!(seen[3].1, json!({ "code": "123456" }));
        assert_eq!(seen[5].1, json!({ "id": "0aff", "name": "Desk" }));
        assert_eq!(seen[9].1, json!({ "proof": "pw", "name": "ansible", "scopes": ["services:read"], "days": 30 }));
    });
}

#[test]
fn a_passkey_is_made_for_the_name_the_browser_used_and_added_with_the_challenge_it_signed() {
    run(async {
        let world = world("passkey-add");
        let session = sign_in(&world, "chase").await;
        let path = "/api/1/account/second-step/passkey";
        let begun = send(&world, post(&format!("{path}/begin"), &json!({ "proof": "pw" })).by(&session)).await;
        let options = &begun.json()["publicKey"];
        assert_eq!(options["challenge"], "AQI");
        assert_eq!(options["rp"], json!({ "id": "nas.example", "name": "Dukovo (nas)" }));
        assert_eq!(options["excludeCredentials"], json!([{ "type": "public-key", "id": "BQY" }]));
        let encode = |text: &str| URL_SAFE_NO_PAD.encode(text);
        let credential = json!({
            "id": "AA",
            "response": {
                "clientDataJSON": encode(r#"{"type":"webauthn.create","challenge":"AQI","origin":"x"}"#),
                "attestationObject": encode("attestation"),
            },
        });
        let added = send(&world, post(&format!("{path}/add"), &json!({ "name": "Desk key", "credential": credential })).by(&session)).await;
        assert_eq!(added.json(), json!({ "codes": [] }));
        let seen = called(&world);
        assert_eq!(seen[1].0, "passkey-begin");
        assert_eq!(seen[1].1, json!({ "proof": "pw", "relying_party": "nas.example" }));
        assert_eq!(seen[2].0, "passkey-add");
        let params = &seen[2].1;
        assert_eq!(params["challenge"], "AQI");
        assert_eq!((params["origin"].as_str(), params["relying_party"].as_str()), (Some(ORIGIN), Some("nas.example")));
        assert_eq!(params["name"], "Desk key");
        assert_eq!(params["attestation"], encode("attestation"));
        // A credential that is not what a browser makes.
        let odd = json!({ "name": "x", "credential": { "response": { "clientDataJSON": "AA", "attestationObject": "AA" } } });
        let refused = send(&world, post(&format!("{path}/add"), &odd).by(&session)).await;
        assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-108"));
    });
}

#[test]
fn what_authd_refuses_through_the_worker_is_told_by_its_words_and_logged() {
    run(async {
        let world = world("account-refused");
        let session = sign_in(&world, "chase").await;
        let wrong = post("/api/1/account/second-step/app/confirm", &json!({ "code": "000000" })).by(&session);
        let answered = send(&world, wrong).await;
        assert_eq!((answered.status, answered.error().as_str()), (401, "ADM-102"));
        assert_eq!(answered.json()["error"]["detail"], "that code is not right");
        // A parameter the route does not take is the browser's mistake.
        let missing = post("/api/1/account/second-step/app/confirm", &json!({})).by(&session);
        assert_eq!(send(&world, missing).await.error(), "ADM-108");
        // The worker heard of the code and of nothing else: its other call
        // was the session's, which asks what the account has.
        let methods: Vec<String> = called(&world).into_iter().map(|(method, _)| method).collect();
        assert_eq!(methods, ["steps", "app-confirm"]);
    });
}

// The stream of events.

#[test]
fn a_tabs_stream_tells_the_unlock_when_it_changes_and_why_the_session_ended() {
    run(async {
        let world = world("events");
        let session = sign_in(&world, "chase").await;
        let request = build(get("/api/1/events").by(&session));
        let Ok(response) = handle(Rc::clone(&world.console.shared), PEER.parse().unwrap(), request).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let mut body = response.into_body();
        let mut next = async || {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap();
            frame.map(|frame| String::from_utf8(frame.unwrap().into_data().unwrap().to_vec()).unwrap())
        };
        assert_eq!(next().await.unwrap(), "event: unlock\ndata: {\"unlock_seconds\":null}\n\n");
        assert_eq!(world.console.shared.with_session(&session.cookie, |s| s.tabs), Some(1));
        let unlock = json!({ "kind": "password", "password": "right" });
        send(&world, post("/api/1/unlock", &unlock).by(&session)).await;
        let told = next().await.unwrap();
        assert!(told.starts_with("event: unlock\ndata: {\"unlock_seconds\":89"), "{told}");
        send(&world, post("/api/1/lock", &json!({})).by(&session)).await;
        assert_eq!(next().await.unwrap(), "event: unlock\ndata: {\"unlock_seconds\":null}\n\n");
        send(&world, post("/api/1/sign-out", &json!({})).by(&session)).await;
        assert_eq!(next().await.unwrap(), "event: session\ndata: {\"ended\":\"signed-out\"}\n\n");
        assert_eq!(next().await, None);
    });
}

#[test]
fn a_tab_that_goes_is_counted_out_and_the_last_one_to_go_has_an_unlocked_session_lock_itself() {
    run(async {
        let world = world("tabs");
        let session = sign_in(&world, "chase").await;
        let unlock = json!({ "kind": "password", "password": "right" });
        send(&world, post("/api/1/unlock", &unlock).by(&session)).await;
        let shared = &world.console.shared;
        let tabs = || shared.with_session(&session.cookie, |s| s.tabs);
        // Two tabs open, as the events route counts them.
        shared.with_session(&session.cookie, |s| s.tabs = 2);
        let authd = shared.with_session(&session.cookie, |s| s.authd.clone()).unwrap();
        let after = Duration::from_millis(40);
        events::left(shared, &session.cookie, &authd, after).await;
        assert_eq!(tabs(), Some(1));
        assert!(!world.console.authd.verbs().contains(&"lock".to_string()));
        // The last one goes: a minute (here, 40 ms) later it is locked.
        events::left(shared, &session.cookie, &authd, after).await;
        assert_eq!(tabs(), Some(0));
        assert_eq!(world.console.authd.verbs().last().map(String::as_str), Some("lock"));
        assert_eq!(shared.with_session(&session.cookie, |s| s.unlocked_until), Some(None));
        // A session that was never unlocked has nothing to lock.
        let before = world.console.authd.verbs().len();
        events::left(shared, &session.cookie, &authd, after).await;
        assert_eq!(world.console.authd.verbs().len(), before);
        // A tab that opens in the meantime keeps it unlocked.
        send(&world, post("/api/1/unlock", &unlock).by(&session)).await;
        let (opening, cookie) = (Rc::clone(shared), session.cookie.clone());
        tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis(15)).await;
            opening.with_session(&cookie, |s| s.tabs += 1);
        });
        events::left(shared, &session.cookie, &authd, after).await;
        assert_eq!(tabs(), Some(1));
        assert_ne!(shared.with_session(&session.cookie, |s| s.unlocked_until), Some(None));
    });
}

#[test]
fn a_session_whose_time_is_up_ends_and_authd_is_told() {
    run(async {
        let world = world("maintain");
        let session = sign_in(&world, "chase").await;
        // 13 hours on, with the sign-in rules' 12.
        let later = std::time::Instant::now() + Duration::from_secs(13 * 3600);
        world.console.shared.maintain(later);
        let again = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!(again.error(), "ADM-107");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(world.console.authd.verbs(), ["begin", "start", "end"]);
        let authd = format!("{:0<32}", "0a0b01");
        assert_eq!(world.console.shared.endings.borrow().reason(&authd), "expired");
    });
}

#[test]
fn a_session_whose_worker_goes_ends_with_it() {
    run(async {
        let world = world("worker-gone");
        let (ours, theirs) = tokio::net::UnixStream::pair().unwrap();
        let link = Link::new(ours);
        let authd = "ab".repeat(16);
        let session = sessions::Session::new("chase".into(), authd.clone(), PEER.parse().unwrap(), false, link.clone());
        world.console.shared.admit(session.unwrap()).unwrap();
        world.console.shared.watch(&link, authd.clone());
        assert_eq!(world.console.shared.sessions.borrow().len(), 1);
        drop(theirs);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(world.console.shared.sessions.borrow().is_empty());
        assert_eq!(world.console.shared.endings.borrow().reason(&authd), "worker");
        assert_eq!(world.console.authd.verbs(), ["end"]);
    });
}
