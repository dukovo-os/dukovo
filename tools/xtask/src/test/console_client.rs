// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console as the boot tests reach it: the host's curl, aimed
//! through a port QEMU forwards at the console's port 9090 in the guest,
//! as a browser at `localhost` would be. The `admin` and `admin-actions`
//! cases both use it, with what they both need of the machine: alice, the
//! owner, whose password the pane asks for once, and the replies,
//! cookies and certificates that curl prints.
//!
//! curl trusts the one certificate the machine served at first and no
//! other, so a case that makes the machine serve another says so with
//! [`Console::trust_what_is_served`].

use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
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
}
