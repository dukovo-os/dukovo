// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console as the boot tests reach it: the host's curl, aimed
//! through a port QEMU forwards at the console's port 9090 in the guest,
//! as a browser at `localhost` would be. The `admin`, `admin-actions`,
//! `admin-journal` and `admin-terminal` cases use it, with what they need
//! of the machine: alice, the owner, whose password the pane asks for
//! once, and the replies, cookies and certificates that curl prints.
//!
//! curl trusts the one certificate the machine served at first and no
//! other, so a case that makes the machine serve another says so with
//! [`Console::trust_what_is_served`].
//!
//! The Terminal's socket is a WebSocket, which curl does not speak and
//! xtask has no library for. A [`Socket`] starts `websocket_client.py`,
//! a Python 3 program of the standard library alone, and drives it one
//! JSON line at a time; the program's own words say what it does.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Vm, answer_prompt};

/// The owner's password, which alice gives at every sign-in.
pub const ALICE_PASSWORD: &str = "queen of hearts";

/// The console's port in the guest.
pub const PORT: u16 = 9090;

/// The console as a browser opens it: by `localhost`, a name that the
/// certificate holds and the console takes for its own, on the guest's
/// port, whatever port the host forwards (the console answers a Host
/// header that names another port with ADM-114).
pub const ORIGIN: &str = "https://localhost:9090";

/// The session's cookie.
pub const SESSION_COOKIE: &str = "__Host-dukovo";

/// The header that carries the session's token.
pub const CSRF_HEADER: &str = "X-Dukovo-CSRF";

/// Alice's password, once, for this pane: elevate asks on the console's
/// bottom line, and the commands after it within 15 minutes do not.
pub fn elevated(vm: &mut Vm) -> Result<(), String> {
    // `-k` locks the pane first, so that the password is asked even where
    // an earlier sign-in of alice's left it unlocked.
    answer_prompt(vm, "$", "clear; elevate -k; elevate true; echo \"elevated $?\"")?;
    answer_prompt(vm, "to run /system/commands/true:", ALICE_PASSWORD)?;
    vm.expect_screen(&["elevated 0"])
}

/// The console as the host reaches it: port 9090 of the guest, through the
/// port QEMU forwards on the host's loopback.
pub struct Console {
    /// The host's port.
    pub port: u16,
    /// The certificate the machine served at first, which curl trusts.
    pub pem: PathBuf,
    /// Its SHA-256 fingerprint, `AB:CD:...`.
    pub fingerprint: String,
}

impl Console {
    /// Forwards a port to the console, and takes the certificate it serves:
    /// the first handshake trusts what it is given, and the test compares
    /// that certificate with the guest's own before it relies on it.
    pub fn open(vm: &mut Vm) -> Result<Console, String> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|err| format!("no free port: {err}"))?
            .port();
        vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:{PORT}"))?;
        let mut console =
            Console { port, pem: vm.dir().join("admind.pem"), fingerprint: String::new() };
        console.trust_what_is_served(Duration::from_secs(30))?;
        Ok(console)
    }

    /// Takes the certificate the console serves now, waiting up to `wait`
    /// for it to answer, and trusts that one from here on, in place of the
    /// one taken before. The caller compares it with the guest's own: this
    /// only says what the handshake gave.
    pub fn trust_what_is_served(&mut self, wait: Duration) -> Result<(), String> {
        let url = format!("{ORIGIN}/api/1/health");
        let deadline = Instant::now() + wait;
        let learned = loop {
            let out = self.curl(&[
                "--insecure",
                "--output",
                "/dev/null",
                "--write-out",
                "%{certs}",
                &url,
            ])?;
            if out.status.success() {
                break String::from_utf8_lossy(&out.stdout).into_owned();
            }
            if Instant::now() >= deadline {
                let why = String::from_utf8_lossy(&out.stderr);
                let port = self.port;
                return Err(format!("the console does not answer on port {port}: {}", why.trim()));
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        let (pem, der) = first_certificate(&learned)
            .ok_or_else(|| format!("curl did not print the console's certificate: {learned:?}"))?;
        std::fs::write(&self.pem, pem)
            .map_err(|err| format!("writing {}: {err}", self.pem.display()))?;
        self.fingerprint = fingerprint_of(&der);
        Ok(())
    }

    /// Runs the host's curl, aimed at the guest's port 9090 as `localhost`,
    /// with `args` after the options every request has: no settings file,
    /// no proxy, HTTP/1.1, a limit on the time.
    pub fn curl(&self, args: &[&str]) -> Result<Output, String> {
        Command::new("curl")
            .args(["--disable", "--silent", "--show-error", "--http1.1", "--max-time", "90"])
            .args(["--noproxy", "*", "--connect-to"])
            .arg(format!("localhost:{PORT}:127.0.0.1:{}", self.port))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("the console tests need the host's curl: {err}"))
    }

    /// One request, trusting the certificate taken at first, with exactly
    /// the headers given besides Host and curl's own: what curl printed,
    /// whether or not it succeeded.
    pub fn run(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
    ) -> Result<Output, String> {
        let url = format!("{ORIGIN}{path}");
        let pem = self.pem.display().to_string();
        let mut args = vec!["--include", "--cacert", &pem, "--request", method];
        let lines: Vec<String> =
            headers.iter().map(|(name, value)| format!("{name}: {value}")).collect();
        for line in &lines {
            args.extend(["--header", line]);
        }
        if let Some(body) = body {
            args.extend(["--data-binary", body]);
        }
        args.push(&url);
        self.curl(&args)
    }

    /// [`Console::run`], for a request that curl must have carried out.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
    ) -> Result<Reply, String> {
        let out = self.run(method, path, headers, body)?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(format!("curl {method} {path} ended with {}: {}", out.status, why.trim()));
        }
        parse_reply(&out.stdout)
    }

    /// A read, as the page makes it. The browser keeps the cookie the
    /// answer sets or clears.
    pub fn get(&self, browser: &mut Browser, path: &str) -> Result<Reply, String> {
        let headers: Vec<_> =
            browser.page_headers().into_iter().filter(|(name, _)| name == "Cookie").collect();
        let reply = self.send("GET", path, &headers, None)?;
        browser.keep(&reply);
        Ok(reply)
    }

    /// A change, as the page makes it, with `body`. A sign-in route wants
    /// no more than ten tries from an address in a minute, and answers
    /// ADM-105 with the seconds to wait; the test waits and asks again.
    pub fn post(&self, browser: &mut Browser, path: &str, body: &Value) -> Result<Reply, String> {
        let body = body.to_string();
        for _ in 0..4 {
            let reply = self.send("POST", path, &browser.page_headers(), Some(&body))?;
            if reply.status != 429 {
                browser.keep(&reply);
                return Ok(reply);
            }
            let seconds = reply.header("retry-after").and_then(|text| text.parse::<u64>().ok());
            std::thread::sleep(Duration::from_secs(seconds.unwrap_or(10).min(70) + 1));
        }
        Err(format!("{path} was still answered 429 after four waits"))
    }

    /// Signs `name` in, and returns the console's answer, which the
    /// browser has taken its cookie from.
    pub fn sign_in(
        &self,
        browser: &mut Browser,
        name: &str,
        password: &str,
    ) -> Result<Reply, String> {
        let body = json!({"name": name, "password": password});
        self.post(browser, "/api/1/sign-in", &body)?.must(&format!("{name}'s sign-in"), 200)
    }

    /// Reads the session of a browser that signed in, and keeps the token
    /// its page would.
    pub fn read_session(&self, browser: &mut Browser) -> Result<Value, String> {
        let session = self.get(browser, "/api/1/session")?.must("the session", 200)?.json()?;
        let token = session["csrf"].as_str().ok_or("the session has no token")?;
        browser.token = Some(token.to_string());
        Ok(session)
    }
}

/// A stream of events as a page opens it, `GET /api/1/events`: curl keeps
/// running with what it receives in a file, which the test reads as the
/// events come. It is stopped, and the file taken away, when this is
/// dropped.
pub struct Events {
    child: Child,
    path: PathBuf,
}

impl Console {
    /// Opens the stream of events for the session of `browser`, which has
    /// signed in. The first event, the unlock, is the stream's start.
    pub fn events(&self, browser: &Browser) -> Result<Events, String> {
        let cookie = browser.cookie.as_ref().ok_or("a stream needs a session's cookie")?;
        let path = self.pem.with_file_name("events.txt");
        let file = File::create(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let errors = file.try_clone().map_err(|err| format!("{}: {err}", path.display()))?;
        let child = Command::new("curl")
            .args(["--disable", "--silent", "--show-error", "--http1.1", "--no-buffer"])
            .args(["--max-time", "1800", "--noproxy", "*", "--connect-to"])
            .arg(format!("localhost:{PORT}:127.0.0.1:{}", self.port))
            .args(["--include", "--cacert"])
            .arg(&self.pem)
            .arg("--header")
            .arg(format!("Cookie: {SESSION_COOKIE}={cookie}"))
            .arg(format!("{ORIGIN}/api/1/events"))
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(errors)
            .spawn()
            .map_err(|err| format!("the console tests need the host's curl: {err}"))?;
        Ok(Events { child, path })
    }
}

impl Events {
    /// What the stream has said so far, headers and events, as text.
    pub fn text(&self) -> String {
        std::fs::read(&self.path)
            .map_or_default(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Waits up to `seconds` for the stream to tell the `change` event of
    /// the journal's row `id`, and gives its data.
    pub fn change(&self, id: i64, seconds: u64) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let text = self.text();
            if let Some(event) = change_events(&text).into_iter().find(|event| event["id"] == id) {
                return Ok(event);
            }
            if Instant::now() >= deadline {
                return Err(format!("the stream did not tell row {id} in {seconds} s: {text:?}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The data of each `change` event in `text`, which is what a stream said:
/// its headers, then `event: NAME` and `data: JSON` lines, with a blank
/// line after each event and a comment, `: keep`, now and then. An event
/// whose data has not come, or is not JSON, is left out.
pub fn change_events(text: &str) -> Vec<Value> {
    let mut named = None;
    let mut found = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("event: ") {
            named = Some(name);
        } else if let Some(data) = line.strip_prefix("data: ") {
            if named == Some("change")
                && let Ok(event) = serde_json::from_str(data)
            {
                found.push(event);
            }
            named = None;
        }
    }
    found
}

/// What a browser keeps of the console: the session's cookie and the token
/// its page reads from the session.
#[derive(Default)]
pub struct Browser {
    pub cookie: Option<String>,
    pub token: Option<String>,
}

impl Browser {
    /// The headers that the console's own page sends with a change.
    pub fn page_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Origin".into(), ORIGIN.into()),
            ("Content-Type".into(), "application/json".into()),
        ];
        if let Some(cookie) = &self.cookie {
            headers.push(("Cookie".into(), format!("{SESSION_COOKIE}={cookie}")));
        }
        if let Some(token) = &self.token {
            headers.push((CSRF_HEADER.into(), token.clone()));
        }
        headers
    }

    /// The headers that the console's own page sends to open the Terminal's
    /// socket: its origin, its cookie, a request from the same site, and
    /// the token in the protocols, as a page cannot set a header of its own
    /// on a socket.
    pub fn socket_headers(&self) -> Headers {
        let mut headers =
            Headers::default().with("Origin", ORIGIN).with("Sec-Fetch-Site", "same-origin");
        if let Some(cookie) = &self.cookie {
            headers = headers.with("Cookie", &format!("{SESSION_COOKIE}={cookie}"));
        }
        let protocols = match &self.token {
            Some(token) => format!("{PROTOCOL}, csrf.{token}"),
            None => PROTOCOL.to_string(),
        };
        headers.with("Sec-WebSocket-Protocol", &protocols)
    }

    /// Takes what `reply` tells a browser to keep: the session's cookie, or
    /// to forget it.
    pub fn keep(&mut self, reply: &Reply) {
        for cookie in reply.headers_named("set-cookie").filter_map(parse_cookie) {
            if cookie.name == SESSION_COOKIE {
                self.cookie = Some(cookie.value).filter(|value| !value.is_empty());
                if self.cookie.is_none() {
                    self.token = None;
                }
            }
        }
    }
}

/// What the console answered.
pub struct Reply {
    pub status: u16,
    /// Names in lower case, in the order they came.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    /// The first header called `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    /// Every header called `name`.
    pub fn headers_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers.iter().filter(move |(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    /// The body as JSON.
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_str(&self.body)
            .map_err(|err| format!("the answer {} {:?} is not JSON: {err}", self.status, self.body))
    }

    /// The error's own words, if the body is the console's error.
    pub fn detail(&self) -> String {
        self.json()
            .ok()
            .and_then(|body| body["error"]["detail"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// This reply if it has `status`; otherwise an error that says what
    /// `what` got.
    pub fn must(self, what: &str, status: u16) -> Result<Reply, String> {
        if self.status == status {
            return Ok(self);
        }
        Err(format!("{what} was answered {} {}, not {status}", self.status, self.body))
    }

    /// This reply if it is the console's error `id` with `status`.
    pub fn refused(self, what: &str, status: u16, id: &str) -> Result<Reply, String> {
        let said =
            self.json().ok().and_then(|body| body["error"]["id"].as_str().map(str::to_string));
        if self.status == status && said.as_deref() == Some(id) {
            return Ok(self);
        }
        Err(format!("{what} was answered {} {}, not {id} with {status}", self.status, self.body))
    }
}

/// The host, as the guest's network sees it.
const HOST: &str = "10.0.2.2";

/// What the guest's pane says to the host: nc, at the end of a typed line,
/// sends what the line's command printed to a port on the host's loopback,
/// where QEMU's user network lets the guest reach it at 10.0.2.2. The
/// screen would have to hold it, and cannot hold a list of services.
pub struct Pane {
    listener: TcpListener,
    port: u16,
}

impl Pane {
    pub fn open() -> Result<Pane, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|err| format!("no port for what the pane says: {err}"))?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        listener.set_nonblocking(true).map_err(|err| err.to_string())?;
        Ok(Pane { listener, port })
    }

    /// What follows a pipe to send the output to the host: nc half-closes
    /// the connection at the end of its input, and ends once the host has
    /// closed its side.
    pub fn sink(&self) -> String {
        format!("nc -N {HOST} {}", self.port)
    }

    /// Takes what the guest sends over one connection, waiting up to
    /// `seconds` for the connection and up to 30 for its end.
    pub fn take(&self, seconds: u64) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).map_err(|err| err.to_string())?;
                    stream
                        .set_read_timeout(Some(Duration::from_secs(30)))
                        .map_err(|err| err.to_string())?;
                    let mut bytes = Vec::new();
                    stream
                        .read_to_end(&mut bytes)
                        .map_err(|err| format!("what the pane sent: {err}"))?;
                    return Ok(String::from_utf8_lossy(&bytes).into_owned());
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "the pane sent nothing to port {} in {seconds} s",
                            self.port
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(format!("taking what the pane sends: {err}")),
            }
        }
    }

    /// Types `command` in the pane, and returns what it printed.
    pub fn output(&self, vm: &mut Vm, command: &str) -> Result<String, String> {
        answer_prompt(vm, "$", &format!("{command} | {}", self.sink()))?;
        self.take(120)
    }

    /// Types `command` in the pane, and returns the status it ended with.
    pub fn status(&self, vm: &mut Vm, command: &str) -> Result<i32, String> {
        answer_prompt(vm, "$", &format!("{command}; echo \"ran $?\" | {}", self.sink()))?;
        let said = self.take(120)?;
        ran_status(&said).ok_or_else(|| format!("`{command}` was followed by {said:?}"))
    }
}

/// The protocol the Terminal's socket speaks, which the page names.
const PROTOCOL: &str = "dukovo-terminal-1";

/// The WebSocket client, which the test writes to the machine's folder and
/// starts with `python3 -I`.
const CLIENT: &str = include_str!("websocket_client.py");

/// The clients started in this run, to give each a file for its errors.
static CLIENTS: AtomicUsize = AtomicUsize::new(0);

/// The seconds, besides the ones a command asks for, that the client has to
/// answer it.
const GRACE: u64 = 10;

/// The headers of a WebSocket handshake that a test gives. The client
/// writes Host, Connection, Upgrade, Sec-WebSocket-Version and a key of
/// its own; a header given here with the same name replaces that one, and
/// a header [left out](Headers::without) is not sent, so that a test can
/// open a socket without Upgrade or with another version.
#[derive(Clone, Default)]
pub struct Headers(Vec<(String, Option<String>)>);

impl Headers {
    /// These headers with `name` as `value`, in place of any header of that
    /// name.
    #[must_use]
    pub fn with(mut self, name: &str, value: &str) -> Headers {
        self.put(name, Some(value.to_string()));
        self
    }

    /// These headers with no `name`: none given, and none of the client's
    /// own.
    #[must_use]
    pub fn without(mut self, name: &str) -> Headers {
        self.put(name, None);
        self
    }

    /// The value given for `name`, if one is.
    #[cfg(test)]
    pub fn value(&self, name: &str) -> Option<&str> {
        let (_, value) = self.0.iter().find(|(have, _)| have.eq_ignore_ascii_case(name))?;
        value.as_deref()
    }

    fn put(&mut self, name: &str, value: Option<String>) {
        match self.0.iter_mut().find(|(have, _)| have.eq_ignore_ascii_case(name)) {
            Some(held) => *held = (name.to_string(), value),
            None => self.0.push((name.to_string(), value)),
        }
    }

    fn json(&self) -> Value {
        Value::Array(self.0.iter().map(|(name, value)| json!([name, value])).collect())
    }
}

/// What a wait of the client came to.
#[derive(Debug)]
pub struct Waited {
    /// What it looked for, as it found it, or null.
    pub found: Value,
    /// Whether the time ran out.
    pub timed_out: bool,
    /// The close frame's code if one came, 0 if the connection ended with
    /// none.
    pub closed: Option<i64>,
    /// The bytes of each pane's output read so far, by the pane's number.
    pub lengths: BTreeMap<u64, u64>,
}

impl Waited {
    /// The bytes of pane `pane`'s output read so far.
    pub fn length(&self, pane: u64) -> u64 {
        self.lengths.get(&pane).copied().unwrap_or(0)
    }

    /// Reads the client's answer to a wait.
    fn of(answer: &Value) -> Waited {
        let lengths = answer["lengths"].as_object().map_or_else(BTreeMap::new, |lengths| {
            lengths
                .iter()
                .filter_map(|(pane, bytes)| Some((pane.parse().ok()?, bytes.as_u64()?)))
                .collect()
        });
        Waited {
            found: answer["found"].clone(),
            timed_out: answer["timed_out"].as_bool().unwrap_or(false),
            closed: answer["closed"].as_i64(),
            lengths,
        }
    }
}

/// All the output the client read of one pane.
#[derive(Debug)]
pub struct Held {
    /// The output, decoded as UTF-8 with replacement.
    pub text: String,
    /// Its length in bytes.
    pub length: u64,
    /// The offset of its first byte, which an `attached` message gave: none
    /// when nothing has been read.
    pub first: Option<u64>,
}

impl Held {
    /// The offset just past the last byte read.
    pub fn end(&self) -> u64 {
        self.first.unwrap_or(0) + self.length
    }
}

/// One WebSocket to the Terminal's route, which `websocket_client.py` holds
/// and this drives: each command is a line to the client's standard input
/// and each answer a line of its output, which a thread reads, so that an
/// answer that does not come is a timeout and not a hang.
pub struct Socket {
    child: Child,
    commands: Option<ChildStdin>,
    answers: Receiver<String>,
    /// Where the client's standard error went.
    errors: PathBuf,
    /// The messages of the console that waits have read and none has asked
    /// for yet, oldest first.
    said: Vec<Value>,
}

impl Socket {
    /// Starts a client and has it open the console's socket with the
    /// handshake `headers`. The answer is the client's: the 101, or the
    /// console's refusal with its status and body.
    pub fn open(vm: &Vm, console: &Console, headers: &Headers) -> Result<(Socket, Value), String> {
        let script = vm.dir().join("websocket_client.py");
        if std::fs::read_to_string(&script).ok().as_deref() != Some(CLIENT) {
            std::fs::write(&script, CLIENT)
                .map_err(|err| format!("writing {}: {err}", script.display()))?;
        }
        let number = CLIENTS.fetch_add(1, Ordering::Relaxed);
        let errors = vm.dir().join(format!("websocket-client-{number}.err"));
        let mut command = Command::new("python3");
        command.arg("-I").arg(&script);
        let mut socket = Socket::start(command, errors)?;
        // The client waits up to a minute each for the connection and for
        // the answer's head: the machine's TLS is slow without KVM.
        let answer = socket.ask(&open_command(console.port, &console.pem, headers), 120)?;
        Ok((socket, answer))
    }

    /// Starts `command` as a client, with its standard error in the file
    /// `errors`.
    fn start(mut command: Command, errors: PathBuf) -> Result<Socket, String> {
        let log = File::create(&errors).map_err(|err| format!("{}: {err}", errors.display()))?;
        let mut child =
            command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(log).spawn().map_err(
                |err| format!("the console's boot tests need Python 3 (ob doctor): {err}"),
            )?;
        let commands = child.stdin.take();
        let stdout = child.stdout.take().ok_or("the client has no standard output")?;
        let (sender, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Socket { child, commands, answers, errors, said: Vec::new() })
    }

    /// Writes `command` to the client and waits up to `seconds` and ten
    /// more for its answer. The client's `{"error": ...}` is an error here,
    /// and so is a client that ended: with its status and what it said.
    pub fn ask(&mut self, command: &Value, seconds: u64) -> Result<Value, String> {
        let what = brief(command);
        let written = match self.commands.as_mut() {
            Some(commands) => writeln!(commands, "{command}").and_then(|()| commands.flush()),
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        };
        if written.is_err() {
            return Err(self.gone(&what));
        }
        match self.answers.recv_timeout(Duration::from_secs(seconds + GRACE)) {
            Ok(line) => {
                let answer: Value = serde_json::from_str(&line).map_err(|err| {
                    format!("the client answered {what} with {line:.200}, which is no JSON: {err}")
                })?;
                match answer.get("error") {
                    None => Ok(answer),
                    Some(error) => {
                        let error =
                            error.as_str().map_or_else(|| error.to_string(), str::to_string);
                        Err(format!("{what}: {error}"))
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                Err(format!("the client did not answer {what} in {} s", seconds + GRACE))
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.gone(&what)),
        }
    }

    /// Says how the client ended, for the command `what` that found it so:
    /// its exit status, and what it wrote to its standard error.
    fn gone(&mut self, what: &str) -> String {
        self.commands = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status.to_string(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => break "still running".to_string(),
                Err(err) => break err.to_string(),
            }
        };
        let said = std::fs::read_to_string(&self.errors).unwrap_or_default();
        let said = said.trim();
        let said = if said.is_empty() { String::new() } else { format!(": {said}") };
        format!("the client ended ({status}) at {what}{said}")
    }

    /// Sends a command that writes to the console. A write that could not
    /// be made is an error only when `strict`: a test that makes the
    /// console close the socket races the write against the close.
    fn send(&mut self, command: &Value, strict: bool) -> Result<(), String> {
        let what = brief(command);
        let answer = self.ask(command, 10)?;
        match answer["send_error"].as_str() {
            Some(why) if strict => Err(format!("{what} was not sent: {why}")),
            _ => Ok(()),
        }
    }

    /// Sends `message` as JSON in a text frame.
    pub fn text(&mut self, message: &Value) -> Result<(), String> {
        self.send(&json!({"text": message}), true)
    }

    /// Sends `text`, as it is, in a text frame.
    pub fn text_raw(&mut self, text: &str) -> Result<(), String> {
        self.send(&json!({"text_raw": text}), true)
    }

    /// Types `keys` in pane `pane`: a binary frame.
    pub fn type_in(&mut self, pane: u64, keys: &str) -> Result<(), String> {
        self.send(&json!({"type": {"pane": pane, "keys": keys}}), true)
    }

    /// Writes `bytes` to the connection as they are, which the console may
    /// answer by closing it before the last is written.
    pub fn frame(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.send(&json!({"frame": {"hex": hex(bytes)}}), false)
    }

    /// Sends a ping with `words`.
    pub fn ping(&mut self, words: &str) -> Result<(), String> {
        self.send(&json!({"ping": words}), true)
    }

    /// Sends a close frame with `code`, which the console may have sent its
    /// own for already.
    pub fn close(&mut self, code: u16) -> Result<(), String> {
        self.send(&json!({"close": code}), false)
    }

    /// Turns the acknowledging of output on or off.
    pub fn acks(&mut self, on: bool) -> Result<(), String> {
        self.ask(&json!({"acks": on}), 10).map(|_| ())
    }

    /// Waits up to `seconds` for what `kind` asks for with `wanted`, as the
    /// client's `wait` takes them. The messages it reads are kept for
    /// [`Socket::wait_said`].
    pub fn wait(&mut self, kind: &str, wanted: Value, seconds: u64) -> Result<Waited, String> {
        // A map, since `json!` takes no variable for a key.
        let mut spec = serde_json::Map::new();
        spec.insert("seconds".into(), json!(seconds));
        spec.insert(kind.into(), wanted);
        let answer = self.ask(&json!({"wait": spec}), seconds)?;
        if let Some(read) = answer["said"].as_array() {
            self.said.extend(read.iter().cloned());
        }
        Ok(Waited::of(&answer))
    }

    /// The oldest message kept that has the key `key`, taken out.
    fn take_said(&mut self, key: &str) -> Option<Value> {
        let at = self.said.iter().position(|said| said.get(key).is_some())?;
        Some(self.said.remove(at))
    }

    /// Forgets the messages kept that have the key `key`.
    pub fn forget_said(&mut self, key: &str) {
        self.said.retain(|said| said.get(key).is_none());
    }

    /// Waits up to `seconds` for a message of the console that has the key
    /// `key`, and gives it. A message that came while an earlier wait read
    /// is there, too.
    pub fn wait_said(&mut self, key: &str, seconds: u64) -> Result<Value, String> {
        if let Some(found) = self.take_said(key) {
            return Ok(found);
        }
        let waited = self.wait("said", json!(key), seconds)?;
        match self.take_said(key) {
            Some(found) => Ok(found),
            None => Err(lacking(&format!("a message with {key:?}"), seconds, &waited, &self.said)),
        }
    }

    /// Waits up to `seconds` for the output of pane `pane` to hold `needle`.
    pub fn wait_output(&mut self, pane: u64, needle: &str, seconds: u64) -> Result<(), String> {
        let waited = self.wait("output", json!([pane, needle]), seconds)?;
        if !waited.found.is_null() {
            return Ok(());
        }
        let held = self.output(pane)?;
        let what = format!("{needle:?} in the output of pane {pane}");
        let mut why = lacking(&what, seconds, &waited, &self.said);
        why.push_str(&format!("; the output ends with {:?}", tail(&held.text, 300)));
        Err(why)
    }

    /// Waits up to `seconds` for pane `pane` to have output of `count`
    /// bytes or more.
    pub fn wait_bytes(&mut self, pane: u64, count: u64, seconds: u64) -> Result<Waited, String> {
        let waited = self.wait("bytes", json!([pane, count]), seconds)?;
        if waited.found.is_null() {
            let what = format!("{count} bytes of output of pane {pane}");
            return Err(lacking(&what, seconds, &waited, &self.said));
        }
        Ok(waited)
    }

    /// Reads for `seconds` and finds nothing to wait for: what is read in
    /// that time is in the pane's lengths.
    pub fn wait_quiet(&mut self, seconds: u64) -> Result<Waited, String> {
        self.wait("quiet", json!(true), seconds)
    }

    /// Waits up to `seconds` for the pong of a ping with `words`.
    pub fn wait_pong(&mut self, words: &str, seconds: u64) -> Result<(), String> {
        let waited = self.wait("pong", json!(words), seconds)?;
        if waited.found.is_null() {
            return Err(lacking(&format!("a pong of {words:?}"), seconds, &waited, &self.said));
        }
        Ok(())
    }

    /// Waits up to `seconds` for a close frame, or the end of the
    /// connection, and gives the frame's code: 0 when the connection ended
    /// with none.
    pub fn wait_closed(&mut self, seconds: u64) -> Result<i64, String> {
        let waited = self.wait("closed", json!(true), seconds)?;
        match waited.closed {
            Some(code) => Ok(code),
            None => Err(lacking("the socket's close", seconds, &waited, &self.said)),
        }
    }

    /// All the output the client read of pane `pane`.
    pub fn output(&mut self, pane: u64) -> Result<Held, String> {
        let answer = self.ask(&json!({"output": pane}), 30)?;
        let text = answer["text"].as_str().ok_or_else(|| format!("no text in {answer}"))?;
        let length = answer["length"].as_u64().ok_or_else(|| format!("no length in {answer}"))?;
        Ok(Held { text: text.to_string(), length, first: answer["first"].as_u64() })
    }

    /// Attaches to pane `pane` with the size `cols` by `rows`, from after
    /// the output at offset `since` if one is given, and gives the console's
    /// `attached` message.
    pub fn attach(
        &mut self,
        pane: u64,
        (cols, rows): (u32, u32),
        since: Option<u64>,
    ) -> Result<Value, String> {
        // An earlier attach's answer is not this one's.
        self.forget_said("attached");
        let mut message = json!({"attach": pane, "cols": cols, "rows": rows});
        if let Some(since) = since {
            message["since"] = json!(since);
        }
        self.text(&message)?;
        self.wait_said("attached", 30)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(mut commands) = self.commands.take() {
            // A client that has ended has no one to tell; its end of input
            // is a quit as well.
            let _ = writeln!(commands, "{{\"quit\":true}}");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Not ended in 2 s: it is stopped. It may have ended just now, and
        // then there is nothing to stop.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The client's `open`: the host's port of the console, the one certificate
/// to trust and the headers to give.
fn open_command(port: u16, cafile: &Path, headers: &Headers) -> Value {
    json!({"open": {
        "port": port,
        "cafile": cafile,
        "host": format!("localhost:{PORT}"),
        "path": "/api/1/terminal",
        "headers": headers.json(),
    }})
}

/// `command` as a line of a message: at most 120 characters of it.
fn brief(command: &Value) -> String {
    let text = command.to_string();
    if text.chars().count() <= 120 {
        return text;
    }
    let head: String = text.chars().take(120).collect();
    format!("{head}... ({} bytes)", text.len())
}

/// The last `count` characters of `text`.
pub fn tail(text: &str, count: usize) -> &str {
    let skip = text.chars().count().saturating_sub(count);
    text.char_indices().nth(skip).map_or("", |(at, _)| &text[at..])
}

/// The bytes as lower-case hexadecimal, which the client's `frame` takes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing to a string does not fail.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// What an error says of a wait that did not find `what` in `seconds`: why
/// it stopped, what the console said on its way, and how much each pane
/// had given.
fn lacking(what: &str, seconds: u64, waited: &Waited, said: &[Value]) -> String {
    let mut why = if waited.timed_out {
        format!("{what} did not come in {seconds} s")
    } else {
        format!("{what} did not come before the socket closed")
    };
    match waited.closed {
        Some(0) => why.push_str("; the connection ended with no close frame"),
        Some(code) => why.push_str(&format!("; the console closed the socket with {code}")),
        None => {}
    }
    if !said.is_empty() {
        why.push_str(&format!("; its other messages were {}", brief(&Value::from(said))));
    }
    if !waited.lengths.is_empty() {
        why.push_str(&format!("; the bytes read of each pane are {:?}", waited.lengths));
    }
    why
}

/// The status in `ran N`, as `Pane::status` has the pane say it.
pub fn ran_status(text: &str) -> Option<i32> {
    text.lines().find_map(|line| line.trim().strip_prefix("ran ")?.trim().parse().ok())
}

// Helpers that read and compute, with no machine to ask: the unit tests
// below run them alone.

/// The bytes of standard base64 `text`, which may have padding and
/// whitespace.
pub fn from_base64(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for letter in text.bytes().filter(|&byte| !byte.is_ascii_whitespace() && byte != b'=') {
        let value = match letter {
            b'A'..=b'Z' => letter - b'A',
            b'a'..=b'z' => letter - b'a' + 26,
            b'0'..=b'9' => letter - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = buffer << 6 | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(bytes)
}

/// The first certificate in `text`, the way curl's `%{certs}` prints a
/// chain with details around the PEM: the PEM as every program reads it,
/// and its DER.
pub fn first_certificate(text: &str) -> Option<(String, Vec<u8>)> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let from = text.find(BEGIN)? + BEGIN.len();
    let body = &text[from..from + text[from..].find(END)?];
    let der = from_base64(body)?;
    let compact: String = body.split_whitespace().collect();
    let mut pem = format!("{BEGIN}\n");
    for line in compact.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).ok()?);
        pem.push('\n');
    }
    pem.push_str(END);
    pem.push('\n');
    Some((pem, der))
}

/// A certificate's SHA-256 fingerprint as `cert show` and the console
/// write it: upper-case bytes with colons.
pub fn fingerprint_of(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
}

/// A `Set-Cookie` value: the cookie and its attributes as written.
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub attributes: Vec<String>,
}

pub fn parse_cookie(text: &str) -> Option<Cookie> {
    let mut parts = text.split("; ");
    let (name, value) = parts.next()?.split_once('=')?;
    Some(Cookie {
        name: name.to_string(),
        value: value.to_string(),
        attributes: parts.map(str::to_string).collect(),
    })
}

/// A reply as `curl --include` prints it: the status, the headers with
/// lower-case names, and the body. An interim reply (1xx) before it is
/// passed over.
pub fn parse_reply(raw: &[u8]) -> Result<Reply, String> {
    let mut rest = raw;
    loop {
        let end = rest
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| format!("no end of the head in {:?}", String::from_utf8_lossy(raw)))?;
        let head = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 4..];
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| format!("no status in {head:?}"))?;
        if !(100..200).contains(&status) {
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
                .collect();
            return Ok(Reply { status, headers, body: String::from_utf8_lossy(rest).into_owned() });
        }
    }
}

/// Unix seconds of an HTTP date, `Sun, 06 Nov 1994 08:49:37 GMT`.
pub fn http_date(text: &str) -> Option<u64> {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let words: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = words[..] else { return None };
    let month = i64::try_from(MONTHS.iter().position(|&name| name == month)?).ok()? + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let clock: Vec<u64> = time.split(':').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [hours, minutes, seconds] = clock[..] else { return None };
    // Days since 1970 by the calendar of the proleptic Gregorian years,
    // counted from March, as Howard Hinnant writes it.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    u64::try_from(days)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(hours * 3600 + minutes * 60 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_change_events_of_a_stream_are_read_from_its_text() {
        let text = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                    event: unlock\ndata: {\"unlock_seconds\":null}\n\n\
                    : keep\n\n\
                    event: change\ndata: {\"at\":1790000000,\"how\":\"console\",\"id\":7}\n\n\
                    event: task\ndata: {\"id\":7,\"state\":\"running\"}\n\n\
                    event: change\ndata: {\"at\":1790000030,\"how\":\"by hand\",\"id\":8}\n\n\
                    event: change\ndata: not json\n\n\
                    event: change\n";
        let found = change_events(text);
        assert_eq!(
            found,
            [
                json!({"at": 1790000000, "how": "console", "id": 7}),
                json!({"at": 1790000030, "how": "by hand", "id": 8}),
            ]
        );
        assert!(change_events("").is_empty());
        // Another event's data is not a change's, even if it is JSON.
        assert!(change_events("event: task\ndata: {\"id\":1}\n\n").is_empty());
        assert!(change_events("data: {\"id\":1}\n\n").is_empty());
    }

    #[test]
    fn what_the_pane_says_is_read_back() {
        assert_eq!(ran_status("ran 0\n"), Some(0));
        assert_eq!(ran_status("ran 3"), Some(3));
        assert_eq!(ran_status("noise\nran 1\n"), Some(1));
        assert_eq!(ran_status(""), None);
    }

    #[test]
    fn base64_reads_the_rfc_4648_vectors_with_or_without_padding() {
        for (text, want) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9v\nYmE=\n", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(from_base64(text).as_deref(), Some(want.as_bytes()), "{text:?}");
        }
        assert_eq!(from_base64("Zm9v!"), None);
    }

    #[test]
    fn the_certificate_is_taken_from_what_curl_prints_and_fingerprinted_as_cert_show_does() {
        // Bytes 1, 2, 3 are no certificate, and need not be: only the
        // base64 and the digest are read.
        let printed = "Subject:CN=duke-nas\nSignature Algorithm:ecdsa\n\
                       Cert:-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n\
                       Subject:CN=another\nCert:-----BEGIN CERTIFICATE-----\nBAUG\n\
                       -----END CERTIFICATE-----\n";
        let (pem, der) = first_certificate(printed).unwrap();
        assert_eq!(pem, "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n");
        assert_eq!(der, [1, 2, 3]);
        assert_eq!(
            fingerprint_of(&der),
            "03:90:58:C6:F2:C0:CB:49:2C:53:3B:0A:4D:14:EF:77:CC:0F:78:AB:CC:CE:D5:28:7D:84:A1:A2:01:1C:FB:81"
        );
        // A body of more than 64 characters is written again in lines of 64.
        let long = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n{}\n-----END CERTIFICATE-----",
            "A".repeat(80),
            "A".repeat(8)
        );
        let (pem, der) = first_certificate(&long).unwrap();
        let lines: Vec<&str> = pem.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!((lines[1].len(), lines[2].len()), (64, 24));
        assert_eq!(der.len(), 66);
        assert!(first_certificate("no certificate here").is_none());
        assert!(first_certificate("-----BEGIN CERTIFICATE-----\nAQID").is_none());
    }

    #[test]
    fn a_reply_is_read_from_curls_output_past_an_interim_one() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\n\
                    Set-Cookie: a=1; Path=/\r\nset-cookie: b=; Max-Age=0\r\nRetry-After: 7\r\n\r\n\
                    {\"error\":{\"id\":\"ADM-109\",\"detail\":\"no\\r\\n\\r\\nmore\"}}";
        let reply = parse_reply(raw).unwrap();
        assert_eq!(reply.status, 403);
        assert_eq!(reply.header("content-type"), Some("application/json"));
        assert_eq!(reply.header("retry-after"), Some("7"));
        assert_eq!(
            reply.headers_named("set-cookie").collect::<Vec<_>>(),
            ["a=1; Path=/", "b=; Max-Age=0"]
        );
        assert_eq!(reply.header("server"), None);
        assert_eq!(reply.json().unwrap()["error"]["id"], "ADM-109");
        assert_eq!(reply.detail(), "no\r\n\r\nmore");
        assert!(reply.refused("x", 403, "ADM-109").is_ok());
        let reply = parse_reply(b"HTTP/1.1 200 OK\r\n\r\nbody\r\n\r\nwith a blank line").unwrap();
        assert_eq!(reply.body, "body\r\n\r\nwith a blank line");
        assert!(reply.must("x", 200).is_ok());
        assert!(parse_reply(b"HTTP/1.1 200 OK\r\nno end").is_err());
        assert!(parse_reply(b"not http\r\n\r\n").is_err());
        assert!(parse_reply(b"").is_err());
    }

    #[test]
    fn a_reply_with_another_id_or_status_is_not_the_refusal_asked_for() {
        let raw = b"HTTP/1.1 401 Unauthorized\r\n\r\n{\"error\":{\"id\":\"ADM-101\"}}";
        assert!(parse_reply(raw).unwrap().refused("x", 401, "ADM-102").is_err());
        assert!(parse_reply(raw).unwrap().refused("x", 403, "ADM-101").is_err());
        assert!(parse_reply(raw).unwrap().must("x", 200).is_err());
        let not_json = parse_reply(b"HTTP/1.1 401 Unauthorized\r\n\r\nplain").unwrap();
        assert!(not_json.refused("x", 401, "ADM-101").is_err());
    }

    #[test]
    fn the_sessions_cookie_reads_with_its_attributes_as_written() {
        let cookie =
            parse_cookie("__Host-dukovo=abc-_9; Path=/; Secure; HttpOnly; SameSite=Strict")
                .unwrap();
        assert_eq!((cookie.name.as_str(), cookie.value.as_str()), ("__Host-dukovo", "abc-_9"));
        assert_eq!(cookie.attributes, ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"]);
        let cleared =
            parse_cookie("__Host-dukovo=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0")
                .unwrap();
        assert!(cleared.value.is_empty());
        assert_eq!(cleared.attributes.last().map(String::as_str), Some("Max-Age=0"));
        assert!(parse_cookie("no equals sign").is_none());
    }

    #[test]
    fn a_browser_keeps_the_sessions_cookie_until_it_is_cleared() {
        let reply = |cookie: &str| Reply {
            status: 200,
            headers: vec![("set-cookie".into(), cookie.into())],
            body: String::new(),
        };
        let mut browser = Browser::default();
        browser.keep(&reply("__Host-dukovo=one; Path=/; Secure"));
        browser.token = Some("token".into());
        assert_eq!(browser.cookie.as_deref(), Some("one"));
        // Another cookie of the console's is not the session's.
        browser.keep(&reply("__Host-dukovo-trust=two; Path=/"));
        assert_eq!(browser.cookie.as_deref(), Some("one"));
        browser.keep(&reply("__Host-dukovo=; Path=/; Max-Age=0"));
        assert_eq!((browser.cookie, browser.token), (None, None));
    }

    #[test]
    fn http_dates_are_read_to_the_second() {
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        // A leap day, and the day after.
        assert_eq!(http_date("Tue, 29 Feb 2000 23:59:59 GMT"), Some(951_868_799));
        assert_eq!(http_date("Wed, 01 Mar 2000 00:00:00 GMT"), Some(951_868_800));
        assert_eq!(http_date("Sat, 14 Nov 2026 10:00:00 GMT"), Some(1_794_650_400));
        for wrong in [
            "",
            "yesterday",
            "Sun, 06 Nov 1994 08:49:37",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49 GMT",
        ] {
            assert_eq!(http_date(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn handshake_headers_are_replaced_by_name_and_left_out_with_none() {
        let headers = Headers::default()
            .with("Origin", ORIGIN)
            .with("Upgrade", "websocket")
            .with("origin", "https://other.example")
            .without("UPGRADE");
        assert_eq!(headers.value("ORIGIN"), Some("https://other.example"));
        assert_eq!(headers.value("Upgrade"), None);
        assert_eq!(headers.value("Cookie"), None);
        // Two headers, the one of each name once; a left out one is null.
        assert_eq!(headers.json(), json!([["origin", "https://other.example"], ["UPGRADE", null]]));
    }

    #[test]
    fn the_page_opens_the_socket_with_its_origin_cookie_and_token() {
        let browser = Browser { cookie: Some("one".into()), token: Some("tok_en-1".into()) };
        let headers = browser.socket_headers();
        assert_eq!(headers.value("Origin"), Some(ORIGIN));
        assert_eq!(headers.value("Cookie"), Some("__Host-dukovo=one"));
        assert_eq!(headers.value("Sec-Fetch-Site"), Some("same-origin"));
        assert_eq!(
            headers.value("Sec-WebSocket-Protocol"),
            Some("dukovo-terminal-1, csrf.tok_en-1")
        );
        // A browser that has signed in to nothing has neither.
        let headers = Browser::default().socket_headers();
        assert_eq!(headers.value("Cookie"), None);
        assert_eq!(headers.value("Sec-WebSocket-Protocol"), Some("dukovo-terminal-1"));
    }

    #[test]
    fn the_open_command_names_the_port_the_certificate_and_the_headers() {
        let headers = Headers::default().with("Origin", ORIGIN).without("Upgrade");
        let open = open_command(45678, Path::new("/tmp/admind.pem"), &headers);
        assert_eq!(
            open,
            json!({"open": {
                "port": 45678,
                "cafile": "/tmp/admind.pem",
                "host": "localhost:9090",
                "path": "/api/1/terminal",
                "headers": [["Origin", ORIGIN], ["Upgrade", null]],
            }})
        );
    }

    #[test]
    fn long_commands_are_cut_in_messages_and_the_tail_of_a_text_is_whole_characters() {
        assert_eq!(brief(&json!({"text": "short"})), r#"{"text":"short"}"#);
        let long = brief(&json!({"frame": {"hex": "ab".repeat(500)}}));
        assert!(long.starts_with(r#"{"frame":{"hex":"abab"#), "{long}");
        assert!(long.ends_with("... (1020 bytes)"), "{long}");
        assert_eq!(long.chars().count(), 120 + "... (1020 bytes)".len());
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("", 3), "");
        // A character of two bytes is not cut in half.
        assert_eq!(tail("a\u{e9}\u{e9}", 2), "\u{e9}\u{e9}");
        assert_eq!(hex(&[0x81, 0x02, 0x7b, 0x00, 0xff]), "81027b00ff");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn a_wait_that_finds_nothing_says_what_came_instead() {
        let mut waited = Waited {
            found: Value::Null,
            timed_out: true,
            closed: None,
            lengths: BTreeMap::from([(3, 17)]),
        };
        let said = [json!({"changed": 1})];
        assert_eq!(
            lacking("a pong of \"x\"", 5, &waited, &said),
            "a pong of \"x\" did not come in 5 s; its other messages were [{\"changed\":1}]; \
             the bytes read of each pane are {3: 17}"
        );
        assert_eq!(waited.length(3), 17);
        assert_eq!(waited.length(4), 0);
        waited.timed_out = false;
        waited.closed = Some(1001);
        assert_eq!(
            lacking("a message", 5, &waited, &[]),
            "a message did not come before the socket closed; the console closed the socket \
             with 1001; the bytes read of each pane are {3: 17}"
        );
        waited.closed = Some(0);
        waited.lengths.clear();
        assert_eq!(
            lacking("a message", 5, &waited, &[]),
            "a message did not come before the socket closed; the connection ended with no \
             close frame"
        );
    }

    #[test]
    fn the_answer_to_a_wait_is_read() {
        let waited = Waited::of(&json!({
            "found": {"attached": 3},
            "timed_out": false,
            "said": [],
            "closed": 1000,
            "lengths": {"3": 5, "12": 40},
        }));
        assert_eq!(waited.found, json!({"attached": 3}));
        assert!(!waited.timed_out);
        assert_eq!(waited.closed, Some(1000));
        assert_eq!((waited.length(3), waited.length(12)), (5, 40));
        let empty = Waited::of(&json!({"found": null, "timed_out": true, "closed": null}));
        assert!(empty.found.is_null() && empty.timed_out && empty.lengths.is_empty());
        assert_eq!(empty.closed, None);
    }

    #[test]
    fn what_a_pane_held_ends_where_its_last_byte_was() {
        let held = Held { text: "abc".into(), length: 3, first: Some(5000) };
        assert_eq!(held.end(), 5003);
        assert_eq!(Held { text: String::new(), length: 0, first: None }.end(), 0);
    }

    /// Whether the host has Python 3; a test that needs it prints a note
    /// and passes where it has none, as the test of `ob doctor` does for
    /// what it cannot find.
    fn has_python() -> bool {
        let found = Command::new("python3").arg("--version").output().is_ok();
        if !found {
            eprintln!("skipped: python3 is not on this host");
        }
        found
    }

    /// The client's own check of its frames, the accept key and its
    /// commands runs, and passes.
    #[test]
    fn the_websocket_client_passes_its_own_checks() {
        if !has_python() {
            return;
        }
        let script =
            std::env::temp_dir().join(format!("dukovo-websocket-client-{}.py", std::process::id()));
        std::fs::write(&script, CLIENT).unwrap();
        let out = Command::new("python3").arg("-I").arg(&script).arg("--self-test").output();
        std::fs::remove_file(&script).unwrap();
        let out = out.unwrap();
        assert!(
            out.status.success(),
            "the client's self-test ended with {}: {}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// What a client does, as far as the plumbing sees it: this answers
    /// each command by its name.
    const STAND_IN: &str = r#"
import json, sys, time
for line in sys.stdin:
    (name, value), = json.loads(line).items()
    if name == "die":
        sys.stderr.write("it broke\n")
        sys.exit(3)
    elif name == "bad":
        answer = {"error": "that cannot be done"}
    elif name == "slip" or name == "frame":
        answer = {"ok": True, "send_error": "BrokenPipeError: gone"}
    elif name == "output":
        answer = {"text": "abc", "length": 3, "sha256": "", "first": 10}
    elif name == "wait":
        kind = [key for key in value if key != "seconds"][0]
        said, found, closed, timed_out = [], None, None, False
        if kind == "said" and value["said"] == "attached":
            said = [{"changed": 1}, {"attached": 3, "offset": 9}]
            found = said[1]
        elif kind == "quiet":
            said = [{"ended": 3, "why": "locked"}]
        elif kind == "closed":
            found = closed = 1001
        elif kind == "bytes" and value["bytes"][1] <= 7:
            found = 7
        else:
            timed_out = True
        answer = {"found": found, "timed_out": timed_out, "said": said,
                  "closed": closed, "lengths": {"3": 7}}
    else:
        answer = {"ok": True}
    print(json.dumps(answer), flush=True)
"#;

    /// A client that runs `program`, with its errors in a file of the test.
    fn stand_in(program: &str) -> Option<Socket> {
        static MADE: AtomicUsize = AtomicUsize::new(0);
        if !has_python() {
            return None;
        }
        let errors = std::env::temp_dir().join(format!(
            "dukovo-stand-in-{}-{}.err",
            std::process::id(),
            MADE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut command = Command::new("python3");
        command.args(["-I", "-c", program]);
        Some(Socket::start(command, errors).unwrap())
    }

    /// Ends a stand-in and takes its file of errors away.
    fn finish(socket: Socket) {
        let errors = socket.errors.clone();
        drop(socket);
        std::fs::remove_file(errors).unwrap();
    }

    #[test]
    fn a_client_error_is_an_error_naming_the_command_and_a_dead_client_gives_its_stderr() {
        let Some(mut socket) = stand_in(STAND_IN) else { return };
        assert_eq!(socket.ask(&json!({"ping": "x"}), 5).unwrap(), json!({"ok": true}));
        assert_eq!(
            socket.ask(&json!({"bad": 1}), 5).unwrap_err(),
            r#"{"bad":1}: that cannot be done"#
        );
        // The client is answering still after an error.
        assert!(socket.ask(&json!({"ping": "y"}), 5).is_ok());
        let ended = socket.ask(&json!({"die": 1}), 5).unwrap_err();
        assert!(ended.starts_with("the client ended (exit status: 3) at {\"die\":1}"), "{ended}");
        assert!(ended.ends_with(": it broke"), "{ended}");
        // And later commands find it so, too.
        let later = socket.ask(&json!({"ping": "z"}), 5).unwrap_err();
        assert!(later.contains("exit status: 3") && later.contains("it broke"), "{later}");
        finish(socket);
    }

    #[test]
    fn a_write_that_failed_is_an_error_only_where_the_test_needs_it_made() {
        let Some(mut socket) = stand_in(STAND_IN) else { return };
        socket.text(&json!({"detach": 1})).unwrap();
        socket.type_in(1, "ls\r").unwrap();
        // The stand-in says that a typed write failed; it names the write.
        let why = socket.send(&json!({"slip": 1}), true).unwrap_err();
        assert_eq!(why, r#"{"slip":1} was not sent: BrokenPipeError: gone"#);
        socket.send(&json!({"slip": 1}), false).unwrap();
        // A frame that provokes a close may fail to be written whole.
        socket.frame(&[0x81, 0x02, 0x7b, 0x7d]).unwrap();
        socket.close(1000).unwrap();
        socket.ping("words").unwrap();
        socket.acks(true).unwrap();
        finish(socket);
    }

    #[test]
    fn messages_that_waits_read_wait_for_the_wait_that_asks_for_them() {
        let Some(mut socket) = stand_in(STAND_IN) else { return };
        let attached = socket.attach(3, (80, 24), Some(7)).unwrap();
        assert_eq!(attached, json!({"attached": 3, "offset": 9}));
        // The message before it is kept.
        assert_eq!(socket.said, [json!({"changed": 1})]);
        // A quiet wait reads `ended`; it is there for the wait that asks.
        let waited = socket.wait_quiet(1).unwrap();
        assert_eq!(waited.length(3), 7);
        assert_eq!(socket.wait_said("ended", 1).unwrap(), json!({"ended": 3, "why": "locked"}));
        assert_eq!(socket.said, [json!({"changed": 1})]);
        // A message nothing has said is a wait that runs out, and says so.
        let why = socket.wait_said("detached", 1).unwrap_err();
        assert!(why.contains("a message with \"detached\" did not come"), "{why}");
        assert!(why.contains(r#"its other messages were [{"changed":1}]"#), "{why}");
        socket.forget_said("changed");
        assert!(socket.said.is_empty());
        finish(socket);
    }

    #[test]
    fn waits_for_output_bytes_pongs_and_closes_say_what_they_lacked() {
        let Some(mut socket) = stand_in(STAND_IN) else { return };
        let why = socket.wait_output(3, "needle", 1).unwrap_err();
        assert!(why.contains("\"needle\" in the output of pane 3 did not come in 1 s"), "{why}");
        assert!(why.ends_with("; the output ends with \"abc\""), "{why}");
        assert_eq!(socket.wait_bytes(3, 7, 1).unwrap().length(3), 7);
        let why = socket.wait_bytes(3, 8, 1).unwrap_err();
        assert!(why.starts_with("8 bytes of output of pane 3 did not come in 1 s"), "{why}");
        let why = socket.wait_pong("words", 1).unwrap_err();
        assert!(why.starts_with("a pong of \"words\" did not come in 1 s"), "{why}");
        assert_eq!(socket.wait_closed(1).unwrap(), 1001);
        let held = socket.output(3).unwrap();
        assert_eq!(
            (held.text.as_str(), held.length, held.first, held.end()),
            ("abc", 3, Some(10), 13)
        );
        finish(socket);
    }

    #[test]
    fn a_client_that_does_not_quit_is_stopped_after_two_seconds() {
        let Some(socket) = stand_in("import time\ntime.sleep(60)\n") else { return };
        let pid = socket.child.id();
        let errors = socket.errors.clone();
        let started = Instant::now();
        drop(socket);
        let took = started.elapsed();
        assert!(took >= Duration::from_secs(2) && took < Duration::from_secs(8), "{took:?}");
        // It was reaped as well as killed: nothing is left of its process.
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        std::fs::remove_file(errors).unwrap();
    }

    #[test]
    fn a_client_that_quits_ends_at_once() {
        let program =
            "import sys\nfor line in sys.stdin:\n    print('{}', flush=True)\n    break\n";
        let Some(socket) = stand_in(program) else { return };
        let started = Instant::now();
        finish(socket);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
