// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console as the boot tests reach it: the host's curl, aimed
//! through a port QEMU forwards at the console's port 9090 in the guest,
//! as a browser at `localhost` would be. The `admin`, `admin-actions`,
//! `admin-journal`, `admin-terminal`, `admin-accounts` and `admin-security`
//! cases use it, with what they need of the machine: alice, the owner,
//! whose password the pane asks for once, and the replies, cookies and
//! certificates that curl prints. It holds the authenticator app too: the
//! codes that `admin` and `admin-security` give at a second step, which the
//! host computes from the secret the console showed.
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

    /// A request of an API token, with no session: `Authorization: Bearer`
    /// and, for a body, JSON. The answer is whatever it is.
    pub fn with_token(
        &self,
        token: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Reply, String> {
        let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        self.send(method, path, &headers, body.map(Value::to_string).as_deref())
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

/// What the host keeps of the authenticator app it made up: its secret, and
/// the last step of its codes that alice used, which authd refuses to
/// take again.
pub struct App {
    /// The secret the app was begun with.
    pub secret: Vec<u8>,
    /// The last step it gave a code for.
    pub last: Option<u64>,
}

impl Console {
    /// Unix seconds by the guest's clock, which the console's own `Date`
    /// header gives: the app's codes follow the clock of the machine that
    /// checks them, which need not be the host's to the second.
    pub fn guest_unix(&self) -> Result<u64, String> {
        let reply = self.send("GET", "/api/1/health", &[], None)?;
        match reply.header("date").and_then(http_date) {
            Some(seconds) => Ok(seconds),
            None => {
                Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs()))
            }
        }
    }

    /// The app's next code, which authd has not taken: the current step's,
    /// or the next one's, which it takes too for a clock a little off. When
    /// that is a step ahead of what it takes, this waits for the guest's
    /// clock to come to it.
    pub fn next_code(&self, app: &mut App) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Some(step) = step_to_give(self.guest_unix()?, app.last) {
                app.last = Some(step);
                return Ok(app_code(&app.secret, step));
            }
            if Instant::now() >= deadline {
                return Err("the guest's clock did not come to the app's next step in 90 s".into());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
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
            if let Some(event) =
                events_named(&text, "change").into_iter().find(|event| event["id"] == id)
            {
                return Ok(event);
            }
            if Instant::now() >= deadline {
                return Err(format!("the stream did not tell row {id} in {seconds} s: {text:?}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

impl Events {
    /// Waits up to `seconds` for the stream to tell that its session
    /// ended, and gives the reason: `by-administrator`, `by-you`,
    /// `signed-out`, `expired`, `idle` or `restarted`.
    pub fn ended(&self, seconds: u64) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let text = self.text();
            let told = events_named(&text, "session");
            if let Some(reason) = told.iter().find_map(|event| event["ended"].as_str()) {
                return Ok(reason.to_string());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the stream did not tell its session ended in {seconds} s: {text:?}"
                ));
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

/// The data of each event called `wanted` in `text`, which is what a
/// stream said: its headers, then `event: NAME` and `data: JSON` lines,
/// with a blank line after each event and a comment, `: keep`, now and
/// then. An event whose data has not come, or is not JSON, is left out.
pub fn events_named(text: &str, wanted: &str) -> Vec<Value> {
    let mut named = None;
    let mut found = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("event: ") {
            named = Some(name);
        } else if let Some(data) = line.strip_prefix("data: ") {
            if named == Some(wanted)
                && let Ok(event) = serde_json::from_str(data)
            {
                found.push(event);
            }
            named = None;
        }
    }
    found
}

/// What curl prints of each request, one line: the status, the version of
/// HTTP and how many connections it opened for the request.
const WRITE_OUT: &str = "%{http_code} %{http_version} %{num_connects}\\n";

/// The three words of a line that [`WRITE_OUT`] printed.
fn summary(line: &str) -> Option<(u16, String, u32)> {
    let mut words = line.split_whitespace();
    let status = words.next()?.parse().ok()?;
    let version = words.next()?.to_string();
    let connects = words.next()?.parse().ok()?;
    words.next().is_none().then_some((status, version, connects))
}

/// The version of HTTP a run of curl asks for. The last of curl's two
/// options for it wins, and each request of a run has its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Http {
    /// HTTP/1.1.
    One,
    /// HTTP/2.
    Two,
}

impl Http {
    /// curl's option for it.
    pub fn flag(self) -> &'static str {
        match self {
            Http::One => "--http1.1",
            Http::Two => "--http2",
        }
    }

    /// The version as `%{http_version}` says it.
    pub fn word(self) -> &'static str {
        match self {
            Http::One => "1.1",
            Http::Two => "2",
        }
    }
}

/// A request of a run of curl.
pub struct Call {
    method: &'static str,
    path: String,
    body: Option<String>,
}

impl Call {
    /// A read of `path`.
    pub fn get(path: &str) -> Call {
        Call { method: "GET", path: path.to_string(), body: None }
    }

    /// A change of `path` with `body`.
    pub fn post(path: &str, body: &Value) -> Call {
        Call { method: "POST", path: path.to_string(), body: Some(body.to_string()) }
    }

    /// The headers the page sends with it: for a read the session's cookie
    /// alone, as [`Console::get`] sends it, for a change those of a change.
    pub fn headers(&self, browser: &Browser) -> Vec<(String, String)> {
        let all = browser.page_headers();
        if self.method == "GET" {
            all.into_iter().filter(|(name, _)| name == "Cookie").collect()
        } else {
            all
        }
    }
}

/// What curl made of one request.
pub struct Carried {
    /// The answer.
    pub reply: Reply,
    /// The version of HTTP it went by.
    pub version: String,
    /// How many connections curl opened for it: none when it took the one
    /// an earlier request had.
    pub connects: u32,
}

/// What one run of curl did.
pub struct Run {
    /// What curl made of each request, in order.
    pub carried: Vec<Carried>,
    /// What curl said on its error output, with the last request's
    /// `--verbose`.
    pub said: String,
    /// Whether curl ended without an error of its own.
    pub ok: bool,
}

/// How a run of curl was told that the console ends the connection after an
/// answer: whether `said`, curl's account of the last request, has the
/// GOAWAY frame that closes an HTTP/2 connection with no error.
pub fn goaway(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    said.contains("goaway") && said.contains("error=0")
}

impl Console {
    /// Runs curl once for `calls`, with the session of `browser`, as one
    /// run: with `--next`, curl takes the connection it has for the next
    /// request, so the requests share one connection as a page's do.
    pub fn carry(&self, browser: &Browser, http: Http, calls: &[Call]) -> Result<Run, String> {
        let place =
            self.pem.with_file_name(format!("carried-{}", CARRIED.fetch_add(1, Ordering::Relaxed)));
        let mut command = Command::new("curl");
        // The first word of the command line, as curl wants it.
        command.arg("--disable");
        let mut files = Vec::new();
        for (number, call) in calls.iter().enumerate() {
            if number > 0 {
                command.arg("--next");
            }
            let file = place.with_extension(number.to_string());
            command.args(["--silent", "--show-error", http.flag(), "--max-time", "90"]);
            command.args(["--noproxy", "*", "--connect-to"]);
            command.arg(format!("localhost:{PORT}:127.0.0.1:{}", self.port));
            command.args(["--include", "--cacert"]).arg(&self.pem);
            command.args(["--request", call.method]);
            for (name, value) in call.headers(browser) {
                command.arg("--header").arg(format!("{name}: {value}"));
            }
            if let Some(body) = &call.body {
                command.arg("--data-binary").arg(body);
            }
            command.arg("--output").arg(&file);
            command.args(["--write-out", WRITE_OUT]);
            // What curl says of the last connection, for how it ended.
            if number + 1 == calls.len() {
                command.arg("--verbose");
            }
            command.arg(format!("{ORIGIN}{}", call.path));
            files.push(file);
        }
        let out = command
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("the console tests need the host's curl: {err}"))?;
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        let summaries = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut carried = Vec::new();
        for (file, line) in files.iter().zip(summaries.lines()) {
            let raw = std::fs::read(file).map_err(|err| format!("{}: {err}", file.display()))?;
            let _ = std::fs::remove_file(file);
            let (_, version, connects) = summary(line)
                .ok_or_else(|| format!("curl's summary of a request is {line:?}, not as asked"))?;
            carried.push(Carried { reply: parse_reply(&raw)?, version, connects });
        }
        if carried.len() != calls.len() {
            return Err(format!(
                "curl carried out {} of {} requests and said {said:?}",
                carried.len(),
                calls.len()
            ));
        }
        Ok(Run { carried, said, ok: out.status.success() })
    }
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

/// How many runs of curl wrote their answers to files so far, to name them.
static CARRIED: AtomicUsize = AtomicUsize::new(0);

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

/// Whether `reply` is the console's error `id` with `status`, as
/// [`Reply::refused`] holds an answer to, for a reply that is only looked at.
pub fn is_refusal(reply: &Reply, status: u16, id: &str) -> bool {
    let said = reply.json().ok().and_then(|body| body["error"]["id"].as_str().map(str::to_string));
    reply.status == status && said.as_deref() == Some(id)
}

/// Calls `look` every second until it gives `Ok(Ok(found))`, up to
/// `seconds`: `what` is then said not to have happened, with the last
/// state `look` gave as `Ok(Err(state))`. An `Err` ends it at once.
pub fn wait_for<T>(
    what: &str,
    seconds: u64,
    mut look: impl FnMut() -> Result<Result<T, String>, String>,
) -> Result<T, String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match look()? {
            Ok(found) => return Ok(found),
            Err(state) if Instant::now() >= deadline => {
                return Err(format!("{what} did not happen in {seconds} s: {state}"));
            }
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

/// The trials in Spark's document.
pub fn trials_of(document: &Value) -> Vec<&Value> {
    document["trials"].as_array().map_or_default(|trials| trials.iter().collect())
}

/// The trial `id` of Spark's document, if it is being tried.
pub fn trial_in<'a>(document: &'a Value, id: &str) -> Option<&'a Value> {
    trials_of(document).into_iter().find(|trial| trial["id"] == id)
}

/// The entries of Spark's document for the trials `id` that were undone,
/// oldest first.
pub fn undone_in<'a>(document: &'a Value, id: &str) -> Vec<&'a Value> {
    let undone = document["undone"].as_array();
    undone.map_or_default(|all| all.iter().filter(|entry| entry["id"] == id).collect())
}

/// Whether the journal's `row` names `file` among its files.
pub fn names(row: &Value, file: &str) -> bool {
    row["files"].as_array().is_some_and(|files| files.iter().any(|one| one["path"] == file))
}

/// Whether `row` is Spark's row for a trial it undid, with its words `why`,
/// that put `file` back.
pub fn is_undone_row(row: &Value, file: &str, why: &str) -> bool {
    row["who"] == "Spark" && row["how"] == "undone" && row["note"] == why && names(row, file)
}

/// SHA-1 (FIPS 180-4), which RFC 6238's codes are made with by default and
/// authd checks them with.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    let (blocks, _) = padded.as_chunks::<64>();
    for block in blocks {
        let mut words = [0u32; 80];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for at in 16..80 {
            words[at] =
                (words[at - 3] ^ words[at - 8] ^ words[at - 14] ^ words[at - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (at, word) in words.into_iter().enumerate() {
            let (mix, constant) = match at {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(mix)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            (a, b, c, d, e) = (next, a, b.rotate_left(30), c, d);
        }
        for (kept, new) in state.iter_mut().zip([a, b, c, d, e]) {
            *kept = kept.wrapping_add(new);
        }
    }
    let mut hash = [0u8; 20];
    for (bytes, word) in hash.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    hash
}

/// HMAC-SHA1 (RFC 2104).
pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|byte| byte ^ 0x36).collect();
    inner.extend_from_slice(data);
    let mut outer: Vec<u8> = block.iter().map(|byte| byte ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// The app's six digits for 30-second `step` since 1970 (RFC 6238 with
/// HMAC-SHA1, truncated as RFC 4226 says).
pub fn app_code(secret: &[u8], step: u64) -> String {
    let mac = hmac_sha1(secret, &step.to_be_bytes());
    let at = usize::from(mac[19] & 0x0f);
    let number = u32::from_be_bytes([mac[at], mac[at + 1], mac[at + 2], mac[at + 3]]) & 0x7fff_ffff;
    format!("{:06}", number % 1_000_000)
}

/// The step of the app's code to give at guest time `now` when `after` is
/// the last step authd took a code for: this one, or the next when this
/// was taken already, which authd takes as well for a clock a little off.
/// `None` when even that is not one it takes yet: the clock has to come to
/// it.
pub fn step_to_give(now: u64, after: Option<u64>) -> Option<u64> {
    let current = now / 30;
    let step = after.map_or(current, |after| current.max(after + 1));
    (step <= current + 1).then_some(step)
}

/// Six digits that are none of the app's codes for the steps around `now`.
pub fn wrong_code(secret: &[u8], now: u64) -> String {
    let current = now / 30;
    let right: Vec<String> =
        (current.saturating_sub(2)..=current + 2).map(|step| app_code(secret, step)).collect();
    (1u32..)
        .map(|n| format!("{:06}", n * 123_457 % 1_000_000))
        .find(|code| !right.contains(code))
        .unwrap_or_default()
}

/// The bytes of RFC 4648's base32 `text`, with or without padding.
pub fn from_base32(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for letter in text.trim_end_matches('=').bytes() {
        let value = match letter {
            b'A'..=b'Z' => letter - b'A',
            b'2'..=b'7' => letter - b'2' + 26,
            _ => return None,
        };
        buffer = buffer << 5 | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(bytes)
}

/// The secret in an `otpauth://` URI.
pub fn secret_of(uri: &str) -> Result<Vec<u8>, String> {
    let query = uri.split_once('?').map_or("", |(_, query)| query);
    let text = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("secret="))
        .ok_or_else(|| format!("the app's URI {uri:?} has no secret"))?;
    from_base32(text)
        .filter(|secret| secret.len() >= 10)
        .ok_or_else(|| format!("the secret {text:?} does not read"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_of_a_stream_are_read_from_its_text_by_name() {
        let text = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                    event: unlock\ndata: {\"unlock_seconds\":null}\n\n\
                    : keep\n\n\
                    event: change\ndata: {\"at\":1790000000,\"how\":\"console\",\"id\":7}\n\n\
                    event: task\ndata: {\"id\":7,\"state\":\"running\"}\n\n\
                    event: change\ndata: {\"at\":1790000030,\"how\":\"by hand\",\"id\":8}\n\n\
                    event: change\ndata: not json\n\n\
                    event: change\n";
        let found = events_named(text, "change");
        assert_eq!(
            found,
            [
                json!({"at": 1790000000, "how": "console", "id": 7}),
                json!({"at": 1790000030, "how": "by hand", "id": 8}),
            ]
        );
        assert!(events_named("", "change").is_empty());
        // Another event's data is not a change's, even if it is JSON.
        assert!(events_named("event: task\ndata: {\"id\":1}\n\n", "change").is_empty());
        assert!(events_named("data: {\"id\":1}\n\n", "change").is_empty());
        let sessions = "event: unlock\ndata: {\"unlock_seconds\":null}\n\n\
                        event: session\ndata: {\"ended\":\"by-administrator\"}\n\n";
        assert_eq!(events_named(sessions, "session"), [json!({"ended": "by-administrator"})]);
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

    #[test]
    fn sha1_has_the_standard_answers_across_block_ends() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha1(two_blocks)), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        // 55, 56 and 64 bytes: where the padding and the length change blocks.
        assert_eq!(hex(&sha1(&[b'a'; 55])), "c1c8bbdc22796e28c0e15163d20899b65621d65a");
        assert_eq!(hex(&sha1(&[b'a'; 56])), "c2db330f6083854c99d4b5bfb6e8f29f201be699");
        assert_eq!(hex(&sha1(&[b'a'; 64])), "0098ba824b5c16427bd7a1122a5a442a25ec644d");
        assert_eq!(hex(&sha1(&vec![b'a'; 1_000_000])), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }

    #[test]
    fn hmac_sha1_has_the_answers_of_rfc_2202() {
        assert_eq!(
            hex(&hmac_sha1(&[0x0b; 20], b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        // A key longer than a block is hashed first.
        assert_eq!(
            hex(&hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    #[test]
    fn the_apps_codes_are_those_of_rfc_6238() {
        // The RFC's SHA-1 secret, and its 8-digit codes cut to the last 6.
        let secret = b"12345678901234567890";
        for (time, code) in [
            (59u64, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(app_code(secret, time / 30), code, "at {time}");
        }
    }

    #[test]
    fn a_code_is_given_for_this_step_or_the_next_and_otherwise_waits() {
        // 1000 s is in step 33, which runs from 990 to 1019.
        assert_eq!(step_to_give(1000, None), Some(33));
        assert_eq!(step_to_give(1000, Some(10)), Some(33));
        assert_eq!(step_to_give(1000, Some(32)), Some(33));
        // Taken already: the next, which authd takes for a clock a little off.
        assert_eq!(step_to_give(1000, Some(33)), Some(34));
        // The next was taken too: wait for the clock.
        assert_eq!(step_to_give(1000, Some(34)), None);
        assert_eq!(step_to_give(1020, Some(34)), Some(35));
    }

    #[test]
    fn a_wrong_code_is_none_of_the_codes_around_now() {
        let secret = b"12345678901234567890";
        for now in [0, 59, 1_111_111_109, 2_000_000_000] {
            let wrong = wrong_code(secret, now);
            assert_eq!(wrong.len(), 6);
            for step in (now / 30).saturating_sub(2)..=now / 30 + 2 {
                assert_ne!(wrong, app_code(secret, step), "now {now}, step {step}");
            }
        }
    }

    #[test]
    fn base32_reads_the_rfc_4648_vectors_with_or_without_padding() {
        for (text, want) in [
            ("", ""),
            ("MY======", "f"),
            ("MZXQ====", "fo"),
            ("MZXW6", "foo"),
            ("MZXW6YQ=", "foob"),
            ("MZXW6YTB", "fooba"),
            ("MZXW6YTBOI======", "foobar"),
        ] {
            assert_eq!(from_base32(text).as_deref(), Some(want.as_bytes()), "{text}");
        }
        assert_eq!(from_base32("mzxw6"), None);
        assert_eq!(from_base32("MZXW1"), None);
    }

    #[test]
    fn the_secret_is_read_from_the_uri_as_authd_writes_it() {
        let uri = "otpauth://totp/Dukovo%20(duke-nas):alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\
                   &issuer=Dukovo%20(duke-nas)&algorithm=SHA1&digits=6&period=30";
        assert_eq!(secret_of(uri).unwrap(), b"12345678901234567890");
        assert!(secret_of("otpauth://totp/x?issuer=y").is_err());
        assert!(secret_of("otpauth://totp/x?secret=!!!").is_err());
        // Too short to be an app's secret.
        assert!(secret_of("otpauth://totp/x?secret=MZXW6").is_err());
    }

    fn document() -> Value {
        json!({
            "now_ms": 51234,
            "services": [],
            "trials": [
                {"id": "firewall", "files": ["/system/settings/firewall"], "services": ["firewall"], "user": 1000, "left_ms": 42000},
            ],
            "undone": [
                {"id": "network", "why": "it was not kept in time", "at_ms": 100, "files": ["/system/settings/network"]},
                {"id": "firewall", "why": "it was not kept in time", "at_ms": 200, "files": ["/system/settings/firewall"]},
                {"id": "firewall", "why": "the machine started again before it was kept", "at_ms": 300, "files": ["/system/settings/firewall"]},
            ],
        })
    }

    #[test]
    fn a_row_is_sparks_undo_of_a_file_by_its_words() {
        let row = json!({
            "id": 9, "who": "Spark", "how": "undone", "action": null, "note": "it was not kept in time",
            "files": [{"path": "/system/settings/firewall", "before": {}, "after": {}}],
        });
        assert!(is_undone_row(&row, "/system/settings/firewall", "it was not kept in time"));
        assert!(!is_undone_row(&row, "/system/settings/network", "it was not kept in time"));
        assert!(!is_undone_row(
            &row,
            "/system/settings/firewall",
            "the machine started again before it was kept"
        ));
        let mut other = row.clone();
        other["how"] = json!("by hand");
        assert!(!is_undone_row(&other, "/system/settings/firewall", "it was not kept in time"));
        other["how"] = json!("undone");
        other["who"] = json!("alice");
        assert!(!is_undone_row(&other, "/system/settings/firewall", "it was not kept in time"));
        assert!(
            names(&row, "/system/settings/firewall") && !names(&row, "/system/settings/network")
        );
        assert!(!names(&json!({}), "/system/settings/firewall"));
    }

    #[test]
    fn curls_summary_of_a_request_has_three_words() {
        assert_eq!(summary("409 2 0"), Some((409, "2".to_string(), 0)));
        assert_eq!(summary("200 1.1 1"), Some((200, "1.1".to_string(), 1)));
        assert_eq!(summary("200 1.1"), None);
        assert_eq!(summary("200 1.1 1 extra"), None);
        assert_eq!(summary("ok 1.1 1"), None);
        assert_eq!(summary(""), None);
    }

    #[test]
    fn a_goaway_with_no_error_is_how_curl_says_the_server_ended_the_connection() {
        let said = "* Connection #0 to host localhost left intact\n\
                    < HTTP/2 409 \n* received GOAWAY, error=0, last_stream=5\n* Closing connection\n";
        assert!(goaway(said));
        assert!(goaway("* Received GOAWAY, error=0, last_stream=1"));
        assert!(!goaway("* received GOAWAY, error=2, last_stream=5"));
        assert!(!goaway("* Closing connection"));
        assert!(!goaway(""));
    }

    #[test]
    fn a_request_sends_the_cookie_alone_for_a_read_and_the_pages_headers_for_a_change() {
        let browser = Browser { cookie: Some("abc".into()), token: Some("tok".into()) };
        let read = Call::get("/api/1/session").headers(&browser);
        assert_eq!(read, [("Cookie".to_string(), "__Host-dukovo=abc".to_string())]);
        let change = Call::post("/api/1/firewall/add", &json!({})).headers(&browser);
        assert!(change.iter().any(|(name, _)| name == "Origin"));
        assert!(change.iter().any(|(name, value)| name == "X-Dukovo-CSRF" && value == "tok"));
        assert!(change.iter().any(|(name, _)| name == "Cookie"));
    }

    #[test]
    fn a_wait_gives_what_it_found_or_the_last_state_it_saw() {
        let mut calls = 0;
        let found = wait_for("a number", 5, || {
            calls += 1;
            Ok(if calls == 3 { Ok(calls) } else { Err(format!("call {calls}")) })
        });
        assert_eq!(found, Ok(3));
        let never: Result<(), String> = wait_for("nothing", 0, || Ok(Err("still no".into())));
        assert_eq!(never, Err("nothing did not happen in 0 s: still no".into()));
        let failed: Result<(), String> = wait_for("it", 5, || Err("gone".into()));
        assert_eq!(failed, Err("gone".into()));
    }
    #[test]
    fn a_trial_and_the_undone_ones_are_found_by_their_name_in_sparks_document() {
        let document = document();
        assert_eq!(
            trial_in(&document, "firewall").map(|trial| &trial["left_ms"]),
            Some(&json!(42000))
        );
        assert!(trial_in(&document, "network").is_none());
        let undone = undone_in(&document, "firewall");
        assert_eq!(undone.len(), 2);
        assert_eq!(undone[1]["why"], json!("the machine started again before it was kept"));
        assert_eq!(undone_in(&document, "network").len(), 1);
        assert!(undone_in(&json!({}), "firewall").is_empty());
        assert!(trials_of(&json!({"trials": []})).is_empty());
        assert_eq!(trials_of(&document).len(), 1);
    }
}
