// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Email through the provider's submission server (ADR 0485): RFC 6409 on
//! port 587 with STARTTLS (RFC 3207), or TLS from the first byte on port
//! 465 (RFC 8314). A server that does not offer STARTTLS on 587 is
//! refused before anything is sent: an alert never goes in the clear, and
//! neither does a password. The certificate must chain to the system's
//! roots or to the file `ca` names, and be valid for the server's name.
//!
//! A message follows RFC 5322: plain text in UTF-8, quoted-printable,
//! with `Auto-Submitted: auto-generated` (RFC 3834) so that vacation
//! responders do not answer, and a subject that is not ASCII as an RFC
//! 2047 word. Authentication is AUTH PLAIN (RFC 4616), or AUTH LOGIN when
//! that is all the server offers.
//!
//! Every step checks the server's reply code, and an error names the step
//! and quotes the server's own words, so that `alerts test` shows why a
//! provider refused ("the server refused the password (535 5.7.8 ...)").
//! The unit tests run the protocol against a server on lo with the
//! certificates of tls-test.

use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rustls::RootCertStore;
use rustls::pki_types::ServerName;

use super::settings::Secret;
use crate::{logd, tls, util};

/// How long connecting may take, and a read or a write.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest reply line and the most reply lines read: RFC 5321 allows
/// 512 bytes of reply line.
const REPLY_LINE_MAX: usize = 1000;
const REPLY_LINES_MAX: usize = 100;

/// The most characters of a server's words in an error.
const SAID_MAX: usize = 200;

/// A line of a message and of a header is folded to at most this many
/// columns (RFC 5322 §2.2.3 asks for 78).
const FOLD_AT: usize = 76;

/// How many bytes of a subject go in one RFC 2047 word: 36 bytes make 48
/// characters of base64 and 60 with the word's own marks, so that a word
/// and `Subject: ` fit a folded line.
const WORD_BYTES: usize = 36;

/// The submission server, and how to sign in to it.
#[derive(Clone, Debug)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<Secret>,
    /// The authorities whose certificates are trusted.
    pub roots: Arc<RootCertStore>,
}

/// A message to send.
#[derive(Clone, Debug)]
pub struct Message {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// Counts the messages of this run, so that their Message-IDs differ even
/// within a second.
static SENT: AtomicU64 = AtomicU64::new(0);

/// Sends `message` through `server`, saying `hostname` for itself in
/// EHLO.
///
/// # Errors
///
/// The step that failed and the server's words: the connection, the
/// certificate, the lack of STARTTLS, the refusal of the password, of
/// the sender or of a recipient, or of the message.
pub fn send(server: &Server, message: &Message, hostname: &str) -> Result<(), String> {
    send_over(server, message, hostname, server.port == 465)
}

/// As [`send`], with TLS from the first byte when `implicit`, whatever the
/// port, as the tests need.
fn send_over(
    server: &Server,
    message: &Message,
    hostname: &str,
    implicit: bool,
) -> Result<(), String> {
    let hello = format!("EHLO {}", greeting_name(hostname));
    let tls = Tls::new(server)?;
    let tcp = connect(&server.host, server.port)?;
    let mut session = if implicit {
        Session::new(Link::Tls(Box::new(tls.handshake(tcp)?)))
    } else {
        Session::new(Link::Plain(tcp))
    };
    session.expect(&[220], "the server turned the connection away")?;
    let mut offered = session.command(&hello, &[250], "the server refused EHLO")?;
    if !implicit {
        if !offered.offers("STARTTLS") {
            return Err(format!(
                "the server does not offer STARTTLS, so nothing was sent; an alert is never sent \
                 without TLS (it offers: {})",
                offered.extensions()
            ));
        }
        session.command("STARTTLS", &[220], "the server refused STARTTLS")?;
        session = session.upgrade(&tls)?;
        offered = session.command(&hello, &[250], "the server refused EHLO after STARTTLS")?;
    }
    if let (Some(user), Some(password)) = (&server.user, &server.password) {
        session.authenticate(&offered, user, password.expose())?;
    }
    let count = SENT.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let text = format(message, hostname, now, &format!("{now}.{count}"));
    session.deliver(message, &text)?;
    // The message is accepted; a server that cannot say goodbye changes
    // nothing.
    let _ = session.command("QUIT", &[221], "");
    Ok(())
}

/// The name EHLO gives: the machine's, or `localhost` when it is not a
/// name SMTP takes.
fn greeting_name(hostname: &str) -> &str {
    let name = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    if !hostname.is_empty() && hostname.chars().all(name) { hostname } else { "localhost" }
}

/// Connects to `host`'s first address that answers.
fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let cannot = |why: &str| format!("cannot connect to {host}:{port}: {why}");
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("cannot find {host}: {}", util::strip_os_error(&err)))?;
    let mut last = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(IO_TIMEOUT))
                    .and_then(|()| tcp.set_write_timeout(Some(IO_TIMEOUT)))
                    .map_err(|err| cannot(&util::strip_os_error(&err)))?;
                // A command and its reply are small; Nagle would hold each.
                let _ = tcp.set_nodelay(true);
                return Ok(tcp);
            }
            Err(err) => last = Some(err),
        }
    }
    Err(cannot(&last.map_or("no address".into(), |err| util::strip_os_error(&err))))
}

/// What the TLS handshake needs: the settings that check the server's
/// certificate, and its name.
struct Tls {
    config: Arc<rustls::ClientConfig>,
    name: ServerName<'static>,
    host: String,
    port: u16,
}

impl Tls {
    fn new(server: &Server) -> Result<Tls, String> {
        let name = ServerName::try_from(server.host.clone())
            .map_err(|_| format!("'{}' is not a server name", server.host))?;
        Ok(Tls {
            config: tls::verified((*server.roots).clone(), &[]),
            name,
            host: server.host.clone(),
            port: server.port,
        })
    }

    fn handshake(&self, tcp: TcpStream) -> Result<tls::Stream, String> {
        tls::handshake(tcp, self.name.clone(), &self.config)
            .map_err(|err| format!("TLS with {}:{}: {}", self.host, self.port, self.refusal(&err)))
    }

    /// Why the handshake failed, in words for `alerts test`.
    fn refusal(&self, err: &io::Error) -> String {
        use rustls::CertificateError as Certificate;
        let Some(error) = err.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>())
        else {
            return match err.kind() {
                io::ErrorKind::UnexpectedEof => "the server closed the connection".to_string(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                    format!("no handshake within {} seconds", IO_TIMEOUT.as_secs())
                }
                _ => util::strip_os_error(err),
            };
        };
        let host = &self.host;
        match error {
            rustls::Error::InvalidCertificate(Certificate::UnknownIssuer) => {
                "its certificate is not from an authority the system trusts or the file `ca` \
                 names"
                    .into()
            }
            rustls::Error::InvalidCertificate(
                Certificate::NotValidForName | Certificate::NotValidForNameContext { .. },
            ) => format!("its certificate is not for {host}"),
            rustls::Error::InvalidCertificate(
                Certificate::Expired | Certificate::ExpiredContext { .. },
            ) => "its certificate has expired".into(),
            rustls::Error::InvalidCertificate(
                Certificate::NotValidYet | Certificate::NotValidYetContext { .. },
            ) => "its certificate is not valid yet".into(),
            other => other.to_string(),
        }
    }
}

/// A connection, before and after STARTTLS.
enum Link {
    Plain(TcpStream),
    Tls(Box<tls::Stream>),
}

impl Read for Link {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Link::Plain(tcp) => tcp.read(buffer),
            Link::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Link {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Link::Plain(tcp) => tcp.write(data),
            Link::Tls(stream) => stream.write(data),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Link::Plain(tcp) => tcp.flush(),
            Link::Tls(stream) => stream.flush(),
        }
    }
}

/// A server's reply: its code, and the text of each line.
#[derive(Debug)]
struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    /// Its text as one line, cut to [`SAID_MAX`] characters: for the words
    /// of an error.
    fn said(&self) -> String {
        let text = self.lines.join(" ");
        let mut said: String = text.chars().take(SAID_MAX).collect();
        if said.len() < text.len() {
            said.push_str("...");
        }
        format!("{} {said}", self.code)
    }

    /// Whether an EHLO reply offers `extension`.
    fn offers(&self, extension: &str) -> bool {
        self.lines.iter().skip(1).any(|line| {
            line.split([' ', '=']).next().is_some_and(|word| word.eq_ignore_ascii_case(extension))
        })
    }

    /// The extensions an EHLO reply lists, for an error.
    fn extensions(&self) -> String {
        let listed: Vec<&str> =
            self.lines.iter().skip(1).filter_map(|line| line.split([' ', '=']).next()).collect();
        if listed.is_empty() { "none".into() } else { listed.join(", ") }
    }

    /// The mechanisms of the AUTH extension, in capitals.
    fn mechanisms(&self) -> Vec<String> {
        let auth = self.lines.iter().skip(1).find(|line| {
            line.split([' ', '=']).next().is_some_and(|word| word.eq_ignore_ascii_case("AUTH"))
        });
        auth.map_or_else(Vec::new, |line| {
            line.split([' ', '=']).skip(1).map(str::to_ascii_uppercase).collect()
        })
    }
}

/// One conversation with a server.
struct Session {
    link: BufReader<Link>,
}

impl Session {
    fn new(link: Link) -> Session {
        Session { link: BufReader::new(link) }
    }

    /// Starts TLS over the connection, after the server's 220 to STARTTLS.
    fn upgrade(self, tls: &Tls) -> Result<Session, String> {
        // Anything the server sent after its 220 and before the handshake
        // is not from the TLS peer: it could be a plaintext command
        // slipped in (RFC 3207's note on command injection).
        if !self.link.buffer().is_empty() {
            return Err("the server sent data after its answer to STARTTLS, before the TLS \
                        handshake; nothing was sent"
                .into());
        }
        let Link::Plain(tcp) = self.link.into_inner() else {
            return Err("TLS is on already".into());
        };
        Ok(Session::new(Link::Tls(Box::new(tls.handshake(tcp)?))))
    }

    /// Writes `line` and CRLF.
    fn write_line(&mut self, line: &str) -> Result<(), String> {
        let link = self.link.get_mut();
        link.write_all(line.as_bytes())
            .and_then(|()| link.write_all(b"\r\n"))
            .and_then(|()| link.flush())
            .map_err(|err| format!("cannot write to the server: {}", util::strip_os_error(&err)))
    }

    /// Reads one line of a reply.
    fn read_line(&mut self) -> Result<String, String> {
        let mut line = Vec::new();
        let limit = (REPLY_LINE_MAX + 1) as u64;
        let read = (&mut self.link).take(limit).read_until(b'\n', &mut line);
        match read {
            Ok(0) => Err("the server closed the connection".into()),
            Ok(_) if !line.ends_with(b"\n") => {
                Err(format!("the server sent a reply line longer than {REPLY_LINE_MAX} bytes"))
            }
            Ok(_) => Ok(String::from_utf8_lossy(&line).trim_end().to_string()),
            Err(err)
                if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
            {
                Err(format!("the server did not answer within {} seconds", IO_TIMEOUT.as_secs()))
            }
            Err(err) => Err(format!("cannot read from the server: {}", util::strip_os_error(&err))),
        }
    }

    /// Reads a whole reply: lines `250-...` up to the one `250 ...`.
    fn reply(&mut self) -> Result<Reply, String> {
        let mut lines = Vec::new();
        for _ in 0..REPLY_LINES_MAX {
            let line = self.read_line()?;
            let code: u16 = line
                .get(..3)
                .and_then(|code| code.parse().ok())
                .ok_or_else(|| format!("the server sent {line:?}, which is not an SMTP reply"))?;
            let rest = line.get(3..).unwrap_or_default();
            lines.push(rest.strip_prefix(['-', ' ']).unwrap_or(rest).to_string());
            if !rest.starts_with('-') {
                return Ok(Reply { code, lines });
            }
        }
        Err(format!("the server sent more than {REPLY_LINES_MAX} lines in one reply"))
    }

    /// Reads a reply, which must have one of `codes`; `refused` says what
    /// the server did otherwise.
    fn expect(&mut self, codes: &[u16], refused: &str) -> Result<Reply, String> {
        let reply = self.reply()?;
        if codes.contains(&reply.code) {
            Ok(reply)
        } else {
            Err(format!("{refused} ({})", reply.said()))
        }
    }

    /// Sends `line` and reads the reply to it, as [`Session::expect`].
    fn command(&mut self, line: &str, codes: &[u16], refused: &str) -> Result<Reply, String> {
        self.write_line(line)?;
        self.expect(codes, refused)
    }

    /// Signs in with AUTH PLAIN, or AUTH LOGIN when that is all the
    /// server offers. Only ever called over TLS.
    fn authenticate(&mut self, offered: &Reply, user: &str, password: &str) -> Result<(), String> {
        let mechanisms = offered.mechanisms();
        let refused = |reply: &Reply| {
            let what = if reply.code == 535 {
                "the server refused the password"
            } else {
                "the server could not sign in"
            };
            format!("{what} ({})", reply.said())
        };
        let wanted = |step: &str| format!("the server refused AUTH {step}");
        let done = if mechanisms.iter().any(|name| name == "PLAIN") {
            let initial = BASE64.encode(format!("\0{user}\0{password}"));
            self.write_line(&format!("AUTH PLAIN {initial}"))?;
            self.reply()?
        } else if mechanisms.iter().any(|name| name == "LOGIN") {
            self.command("AUTH LOGIN", &[334], &wanted("LOGIN"))?;
            self.command(&BASE64.encode(user), &[334], "the server refused the user name")?;
            self.write_line(&BASE64.encode(password))?;
            self.reply()?
        } else if mechanisms.is_empty() {
            return Err("the server offers no sign-in, but alerts.toml names a user".into());
        } else {
            return Err(format!(
                "the server offers sign-in by {}, and only PLAIN and LOGIN are used",
                mechanisms.join(", ")
            ));
        };
        if done.code == 235 { Ok(()) } else { Err(refused(&done)) }
    }

    /// Gives the server the envelope and `text`, dot-stuffed.
    fn deliver(&mut self, message: &Message, text: &[u8]) -> Result<(), String> {
        let from = format!("MAIL FROM:<{}>", message.from);
        self.command(&from, &[250], "the server refused the sender")?;
        for address in &message.to {
            let to = format!("RCPT TO:<{address}>");
            self.command(&to, &[250, 251], &format!("the server refused the recipient {address}"))?;
        }
        self.command("DATA", &[354], "the server refused DATA")?;
        let link = self.link.get_mut();
        link.write_all(&dot_stuffed(text))
            .and_then(|()| link.write_all(b".\r\n"))
            .and_then(|()| link.flush())
            .map_err(|err| format!("cannot write to the server: {}", util::strip_os_error(&err)))?;
        self.expect(&[250], "the server refused the message").map(drop)
    }
}

/// `text` with a dot added to each line that starts with one, so that the
/// server does not take it for the end of the message (RFC 5321 §4.5.2).
fn dot_stuffed(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 8);
    let mut line_start = true;
    for &byte in text {
        if line_start && byte == b'.' {
            out.push(b'.');
        }
        out.push(byte);
        line_start = byte == b'\n';
    }
    out
}

/// The text of `message` as RFC 5322 has it, CRLF at each line's end:
/// sent at `now`, seconds since the epoch, from machine `hostname`, with
/// Message-ID `id`.
pub fn format(message: &Message, hostname: &str, now: u64, id: &str) -> Vec<u8> {
    let host = greeting_name(hostname);
    let mut text = String::new();
    let mut header = |name: &str, value: &str| {
        text.push_str(&fold(&format!("{name}: {value}")));
        text.push_str("\r\n");
    };
    header("Date", &date(now));
    header("From", &message.from);
    header("To", &message.to.join(", "));
    header("Subject", &mime_words(&message.subject));
    header("Message-ID", &format!("<{id}@{host}>"));
    header("MIME-Version", "1.0");
    header("Content-Type", "text/plain; charset=utf-8");
    header("Content-Transfer-Encoding", "quoted-printable");
    header("Auto-Submitted", "auto-generated");
    text.push_str("\r\n");
    text.push_str(&quoted_printable(&message.body));
    text.into_bytes()
}

/// `seconds` since the epoch as RFC 5322's date, in UTC:
/// `Fri, 13 Nov 2026 10:42:11 +0000`.
fn date(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (year, month, day) = logd::civil(days);
    let rest = seconds % 86_400;
    // 1970-01-01 was a Thursday.
    let weekday = DAYS[usize::try_from(days % 7).unwrap_or(0)];
    let month = MONTHS[usize::try_from(month - 1).unwrap_or(0)];
    format!(
        "{weekday}, {day} {month} {year} {:02}:{:02}:{:02} +0000",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// `text` as header text: unchanged when it is printable ASCII, else as
/// RFC 2047 words of base64, separated by spaces, which [`fold`] may put
/// on lines of their own.
pub fn mime_words(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ");
    if text.bytes().all(|byte| byte == b' ' || byte.is_ascii_graphic()) {
        return text;
    }
    let mut words = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut end = WORD_BYTES.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (word, after) = rest.split_at(end);
        words.push(format!("=?utf-8?B?{}?=", BASE64.encode(word)));
        rest = after;
    }
    words.join(" ")
}

/// `header` with a line break and a space (RFC 5322's folding) put at the
/// spaces that would take a line past [`FOLD_AT`] columns.
fn fold(header: &str) -> String {
    let mut out = String::new();
    let mut column = 0;
    for (at, word) in header.split(' ').enumerate() {
        if at > 0 && column + 1 + word.len() > FOLD_AT {
            out.push_str("\r\n ");
            column = 1;
        } else if at > 0 {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += word.len();
    }
    out
}

/// `text` in quoted-printable (RFC 2045 §6.7), with CRLF at each line's
/// end and lines of at most [`FOLD_AT`] columns: each byte that is not
/// printable ASCII, and each `=`, as `=` and two capital hex digits, a
/// space or tab at a line's end the same, and a soft line break, `=` at a
/// line's end, where a line would be too long.
fn quoted_printable(text: &str) -> String {
    let mut out = String::new();
    if text.is_empty() {
        return out;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = if text.ends_with('\n') { lines.len() - 1 } else { lines.len() };
    for line in &lines[..last] {
        let bytes = line.trim_end_matches('\r').as_bytes();
        let mut column = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            let at_end = at + 1 == bytes.len();
            let mut token = String::new();
            if (byte == b' ' || byte == b'\t') && !at_end {
                token.push(char::from(byte));
            } else if byte.is_ascii_graphic() && byte != b'=' {
                token.push(char::from(byte));
            } else {
                let _ = write!(token, "={byte:02X}");
            }
            // The last token of a line has no soft break after it, so it
            // may take the column a soft break's `=` would.
            let limit = if at_end { FOLD_AT } else { FOLD_AT - 1 };
            if column + token.len() > limit {
                out.push_str("=\r\n");
                column = 0;
            }
            out.push_str(&token);
            column += token.len();
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::thread::{self, JoinHandle};

    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::PrivateKeyDer;
    use rustls::pki_types::pem::PemObject;
    use rustls::{ServerConfig, ServerConnection, StreamOwned};

    /// tls-test's certificates: servers for tls.test and 127.0.0.1 from
    /// "Dukovo test CA" (`rsa-2048`), and one from another authority
    /// (`untrusted`).
    fn testdata(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tls-test/testdata").join(name)
    }

    fn roots() -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        let ca = CertificateDer::pem_file_iter(testdata("ca.pem")).unwrap().flatten();
        roots.add_parsable_certificates(ca);
        Arc::new(roots)
    }

    /// What the server in a test does.
    #[derive(Clone)]
    struct Script {
        /// The certificate it shows, a name in tls-test's testdata.
        certificate: &'static str,
        /// Whether it offers STARTTLS (when it is not TLS already).
        starttls: bool,
        /// The mechanisms it offers once TLS is on.
        auth: &'static [&'static str],
        /// The user and password it takes.
        account: (&'static str, &'static str),
        /// A recipient it refuses.
        refuses: Option<&'static str>,
    }

    impl Script {
        fn new() -> Script {
            Script {
                certificate: "rsa-2048",
                starttls: true,
                auth: &["PLAIN", "LOGIN"],
                account: ("alerts@example.com", "app-password"),
                refuses: None,
            }
        }
    }

    /// What the server saw: each command, and the message it was given,
    /// with its dots unstuffed.
    #[derive(Debug, Default)]
    struct Seen {
        commands: Vec<String>,
        message: Option<String>,
        /// Whether any command came before TLS began.
        plaintext: Vec<String>,
    }

    enum Wire {
        Plain(TcpStream),
        Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
    }

    impl Read for Wire {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            match self {
                Wire::Plain(tcp) => tcp.read(buffer),
                Wire::Tls(stream) => stream.read(buffer),
            }
        }
    }

    impl Write for Wire {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            match self {
                Wire::Plain(tcp) => tcp.write(data),
                Wire::Tls(stream) => stream.write(data),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self {
                Wire::Plain(tcp) => tcp.flush(),
                Wire::Tls(stream) => stream.flush(),
            }
        }
    }

    /// Says `text` and CRLF.
    fn say(wire: &mut BufReader<Wire>, text: &str) {
        let out = wire.get_mut();
        let _ = out.write_all(format!("{text}\r\n").as_bytes()).and_then(|()| out.flush());
    }

    fn server_config(certificate: &str) -> Arc<ServerConfig> {
        let chain: Vec<_> = CertificateDer::pem_file_iter(testdata(&format!("{certificate}.pem")))
            .unwrap()
            .flatten()
            .collect();
        let key = PrivateKeyDer::from_pem_file(testdata(&format!("{certificate}.key"))).unwrap();
        let config = ServerConfig::builder_with_provider(tls::provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        Arc::new(config)
    }

    /// A submission server on lo for one connection: its port, and what it
    /// saw. `implicit` has TLS begin at once.
    fn smtp_server(script: Script, implicit: bool) -> (u16, JoinHandle<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = server_config(script.certificate);
        let server = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let start = |tcp: TcpStream| {
                let connection = ServerConnection::new(config.clone()).unwrap();
                Wire::Tls(Box::new(StreamOwned::new(connection, tcp)))
            };
            let mut wire = BufReader::new(if implicit { start(tcp) } else { Wire::Plain(tcp) });
            let mut seen = Seen::default();
            let mut tls_on = implicit;
            say(&mut wire, "220 mail.test ESMTP ready");
            loop {
                let mut line = String::new();
                if wire.read_line(&mut line).unwrap_or(0) == 0 {
                    return seen;
                }
                let line = line.trim_end().to_string();
                seen.commands.push(line.clone());
                if !tls_on {
                    seen.plaintext.push(line.clone());
                }
                let verb = line.split(' ').next().unwrap_or_default().to_ascii_uppercase();
                match verb.as_str() {
                    "EHLO" => {
                        say(&mut wire, "250-mail.test");
                        if script.starttls && !tls_on {
                            say(&mut wire, "250-STARTTLS");
                        }
                        if tls_on && !script.auth.is_empty() {
                            say(&mut wire, &format!("250-AUTH {}", script.auth.join(" ")));
                        }
                        say(&mut wire, "250 8BITMIME");
                    }
                    "STARTTLS" => {
                        say(&mut wire, "220 2.0.0 Ready to start TLS");
                        let Wire::Plain(tcp) = wire.into_inner() else { return seen };
                        wire = BufReader::new(start(tcp));
                        tls_on = true;
                    }
                    "AUTH" => {
                        let given = authenticate(&mut wire, &line, &script);
                        let ok = given == Some(script.account);
                        say(
                            &mut wire,
                            if ok {
                                "235 2.7.0 Authentication successful"
                            } else {
                                "535 5.7.8 Error: authentication failed: wrong user or password"
                            },
                        );
                    }
                    "MAIL" => say(&mut wire, "250 2.1.0 Ok"),
                    "RCPT" => {
                        let refused = script.refuses.is_some_and(|who| line.contains(who));
                        say(
                            &mut wire,
                            if refused {
                                "550 5.1.1 <who>: Recipient address rejected: User unknown"
                            } else {
                                "250 2.1.5 Ok"
                            },
                        );
                    }
                    "DATA" => {
                        say(&mut wire, "354 End data with <CR><LF>.<CR><LF>");
                        let mut message = String::new();
                        loop {
                            let mut line = String::new();
                            wire.read_line(&mut line).unwrap();
                            if line == ".\r\n" {
                                break;
                            }
                            message.push_str(line.strip_prefix('.').unwrap_or(&line));
                        }
                        seen.message = Some(message);
                        say(&mut wire, "250 2.0.0 Ok: queued as 4F1C2");
                    }
                    "QUIT" => {
                        say(&mut wire, "221 2.0.0 Bye");
                        return seen;
                    }
                    _ => say(&mut wire, "502 5.5.2 Error: command not recognized"),
                }
            }
        });
        (port, server)
    }

    /// Reads what follows AUTH, and gives the user and password it names.
    fn authenticate(
        wire: &mut BufReader<Wire>,
        line: &str,
        script: &Script,
    ) -> Option<(&'static str, &'static str)> {
        let words: Vec<&str> = line.split(' ').collect();
        let decode = |text: &str| String::from_utf8(BASE64.decode(text.trim()).unwrap()).unwrap();
        let (user, password) = match (words.get(1).copied(), words.get(2).copied()) {
            (Some("PLAIN"), Some(initial)) => {
                let text = decode(initial);
                let mut parts = text.split('\0').skip(1);
                (parts.next()?.to_string(), parts.next()?.to_string())
            }
            (Some("LOGIN"), None) => {
                say(wire, "334 VXNlcm5hbWU6");
                let mut user = String::new();
                wire.read_line(&mut user).ok()?;
                say(wire, "334 UGFzc3dvcmQ6");
                let mut password = String::new();
                wire.read_line(&mut password).ok()?;
                (decode(&user), decode(&password))
            }
            _ => return None,
        };
        let known = script.account;
        (user == known.0 && password == known.1).then_some(known)
    }

    fn client(port: u16, user: Option<(&str, &str)>) -> Server {
        Server {
            host: "127.0.0.1".into(),
            port,
            user: user.map(|(user, _)| user.to_string()),
            password: user.map(|(_, password)| Secret::new(password)),
            roots: roots(),
        }
    }

    fn message() -> Message {
        Message {
            from: "alerts@server.example.com".into(),
            to: vec!["ops@example.com".into(), "me@example.org".into()],
            subject: "[server] CRITICAL: /data is 97% full".into(),
            body: "Raised, critical: /data is 97% full\n    Free space.\n".into(),
        }
    }

    #[test]
    fn a_message_goes_over_starttls_after_the_server_offers_it() {
        let (port, server) = smtp_server(Script::new(), false);
        send_over(&client(port, None), &message(), "server", false).unwrap();
        let seen = server.join().unwrap();
        assert_eq!(
            seen.commands,
            [
                "EHLO server",
                "STARTTLS",
                "EHLO server",
                "MAIL FROM:<alerts@server.example.com>",
                "RCPT TO:<ops@example.com>",
                "RCPT TO:<me@example.org>",
                "DATA",
                "QUIT"
            ]
        );
        let text = seen.message.unwrap();
        assert!(text.contains("Subject: [server] CRITICAL: /data is 97% full\r\n"), "{text}");
        assert!(text.ends_with("Free space.\r\n"), "{text}");
        // Nothing but EHLO and STARTTLS went before the encryption.
        assert_eq!(seen.plaintext, ["EHLO server", "STARTTLS"]);
    }

    #[test]
    fn a_message_goes_over_tls_from_the_first_byte_on_port_465() {
        let (port, server) = smtp_server(Script::new(), true);
        send_over(&client(port, None), &message(), "server", true).unwrap();
        let seen = server.join().unwrap();
        assert_eq!(seen.commands[0], "EHLO server");
        assert!(seen.commands.contains(&"DATA".to_string()));
        assert!(!seen.commands.contains(&"STARTTLS".to_string()));
        assert!(seen.message.is_some());
    }

    #[test]
    fn a_server_without_starttls_is_refused_and_nothing_is_sent() {
        let script = Script { starttls: false, ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(
            &client(port, Some(("alerts@example.com", "app-password"))),
            &message(),
            "server",
            false,
        )
        .unwrap_err();
        assert_eq!(
            refused,
            "the server does not offer STARTTLS, so nothing was sent; an alert is never sent \
             without TLS (it offers: 8BITMIME)"
        );
        let seen = server.join().unwrap();
        // Only the greeting's EHLO went, and no password, sender or text.
        assert_eq!(seen.commands, ["EHLO server"]);
        assert!(seen.message.is_none());
    }

    #[test]
    fn auth_plain_is_used_when_offered() {
        let (port, server) = smtp_server(Script::new(), false);
        let account = ("alerts@example.com", "app-password");
        send_over(&client(port, Some(account)), &message(), "server", false).unwrap();
        let seen = server.join().unwrap();
        let auth = seen.commands.iter().find(|line| line.starts_with("AUTH")).unwrap();
        let initial = BASE64.encode("\0alerts@example.com\0app-password");
        assert_eq!(*auth, format!("AUTH PLAIN {initial}"));
        // The password was sent only after TLS began.
        assert!(!seen.plaintext.iter().any(|line| line.starts_with("AUTH")));
    }

    #[test]
    fn auth_login_is_used_when_it_is_all_the_server_offers() {
        let script = Script { auth: &["LOGIN"], ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let account = ("alerts@example.com", "app-password");
        send_over(&client(port, Some(account)), &message(), "server", false).unwrap();
        let seen = server.join().unwrap();
        assert!(seen.commands.contains(&"AUTH LOGIN".to_string()), "{:?}", seen.commands);
        // The server took the user and the password it was given, and so
        // took the message.
        assert!(seen.message.is_some());
    }

    #[test]
    fn a_wrong_password_is_said_in_the_servers_words() {
        let (port, server) = smtp_server(Script::new(), false);
        let refused = send_over(
            &client(port, Some(("alerts@example.com", "wrong"))),
            &message(),
            "server",
            false,
        )
        .unwrap_err();
        assert_eq!(
            refused,
            "the server refused the password (535 5.7.8 Error: authentication failed: wrong user \
             or password)"
        );
        assert!(!refused.contains("wrong\0"), "the password is not in the error");
        assert!(server.join().unwrap().message.is_none());
    }

    #[test]
    fn a_user_for_a_server_that_offers_no_sign_in_is_said() {
        let script = Script { auth: &[], ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(
            &client(port, Some(("alerts@example.com", "app-password"))),
            &message(),
            "server",
            false,
        )
        .unwrap_err();
        assert_eq!(refused, "the server offers no sign-in, but alerts.toml names a user");
        server.join().unwrap();
    }

    #[test]
    fn a_refused_recipient_fails_the_send_and_is_named() {
        let script = Script { refuses: Some("me@example.org"), ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(&client(port, None), &message(), "server", false).unwrap_err();
        assert_eq!(
            refused,
            "the server refused the recipient me@example.org (550 5.1.1 <who>: Recipient address \
             rejected: User unknown)"
        );
        assert!(server.join().unwrap().message.is_none());
    }

    #[test]
    fn a_line_that_starts_with_a_dot_is_stuffed_and_the_server_gets_it_back() {
        let mut dotted = message();
        dotted.body = "first\n.hidden\n..two\nlast\n".into();
        let (port, server) = smtp_server(Script::new(), false);
        send_over(&client(port, None), &dotted, "server", false).unwrap();
        let text = server.join().unwrap().message.unwrap();
        assert!(text.ends_with("first\r\n.hidden\r\n..two\r\nlast\r\n"), "{text}");
        assert_eq!(dot_stuffed(b".a\r\nb\r\n..c\r\n"), b"..a\r\nb\r\n...c\r\n");
        assert_eq!(dot_stuffed(b"no dots. here\r\n"), b"no dots. here\r\n");
    }

    #[test]
    fn a_certificate_from_another_authority_is_refused() {
        let script = Script { certificate: "untrusted", ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(&client(port, None), &message(), "server", false).unwrap_err();
        assert_eq!(
            refused,
            format!(
                "TLS with 127.0.0.1:{port}: its certificate is not from an authority the system \
                 trusts or the file `ca` names"
            )
        );
        assert!(server.join().unwrap().message.is_none());
    }

    #[test]
    fn a_certificate_for_another_name_is_refused() {
        let (port, server) = smtp_server(Script::new(), true);
        let mut other = client(port, None);
        other.host = "localhost".into();
        let refused = send_over(&other, &message(), "server", true).unwrap_err();
        assert_eq!(
            refused,
            format!("TLS with localhost:{port}: its certificate is not for localhost")
        );
        server.join().unwrap();
    }

    #[test]
    fn a_server_that_is_not_there_is_named() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let refused = send(&client(port, None), &message(), "server").unwrap_err();
        assert_eq!(refused, format!("cannot connect to 127.0.0.1:{port}: Connection refused"));
    }

    #[test]
    fn multi_line_replies_end_at_the_line_with_a_space() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            tcp.write_all(b"250-first\r\n250-STARTTLS\r\n250-AUTH PLAIN LOGIN\r\n250 last\r\n")
                .unwrap();
            tcp.write_all(b"554-no\r\n554 way\r\n").unwrap();
            tcp.write_all(b"hello\r\n").unwrap();
        });
        let mut session =
            Session::new(Link::Plain(TcpStream::connect(("127.0.0.1", port)).unwrap()));
        let reply = session.reply().unwrap();
        assert_eq!((reply.code, reply.lines.len()), (250, 4));
        assert!(reply.offers("starttls") && reply.offers("AUTH") && !reply.offers("first"));
        assert_eq!(reply.mechanisms(), ["PLAIN", "LOGIN"]);
        assert_eq!(session.reply().unwrap().said(), "554 no way");
        assert!(session.reply().unwrap_err().contains("not an SMTP reply"));
        server.join().unwrap();
    }

    #[test]
    fn a_servers_words_in_an_error_are_cut_to_two_hundred_characters() {
        let reply = Reply { code: 550, lines: vec!["x".repeat(500)] };
        let said = reply.said();
        assert_eq!(said.len(), "550 ".len() + SAID_MAX + "...".len());
        assert!(said.ends_with("..."));
    }

    #[test]
    fn a_message_has_the_headers_rfc_5322_asks_for() {
        let text = String::from_utf8(format(&message(), "server", 1_763_030_531, "17.3")).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let lines: Vec<&str> = head.split("\r\n").collect();
        assert_eq!(
            lines,
            [
                "Date: Thu, 13 Nov 2025 10:42:11 +0000",
                "From: alerts@server.example.com",
                "To: ops@example.com, me@example.org",
                "Subject: [server] CRITICAL: /data is 97% full",
                "Message-ID: <17.3@server>",
                "MIME-Version: 1.0",
                "Content-Type: text/plain; charset=utf-8",
                "Content-Transfer-Encoding: quoted-printable",
                "Auto-Submitted: auto-generated",
            ]
        );
        assert_eq!(body, "Raised, critical: /data is 97% full\r\n    Free space.\r\n");
    }

    #[test]
    fn the_date_has_its_weekday_and_month() {
        assert_eq!(date(0), "Thu, 1 Jan 1970 00:00:00 +0000");
        assert_eq!(date(1_763_030_531), "Thu, 13 Nov 2025 10:42:11 +0000");
        assert_eq!(date(951_782_400), "Tue, 29 Feb 2000 00:00:00 +0000");
        assert_eq!(date(4_102_444_799), "Thu, 31 Dec 2099 23:59:59 +0000");
    }

    /// Reads quoted-printable back, as a mail program does.
    fn decoded(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'=' if bytes[at + 1..].starts_with(b"\r\n") => at += 3,
                b'=' => {
                    let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    at += 3;
                }
                byte => {
                    out.push(byte);
                    at += 1;
                }
            }
        }
        out
    }

    #[test]
    fn quoted_printable_keeps_ascii_and_encodes_the_rest() {
        assert_eq!(quoted_printable("plain text\n"), "plain text\r\n");
        assert_eq!(quoted_printable("a=b\n"), "a=3Db\r\n");
        assert_eq!(quoted_printable("é\n"), "=C3=A9\r\n");
        assert_eq!(quoted_printable("trailing space \n"), "trailing space=20\r\n");
        assert_eq!(quoted_printable("tab\t\n"), "tab=09\r\n");
        assert_eq!(quoted_printable("a b\tc\n"), "a b\tc\r\n");
        assert_eq!(quoted_printable("one\n\ntwo"), "one\r\n\r\ntwo\r\n");
        assert_eq!(quoted_printable("crlf\r\nlines\r\n"), "crlf\r\nlines\r\n");
        assert_eq!(quoted_printable(""), "");
    }

    #[test]
    fn quoted_printable_lines_are_at_most_76_characters_and_read_back() {
        let long = "word ".repeat(60);
        let accents = "é=ü ".repeat(40);
        let unbroken = "x".repeat(200);
        let edge = format!("{}é\n", "y".repeat(74));
        for text in [long, accents, unbroken, edge] {
            let encoded = quoted_printable(&text);
            for line in encoded.split("\r\n") {
                assert!(line.len() <= 76, "{} columns: {line}", line.len());
            }
            let mut wanted = text.replace('\n', "\r\n");
            if !text.ends_with('\n') {
                wanted.push_str("\r\n");
            }
            assert_eq!(String::from_utf8(decoded(&encoded)).unwrap(), wanted);
        }
    }

    #[test]
    fn a_subject_that_is_not_ascii_is_rfc_2047_words() {
        assert_eq!(mime_words("[server] Warning: disk"), "[server] Warning: disk");
        let words = mime_words("[serveur] Avertissement : le disque été plein");
        assert!(words.starts_with("=?utf-8?B?") && words.ends_with("?="), "{words}");
        let mut text = Vec::new();
        for word in words.split(' ') {
            let payload = word.strip_prefix("=?utf-8?B?").unwrap().strip_suffix("?=").unwrap();
            assert!(word.len() <= 75, "{word}");
            text.extend(BASE64.decode(payload).unwrap());
        }
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "[serveur] Avertissement : le disque été plein"
        );
    }

    #[test]
    fn a_long_non_ascii_subject_is_split_at_characters_and_folded() {
        let subject = "é".repeat(60);
        let mut mail = message();
        mail.subject = subject.clone();
        let text = String::from_utf8(format(&mail, "server", 0, "1")).unwrap();
        let head = text.split("\r\n\r\n").next().unwrap();
        let folded: Vec<&str> =
            head.split("\r\n").skip_while(|line| !line.starts_with("Subject:")).collect();
        let mut got = String::new();
        for line in
            folded.iter().take_while(|line| line.starts_with("Subject:") || line.starts_with(' '))
        {
            assert!(line.len() <= 76, "{line}");
            for word in line.trim_start_matches("Subject:").split_whitespace() {
                let payload = word.strip_prefix("=?utf-8?B?").unwrap().strip_suffix("?=").unwrap();
                got.push_str(&String::from_utf8(BASE64.decode(payload).unwrap()).unwrap());
            }
        }
        assert_eq!(got, subject);
    }

    #[test]
    fn a_subject_cannot_start_another_header() {
        assert_eq!(mime_words("one\r\nBcc: x@y.test"), "one  Bcc: x@y.test");
    }

    #[test]
    fn ehlo_names_the_machine_or_localhost() {
        assert_eq!(greeting_name("server.example.com"), "server.example.com");
        assert_eq!(greeting_name("my server"), "localhost");
        assert_eq!(greeting_name(""), "localhost");
        assert_eq!(greeting_name("a\\r\\nb"), "localhost");
    }
}
