// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sending one message through the submission server (ADR 0485): RFC 6409
//! on port 587 with STARTTLS (RFC 3207), or TLS from the first byte on
//! port 465 (RFC 8314). A server that does not offer STARTTLS on 587 is
//! refused before anything is sent: mail never goes in the clear, and
//! neither does a password. The certificate must chain to the system's
//! roots or to the file `ca` names, and be valid for the server's name.
//!
//! alertd(8) sends its email through [`deliver`], with the server its
//! `[email]` table names; sendmail(8) and the programs that mail a
//! person's output, timerd(8) for a crontab's `MAILTO`, send through
//! [`send`], which reads the same table (alerts.toml(5)), so that one
//! account serves them all. There is no queue: a message that cannot go
//! now is the caller's failure, and [`Error::kind`] says whether trying
//! again later could help.
//!
//! Every step checks the server's reply code, and an error names the step
//! and quotes the server's own words, so that `alerts test` shows why a
//! provider refused ("the server refused the password (535 5.7.8 ...)").
//! Authentication is AUTH PLAIN (RFC 4616), or AUTH LOGIN when that is all
//! the server offers. The unit tests run the protocol against a server on
//! lo with the certificates of tls-test.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rustls::RootCertStore;
use rustls::pki_types::ServerName;

use crate::alertd::settings::{self, Secret, Target};
use crate::alertd::trouble;
use crate::{logd, tls, util};

/// How long connecting may take, and a read or a write.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = trouble::SILENCE;

/// The longest reply line and the most reply lines read: RFC 5321 allows
/// 512 bytes of reply line.
const REPLY_LINE_MAX: usize = 1000;
const REPLY_LINES_MAX: usize = 100;

/// Whose certificates count, for the words of a refused certificate.
const TRUSTED: &str = "the system trusts or the file `ca` names";

/// The most characters of a server's words in an error.
const SAID_MAX: usize = 200;

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

/// What a failed send says about trying again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No settings name a server, or they cannot be read.
    NoServer,
    /// The server could not be reached, went silent, hung up, or said
    /// to try later (a 4yz reply): the same message may go later.
    Temporary,
    /// The server, or its certificate, refused it for good: sending the
    /// same message again fails the same way until something changes.
    Refused,
}

/// Why a message was not sent: in words, with what to do about it.
#[derive(Clone, Debug)]
pub struct Error {
    pub kind: Kind,
    pub message: String,
}

impl Error {
    fn refused(message: impl Into<String>) -> Error {
        Error { kind: Kind::Refused, message: message.into() }
    }

    fn no_server(message: impl Into<String>) -> Error {
        Error { kind: Kind::NoServer, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// A failure of the connection: trying again later may work.
impl From<String> for Error {
    fn from(message: String) -> Error {
        Error { kind: Kind::Temporary, message }
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Error {
        Error::from(message.to_string())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> String {
        error.message
    }
}

/// Sends `message`, a whole RFC 5322 message with CRLF line ends, from
/// envelope sender `from` (the `[email]` table's `from` when none) to
/// each of `to`, through the server alerts.toml(5)'s `[email]` table
/// names.
///
/// # Errors
///
/// No server named in the settings, or the step that failed and the
/// server's words, as [`deliver`].
pub fn send(from: Option<&str>, to: &[String], message: &[u8]) -> Result<(), Error> {
    let host = util::hostname();
    let (channels, wrong) = settings::load(&host);
    let email = channels.channels.into_iter().find_map(|channel| match channel.target {
        Target::Email(email) => Some(email),
        Target::Webhook(_) => None,
    });
    let Some(email) = email else {
        let why = wrong.first().cloned().unwrap_or_else(|| {
            format!("{} names no [email] server to send through (alerts.toml(5))", settings::FILE)
        });
        return Err(Error::no_server(why));
    };
    let extra: Vec<String> =
        email.ca.iter().filter_map(|path| path.to_str().map(str::to_string)).collect();
    let server = Server {
        host: email.server,
        port: email.port,
        user: email.user,
        password: email.password,
        roots: Arc::new(tls::roots(&extra, "mail")),
    };
    deliver(&server, from.unwrap_or(&email.from), to, message, &host)
}

/// Sends `message` from `from` to each of `to` through `server`, saying
/// `hostname` for itself in EHLO.
///
/// # Errors
///
/// The step that failed and the server's words: the connection, the
/// certificate, the lack of STARTTLS, the refusal of the password, of
/// the sender or of a recipient, or of the message.
pub fn deliver(
    server: &Server,
    from: &str,
    to: &[String],
    message: &[u8],
    hostname: &str,
) -> Result<(), Error> {
    deliver_over(server, from, to, message, hostname, server.port == 465)
}

/// As [`deliver`], with TLS from the first byte when `implicit`, whatever
/// the port, as the tests need.
pub(crate) fn deliver_over(
    server: &Server,
    from: &str,
    to: &[String],
    message: &[u8],
    hostname: &str,
    implicit: bool,
) -> Result<(), Error> {
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
            return Err(Error::refused(format!(
                "the server does not offer STARTTLS, so nothing was sent; mail is never sent \
                 without TLS (it offers: {})",
                offered.extensions()
            )));
        }
        session.command("STARTTLS", &[220], "the server refused STARTTLS")?;
        session = session.upgrade(&tls)?;
        offered = session.command(&hello, &[250], "the server refused EHLO after STARTTLS")?;
    }
    if let (Some(user), Some(password)) = (&server.user, &server.password) {
        session.authenticate(&offered, user, password.expose())?;
    }
    session.deliver(from, to, message)?;
    // The message is accepted; a server that cannot say goodbye changes
    // nothing.
    let _ = session.command("QUIT", &[221], "");
    Ok(())
}

/// name SMTP takes.
pub(crate) fn greeting_name(hostname: &str) -> &str {
    let name = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    if !hostname.is_empty() && hostname.chars().all(name) { hostname } else { "localhost" }
}

/// Connects to `host`'s first address that answers.
fn connect(host: &str, port: u16) -> Result<TcpStream, Error> {
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
    Err(cannot(&last.map_or("no address".into(), |err| util::strip_os_error(&err))).into())
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
    fn new(server: &Server) -> Result<Tls, Error> {
        let name = ServerName::try_from(server.host.clone())
            .map_err(|_| Error::refused(format!("'{}' is not a server name", server.host)))?;
        Ok(Tls {
            config: tls::verified((*server.roots).clone(), &[]),
            name,
            host: server.host.clone(),
            port: server.port,
        })
    }

    fn handshake(&self, tcp: TcpStream) -> Result<tls::Stream, Error> {
        tls::handshake(tcp, self.name.clone(), &self.config).map_err(|err| {
            let why = trouble::connection(&err, &self.host, TRUSTED);
            let message = format!("TLS with {}:{}: {why}", self.host, self.port);
            // A certificate or protocol refusal stays the same on the next
            // try; a connection that broke does not.
            if err.get_ref().is_some_and(<dyn std::error::Error + Send + Sync>::is::<rustls::Error>)
            {
                Error::refused(message)
            } else {
                Error::from(message)
            }
        })
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
        format!("{} {}", self.code, trouble::cut(&self.lines.join(" "), SAID_MAX))
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
    fn upgrade(self, tls: &Tls) -> Result<Session, Error> {
        // Anything the server sent after its 220 and before the handshake
        // is not from the TLS peer: it could be a plaintext command
        // slipped in (RFC 3207's note on command injection).
        if !self.link.buffer().is_empty() {
            return Err(Error::refused(
                "the server sent data after its answer to STARTTLS, before the TLS handshake; \
                 nothing was sent",
            ));
        }
        let Link::Plain(tcp) = self.link.into_inner() else {
            return Err(Error::refused("TLS is on already"));
        };
        Ok(Session::new(Link::Tls(Box::new(tls.handshake(tcp)?))))
    }

    /// Writes `line` and CRLF.
    fn write_line(&mut self, line: &str) -> Result<(), Error> {
        let link = self.link.get_mut();
        link.write_all(line.as_bytes())
            .and_then(|()| link.write_all(b"\r\n"))
            .and_then(|()| link.flush())
            .map_err(|err| {
                Error::from(format!("cannot write to the server: {}", util::strip_os_error(&err)))
            })
    }

    /// Reads one line of a reply.
    fn read_line(&mut self) -> Result<String, Error> {
        let mut line = Vec::new();
        let limit = (REPLY_LINE_MAX + 1) as u64;
        let read = (&mut self.link).take(limit).read_until(b'\n', &mut line);
        match read {
            Ok(0) => Err("the server closed the connection".into()),
            Ok(_) if !line.ends_with(b"\n") => {
                Err(format!("the server sent a reply line longer than {REPLY_LINE_MAX} bytes")
                    .into())
            }
            Ok(_) => Ok(String::from_utf8_lossy(&line).trim_end().to_string()),
            Err(err)
                if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
            {
                Err(format!("the server did not answer within {} seconds", IO_TIMEOUT.as_secs())
                    .into())
            }
            Err(err) => {
                Err(format!("cannot read from the server: {}", util::strip_os_error(&err)).into())
            }
        }
    }

    /// Reads a whole reply: lines `250-...` up to the one `250 ...`.
    fn reply(&mut self) -> Result<Reply, Error> {
        let mut lines = Vec::new();
        for _ in 0..REPLY_LINES_MAX {
            let line = self.read_line()?;
            let code: u16 = line.get(..3).and_then(|code| code.parse().ok()).ok_or_else(|| {
                Error::refused(format!("the server sent {line:?}, which is not an SMTP reply"))
            })?;
            let rest = line.get(3..).unwrap_or_default();
            lines.push(rest.strip_prefix(['-', ' ']).unwrap_or(rest).to_string());
            if !rest.starts_with('-') {
                return Ok(Reply { code, lines });
            }
        }
        Err(Error::refused(format!(
            "the server sent more than {REPLY_LINES_MAX} lines in one reply"
        )))
    }

    /// Reads a reply, which must have one of `codes`; `refused` says what
    /// the server did otherwise.
    fn expect(&mut self, codes: &[u16], refused: &str) -> Result<Reply, Error> {
        let reply = self.reply()?;
        if codes.contains(&reply.code) {
            Ok(reply)
        } else {
            let message = format!("{refused} ({})", reply.said());
            // A 4yz reply asks to try again later; 5yz does not.
            Err(if reply.code / 100 == 4 { Error::from(message) } else { Error::refused(message) })
        }
    }

    /// Sends `line` and reads the reply to it, as [`Session::expect`].
    fn command(&mut self, line: &str, codes: &[u16], refused: &str) -> Result<Reply, Error> {
        self.write_line(line)?;
        self.expect(codes, refused)
    }

    /// Signs in with AUTH PLAIN, or AUTH LOGIN when that is all the
    /// server offers. Only ever called over TLS.
    fn authenticate(&mut self, offered: &Reply, user: &str, password: &str) -> Result<(), Error> {
        let mechanisms = offered.mechanisms();
        let refused = |reply: &Reply| {
            let what = if reply.code == 535 {
                "the server refused the password"
            } else {
                "the server could not sign in"
            };
            let message = format!("{what} ({})", reply.said());
            if reply.code / 100 == 4 { Error::from(message) } else { Error::refused(message) }
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
            return Err(Error::refused(
                "the server offers no sign-in, but alerts.toml names a user",
            ));
        } else {
            return Err(Error::refused(format!(
                "the server offers sign-in by {}, and only PLAIN and LOGIN are used",
                mechanisms.join(", ")
            )));
        };
        if done.code == 235 { Ok(()) } else { Err(refused(&done)) }
    }

    /// Gives the server the envelope and `text`, dot-stuffed.
    fn deliver(&mut self, from: &str, to: &[String], text: &[u8]) -> Result<(), Error> {
        let from = format!("MAIL FROM:<{from}>");
        self.command(&from, &[250], "the server refused the sender")?;
        for address in to {
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

/// `seconds` since the epoch as RFC 5322's date, in UTC:
/// `Fri, 13 Nov 2026 10:42:11 +0000`.
pub fn date(seconds: u64) -> String {
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

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::alertd::settings::Secret;
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

    pub(crate) fn roots() -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        let ca = CertificateDer::pem_file_iter(testdata("ca.pem")).unwrap().flatten();
        roots.add_parsable_certificates(ca);
        Arc::new(roots)
    }

    /// What the server in a test does.
    #[derive(Clone)]
    pub(crate) struct Script {
        /// The certificate it shows, a name in tls-test's testdata.
        pub(crate) certificate: &'static str,
        /// Whether it offers STARTTLS (when it is not TLS already).
        pub(crate) starttls: bool,
        /// The mechanisms it offers once TLS is on.
        pub(crate) auth: &'static [&'static str],
        /// The user and password it takes.
        pub(crate) account: (&'static str, &'static str),
        /// A recipient it refuses.
        pub(crate) refuses: Option<&'static str>,
    }

    impl Script {
        pub(crate) fn new() -> Script {
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
    pub(crate) struct Seen {
        pub(crate) commands: Vec<String>,
        pub(crate) message: Option<String>,
        /// Whether any command came before TLS began.
        pub(crate) plaintext: Vec<String>,
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
    pub(crate) fn smtp_server(script: Script, implicit: bool) -> (u16, JoinHandle<Seen>) {
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

    pub(crate) fn client(port: u16, user: Option<(&str, &str)>) -> Server {
        Server {
            host: "127.0.0.1".into(),
            port,
            user: user.map(|(user, _)| user.to_string()),
            password: user.map(|(_, password)| Secret::new(password)),
            roots: roots(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Script, client, smtp_server};
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn to() -> Vec<String> {
        vec!["ops@example.com".into(), "me@example.org".into()]
    }

    const FROM: &str = "alerts@server.example.com";
    const TEXT: &[u8] = b"Subject: hello\r\n\r\nbody\r\n";

    fn send_over(server: &Server, text: &[u8], implicit: bool) -> Result<(), Error> {
        deliver_over(server, FROM, &to(), text, "server", implicit)
    }

    #[test]
    fn a_message_goes_over_starttls_after_the_server_offers_it() {
        let (port, server) = smtp_server(Script::new(), false);
        send_over(&client(port, None), TEXT, false).unwrap();
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
        assert!(text.contains("Subject: hello\r\n"), "{text}");
        assert!(text.ends_with("body\r\n"), "{text}");
        // Nothing but EHLO and STARTTLS went before the encryption.
        assert_eq!(seen.plaintext, ["EHLO server", "STARTTLS"]);
    }

    #[test]
    fn a_message_goes_over_tls_from_the_first_byte_on_port_465() {
        let (port, server) = smtp_server(Script::new(), true);
        send_over(&client(port, None), TEXT, true).unwrap();
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
        let refused =
            send_over(&client(port, Some(("alerts@example.com", "app-password"))), TEXT, false)
                .unwrap_err()
                .message;
        assert_eq!(
            refused,
            "the server does not offer STARTTLS, so nothing was sent; mail is never sent \
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
        send_over(&client(port, Some(account)), TEXT, false).unwrap();
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
        send_over(&client(port, Some(account)), TEXT, false).unwrap();
        let seen = server.join().unwrap();
        assert!(seen.commands.contains(&"AUTH LOGIN".to_string()), "{:?}", seen.commands);
        // The server took the user and the password it was given, and so
        // took the message.
        assert!(seen.message.is_some());
    }

    #[test]
    fn a_wrong_password_is_said_in_the_servers_words() {
        let (port, server) = smtp_server(Script::new(), false);
        let refused = send_over(&client(port, Some(("alerts@example.com", "wrong"))), TEXT, false)
            .unwrap_err()
            .message;
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
        let refused =
            send_over(&client(port, Some(("alerts@example.com", "app-password"))), TEXT, false)
                .unwrap_err()
                .message;
        assert_eq!(refused, "the server offers no sign-in, but alerts.toml names a user");
        server.join().unwrap();
    }

    #[test]
    fn a_refused_recipient_fails_the_send_and_is_named() {
        let script = Script { refuses: Some("me@example.org"), ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(&client(port, None), TEXT, false).unwrap_err().message;
        assert_eq!(
            refused,
            "the server refused the recipient me@example.org (550 5.1.1 <who>: Recipient address \
             rejected: User unknown)"
        );
        assert!(server.join().unwrap().message.is_none());
    }

    #[test]
    fn a_line_that_starts_with_a_dot_is_stuffed_and_the_server_gets_it_back() {
        let dotted = b"Subject: dots\r\n\r\nfirst\r\n.hidden\r\n..two\r\nlast\r\n";
        let (port, server) = smtp_server(Script::new(), false);
        send_over(&client(port, None), dotted, false).unwrap();
        let text = server.join().unwrap().message.unwrap();
        assert!(text.ends_with("first\r\n.hidden\r\n..two\r\nlast\r\n"), "{text}");
        assert_eq!(dot_stuffed(b".a\r\nb\r\n..c\r\n"), b"..a\r\nb\r\n...c\r\n");
        assert_eq!(dot_stuffed(b"no dots. here\r\n"), b"no dots. here\r\n");
    }

    #[test]
    fn a_certificate_from_another_authority_is_refused() {
        let script = Script { certificate: "untrusted", ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(&client(port, None), TEXT, false).unwrap_err().message;
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
        let refused = send_over(&other, TEXT, true).unwrap_err().message;
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
        let refused =
            deliver(&client(port, None), FROM, &to(), TEXT, "server").unwrap_err().message;
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
        assert!(session.reply().unwrap_err().message.contains("not an SMTP reply"));
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
    fn a_failure_says_whether_trying_again_could_help() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let unreachable = send_over(&client(port, None), TEXT, false).unwrap_err();
        assert_eq!(unreachable.kind, Kind::Temporary);
        let script = Script { starttls: false, ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let no_tls = send_over(&client(port, None), TEXT, false).unwrap_err();
        assert_eq!(no_tls.kind, Kind::Refused);
        server.join().unwrap();
        let script = Script { refuses: Some("me@example.org"), ..Script::new() };
        let (port, server) = smtp_server(script, false);
        let refused = send_over(&client(port, None), TEXT, false).unwrap_err();
        assert_eq!(refused.kind, Kind::Refused);
        server.join().unwrap();
        let (port, server) =
            smtp_server(Script { certificate: "untrusted", ..Script::new() }, false);
        assert_eq!(send_over(&client(port, None), TEXT, false).unwrap_err().kind, Kind::Refused);
        server.join().unwrap();
    }

    #[test]
    fn the_greeting_names_the_machine_or_localhost() {
        assert_eq!(greeting_name("server.example.com"), "server.example.com");
        assert_eq!(greeting_name("my server"), "localhost");
        assert_eq!(greeting_name(""), "localhost");
    }
}
