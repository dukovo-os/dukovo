// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The mail server of the boot test: a submission server (RFC 6409) that
//! offers STARTTLS (RFC 3207) and takes AUTH PLAIN or LOGIN only once TLS
//! is on, as a provider's does. It refuses MAIL before TLS and before a
//! sign-in with 530, so that a message that arrives has gone over TLS, and
//! it writes each message it takes to [`MAIL`](crate::records::MAIL) with
//! the envelope, the account and whether TLS was on.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::base64;
use crate::config::{PASSWORD, USER};
use crate::records::{MAIL, Records};

/// How long a client may be silent before the connection ends.
const PATIENCE: Duration = Duration::from_secs(20);

/// The longest line a client may send, and the largest message.
const LINE_MAX: u64 = 64 * 1024;
const MESSAGE_MAX: usize = 1 << 20;

/// A connection, before and after STARTTLS.
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

/// What a command leads to.
enum Next {
    Go,
    Quit,
    /// The 220 to STARTTLS is said; the handshake is next.
    Encrypt,
}

/// One client's session.
struct Session {
    wire: BufReader<Wire>,
    config: Arc<ServerConfig>,
    /// The account the client signed in as.
    user: Option<String>,
    sender: Option<String>,
    recipients: Vec<String>,
}

/// Serves the client on `tcp` until it quits or the connection ends,
/// writing each message to `records`.
///
/// # Errors
///
/// Why the connection failed: the client left, was silent for too long,
/// or its handshake was wrong.
pub fn serve(tcp: TcpStream, config: Arc<ServerConfig>, records: &Records) -> io::Result<()> {
    tcp.set_read_timeout(Some(PATIENCE))?;
    let session = Session::new(Wire::Plain(tcp), config);
    session.run(records)
}

impl Session {
    fn new(wire: Wire, config: Arc<ServerConfig>) -> Session {
        Session {
            wire: BufReader::new(wire),
            config,
            user: None,
            sender: None,
            recipients: Vec::new(),
        }
    }

    fn run(mut self, records: &Records) -> io::Result<()> {
        self.say("220 monitoring-test ESMTP ready")?;
        while let Some(line) = self.line()? {
            match self.command(&line, records)? {
                Next::Go => {}
                Next::Quit => break,
                Next::Encrypt => self = self.encrypted()?,
            }
        }
        Ok(())
    }

    /// The session after the handshake: all the client said is forgotten
    /// (RFC 3207), and what follows is in TLS.
    fn encrypted(self) -> io::Result<Session> {
        // The client waits for the 220 before it sends the handshake, so
        // nothing is buffered that the handshake needs.
        let Wire::Plain(tcp) = self.wire.into_inner() else {
            return Err(io::Error::other("TLS began twice"));
        };
        let connection = ServerConnection::new(self.config.clone()).map_err(io::Error::other)?;
        let wire = Wire::Tls(Box::new(StreamOwned::new(connection, tcp)));
        Ok(Session::new(wire, self.config))
    }

    fn is_encrypted(&self) -> bool {
        matches!(self.wire.get_ref(), Wire::Tls(_))
    }

    /// Says `text` and CRLF.
    fn say(&mut self, text: &str) -> io::Result<()> {
        let wire = self.wire.get_mut();
        wire.write_all(format!("{text}\r\n").as_bytes())?;
        wire.flush()
    }

    /// The next line the client sends, without its line break, or `None`
    /// when it has gone.
    fn line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        let got = (&mut self.wire).take(LINE_MAX).read_line(&mut line)?;
        if got == 0 {
            return Ok(None);
        }
        if !line.ends_with('\n') {
            return Err(io::Error::other("a line of more than 64 KiB"));
        }
        Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
    }

    /// Forgets the message being made.
    fn forget(&mut self) {
        self.sender = None;
        self.recipients.clear();
    }

    fn command(&mut self, line: &str, records: &Records) -> io::Result<Next> {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        match verb.to_ascii_uppercase().as_str() {
            "EHLO" | "HELO" => self.hello()?,
            "STARTTLS" => return self.start_tls(),
            "AUTH" => self.auth(rest)?,
            "MAIL" => self.mail(rest)?,
            "RCPT" => self.rcpt(rest)?,
            "DATA" => self.data(records)?,
            "RSET" => {
                self.forget();
                self.say("250 2.0.0 Ok")?;
            }
            "NOOP" => self.say("250 2.0.0 Ok")?,
            "QUIT" => {
                self.say("221 2.0.0 Bye")?;
                return Ok(Next::Quit);
            }
            _ => self.say("502 5.5.2 Command not recognized")?,
        }
        Ok(Next::Go)
    }

    /// EHLO: the extensions, with the sign-in only once TLS is on.
    fn hello(&mut self) -> io::Result<()> {
        self.forget();
        self.say("250-monitoring-test")?;
        if self.is_encrypted() {
            self.say("250-AUTH PLAIN LOGIN")?;
        } else {
            self.say("250-STARTTLS")?;
        }
        self.say("250 8BITMIME")
    }

    fn start_tls(&mut self) -> io::Result<Next> {
        if self.is_encrypted() {
            self.say("503 5.5.1 TLS is on already")?;
            return Ok(Next::Go);
        }
        self.say("220 2.0.0 Ready to start TLS")?;
        Ok(Next::Encrypt)
    }

    /// AUTH, with the account of [`config`](crate::config) alone.
    fn auth(&mut self, rest: &str) -> io::Result<()> {
        if !self.is_encrypted() {
            return self.say("530 5.7.0 Must issue a STARTTLS command first");
        }
        let mut words = rest.split_whitespace();
        let mechanism = words.next().unwrap_or_default().to_ascii_uppercase();
        let given = match (mechanism.as_str(), words.next()) {
            ("PLAIN", Some(initial)) => plain(initial),
            ("PLAIN", None) => {
                self.say("334 ")?;
                let reply = self.line()?.unwrap_or_default();
                plain(&reply)
            }
            ("LOGIN", None) => self.login()?,
            _ => return self.say("504 5.5.4 Authentication mechanism not supported"),
        };
        if given.is_some_and(|(user, password)| user == USER && password == PASSWORD) {
            self.user = Some(USER.to_string());
            self.say("235 2.7.0 Authentication successful")
        } else {
            self.say("535 5.7.8 Authentication credentials invalid")
        }
    }

    /// AUTH LOGIN's two prompts and the answers to them.
    fn login(&mut self) -> io::Result<Option<(String, String)>> {
        self.say("334 VXNlcm5hbWU6")?;
        let user = self.line()?.and_then(|line| text(&line));
        self.say("334 UGFzc3dvcmQ6")?;
        let password = self.line()?.and_then(|line| text(&line));
        Ok(user.zip(password))
    }

    fn mail(&mut self, rest: &str) -> io::Result<()> {
        if !self.is_encrypted() {
            return self.say("530 5.7.0 Must issue a STARTTLS command first");
        }
        if self.user.is_none() {
            return self.say("530 5.7.0 Authentication required");
        }
        self.forget();
        self.sender = Some(address(rest, "FROM:"));
        self.say("250 2.1.0 Ok")
    }

    fn rcpt(&mut self, rest: &str) -> io::Result<()> {
        if self.sender.is_none() {
            return self.say("503 5.5.1 Error: need MAIL command");
        }
        self.recipients.push(address(rest, "TO:"));
        self.say("250 2.1.5 Ok")
    }

    /// DATA: the message up to the line with the dot alone, with the dots
    /// the client added taken off (RFC 5321 §4.5.2).
    fn data(&mut self, records: &Records) -> io::Result<()> {
        let Some(sender) = self.sender.clone() else {
            return self.say("503 5.5.1 Error: need MAIL command");
        };
        if self.recipients.is_empty() {
            return self.say("503 5.5.1 Error: need RCPT command");
        }
        self.say("354 End data with <CR><LF>.<CR><LF>")?;
        let mut message = String::new();
        loop {
            let Some(line) = self.line()? else {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "ended inside DATA"));
            };
            if line == "." {
                break;
            }
            message.push_str(line.strip_prefix('.').unwrap_or(&line));
            message.push('\n');
            if message.len() > MESSAGE_MAX {
                return Err(io::Error::other("a message of more than 1 MiB"));
            }
        }
        let block = block(&sender, &self.recipients, self.user.as_deref(), &message);
        records.add(MAIL, &block)?;
        self.forget();
        self.say("250 2.0.0 Ok: queued")
    }
}

/// The user and password of an AUTH PLAIN answer (RFC 4616): the bytes
/// `authorization-id NUL user NUL password`.
fn plain(encoded: &str) -> Option<(String, String)> {
    let decoded = String::from_utf8(base64::decode(encoded)?).ok()?;
    let mut parts = decoded.split('\0').skip(1);
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

/// The text a line of base64 stands for.
fn text(encoded: &str) -> Option<String> {
    String::from_utf8(base64::decode(encoded)?).ok()
}

/// The address in `<...>` of what follows MAIL or RCPT, after `prefix`.
fn address(rest: &str, prefix: &str) -> String {
    let argument = rest.get(..prefix.len()).filter(|start| start.eq_ignore_ascii_case(prefix));
    let after = argument.map_or(rest, |_| &rest[prefix.len()..]);
    let first = after.split_whitespace().next().unwrap_or_default();
    first.trim_start_matches('<').trim_end_matches('>').to_string()
}

/// What goes in the file for one message.
fn block(sender: &str, recipients: &[String], user: Option<&str>, message: &str) -> String {
    let mut block = String::from("== mail\n");
    block.push_str(&format!("envelope-from: <{sender}>\n"));
    for recipient in recipients {
        block.push_str(&format!("envelope-to: <{recipient}>\n"));
    }
    block.push_str(&format!("user: {}\n", user.unwrap_or("none")));
    block.push_str("tls: yes\n-- message\n");
    block.push_str(message);
    block.push_str("== end\n");
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls;

    use std::fs;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use rustls::crypto::ring;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore};

    /// The client's end of a connection, before and after STARTTLS.
    enum Link {
        Plain(TcpStream),
        Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
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

    /// A client that reads a reply a line at a time.
    struct Client {
        link: BufReader<Link>,
    }

    impl Client {
        /// Sends `line` and gives the reply: its lines, the last one's
        /// code a space after.
        fn ask(&mut self, line: &str) -> Vec<String> {
            let link = self.link.get_mut();
            link.write_all(format!("{line}\r\n").as_bytes()).unwrap();
            link.flush().unwrap();
            self.reply()
        }

        fn reply(&mut self) -> Vec<String> {
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                self.link.read_line(&mut line).unwrap();
                let line = line.trim_end().to_string();
                let last = line.as_bytes().get(3) != Some(&b'-');
                lines.push(line);
                if last {
                    return lines;
                }
            }
        }

        /// Asks for STARTTLS and begins the handshake, as a client that
        /// trusts the test authority does.
        fn encrypted(mut self) -> Client {
            assert!(self.ask("STARTTLS")[0].starts_with("220"));
            let mut roots = RootCertStore::empty();
            roots.add_parsable_certificates(
                CertificateDer::pem_slice_iter(tls::authority()).flatten(),
            );
            let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = ServerName::try_from("127.0.0.1").unwrap();
            let connection = ClientConnection::new(Arc::new(config), name).unwrap();
            let Link::Plain(tcp) = self.link.into_inner() else { panic!("TLS began twice") };
            let link = Link::Tls(Box::new(StreamOwned::new(connection, tcp)));
            Client { link: BufReader::new(link) }
        }
    }

    /// The folder a test's records go to.
    fn folder(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("monitoring-smtp-{name}-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// The server for one connection, and a client of it that has read
    /// the greeting.
    fn connect(dir: &std::path::Path) -> (Client, JoinHandle<io::Result<()>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let records = Records::start(dir).unwrap();
        let config = tls::server_config().unwrap();
        let server = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            serve(tcp, config, &records)
        });
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut client = Client { link: BufReader::new(Link::Plain(tcp)) };
        assert_eq!(client.reply(), ["220 monitoring-test ESMTP ready"]);
        (client, server)
    }

    /// AUTH PLAIN's initial response for `user` and `password`.
    fn plain_for(user: &str, password: &str) -> String {
        // Base64 of the three parts, made by hand to keep the test
        // independent of the decoder it checks.
        let bytes = format!("\0{user}\0{password}").into_bytes();
        const DIGITS: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let word = chunk
                .iter()
                .enumerate()
                .fold(0u32, |word, (at, byte)| word | (u32::from(*byte) << (16 - 8 * at)));
            for at in 0..4 {
                if at <= chunk.len() {
                    out.push(DIGITS[((word >> (18 - 6 * at)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn a_message_goes_over_starttls_after_a_sign_in_and_is_recorded_whole() {
        let dir = folder("whole");
        let (mut client, server) = connect(&dir);
        let offered = client.ask("EHLO test");
        assert!(offered.contains(&"250-STARTTLS".to_string()), "{offered:?}");
        assert!(!offered.iter().any(|line| line.contains("AUTH")), "{offered:?}");
        // Nothing is taken before TLS.
        assert!(client.ask("MAIL FROM:<a@example.test>")[0].starts_with("530 5.7.0 Must issue"));
        assert!(client.ask("AUTH PLAIN AGE=")[0].starts_with("530"));

        let mut client = client.encrypted();
        let offered = client.ask("EHLO test");
        assert!(offered.contains(&"250-AUTH PLAIN LOGIN".to_string()), "{offered:?}");
        assert!(!offered.contains(&"250-STARTTLS".to_string()));
        assert!(
            client.ask("MAIL FROM:<a@example.test>")[0].starts_with("530 5.7.0 Authentication")
        );
        let wrong = client.ask(&format!("AUTH PLAIN {}", plain_for(USER, "coffee")));
        assert_eq!(wrong, ["535 5.7.8 Authentication credentials invalid"]);
        let right = client.ask(&format!("AUTH PLAIN {}", plain_for(USER, PASSWORD)));
        assert_eq!(right, ["235 2.7.0 Authentication successful"]);

        assert!(client.ask("DATA")[0].starts_with("503"));
        assert!(client.ask("MAIL FROM:<alerts@example.test>")[0].starts_with("250"));
        assert!(client.ask("RCPT TO:<ops@example.test>")[0].starts_with("250"));
        assert!(client.ask("RCPT TO:<me@example.test> NOTIFY=NEVER")[0].starts_with("250"));
        assert!(client.ask("DATA")[0].starts_with("354"));
        let link = client.link.get_mut();
        link.write_all(b"Subject: Test\r\n\r\n..one dot\r\nplain\r\n.\r\n").unwrap();
        link.flush().unwrap();
        assert!(client.reply()[0].starts_with("250 2.0.0 Ok"));
        assert!(client.ask("QUIT")[0].starts_with("221"));
        server.join().unwrap().unwrap();

        let mail = fs::read_to_string(dir.join(MAIL)).unwrap();
        assert_eq!(
            mail,
            "== mail\nenvelope-from: <alerts@example.test>\nenvelope-to: <ops@example.test>\n\
             envelope-to: <me@example.test>\nuser: alerts@example.test\ntls: yes\n-- message\n\
             Subject: Test\n\n.one dot\nplain\n== end\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn auth_login_asks_for_the_user_and_then_the_password() {
        let dir = folder("login");
        let (client, server) = connect(&dir);
        let mut client = client.encrypted();
        assert_eq!(client.ask("AUTH LOGIN"), ["334 VXNlcm5hbWU6"]);
        // "alerts@example.test" and "tea party" in base64.
        assert_eq!(client.ask("YWxlcnRzQGV4YW1wbGUudGVzdA=="), ["334 UGFzc3dvcmQ6"]);
        assert_eq!(client.ask("dGVhIHBhcnR5"), ["235 2.7.0 Authentication successful"]);
        assert!(client.ask("QUIT")[0].starts_with("221"));
        server.join().unwrap().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_wrong_password_in_auth_login_is_refused() {
        let dir = folder("login-wrong");
        let (client, server) = connect(&dir);
        let mut client = client.encrypted();
        client.ask("AUTH LOGIN");
        client.ask("YWxlcnRzQGV4YW1wbGUudGVzdA==");
        // "coffee".
        assert_eq!(client.ask("Y29mZmVl"), ["535 5.7.8 Authentication credentials invalid"]);
        assert!(client.ask("MAIL FROM:<a@example.test>")[0].starts_with("530"));
        client.ask("QUIT");
        server.join().unwrap().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_address_is_what_is_in_the_angle_brackets() {
        assert_eq!(address("FROM:<a@example.test> SIZE=100", "FROM:"), "a@example.test");
        assert_eq!(address("from:<a@example.test>", "FROM:"), "a@example.test");
        assert_eq!(address("TO: <b@example.test>", "TO:"), "b@example.test");
        assert_eq!(address("TO:<>", "TO:"), "");
    }
}
