// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Terminal's routes, through the console with the test authd, whose
//! shell (see `shell` in the parent) writes `ready$ `, answers each line
//! with `you typed LINE`, and ends at `exit`. The last tests open the
//! socket as a browser does, over TLS to a console on a loopback port.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::*;
use crate::admin_settings::Allowed;
use crate::admind::serve::tests::{connect, serve_on_loopback};

/// The terminal calls authd has had, each as its fields.
fn shells_asked(world: &World) -> Vec<Value> {
    let calls = world.calls.lock().unwrap();
    calls.iter().filter(|(verb, _)| verb == "terminal").map(|(_, fields)| fields.clone()).collect()
}

async fn open(world: &World, session: &Session, body: &Value) -> Answered {
    send(world, post("/api/1/terminals", body).by(session)).await
}

async fn list(world: &World, session: &Session) -> Value {
    let listed = send(world, get("/api/1/terminals").by(session)).await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    listed.json()
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// The id of `value`'s field `key`.
fn id(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or_else(|| panic!("{key} in {value}"))
}

#[test]
fn a_terminal_is_opened_named_laid_out_given_panes_and_ended() {
    run(async {
        let world = world("terminals");
        let chase = sign_in(&world, "chase").await;
        let empty = json!({ "terminals": [], "most": 32, "sockets": { "open": 0, "most": 4 } });
        assert_eq!(list(&world, &chase).await, empty);

        let opened = open(&world, &chase, &json!({ "cols": 80, "rows": 24 })).await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let terminal = opened.json()["terminal"].clone();
        assert_eq!(terminal["name"], "chase@nas");
        assert_eq!(terminal["layout"], "one");
        assert_eq!(terminal["from"], PEER);
        assert_eq!(terminal["keep_days"], 7);
        let pane = &terminal["panes"][0];
        assert_eq!(
            (&pane["admin"], &pane["cols"], &pane["rows"]),
            (&json!(false), &json!(80), &json!(24))
        );
        assert_eq!(pane["attached"], 0);
        // authd started chase's own shell on the session, for the page's
        // emulator.
        let asked = shells_asked(&world);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].as_array().unwrap()[1..], [json!("own"), json!("xterm-256color")]);
        assert_eq!(asked[0][0].as_str().unwrap().len(), 32);
        let terminal_id = id(&terminal, "id");
        let path = |what: &str| format!("/api/1/terminals/{terminal_id}/{what}");

        let renamed =
            send(&world, post(&path("rename"), &json!({ "name": " build box " })).by(&chase)).await;
        assert_eq!(renamed.json()["terminal"]["name"], "build box");
        for name in ["", "   ", "bell\u{7}", &"x".repeat(41)] {
            let refused =
                send(&world, post(&path("rename"), &json!({ "name": name })).by(&chase)).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-125"), "{name:?}");
        }

        // A second pane lays the terminal out in columns.
        let body = json!({ "cols": 40, "rows": 24 });
        let added = send(&world, post(&path("panes"), &body).by(&chase)).await;
        assert_eq!(added.status, 200, "{}", added.body);
        assert_eq!(added.json()["terminal"]["layout"], "columns");
        assert_eq!(added.json()["terminal"]["panes"].as_array().unwrap().len(), 2);
        let laid = json!({ "layout": "grid", "sizes": [0.5, 0.25] });
        let laid_out = send(&world, post(&path("layout"), &laid).by(&chase)).await;
        assert_eq!(laid_out.json()["terminal"]["layout"], "grid");
        assert_eq!(laid_out.json()["terminal"]["sizes"], json!([0.5, 0.25]));
        for wrong in [
            json!({ "layout": "spiral" }),
            json!({ "layout": "rows", "sizes": [0.9] }),
            json!({ "layout": "rows", "sizes": [0.5, 0.5, 0.5, 0.5] }),
        ] {
            let refused = send(&world, post(&path("layout"), &wrong).by(&chase)).await;
            assert_eq!(refused.error(), "ADM-125", "{wrong}");
        }
        // A layout named with the pane keeps to it; four panes at most.
        let body = json!({ "cols": 40, "rows": 12, "layout": "main-left" });
        let third = send(&world, post(&path("panes"), &body).by(&chase)).await;
        assert_eq!(third.json()["terminal"]["layout"], "main-left", "{}", third.body);
        assert_eq!(send(&world, post(&path("panes"), &body).by(&chase)).await.status, 200);
        let fifth = send(&world, post(&path("panes"), &body).by(&chase)).await;
        assert_eq!((fifth.status, fifth.error().as_str()), (409, "ADM-130"));
        assert_eq!(shells_asked(&world).len(), 4);

        // A pane ends, and then the terminal with the rest.
        let listed = list(&world, &chase).await;
        let panes = listed["terminals"][0]["panes"].as_array().unwrap().clone();
        assert_eq!(panes.len(), 4);
        let first = id(&panes[0], "id");
        let ended = send(
            &world,
            post(&format!("/api/1/terminals/panes/{first}/end"), &json!({})).by(&chase),
        )
        .await;
        assert_eq!(ended.json(), json!({ "done": true }));
        let listed = list(&world, &chase).await;
        assert_eq!(listed["terminals"][0]["panes"].as_array().unwrap().len(), 3);
        let again = send(
            &world,
            post(&format!("/api/1/terminals/panes/{first}/end"), &json!({})).by(&chase),
        )
        .await;
        assert_eq!((again.status, again.error().as_str()), (404, "ADM-131"));
        let gone = send(&world, post(&path("end"), &json!({})).by(&chase)).await;
        assert_eq!(gone.json(), json!({ "done": true }));
        assert_eq!(list(&world, &chase).await, empty);
        assert_eq!(
            send(&world, post(&path("end"), &json!({})).by(&chase)).await.error(),
            "ADM-131"
        );
    });
}

#[test]
fn a_terminal_is_its_accounts_alone_and_its_size_name_and_days_are_checked() {
    run(async {
        let world = world("terminal-checks");
        let chase = sign_in(&world, "chase").await;
        let dave = sign_in(&world, "dave").await;
        let opened = open(&world, &chase, &json!({ "cols": 80, "rows": 24, "name": "logs" })).await;
        let terminal = id(&opened.json()["terminal"], "id");
        let pane = id(&opened.json()["terminal"]["panes"][0], "id");
        assert_eq!(opened.json()["terminal"]["name"], "logs");

        // dave sees none of chase's, and can change none of them.
        assert_eq!(list(&world, &dave).await["terminals"], json!([]));
        for (what, body) in [
            ("rename", json!({ "name": "mine" })),
            ("layout", json!({ "layout": "rows" })),
            ("panes", json!({ "cols": 80, "rows": 24 })),
            ("end", json!({})),
        ] {
            let path = format!("/api/1/terminals/{terminal}/{what}");
            let refused = send(&world, post(&path, &body).by(&dave)).await;
            assert_eq!((refused.status, refused.error().as_str()), (404, "ADM-131"), "{what}");
        }
        let path = format!("/api/1/terminals/panes/{pane}/end");
        assert_eq!(send(&world, post(&path, &json!({})).by(&dave)).await.error(), "ADM-131");
        let path = "/api/1/terminals/nine/rename";
        let refused = send(&world, post(path, &json!({ "name": "x" })).by(&chase)).await;
        assert_eq!(refused.error(), "ADM-131");

        // Sizes, days and fields out of their ranges.
        for wrong in [
            json!({ "cols": 1, "rows": 24 }),
            json!({ "cols": 501, "rows": 24 }),
            json!({ "cols": 80, "rows": 0 }),
            json!({ "cols": 80, "rows": 301 }),
            json!({ "cols": 80, "rows": 24, "keep_days": 0 }),
            json!({ "cols": 80, "rows": 24, "keep_days": 31 }),
            json!({ "cols": 80, "rows": 24, "name": "" }),
        ] {
            let refused = open(&world, &chase, &wrong).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-125"), "{wrong}");
        }
        let unknown = open(&world, &chase, &json!({ "cols": 80, "rows": 24, "shell": "sh" })).await;
        assert_eq!(unknown.status, 400, "{}", unknown.body);
        let kept = open(&world, &chase, &json!({ "cols": 80, "rows": 24, "keep_days": 30 })).await;
        assert_eq!(kept.json()["terminal"]["keep_days"], 30);

        // As many terminals as the settings let an account keep.
        world.console.shared.settings.borrow_mut().terminal.sessions_per_account = 2;
        let third = open(&world, &chase, &json!({ "cols": 80, "rows": 24 })).await;
        assert_eq!((third.status, third.error().as_str()), (409, "ADM-130"));
        assert_eq!(list(&world, &chase).await["most"], 2);
        // dave holds none, so may still open one; authd, which reads the
        // settings again, may refuse it with an ID of its own.
        let refused = open(&world, &dave, &json!({ "cols": 80, "rows": 24 })).await;
        assert_eq!(
            (refused.status, refused.error().as_str()),
            (403, "ADM-132"),
            "{}",
            refused.body
        );
        assert!(refused.body.contains("do not let dave"), "{}", refused.body);
    });
}

#[test]
fn the_settings_say_who_may_have_a_terminal_and_a_token_never_may() {
    run(async {
        let world = world("terminal-who");
        let chase = sign_in(&world, "chase").await;
        let dave = sign_in(&world, "dave").await;
        let offered = |answer: &Answered| answer.json()["terminal"].clone();
        let shown = send(&world, get("/api/1/session").by(&dave)).await;
        assert_eq!(offered(&shown), json!(true));

        world.console.shared.settings.borrow_mut().terminal.allowed = Allowed::Administrators;
        let shown = send(&world, get("/api/1/session").by(&dave)).await;
        assert_eq!(offered(&shown), json!(false));
        let refused = send(&world, get("/api/1/terminals").by(&dave)).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-132"));
        assert!(refused.body.contains("allowed = \\\"administrators\\\""), "{}", refused.body);
        assert_eq!(list(&world, &chase).await["terminals"], json!([]));
        let shown = send(&world, get("/api/1/session").by(&chase)).await;
        assert_eq!(offered(&shown), json!(true));

        world.console.shared.settings.borrow_mut().terminal.allowed = Allowed::Nobody;
        let refused = open(&world, &chase, &json!({ "cols": 80, "rows": 24 })).await;
        assert_eq!(refused.error(), "ADM-132");
        let shown = send(&world, get("/api/1/session").by(&chase)).await;
        assert_eq!(offered(&shown), json!(false));
        assert!(shells_asked(&world).is_empty());

        // A token is a script's, and a script has SSH.
        world.console.shared.settings.borrow_mut().terminal.allowed = Allowed::Everyone;
        let bearer = format!("Bearer {TOKEN_ALL}");
        for req in [
            get("/api/1/terminals"),
            post("/api/1/terminals", &json!({ "cols": 80, "rows": 24 })).with("origin", None),
            get("/api/1/terminal"),
        ] {
            let refused = send(&world, req.with("authorization", Some(&bearer))).await;
            assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-124"));
        }
    });
}

#[test]
fn an_administrators_pane_needs_an_unlocked_administrator_and_ends_when_the_session_locks() {
    run(async {
        let world = world("terminal-admin");
        let chase = sign_in(&world, "chase").await;
        let dave = sign_in(&world, "dave").await;
        let admin = json!({ "cols": 80, "rows": 24, "admin": true });
        let refused = open(&world, &dave, &admin).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-122"));
        let refused = open(&world, &chase, &admin).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-106"));
        assert!(shells_asked(&world).is_empty());

        unlock(&world, &chase).await;
        let opened = open(&world, &chase, &admin).await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let terminal = opened.json()["terminal"].clone();
        assert_eq!(terminal["name"], "root@nas");
        assert_eq!(terminal["panes"][0]["admin"], true);
        assert_eq!(shells_asked(&world)[0][1], "admin");
        // An own pane beside it.
        let path = format!("/api/1/terminals/{}/panes", id(&terminal, "id"));
        let own = send(&world, post(&path, &json!({ "cols": 80, "rows": 24 })).by(&chase)).await;
        assert_eq!(own.json()["terminal"]["panes"].as_array().unwrap().len(), 2);

        // chase's session in another browser sees the terminal, but not
        // the pane that is the first session's as an administrator.
        let other = sign_in_with(&world, "chase", "elsewhere").await;
        let seen = list(&world, &other).await;
        let panes = seen["terminals"][0]["panes"].as_array().unwrap().clone();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["admin"], false);

        // Locking ends the administrator's pane, and only that.
        let locked = send(&world, post("/api/1/lock", &json!({})).by(&chase)).await;
        assert_eq!(locked.status, 200, "{}", locked.body);
        let seen = list(&world, &chase).await;
        let panes = seen["terminals"][0]["panes"].as_array().unwrap().clone();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["admin"], false);
    });
}

/// A WebSocket's opening request for `session`, its headers as given.
fn opening(session: &Session) -> Req {
    let protocols = format!("dukovo-terminal-1, csrf.{}", session.csrf);
    get("/api/1/terminal")
        .by(session)
        .with("x-dukovo-csrf", None)
        .with("origin", Some(ORIGIN))
        .with("connection", Some("Upgrade"))
        .with("upgrade", Some("websocket"))
        .with("sec-websocket-version", Some("13"))
        .with("sec-websocket-key", Some("dGhlIHNhbXBsZSBub25jZQ=="))
        .with("sec-websocket-protocol", Some(&protocols))
}

#[test]
fn the_socket_is_opened_only_by_the_consoles_own_page_with_the_sessions_token() {
    run(async {
        let world = world("terminal-opening");
        let chase = sign_in(&world, "chase").await;
        let refusal = |answered: &Answered| (answered.status, answered.error());
        // A plain request is no WebSocket's.
        let plain = send(&world, get("/api/1/terminal").by(&chase)).await;
        assert_eq!(refusal(&plain), (400, "ADM-108".into()));
        let old = send(&world, opening(&chase).with("sec-websocket-version", Some("8"))).await;
        assert_eq!(refusal(&old), (400, "ADM-108".into()));
        let no_token = opening(&chase).with("sec-websocket-protocol", Some("dukovo-terminal-1"));
        assert_eq!(refusal(&send(&world, no_token).await), (400, "ADM-108".into()));
        // Another page, or another session's token.
        let foreign = opening(&chase).with("origin", Some("https://evil.example"));
        assert_eq!(refusal(&send(&world, foreign).await), (403, "ADM-109".into()));
        let site = opening(&chase).with("sec-fetch-site", Some("cross-site"));
        assert_eq!(refusal(&send(&world, site).await), (403, "ADM-109".into()));
        let protocols = "dukovo-terminal-1, csrf.bm90LXRoZS10b2tlbg";
        let wrong = opening(&chase).with("sec-websocket-protocol", Some(protocols));
        assert_eq!(refusal(&send(&world, wrong).await), (403, "ADM-109".into()));
        // No session at all.
        let signed_out = opening(&chase).with("cookie", None);
        assert_eq!(send(&world, signed_out).await.status, 401);
        // A request that would do is still refused where no connection
        // can be handed over.
        let right = send(&world, opening(&chase)).await;
        assert_eq!(refusal(&right), (400, "ADM-108".into()));
        assert!(right.body.contains("cannot be handed over"), "{}", right.body);
        assert_eq!(list(&world, &chase).await["sockets"]["open"], 0);
    });
}

/// A page's side of a terminal socket: what it has been sent, by kind.
struct Page<S> {
    stream: S,
    /// Pane output, as it came, from the offset the attach gave.
    output: Vec<u8>,
    /// Where the next output starts.
    offset: u64,
    /// The text messages, in order.
    said: Vec<Value>,
    /// The close frame's code, once one came.
    closed: Option<u16>,
    pongs: Vec<Vec<u8>>,
}

const TEXT: u8 = 0x1;
const BINARY: u8 = 0x2;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xa;

impl<S: AsyncRead + AsyncWrite + Unpin> Page<S> {
    fn new(stream: S) -> Page<S> {
        Page {
            stream,
            output: Vec::new(),
            offset: 0,
            said: Vec::new(),
            closed: None,
            pongs: Vec::new(),
        }
    }

    /// Sends a frame as a browser does, masked.
    async fn send(&mut self, opcode: u8, body: &[u8]) {
        let mask = [0x37u8, 0xfa, 0x21, 0x3d];
        let mut frame = vec![0x80 | opcode];
        match u16::try_from(body.len()) {
            Ok(short) if short < 126 => frame.push(0x80 | u8::try_from(short).unwrap()),
            Ok(medium) => {
                frame.push(0x80 | 0x7e);
                frame.extend_from_slice(&medium.to_be_bytes());
            }
            Err(_) => {
                frame.push(0x80 | 0x7f);
                frame.extend_from_slice(&(body.len() as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(body.iter().zip(mask.iter().cycle()).map(|(byte, key)| byte ^ key));
        self.stream.write_all(&frame).await.unwrap();
    }

    async fn ask(&mut self, message: &Value) {
        self.send(TEXT, message.to_string().as_bytes()).await;
    }

    async fn type_in(&mut self, pane: u64, keys: &str) {
        let mut message = u32::try_from(pane).unwrap().to_be_bytes().to_vec();
        message.extend_from_slice(keys.as_bytes());
        self.send(BINARY, &message).await;
    }

    /// Reads one frame, which a server sends whole and unmasked, and
    /// keeps what it says; `false` once the connection ended.
    async fn read(&mut self, pane: u64) -> bool {
        let mut head = [0u8; 2];
        let read = tokio::time::timeout(Duration::from_secs(5), self.stream.read_exact(&mut head));
        match read.await.expect("a frame in 5 seconds") {
            Ok(_) => {}
            Err(_) => return false,
        }
        assert_eq!(head[0] & 0xf0, 0x80, "a whole frame, with no extension's bits");
        assert_eq!(head[1] & 0x80, 0, "a server's frame is not masked");
        let length = match head[1] & 0x7f {
            0x7e => u64::from(self.stream.read_u16().await.unwrap()),
            0x7f => self.stream.read_u64().await.unwrap(),
            short => u64::from(short),
        };
        let mut body = vec![0u8; usize::try_from(length).unwrap()];
        self.stream.read_exact(&mut body).await.unwrap();
        match head[0] & 0x0f {
            TEXT => self.said.push(serde_json::from_slice(&body).unwrap()),
            BINARY => {
                let (from, rest) = body.split_first_chunk::<4>().unwrap();
                let (at, bytes) = rest.split_first_chunk::<8>().unwrap();
                assert_eq!(u64::from(u32::from_be_bytes(*from)), pane);
                assert_eq!(u64::from_be_bytes(*at), self.offset, "output that follows on");
                self.offset += bytes.len() as u64;
                self.output.extend_from_slice(bytes);
            }
            CLOSE => self.closed = Some(u16::from_be_bytes([body[0], body[1]])),
            PONG => self.pongs.push(body),
            PING => self.send(PONG, &body).await,
            other => panic!("opcode {other}"),
        }
        true
    }

    /// Reads until the output holds `wanted`.
    async fn until_output(&mut self, pane: u64, wanted: &str) {
        while !String::from_utf8_lossy(&self.output).contains(wanted) {
            assert!(self.read(pane).await, "ended before {wanted:?}: {:?}", self.said);
        }
    }

    /// Reads until a text message with `key` came, and gives it.
    async fn until_said(&mut self, pane: u64, key: &str) -> Value {
        loop {
            if let Some(found) = self.said.iter().find(|said| said.get(key).is_some()) {
                return found.clone();
            }
            assert!(self.read(pane).await, "ended before {key:?}: {:?}", self.said);
        }
    }
}

/// Reads an answer's head, up to the blank line, and nothing past it.
async fn head_of<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let byte = tokio::time::timeout(Duration::from_secs(5), stream.read_u8()).await;
        head.push(byte.expect("an answer in 5 seconds").unwrap());
    }
    String::from_utf8(head).unwrap()
}

/// The opening request as a browser writes it.
fn opening_text(session: &Session) -> String {
    format!(
        "GET /api/1/terminal HTTP/1.1\r\nHost: {HOST}\r\nOrigin: {ORIGIN}\r\n\
         Cookie: {SESSION_COOKIE}={}\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Protocol: dukovo-terminal-1, csrf.{}\r\nSec-Fetch-Site: same-origin\r\n\r\n",
        session.cookie, session.csrf
    )
}

#[test]
fn a_page_attaches_over_the_socket_sees_the_shell_types_to_it_and_sees_it_end() {
    run(async {
        let world = world("terminal-socket");
        let chase = sign_in(&world, "chase").await;
        let opened = open(&world, &chase, &json!({ "cols": 80, "rows": 24 })).await;
        let terminal = id(&opened.json()["terminal"], "id");
        let pane = id(&opened.json()["terminal"]["panes"][0], "id");
        let port = serve_on_loopback(&world.console.shared, Duration::from_secs(10));

        let mut stream = connect(&world.console, port).await;
        stream.write_all(opening_text(&chase).as_bytes()).await.unwrap();
        let head = head_of(&mut stream).await;
        let lower = head.to_ascii_lowercase();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        // RFC 6455's own example key and its answer.
        assert!(lower.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo=\r\n"), "{head}");
        assert!(lower.contains("sec-websocket-protocol: dukovo-terminal-1\r\n"), "{head}");
        assert!(lower.contains("upgrade: websocket\r\n"), "{head}");
        assert!(!lower.contains("csrf."), "the token is not said back: {head}");
        assert_eq!(list(&world, &chase).await["sockets"]["open"], 1);

        let mut page = Page::new(stream);
        page.ask(&json!({ "attach": pane, "cols": 80, "rows": 24 })).await;
        let attached = page.until_said(pane, "attached").await;
        assert_eq!(attached["terminal"], terminal);
        assert_eq!((&attached["replay"], &attached["admin"]), (&json!(true), &json!(false)));
        assert_eq!(attached["modes"], "");
        page.offset = id(&attached, "offset");
        page.until_output(pane, "ready$ ").await;
        assert_eq!(list(&world, &chase).await["terminals"][0]["panes"][0]["attached"], 1);

        // Typing goes to the shell; the terminal echoes it, and the shell
        // answers.
        page.type_in(pane, "echo hi\r").await;
        page.until_output(pane, "you typed echo hi\r\nready$ ").await;
        assert!(String::from_utf8_lossy(&page.output).contains("echo hi\r\n"));
        page.ask(&json!({ "ack": pane, "offset": page.offset })).await;
        // A page's ping is answered with its words.
        page.send(PING, b"still there?").await;
        while page.pongs.is_empty() {
            assert!(page.read(pane).await);
        }
        assert_eq!(page.pongs, [b"still there?".to_vec()]);
        // An attach to a pane that is not there is refused about it, and
        // the socket stays open.
        page.ask(&json!({ "attach": 999_999, "cols": 80, "rows": 24 })).await;
        let refused = page.until_said(pane, "error").await;
        assert_eq!(refused["error"]["id"], "ADM-131", "{refused}");

        // The shell ends: the pane does, and the page is told.
        page.type_in(pane, "exit\r").await;
        let ended = page.until_said(pane, "ended").await;
        assert_eq!(ended, json!({ "ended": pane, "why": "exited" }));
        assert_eq!(list(&world, &chase).await["terminals"], json!([]));

        // The page closes; admind answers the close and ends the
        // connection.
        page.send(CLOSE, &1000u16.to_be_bytes()).await;
        while page.read(pane).await {}
        assert_eq!(page.closed, Some(1000));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while list(&world, &chase).await["sockets"]["open"] != 0 {
            assert!(std::time::Instant::now() < deadline, "the socket was not closed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(world.console.shared.connections.borrow().total(), 0);
    });
}

#[test]
fn a_socket_that_breaks_the_rules_or_outlives_its_session_is_closed_with_its_code() {
    run(async {
        let world = world("terminal-socket-ends");
        let chase = sign_in(&world, "chase").await;
        let opened = open(&world, &chase, &json!({ "cols": 80, "rows": 24 })).await;
        let pane = id(&opened.json()["terminal"]["panes"][0], "id");
        let port = serve_on_loopback(&world.console.shared, Duration::from_secs(10));
        let socket = |session: &Session| {
            let text = opening_text(session);
            let console = &world.console;
            async move {
                let mut stream = connect(console, port).await;
                stream.write_all(text.as_bytes()).await.unwrap();
                assert!(head_of(&mut stream).await.starts_with("HTTP/1.1 101"));
                Page::new(stream)
            }
        };

        // A message that is not the protocol's is a policy's breach.
        let mut page = socket(&chase).await;
        page.ask(&json!({ "attach": pane, "cols": 80, "rows": 24, "shell": "sh" })).await;
        while page.read(pane).await {}
        assert_eq!(page.closed, Some(1008));
        // A text frame that is not UTF-8.
        let mut page = socket(&chase).await;
        page.send(TEXT, &[0xff, 0xfe]).await;
        while page.read(pane).await {}
        assert_eq!(page.closed, Some(1007));
        // An unmasked frame.
        let mut page = socket(&chase).await;
        page.stream.write_all(&[0x81, 0x02, b'{', b'}']).await.unwrap();
        while page.read(pane).await {}
        assert_eq!(page.closed, Some(1002));

        // Signing out closes the session's sockets with 4001; the pane,
        // which is the account's own, lives on.
        let mut page = socket(&chase).await;
        page.ask(&json!({ "attach": pane, "cols": 80, "rows": 24 })).await;
        page.until_said(pane, "attached").await;
        let out = send(&world, post("/api/1/sign-out", &json!({})).by(&chase)).await;
        assert_eq!(out.status, 200, "{}", out.body);
        while page.read(pane).await {}
        assert_eq!(page.closed, Some(4001));
        let again = sign_in(&world, "chase").await;
        let listed = list(&world, &again).await;
        assert_eq!(listed["terminals"][0]["panes"][0]["id"], pane);
        assert_eq!(listed["terminals"][0]["panes"][0]["attached"], 0);
    });
}
