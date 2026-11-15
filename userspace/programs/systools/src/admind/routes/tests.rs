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

mod act;
mod changes;
mod fuzz;
mod tasks;
mod terminals;

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

/// The preferences the test worker has after `change`: the defaults, and
/// what the change gave.
fn preferences(change: &Value) -> Value {
    let mut all = defaults();
    for (key, value) in change.as_object().into_iter().flatten() {
        match value.as_object() {
            // The terminal's keys are changed one by one.
            Some(keys) if key == "terminal" => {
                for (name, value) in keys {
                    all[key][name] = value.clone();
                }
            }
            _ => all[key] = value.clone(),
        }
    }
    all
}

/// The preferences of a person who has changed none.
fn defaults() -> Value {
    json!({
        "theme": "dark", "text": 100, "motion": "full", "zone": "", "commands": "hidden",
        "terminal": {
            "size": 14, "scrollback": 10000, "copy-on-select": false, "keep-days": 7,
            "announce": false, "shortcuts": true,
        },
    })
}

/// What the test worker answers. `calls` holds what it has been asked,
/// this call the last of them.
fn worker_answers(method: &str, params: &Value, calls: &Calls) -> Result<Value, Fault> {
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
        "preferences" => Ok(preferences(&json!({}))),
        // A theme that the worker cannot keep, as a file it cannot write.
        "preferences-change" if params["theme"] == "device" => Err(Fault::new(
            Kind::Internal,
            "cannot write /home/chase/.config/dukovo-admin/settings.toml: permission denied",
        )),
        "preferences-change" => Ok(preferences(params)),
        "run" if params["argv"] == json!(["service", "list", "--json"]) => {
            Ok(json!({ "status": 0, "output": "[{\"name\":\"nginx\"}]\n", "errors": "" }))
        }
        "run" if params["argv"][0] == "probe" => {
            Ok(probe_answer(params["argv"][1].as_str().unwrap_or_default(), calls))
        }
        "run" => {
            let program = params["argv"][0].as_str().unwrap_or_default();
            Err(Fault::new(Kind::NoProgram, format!("{program}: command not found")))
        }
        _ => Ok(json!({ "done": true })),
    }
}

/// What the test worker prints for `probe NAME`, which a change's probe
/// runs: a part that the change moves, a semicolon, and a part that moves
/// by itself, the number of times the probe has been asked for. By its
/// name, `moving` says `off` the first time and `on` after, `unreadable`
/// says something that is not what the change moves, `fails` ends with
/// status 1, `fails-after` does from its second time, and any other name
/// says `on` every time.
fn probe_answer(name: &str, calls: &Calls) -> Value {
    let asked = json!({ "argv": ["probe", name] });
    let nth = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, params)| method == "run" && *params == asked)
        .count();
    let (status, output) = match (name, nth) {
        ("moving", 1) => (0, format!("off; {nth}")),
        ("unreadable", _) => (0, "?".to_string()),
        ("fails", _) | ("fails-after", 2..) => (1, String::new()),
        _ => (0, format!("on; {nth}")),
    };
    let errors = if status == 0 { "" } else { "probe: cannot read\n" };
    json!({ "status": status, "output": format!("{output}\n"), "errors": errors })
}

/// What the test authd answers: four accounts that sign in, as
/// `chase` (an administrator), `alice` (who has a second step), `eve`
/// (who needs to add one) and `dave` (who is not an administrator), with
/// the password `right`, and chase with `elsewhere` too, which starts
/// another authd session, as a second browser's sign-in does; the code
/// `123456`;
/// the trusted browser `aa55bb66`.
fn script(calls: Calls) -> Arc<fixture::Script> {
    let answers: Arc<fixture::Answer> = {
        let calls = Arc::clone(&calls);
        Arc::new(move |method: &str, params: &Value| worker_answers(method, params, &calls))
    };
    Arc::new(move |asked: &Asked, mut fds: Vec<OwnedFd>| -> Vec<String> {
        let no = |why: &str| vec![format!("no {why}")];
        let ok = || vec![String::from("ok")];
        let in_15_minutes = || vec![format!("ok {}", unix_now() + 900)];
        match asked.verb.as_str() {
            "begin" => match (asked.text(1).as_str(), asked.text(2).as_str()) {
                ("chase", "right") => vec!["ok done 0a0b01".into()],
                ("chase", "elsewhere") => vec!["ok done 0a0b11".into()],
                ("alice", "right") => vec!["ok step 0a0b02 app,recovery".into()],
                ("eve", "right") => vec!["ok needs-step 0a0b03".into()],
                ("dave", "right") => vec!["ok done 0a0b04".into()],
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
                fixture::worker(fixture::ends(&mut fds), Arc::clone(&calls), Arc::clone(&answers));
                vec![format!("ok 4242 {session}{trust}")]
            }
            "unlock" => match (asked.text(1).as_str(), asked.text(2).as_str()) {
                ("password", "right") | ("code", "123456") => in_15_minutes(),
                ("code", _) => no("that code is not right"),
                _ => no(WRONG_PASSWORD),
            },
            "extend" => in_15_minutes(),
            "lock" | "end" => ok(),
            "token" => match token_answer(&asked.text(0)) {
                Some(answer) => vec![format!("ok {answer}")],
                None => no("that API token is not known"),
            },
            "token-start" => {
                fixture::worker(fixture::ends(&mut fds), Arc::clone(&calls), Arc::clone(&answers));
                ok()
            }
            "elevate" => elevated(asked, fds, &calls),
            "terminal" => shell(asked, fds, &calls),
            _ => no("the test's authd does not answer that"),
        }
    })
}

/// The API tokens the test authd knows: chase's `ci`, which may do
/// anything, `reader`, which may read services, and `other`, which may read
/// the firewall.
const TOKEN_ALL: &str = "dkv_chase_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN_READ: &str = "dkv_chase_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TOKEN_OTHER: &str = "dkv_chase_cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// What authd answers for the token whose hex is `hex`.
fn token_answer(hex: &str) -> Option<&'static str> {
    let is = |token: &str| accounts::request::hex(token.as_bytes()) == hex;
    if is(TOKEN_ALL) {
        Some("chase ci * 1800000000")
    } else if is(TOKEN_READ) {
        Some("chase reader services:read 1800000000")
    } else if is(TOKEN_OTHER) {
        Some("chase other firewall:read 1800000000")
    } else {
        None
    }
}

/// The test authd's elevated command, whose four descriptors are its
/// input, its output, its error output and its relay. `.../locked` is
/// refused as a locked session's; a token's command for `guarded` is
/// refused as authd refuses one its scopes do not name; `broken` fails
/// with a service's error ID; `/system/commands/echo` writes its words
/// and ends with the status 3; anything else writes its words and ends
/// well. The command is noted in `calls` as a call of `elevate`, so that a
/// test sees it among the worker's.
fn elevated(asked: &Asked, mut fds: Vec<OwnedFd>, calls: &Calls) -> Vec<String> {
    use std::io::Write;
    let fields: Vec<String> = (0..asked.fields.len()).map(|at| asked.text(at)).collect();
    let command = &fields[fields.iter().position(|field| field == "--").unwrap() + 1..];
    calls.lock().unwrap().push(("elevate".into(), json!(command)));
    if command[0] == "/system/commands/locked" {
        return vec!["no the session is locked: unlock it to make changes".into()];
    }
    if fields[0] == "token" && command.get(1).is_some_and(|word| word == "guarded") {
        return vec![format!("no the API token may not change {} as an administrator", fields[2])];
    }
    let mut output = std::fs::File::from(fds.remove(1));
    let mut errors = std::fs::File::from(fds.remove(1));
    if command.get(1).is_some_and(|word| word == "broken") {
        errors.write_all(b"starting broken\nbroken.conf did not load (SVC-104)\n").unwrap();
        return vec!["ok 4321".into(), "ended 256".into()];
    }
    if command[0] == "admin" && command.get(1).is_some_and(|word| word == "lab-steps") {
        let mode = command.get(2).and_then(|word| word.parse().ok()).unwrap_or(0);
        return lab_steps(mode, output, errors, fds);
    }
    output.write_all(format!("{}\n", command[1..].join(" ")).as_bytes()).unwrap();
    let status = if command[0] == "/system/commands/echo" { 768 } else { 0 };
    vec!["ok 4321".into(), format!("ended {status}")]
}

/// The test authd's shell, on the terminal that came with the request,
/// noted in `calls` as a call of `terminal` with its fields. It writes
/// `ready$ `, answers each line typed with `you typed LINE`, and at `exit`
/// closes its side of the terminal, as a shell that ends does. dave's
/// session is refused with an ID, as authd refuses one its settings do not
/// let open a terminal.
fn shell(asked: &Asked, mut fds: Vec<OwnedFd>, calls: &Calls) -> Vec<String> {
    use std::io::{BufRead, Write};
    let fields: Vec<String> = (0..asked.fields.len()).map(|at| asked.text(at)).collect();
    calls.lock().unwrap().push(("terminal".into(), json!(fields)));
    if fields[0].starts_with("0a0b04") {
        return vec!["no ADM-132: the console's settings do not let dave open a terminal".into()];
    }
    let terminal = std::fs::File::from(fds.remove(0));
    std::thread::spawn(move || {
        let mut output = terminal.try_clone().unwrap();
        let _ = output.write_all(b"ready$ ");
        for line in std::io::BufReader::new(terminal).lines() {
            match line {
                Ok(line) if line != "exit" => {
                    // The terminal writes a line end as CR LF, as it does a
                    // shell's.
                    let _ = write!(output, "you typed {line}\nready$ ");
                }
                _ => break,
            }
        }
    });
    vec!["ok 4242".into()]
}

/// What `admin lab-steps N` does in the test authd, by `mode` N, which
/// is not the seconds it means: 1 runs well and ends at once, 2 writes an
/// error and fails, 3 runs until SIGTERM and ends well, 4 ignores SIGTERM
/// and ends when its relay is closed, as hung up, and 5 writes 2,500
/// lines and ends well. `fds` are its input and its relay.
fn lab_steps(
    mode: u32,
    mut output: std::fs::File,
    mut errors: std::fs::File,
    mut fds: Vec<OwnedFd>,
) -> Vec<String> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    let mut relay = std::fs::File::from(fds.remove(1));
    let ended = |status: u32| vec!["ok 4321".to_string(), format!("ended {status}")];
    match mode {
        1 => {
            output
                .write_all(b"step 1 of 2: One\nprogress 50%\na log line\nstep 2 of 2: Two\nprogress 100%\n")
                .unwrap();
            ended(0)
        }
        2 => {
            output.write_all(b"step 1 of 3: Begin\n").unwrap();
            errors.write_all(b"lab-steps: it broke\n").unwrap();
            ended(256)
        }
        5 => {
            for line in 0..2500 {
                writeln!(output, "line {line}").unwrap();
            }
            ended(0)
        }
        _ => {
            output.write_all(b"step 1 of 4: Getting ready\nprogress 10%\nworking\n").unwrap();
            fixture::then(move || {
                let (mut got, mut buffer) = (String::new(), [0u8; 64]);
                for _ in 0..1500 {
                    let mut poll =
                        libc::pollfd { fd: relay.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                    // SAFETY: one pollfd, which lives through the call.
                    let ready = unsafe { libc::poll(&raw mut poll, 1, 20) };
                    if ready <= 0 {
                        continue;
                    }
                    match relay.read(&mut buffer).unwrap_or(0) {
                        // The relay was closed: authd hangs the command up.
                        0 => return vec!["ended 1".to_string()],
                        n => got.push_str(&String::from_utf8_lossy(&buffer[..n])),
                    }
                    if mode == 3 && got.contains("signal 15\n") {
                        output.write_all(b"stopping\n").unwrap();
                        return vec!["ended 0".to_string()];
                    }
                }
                vec!["ended 2".to_string()]
            });
            vec!["ok 4321".to_string()]
        }
    }
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
    Req {
        method: "GET",
        path: path.into(),
        headers: vec![("host".into(), HOST.into())],
        body: Vec::new(),
    }
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
        self.json()["error"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("an error: {}", self.body))
            .into()
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
    Answered {
        status: head.status.as_u16(),
        headers: head.headers,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
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
    sign_in_with(world, name, "right").await
}

/// Signs `name` in with `password`, which signs in at once.
async fn sign_in_with(world: &World, name: &str, password: &str) -> Session {
    let body = json!({ "name": name, "password": password });
    let answered = send(world, post("/api/1/sign-in", &body)).await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    let cookie = answered.cookie(SESSION_COOKIE).expect("a session cookie");
    let cookie_header = format!("{SESSION_COOKIE}={cookie}");
    let shown = send(world, get("/api/1/session").with("cookie", Some(&cookie_header))).await;
    Session { csrf: shown.json()["csrf"].as_str().unwrap().into(), cookie }
}

/// Signs `name` in with the right password, when what is left to do is to
/// add a second step.
async fn sign_in_to_add_a_step(world: &World, name: &str) -> Session {
    let body = json!({ "name": name, "password": "right" });
    let answered = send(world, post("/api/1/sign-in", &body)).await;
    assert_eq!(answered.json(), json!({ "needs_step": true }));
    let cookie = answered.cookie(SESSION_COOKIE).expect("a session cookie");
    let header = format!("{SESSION_COOKIE}={cookie}");
    let shown = send(world, get("/api/1/session").with("cookie", Some(&header))).await;
    assert_eq!(shown.json()["needs_step"], json!(true));
    Session { csrf: shown.json()["csrf"].as_str().unwrap().into(), cookie }
}

/// The calls the worker has had, as `method params`.
fn called(world: &World) -> Vec<(String, Value)> {
    world.calls.lock().unwrap().clone()
}

// The table and the small decisions.

#[test]
fn every_route_is_found_by_its_own_verb_and_no_other_and_no_route_is_in_the_table_twice() {
    let mut routes = HashSet::new();
    for entry in TABLE {
        assert!(routes.insert(entry.route as usize), "{:?} twice", entry.route);
        let twins = TABLE.iter().filter(|other| other.path == entry.path);
        assert_eq!(
            twins.filter(|other| other.verb == entry.verb).count(),
            1,
            "{} {:?} twice",
            entry.path,
            entry.verb
        );
        assert!(entry.path.starts_with("/api/1/"), "{}", entry.path);
        assert!(
            matches!(lookup(entry.verb, entry.path), Lookup::Found(found, _) if found.route == entry.route)
        );
        // No verb that the table does not give the address finds it.
        assert!(matches!(lookup(Verb::Head, entry.path), Lookup::WrongVerb), "{}", entry.path);
        // What only reads is a GET, and what changes anything is a POST.
        let reads = [
            Route::Health,
            Route::Fingerprint,
            Route::OpenApi,
            Route::Session,
            Route::Events,
            Route::Steps,
            Route::Preferences,
            Route::Actions,
            Route::Changes,
            Route::Change,
            Route::Tasks,
            Route::Task,
            Route::Terminals,
            Route::TerminalSocket,
        ];
        assert_eq!(entry.verb == Verb::Get, reads.contains(&entry.route), "{}", entry.path);
    }
    assert_eq!(TABLE.len(), 46 + usize::from(cfg!(feature = "lab")));
    assert!(matches!(lookup(Verb::Get, "/api/1/nothing"), Lookup::Missing));
    assert!(matches!(lookup(Verb::Get, "/api/1/health/"), Lookup::Missing));
    assert!(matches!(lookup(Verb::Get, "/api"), Lookup::Missing));
    assert!(matches!(lookup(Verb::Get, "/api/2/health"), Lookup::Missing));
    // A path that only starts like the API's is a file's.
    assert!(
        matches!(lookup(Verb::Get, "/apiary"), Lookup::Found(entry, _) if entry.route == Route::File)
    );
}

#[test]
fn files_are_for_get_and_head_alone_and_a_post_to_a_page_is_the_wrong_verb() {
    for path in ["/", "/app.js", "/accounts/users", "/x.png"] {
        for verb in [Verb::Get, Verb::Head] {
            assert!(
                matches!(lookup(verb, path), Lookup::Found(entry, _) if entry.route == Route::File),
                "{path}"
            );
        }
        assert!(matches!(lookup(Verb::Post, path), Lookup::WrongVerb), "{path}");
    }
}

#[test]
fn before_sign_in_only_health_fingerprint_the_description_and_the_sign_in_routes_are_open() {
    let access = |route: Route| TABLE.iter().find(|entry| entry.route == route).unwrap().access;
    for open in [Route::Health, Route::Fingerprint, Route::OpenApi] {
        assert_eq!(access(open), Access::Anyone);
    }
    for sign in
        [Route::SignIn, Route::SignInCode, Route::SignInPasskeyOptions, Route::SignInPasskey]
    {
        assert_eq!(access(sign), Access::SignIn);
    }
    // What a session that may only add a second step can use.
    let limited: Vec<Route> = TABLE
        .iter()
        .filter(|entry| entry.access == Access::Limited)
        .map(|entry| entry.route)
        .collect();
    assert_eq!(
        limited,
        [
            Route::Session,
            Route::SignOut,
            Route::Events,
            Route::Steps,
            Route::AppBegin,
            Route::AppConfirm,
            Route::AppRemove,
            Route::PasskeyBegin,
            Route::PasskeyAdd,
            Route::PasskeyRemove,
            Route::PasskeyRename,
            Route::Codes,
            Route::TrustedForget,
        ]
    );
    for api in [Route::Actions, Route::Show, Route::Check] {
        assert_eq!(access(api), Access::Api, "{api:?}");
    }
    for full in [
        Route::Unlock,
        Route::UnlockPasskeyOptions,
        Route::UnlockExtend,
        Route::Lock,
        Route::TokenAdd,
        Route::TokenRemove,
        Route::Preferences,
        Route::PreferencesChange,
    ] {
        assert_eq!(access(full), Access::Full, "{full:?}");
    }
}

#[test]
fn a_body_is_json_with_or_without_parameters_and_the_limit_grows_after_sign_in() {
    for json in [
        Some("application/json"),
        Some("Application/JSON"),
        Some("application/json; charset=utf-8"),
    ] {
        assert!(is_json(json), "{json:?}");
    }
    for other in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("application/jsonp"),
        Some(""),
    ] {
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
        for request in
            [get("/api/1/health"), get("/"), get("/api/1/session"), get("/api/1/nothing")]
        {
            let answered = send(&world, request).await;
            assert!(
                answered
                    .header("content-security-policy")
                    .unwrap()
                    .contains("frame-ancestors 'none'")
            );
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
fn the_description_of_the_api_is_answered_to_anyone_and_names_every_route() {
    run(async {
        let world = world("openapi");
        let answered = send(&world, get("/api/1/openapi.json")).await;
        assert_eq!(answered.status, 200);
        assert_eq!(answered.header("content-type"), Some("application/json"));
        assert_eq!(answered.header("cache-control"), Some("no-store"));
        let described = answered.json();
        assert_eq!(described["openapi"], "3.1.0");
        assert_eq!(described, openapi::document(world.console.shared.actions.get()));
        for entry in TABLE {
            assert!(described["paths"][entry.path].is_object(), "{}", entry.path);
        }
        // An API token has no use of it: it is for the actions.
        let with_token =
            send(&world, get("/api/1/openapi.json").with("authorization", Some("Bearer dkv_abc")))
                .await;
        assert_eq!(with_token.status, 403);
        assert_eq!(with_token.json()["error"]["id"], "ADM-124");
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
        for host in ["evil.example", "evil.example:9090", "nas.example:0", "nas.example:", ""] {
            for path in ["/api/1/health", "/", "/api/1/nothing"] {
                let answered = send(&world, get(path).with("host", Some(host))).await;
                assert_eq!(
                    (answered.status, answered.error().as_str()),
                    (421, "ADM-114"),
                    "{host} {path}"
                );
            }
        }
        let none = send(&world, get("/api/1/health").with("host", None)).await;
        assert_eq!(none.status, 421);
        // The name, the machine's own, `.local`, localhost and an address
        // of its cards are its own; case does not matter.
        for host in [
            "nas.example:9090",
            "NAS.Example:9090",
            "nas:9090",
            "nas.local:9090",
            "localhost:9090",
            "127.0.0.1:9090",
            "[::1]:9090",
            // Through a forwarded port or a tunnel.
            "localhost:8443",
            "nas.local",
        ] {
            let answered = send(&world, get("/api/1/health").with("host", Some(host))).await;
            assert_eq!(answered.status, 200, "{host}");
        }
    });
}

#[test]
fn an_api_token_is_for_the_actions_alone_and_never_with_a_session() {
    run(async {
        let world = world("tokens");
        let bearer = format!("Bearer {TOKEN_ALL}");
        // A route of the table is for the console's pages.
        let health = get("/api/1/health").with("authorization", Some(&bearer));
        let answered = send(&world, health).await;
        assert_eq!((answered.status, answered.error().as_str()), (403, "ADM-124"));
        let session = get("/api/1/session").with("authorization", Some(&bearer));
        assert_eq!(send(&world, session).await.error(), "ADM-124");
        let both = get("/api/1/actions")
            .with("authorization", Some(&bearer))
            .with("cookie", Some("__Host-dukovo=abc"));
        let answered = send(&world, both).await;
        assert_eq!((answered.status, answered.error().as_str()), (400, "ADM-108"));
        // Another kind of authorization is no token, and is let be.
        let basic = get("/api/1/health").with("authorization", Some("Basic YTpi"));
        assert_eq!(send(&world, basic).await.status, 200);
        // The host is checked before the token is.
        let elsewhere = get("/api/1/actions")
            .with("host", Some("evil.example"))
            .with("authorization", Some(&bearer));
        assert_eq!(send(&world, elsewhere).await.status, 421);
        // None of it asked authd about the token.
        assert!(!world.console.authd.verbs().contains(&"token".to_string()));
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
fn a_file_has_the_manifests_hash_as_its_etag_and_one_the_browser_holds_is_304_with_no_body() {
    run(async {
        let world = world("files-etag");
        let hash = |bytes: &[u8]| {
            accounts::request::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
        };
        let app_tag = format!("\"{}\"", hash(b"export {};"));
        let shell_tag = format!("\"{}\"", hash(fixture::PAGE.as_bytes()));
        let app = send(&world, get("/app.js")).await;
        assert_eq!(app.header("etag"), Some(app_tag.as_str()));
        assert_eq!(app.header("cache-control"), Some("no-cache"));
        // The shell has its own, and so has each path that is the router's.
        for path in ["/", "/index.html", "/services/nginx"] {
            let shell = send(&world, get(path)).await;
            assert_eq!(shell.header("etag"), Some(shell_tag.as_str()), "{path}");
        }
        // A browser that holds the file is told so, with the file's headers
        // and no body.
        let held = |tag: &str| get("/app.js").with("if-none-match", Some(tag));
        for tag in
            [app_tag.clone(), format!("W/{app_tag}"), format!("{shell_tag}, {app_tag}"), "*".into()]
        {
            let answered = send(&world, held(&tag)).await;
            assert_eq!((answered.status, answered.body.as_str()), (304, ""), "{tag}");
            assert_eq!(answered.header("etag"), Some(app_tag.as_str()));
            assert_eq!(answered.header("cache-control"), Some("no-cache"));
            assert_eq!(answered.header("content-type"), None);
            assert_eq!(answered.header("x-content-type-options"), Some("nosniff"));
            assert!(answered.header("content-security-policy").is_some());
        }
        // A tag of another file, or of nothing, is the whole file again.
        for tag in [shell_tag.as_str(), "\"old\"", "", "garbage"] {
            let answered = send(&world, held(tag)).await;
            assert_eq!((answered.status, answered.body.as_str()), (200, "export {};"), "{tag}");
            assert_eq!(answered.header("etag"), Some(app_tag.as_str()));
        }
        // Two lines say what one does, and the page router's paths are 304
        // for the shell's tag.
        let mut two = get("/app.js");
        two.headers.push(("if-none-match".into(), "\"old\"".into()));
        two.headers.push(("if-none-match".into(), app_tag.clone()));
        assert_eq!(send(&world, two).await.status, 304);
        let page = get("/accounts/users").with("if-none-match", Some(&shell_tag));
        assert_eq!(send(&world, page).await.status, 304);
        let mut head = get("/app.js").with("if-none-match", Some(&app_tag));
        head.method = "HEAD";
        assert_eq!(send(&world, head).await.status, 304);
        // A file that is not there is a 404 whatever the browser holds, and
        // an answer of the API has no tag and is not 304.
        let none = send(&world, get("/missing.png").with("if-none-match", Some("*"))).await;
        assert_eq!((none.status, none.error().as_str()), (404, "ADM-116"));
        assert_eq!(none.header("etag"), None);
        let health = send(&world, get("/api/1/health").with("if-none-match", Some("*"))).await;
        assert_eq!((health.status, health.header("etag")), (200, None));
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
        // Through a tunnel, the browser asks for another port, and its
        // page is at that port: the origin of the other is not its own.
        let tunnelled = |origin| {
            post("/api/1/sign-in", &body)
                .with("host", Some("localhost:8443"))
                .with("origin", Some(origin))
        };
        let answered = send(&world, tunnelled("https://localhost:9090")).await;
        assert_eq!((answered.status, answered.error().as_str()), (403, "ADM-109"));
        // The page's own requests, with the header a browser adds.
        let same = post("/api/1/sign-in", &body).with("sec-fetch-site", Some("same-origin"));
        assert_eq!(send(&world, same).await.status, 200);
        assert_eq!(send(&world, tunnelled("https://localhost:8443")).await.status, 200);
        assert_eq!(world.console.authd.verbs().first().map(String::as_str), Some("begin"));
        assert_eq!(world.console.authd.verbs().iter().filter(|v| *v == "begin").count(), 2);
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
        let charset = post("/api/1/sign-in", &body)
            .with("content-type", Some("application/json; charset=utf-8"));
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
        let wrong =
            send(&world, post("/api/1/sign-in", &json!({ "name": "nobody", "password": "x" })))
                .await;
        assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-101"));
        assert!(wrong.cookies().is_empty());
        let empty =
            send(&world, post("/api/1/sign-in", &json!({ "name": "chase", "password": "" }))).await;
        assert_eq!(empty.error(), "ADM-101");
        // An account that may not use the console is told so, as authd
        // says it only to someone who knows the password.
        let bob =
            send(&world, post("/api/1/sign-in", &json!({ "name": "bob", "password": "right" })))
                .await;
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
                "kinds": ["password", "app"], "terminal": true,
            })
        );
        assert_eq!(URL_SAFE_NO_PAD.decode(&session.csrf).unwrap().len(), 32);
        // No cookie, a cookie that is no session's, and one sent twice.
        for cookie in
            [None, Some("__Host-dukovo=nothing"), Some("__Host-dukovo=a; __Host-dukovo=b")]
        {
            let answered = send(&world, get("/api/1/session").with("cookie", cookie)).await;
            assert_eq!((answered.status, answered.error().as_str()), (401, "ADM-107"));
            // The browser forgets the cookie it was told is no good. One
            // that sent none, as SameSite has a browser do for another
            // site's page, is told nothing: it may hold a live one.
            let cleared = vec![guard::clear_cookie(SESSION_COOKIE)];
            assert_eq!(answered.cookies(), if cookie.is_some() { cleared } else { Vec::new() });
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
        let code =
            |code: &str| post("/api/1/sign-in/code", &json!({ "ticket": ticket, "code": code }));
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
        let known =
            post("/api/1/sign-in", &body).with("cookie", Some("__Host-dukovo-trust=aa55bb66"));
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
        let session = sign_in_to_add_a_step(&world, "eve").await;
        // What it may use.
        assert_eq!(send(&world, get("/api/1/account/second-step").by(&session)).await.status, 200);
        let begun = send(
            &world,
            post("/api/1/account/second-step/app/begin", &json!({ "proof": "right" })).by(&session),
        )
        .await;
        assert_eq!(begun.status, 200, "{}", begun.body);
        // What it may not, which authd would refuse anyway.
        for (path, body) in [
            ("/api/1/unlock", json!({ "kind": "password", "password": "right" })),
            ("/api/1/unlock/extend", json!({})),
            ("/api/1/lock", json!({})),
            (
                "/api/1/account/tokens/add",
                json!({ "proof": "", "name": "a", "scopes": ["*"], "days": 1 }),
            ),
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
            bare()
                .with("x-dukovo-csrf", Some(&session.csrf))
                .with("origin", Some("https://evil.example")),
            bare()
                .with("x-dukovo-csrf", Some(&session.csrf))
                .with("sec-fetch-site", Some("cross-site")),
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
        // A tab that was not listening when it ended hears why; a cookie
        // admind never gave hears nothing more.
        assert_eq!(again.json()["error"]["detail"], "you signed out");
        let stranger = get("/api/1/session").with("cookie", Some("__Host-dukovo=abc"));
        let stranger = send(&world, stranger).await;
        assert_eq!(
            stranger.json()["error"]["detail"],
            "this browser has no session, or it has ended"
        );
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
        for body in [
            json!({ "kind": "password", "password": "right" }),
            json!({ "kind": "code", "code": "123456" }),
        ] {
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
        let silly: Arc<fixture::Script> =
            Arc::new(move |asked: &Asked, mut fds: Vec<OwnedFd>| match asked.verb.as_str() {
                "begin" => vec!["ok done 0a0b01".into()],
                "start" => {
                    let calls = Arc::clone(&shared_calls);
                    let answers = Arc::new(move |method: &str, params: &Value| {
                        worker_answers(method, params, &calls)
                    });
                    fixture::worker(fixture::ends(&mut fds), Arc::clone(&shared_calls), answers);
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
        let confirmed =
            send(&world, call("second-step/app/confirm", json!({ "code": "123456" }))).await;
        assert_eq!(confirmed.json(), json!({ "codes": ["AAAA-BBBB"] }));
        let removed = send(&world, call("second-step/app/remove", json!({ "proof": "" }))).await;
        assert_eq!(removed.json(), json!({ "done": true }));
        let renamed = send(
            &world,
            call("second-step/passkey/rename", json!({ "id": "0aff", "name": "Desk" })),
        )
        .await;
        assert_eq!(renamed.json(), json!({ "done": true }));
        let gone = send(
            &world,
            call("second-step/passkey/remove", json!({ "proof": "pw", "id": "0aff" })),
        )
        .await;
        assert_eq!(gone.json(), json!({ "done": true }));
        let codes = send(&world, call("second-step/codes", json!({ "proof": "pw" }))).await;
        assert_eq!(codes.json(), json!({ "codes": ["AAAA-BBBB"] }));
        let forgotten =
            send(&world, post(&path("second-step/trusted/forget"), &json!({})).by(&session)).await;
        assert_eq!(forgotten.json(), json!({ "forgotten": 2 }));
        let token =
            json!({ "proof": "pw", "name": "ansible", "scopes": ["services:read"], "days": 30 });
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
            [
                "steps",
                "steps",
                "app-begin",
                "app-confirm",
                "app-remove",
                "passkey-rename",
                "passkey-remove",
                "codes",
                "trusted-forget",
                "token-add",
                "token-remove"
            ]
        );
        assert_eq!(seen[2].1, json!({ "proof": "pw", "host": "nas" }));
        assert_eq!(seen[3].1, json!({ "code": "123456" }));
        assert_eq!(seen[5].1, json!({ "id": "0aff", "name": "Desk" }));
        assert_eq!(
            seen[9].1,
            json!({ "proof": "pw", "name": "ansible", "scopes": ["services:read"], "days": 30 })
        );
    });
}

#[test]
fn a_passkey_is_made_for_the_name_the_browser_used_and_added_with_the_challenge_it_signed() {
    run(async {
        let world = world("passkey-add");
        let session = sign_in(&world, "chase").await;
        let path = "/api/1/account/second-step/passkey";
        let begun =
            send(&world, post(&format!("{path}/begin"), &json!({ "proof": "pw" })).by(&session))
                .await;
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
        let added = send(
            &world,
            post(&format!("{path}/add"), &json!({ "name": "Desk key", "credential": credential }))
                .by(&session),
        )
        .await;
        assert_eq!(added.json(), json!({ "codes": [] }));
        let seen = called(&world);
        assert_eq!(seen[1].0, "passkey-begin");
        assert_eq!(seen[1].1, json!({ "proof": "pw", "relying_party": "nas.example" }));
        assert_eq!(seen[2].0, "passkey-add");
        let params = &seen[2].1;
        assert_eq!(params["challenge"], "AQI");
        assert_eq!(
            (params["origin"].as_str(), params["relying_party"].as_str()),
            (Some(ORIGIN), Some("nas.example"))
        );
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
        let wrong = post("/api/1/account/second-step/app/confirm", &json!({ "code": "000000" }))
            .by(&session);
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

// The person's preferences, through the worker.

#[test]
fn one_address_takes_a_get_and_a_post_and_no_other_verb() {
    let found = |verb| match lookup(verb, "/api/1/account/preferences") {
        Lookup::Found(entry, _) => Some(entry.route),
        Lookup::WrongVerb => None,
        Lookup::Missing => panic!("the address is in the table"),
    };
    assert_eq!(found(Verb::Get), Some(Route::Preferences));
    assert_eq!(found(Verb::Post), Some(Route::PreferencesChange));
    assert_eq!(found(Verb::Head), None);
}

#[test]
fn preferences_are_read_and_changed_by_the_worker_for_a_session_that_is_locked() {
    run(async {
        let world = world("preferences");
        let session = sign_in(&world, "chase").await;
        let seen = called(&world).len();
        let path = "/api/1/account/preferences";
        let defaults = defaults();
        let shown = send(&world, get(path).by(&session)).await;
        assert_eq!(shown.status, 200);
        assert_eq!(shown.json(), defaults);
        let body = json!({ "theme": "light", "text": 112 });
        let changed = send(&world, post(path, &body).by(&session)).await;
        let both = preferences(&body);
        assert_eq!(changed.status, 200);
        assert_eq!(changed.json(), both);
        // No key is a change of nothing, and still all of them are told;
        // an empty zone is the person having none.
        let nothing = send(&world, post(path, &json!({})).by(&session)).await;
        assert_eq!(nothing.json(), defaults);
        let none = send(&world, post(path, &json!({ "zone": "" })).by(&session)).await;
        assert_eq!(none.json(), defaults);
        // The worker is told what the page gave, and no more, with no
        // unlock: the file is the person's own.
        let seen = called(&world).split_off(seen);
        assert_eq!(
            seen,
            [
                ("preferences".to_string(), Value::Null),
                ("preferences-change".to_string(), body),
                ("preferences-change".to_string(), json!({})),
                ("preferences-change".to_string(), json!({ "zone": "" })),
            ]
        );
        assert!(!world.console.authd.verbs().contains(&"unlock".to_string()));
    });
}

#[test]
fn the_commands_and_the_terminals_preferences_are_read_and_changed_with_the_others() {
    run(async {
        let world = world("preferences-terminal");
        let session = sign_in(&world, "chase").await;
        let seen = called(&world).len();
        let path = "/api/1/account/preferences";
        let shown = send(&world, get(path).by(&session)).await.json();
        assert_eq!(shown["commands"], "hidden");
        let terminal = shown["terminal"].as_object().unwrap();
        let keys: Vec<&str> = terminal.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["announce", "copy-on-select", "keep-days", "scrollback", "shortcuts", "size"]
        );
        // The limits themselves work, and the worker is told what was given.
        let body = json!({
            "commands": "shown",
            "terminal": { "size": 20, "scrollback": 1000, "keep-days": 30, "copy-on-select": true },
        });
        let changed = send(&world, post(path, &body).by(&session)).await;
        assert_eq!(changed.status, 200, "{}", changed.body);
        assert_eq!(changed.json(), preferences(&body));
        assert_eq!(changed.json()["terminal"]["shortcuts"], true);
        let low = json!({ "terminal": { "size": 12, "scrollback": 50000, "keep-days": 1 } });
        assert_eq!(send(&world, post(path, &low).by(&session)).await.status, 200);
        // An empty table changes nothing and is still sent as the page gave it.
        let empty = json!({ "terminal": {} });
        assert_eq!(send(&world, post(path, &empty).by(&session)).await.json(), defaults());
        let seen = called(&world).split_off(seen);
        assert_eq!(
            seen,
            [
                ("preferences".to_string(), Value::Null),
                ("preferences-change".to_string(), body),
                ("preferences-change".to_string(), low),
                ("preferences-change".to_string(), empty),
            ]
        );
    });
}

#[test]
fn a_value_that_cannot_work_is_refused_before_the_worker_hears_of_it() {
    run(async {
        let world = world("preferences-values");
        let session = sign_in(&world, "chase").await;
        let seen = called(&world).len();
        let path = "/api/1/account/preferences";
        // The test machine has no zone database, so no zone but none is one.
        for (body, field) in [
            (json!({ "theme": "purple" }), "theme"),
            (json!({ "theme": "" }), "theme"),
            (json!({ "text": 130 }), "text"),
            (json!({ "text": -1 }), "text"),
            (json!({ "motion": "none" }), "motion"),
            (json!({ "zone": "Mars/Base" }), "zone"),
            (json!({ "zone": "../../etc/passwd" }), "zone"),
            (json!({ "theme": "light", "text": 99 }), "text"),
            (json!({ "commands": "loud" }), "commands"),
            (json!({ "commands": "" }), "commands"),
            (json!({ "terminal": { "size": 11 } }), "terminal.size"),
            (json!({ "terminal": { "size": 21 } }), "terminal.size"),
            (json!({ "terminal": { "scrollback": 999 } }), "terminal.scrollback"),
            (json!({ "terminal": { "scrollback": 50_001 } }), "terminal.scrollback"),
            (json!({ "terminal": { "keep-days": 0 } }), "terminal.keep-days"),
            (json!({ "terminal": { "keep-days": 31 } }), "terminal.keep-days"),
            (json!({ "terminal": { "announce": true, "size": -4 } }), "terminal.size"),
        ] {
            let refused = send(&world, post(path, &body).by(&session)).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-125"), "{body}");
            assert_eq!(refused.json()["error"]["field"], field, "{body}");
        }
        // A body that is not what the route takes is the browser's mistake.
        for body in [
            json!({ "themes": "light" }),
            json!({ "text": "112" }),
            json!({ "text": 112.5 }),
            json!({ "theme": ["light"] }),
            json!({ "commands": true }),
            json!({ "terminal": 14 }),
            json!({ "terminal": "size" }),
            json!({ "terminal": { "size": "14" } }),
            json!({ "terminal": { "size": 14.5 } }),
            json!({ "terminal": { "announce": "yes" } }),
            json!({ "terminal": { "shortcuts": 1 } }),
            json!({ "terminal": { "keepdays": 7 } }),
            json!({ "terminal": { "keep_days": 7 } }),
            json!("light"),
            json!(null),
        ] {
            let refused = send(&world, post(path, &body).by(&session)).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-108"), "{body}");
        }
        let mut empty = post(path, &json!({})).by(&session);
        empty.body.clear();
        assert_eq!(send(&world, empty).await.error(), "ADM-108");
        assert_eq!(called(&world).len(), seen);
    });
}

#[test]
fn a_file_the_worker_cannot_write_is_told_in_its_words_and_logged() {
    run(async {
        let world = world("preferences-fault");
        let session = sign_in(&world, "chase").await;
        let path = "/api/1/account/preferences";
        let answered = send(&world, post(path, &json!({ "theme": "device" })).by(&session)).await;
        assert_eq!((answered.status, answered.error().as_str()), (500, "ADM-121"));
        let detail = answered.json()["error"]["detail"].as_str().unwrap().to_string();
        assert!(detail.starts_with("cannot write /home/chase/.config/dukovo-admin/"), "{detail}");
    });
}

#[test]
fn preferences_are_for_a_signed_in_session_that_may_do_all_and_not_for_a_token() {
    run(async {
        let world = world("preferences-access");
        let path = "/api/1/account/preferences";
        // Nobody is signed in.
        assert_eq!(send(&world, get(path)).await.error(), "ADM-107");
        let anonymous = post(path, &json!({})).with("cookie", Some("__Host-dukovo=abc"));
        assert_eq!(send(&world, anonymous).await.error(), "ADM-107");
        // A token is for the actions.
        let bearer = format!("Bearer {TOKEN_ALL}");
        for request in [get(path), post(path, &json!({}))] {
            let answered = send(&world, request.with("authorization", Some(&bearer))).await;
            assert_eq!((answered.status, answered.error().as_str()), (403, "ADM-124"));
        }
        // A change that does not come from the console's own page is refused.
        let session = sign_in(&world, "chase").await;
        let seen = called(&world).len();
        let foreign = post(path, &json!({ "theme": "light" })).by(&session).with("origin", None);
        assert_eq!(send(&world, foreign).await.error(), "ADM-109");
        assert_eq!(called(&world).len(), seen);
        // A session that may only add a second step may not.
        let eve = sign_in_to_add_a_step(&world, "eve").await;
        let seen = called(&world).len();
        for request in [get(path), post(path, &json!({}))] {
            let refused = send(&world, request.by(&eve)).await;
            assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-118"));
        }
        // The worker was asked for none of it.
        assert_eq!(called(&world).len(), seen);
    });
}

// The stream of events.

#[test]
fn a_tabs_stream_tells_the_unlock_when_it_changes_and_why_the_session_ended() {
    run(async {
        let world = world("events");
        let session = sign_in(&world, "chase").await;
        let request = build(get("/api/1/events").by(&session));
        let Ok(response) =
            handle(Rc::clone(&world.console.shared), PEER.parse().unwrap(), request).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let mut body = response.into_body();
        let mut next = async || {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap();
            frame.map(|frame| {
                String::from_utf8(frame.unwrap().into_data().unwrap().to_vec()).unwrap()
            })
        };
        assert_eq!(next().await.unwrap(), "event: unlock\ndata: {\"unlock_seconds\":null}\n\n");
        assert_eq!(world.console.shared.with_session(&session.cookie, |s| s.tabs), Some(1));
        let unlock = json!({ "kind": "password", "password": "right" });
        send(&world, post("/api/1/unlock", &unlock).by(&session)).await;
        let told = next().await.unwrap();
        let data =
            told.strip_prefix("event: unlock\ndata: ").and_then(|rest| rest.strip_suffix("\n\n"));
        let seconds =
            serde_json::from_str::<Value>(data.expect(&told)).unwrap()["unlock_seconds"].as_i64();
        assert!((895..=900).contains(&seconds.unwrap()), "{told}");
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
        // Requests while no tab is open, as a script polling the session
        // makes, do not keep it unlocked.
        let (asking, world_cookie) = (Rc::clone(shared), session.cookie.clone());
        tokio::task::spawn_local(async move {
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                asking.with_session(&world_cookie, |s| s.seen = std::time::Instant::now());
            }
        });
        events::left(shared, &session.cookie, &authd, after).await;
        assert_eq!(tabs(), Some(0));
        assert_eq!(shared.with_session(&session.cookie, |s| s.unlocked_until), Some(None));
    });
}

#[test]
fn a_session_whose_time_is_up_ends_and_authd_is_told() {
    run(async {
        let world = world("maintain");
        let session = sign_in(&world, "chase").await;
        // 13 hours on, with the sign-in rules' 12.
        let later = std::time::Instant::now() + Duration::from_hours(13);
        world.console.shared.maintain(later);
        let again = send(&world, get("/api/1/session").by(&session)).await;
        assert_eq!(again.error(), "ADM-107");
        assert_eq!(again.json()["error"]["detail"], "the session reached its time limit");
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
        let (ours, theirs) = crate::admind::link::pipes().unwrap();
        let link = Link::new(ours).unwrap();
        let authd = "ab".repeat(16);
        let session = sessions::Session::new(
            "chase".into(),
            authd.clone(),
            PEER.parse().unwrap(),
            false,
            link.clone(),
        );
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

// The lab's route.

#[cfg(feature = "lab")]
#[test]
fn the_lab_route_runs_a_command_for_an_unlocked_session_and_gives_its_output_and_status() {
    run(async {
        let world = world("lab-elevate");
        let session = sign_in(&world, "chase").await;
        let run = |command: Value| {
            post("/api/1/test/elevate", &json!({ "command": command })).by(&session)
        };
        let done = send(&world, run(json!(["/system/commands/echo", "hello there"]))).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(done.json(), json!({ "output": "hello there\n", "errors": "", "status": 3 }));
        // One line of words is the same command.
        let line = send(&world, run(json!("/system/commands/echo hi"))).await;
        assert_eq!(line.json()["output"], "hi\n");
        // What authd refuses is told as it is: here, a locked session.
        let locked = send(&world, run(json!(["/system/commands/locked"]))).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        let none = send(&world, run(json!([]))).await;
        assert_eq!(none.error(), "ADM-108");
        assert_eq!(world.console.shared.elevating.get(), 0);
        // The route is for a session that is not limited to a second step.
        let eve = sign_in(&world, "eve").await;
        let body = json!({ "command": ["/system/commands/echo"] });
        let refused = send(&world, post("/api/1/test/elevate", &body).by(&eve)).await;
        assert_eq!(refused.error(), "ADM-118");
    });
}
