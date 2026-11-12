// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! HTTP/1.1 GET, over TCP or TLS, read as it arrives: what a TUF client
//! fetches is checked by its signatures and hashes, not by how it came
//! (ADR 0380), so this only has to fetch it, follow redirects and say
//! when a file is not there.
//!
//! One connection per request, closed after it, and no compression. A
//! redirect is followed to http or https alike, five at most.
//!
//! [`exchange`] sends any other request, with its own headers and body,
//! for a client of an interface such as S3's (backup(8), ADR 0465),
//! which signs each request and reads every status itself.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::url::Url;
use rustls::crypto::ring;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// How long a connection or a read may take before the fetch fails.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The most redirects one fetch follows.
pub const REDIRECTS: usize = 5;

/// The longest a status line and its headers may be.
const HEAD: usize = 64 * 1024;

/// The root certificates: the system's, by the name Linux's programs
/// read them by and then its own file (dukovo-certificates(7)), and on a
/// host, where the tests run, the usual Linux one.
const ROOTS: [&str; 3] = [
    "/system/settings/ssl/certs/ca-certificates.crt",
    "/system/resources/certificates/ca-certificates.crt",
    "/etc/ssl/certs/ca-certificates.crt",
];

/// Why a fetch failed.
#[derive(Debug)]
pub enum Error {
    /// The server says the file is not there: 404 or 410, or 403, which
    /// some stores answer for a file that is not there.
    NotFound(u16),
    /// Any other answer than 200, or a redirect without a place.
    Status(u16),
    /// Neither http nor https, or no host.
    Url(String),
    /// More than [`REDIRECTS`] redirects.
    Redirects,
    /// The connection, TLS, or a malformed answer.
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(code) => write!(f, "not there (HTTP {code})"),
            Error::Status(code) => write!(f, "the server answered HTTP {code}"),
            Error::Url(why) => write!(f, "{why}"),
            Error::Redirects => write!(f, "more than {REDIRECTS} redirects"),
            Error::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

/// A connection, plain or over TLS.
enum Connection {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.read(buf),
            Connection::Tls(stream) => match stream.read(buf) {
                // A server that closes without TLS's close_notify has
                // still sent what it framed: the framing says whether the
                // body is whole.
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
                other => other,
            },
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.write(buf),
            Connection::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Connection::Plain(stream) => stream.flush(),
            Connection::Tls(stream) => stream.flush(),
        }
    }
}

/// How the body of an answer ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// After this many more bytes.
    Length(u64),
    /// In chunks: this many bytes left of the chunk being read, or none
    /// read yet; then the last chunk.
    Chunked { left: u64, started: bool, done: bool },
    /// When the server closes the connection.
    Close,
}

/// The body of an answer, read as it arrives.
pub struct Body {
    reader: BufReader<Box<dyn Read + Send>>,
    framing: Framing,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body").field("framing", &self.framing).finish_non_exhaustive()
    }
}

impl Body {
    fn new(reader: BufReader<Box<dyn Read + Send>>, framing: Framing) -> Self {
        Body { reader, framing }
    }

    /// Reads the size line of the next chunk, and the trailer after the
    /// last.
    fn next_chunk(&mut self) -> io::Result<u64> {
        let line = read_line(&mut self.reader)?;
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = u64::from_str_radix(size, 16).map_err(|_| malformed("a chunk's size"))?;
        if size == 0 {
            // The trailer's fields, up to an empty line.
            while !read_line(&mut self.reader)?.is_empty() {}
        }
        Ok(size)
    }
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.framing {
            Framing::Close => self.reader.read(buf),
            Framing::Length(0) => Ok(0),
            Framing::Length(left) => {
                let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
                let read = self.reader.read(&mut buf[..want])?;
                if read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the server closed before the end of the file",
                    ));
                }
                self.framing = Framing::Length(left - read as u64);
                Ok(read)
            }
            Framing::Chunked { done: true, .. } => Ok(0),
            Framing::Chunked { left: 0, started, .. } => {
                if started {
                    // The CRLF after a chunk's data.
                    if !read_line(&mut self.reader)?.is_empty() {
                        return Err(malformed("the end of a chunk"));
                    }
                }
                let size = self.next_chunk()?;
                self.framing = Framing::Chunked { left: size, started: true, done: size == 0 };
                self.read(buf)
            }
            Framing::Chunked { left, .. } => {
                let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
                let read = self.reader.read(&mut buf[..want])?;
                if read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the server closed inside a chunk",
                    ));
                }
                self.framing =
                    Framing::Chunked { left: left - read as u64, started: true, done: false };
                Ok(read)
            }
        }
    }
}

fn malformed(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the server's answer has a malformed {what}"),
    )
}

/// Reads a line ended by LF (or CRLF), without its end; fails on a line
/// longer than [`HEAD`] or the end of the connection.
fn read_line<R: BufRead>(reader: &mut R) -> io::Result<String> {
    let mut line = Vec::new();
    let read = reader.take(HEAD as u64).read_until(b'\n', &mut line)?;
    if read == 0 || line.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the server closed in the middle of its answer",
        ));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| malformed("line"))
}

/// The status and headers of an answer, and how its body ends.
#[derive(Debug, PartialEq, Eq)]
struct Head {
    status: u16,
    location: Option<String>,
    framing: Framing,
}

/// Reads the status line and headers.
fn read_head<R: BufRead>(reader: &mut R) -> io::Result<Head> {
    let status_line = read_line(reader)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(malformed("status line"));
    }
    let status: u16 =
        parts.next().and_then(|code| code.parse().ok()).ok_or_else(|| malformed("status"))?;
    let mut location = None;
    let mut length = None;
    let mut chunked = false;
    let mut total = status_line.len();
    loop {
        let line = read_line(reader)?;
        total += line.len();
        if total > HEAD {
            return Err(malformed("set of headers, which is too long,"));
        }
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(malformed("header"));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "location" => location = Some(value.to_string()),
            "content-length" => {
                length = Some(value.parse::<u64>().map_err(|_| malformed("Content-Length"))?);
            }
            "transfer-encoding" => {
                chunked = value
                    .rsplit(',')
                    .next()
                    .is_some_and(|last| last.trim().eq_ignore_ascii_case("chunked"));
            }
            _ => {}
        }
    }
    // RFC 9112 6.3: chunked wins over a length; no body for these.
    let framing = if status == 204 || status == 304 || (100..200).contains(&status) {
        Framing::Length(0)
    } else if chunked {
        Framing::Chunked { left: 0, started: false, done: false }
    } else if let Some(length) = length {
        Framing::Length(length)
    } else {
        Framing::Close
    };
    Ok(Head { status, location, framing })
}

/// The TLS settings, made once: ring's provider and the system's roots.
fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            for path in ROOTS {
                if let Ok(certificates) = CertificateDer::pem_file_iter(path) {
                    let (added, _) = roots.add_parsable_certificates(certificates.flatten());
                    if added > 0 {
                        break;
                    }
                }
            }
            let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring's provider has the default versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Connects to `url`'s host, over TLS for https.
fn connect(url: &Url) -> Result<Connection, Error> {
    let tls = match url.scheme() {
        "http" => false,
        "https" => true,
        other => return Err(Error::Url(format!("{other}: is not http or https"))),
    };
    let host = url.host();
    let port = url.port().unwrap_or(if tls { 443 } else { 80 });
    // A literal IPv6 address comes in brackets, which the resolver and
    // TLS do not take.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host} has no address"));
    let mut tcp = None;
    for address in (bare, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(err) => last = err,
        }
    }
    let tcp = tcp.ok_or(last)?;
    tcp.set_read_timeout(Some(TIMEOUT))?;
    tcp.set_write_timeout(Some(TIMEOUT))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return Ok(Connection::Plain(tcp));
    }
    let name = ServerName::try_from(bare.to_string())
        .map_err(|_| Error::Url(format!("{host} is not a server name")))?;
    let connection = ClientConnection::new(tls_config(), name).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(connection, tcp);
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock)?;
    }
    Ok(Connection::Tls(Box::new(stream)))
}

/// The Host header of a request for `url`: its host, and its port when
/// the address names one.
#[must_use]
pub fn host_header(url: &Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url.host()),
        None => url.host().to_string(),
    }
}

/// The request for `url`, from byte `offset` on when it is not 0.
fn request(url: &Url, offset: u64) -> String {
    let target = url.path_and_query();
    let host = host_header(url);
    let range = if offset > 0 { format!("Range: bytes={offset}-\r\n") } else { String::new() };
    format!(
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Dukovo-tuf/1\r\n\
         Accept: */*\r\nAccept-Encoding: identity\r\n{range}Connection: close\r\n\r\n"
    )
}

/// Fetches `url`, following redirects: the body of its 200 answer, to
/// read as it arrives.
///
/// # Errors
///
/// [`Error::NotFound`] when the server says the file is not there; any
/// other failure to connect or to get a 200 answer.
pub fn get(url: &Url) -> Result<Body, Error> {
    fetch(url, false, 0).map(|(body, _)| body)
}

/// Fetches `url` from byte `offset` on, to go on with a download cut
/// short, following redirects as [`get`] does: the body, and whether it
/// starts at `offset` (206) or is the whole file (200), as from a server
/// that does not send parts.
///
/// # Errors
///
/// As [`get`]; [`Error::Status`] with 416 when the file is no longer than
/// `offset`.
pub fn get_from(url: &Url, offset: u64) -> Result<(Body, bool), Error> {
    fetch(url, false, offset)
}

/// Fetches `url` as [`get`] does, over TLS only: an `http://` address,
/// or a redirect to one, is refused before anything is asked of it. For
/// what no signature checks, such as an installer's setup file, whose
/// only protection on the way is TLS.
///
/// # Errors
///
/// [`Error::Url`] for an address or a redirect that is not `https://`;
/// otherwise as [`get`].
pub fn get_https(url: &Url) -> Result<Body, Error> {
    fetch(url, true, 0).map(|(body, _)| body)
}

/// An answer to [`exchange`]: its status, and its body to read as it
/// arrives.
#[derive(Debug)]
pub struct Answer {
    /// The status, such as 200 or 404.
    pub status: u16,
    /// The body; none for a HEAD request.
    pub body: Body,
}

/// Sends `method` for `url` with `headers` besides Host, Content-Length
/// and Connection, and `body`, and gives the answer whatever its status,
/// following no redirect.
///
/// # Errors
///
/// [`Error::Url`] for an address that is not http or https, or a header
/// with a line break in it; a failure to connect, send or read the
/// answer's head.
pub fn exchange(
    method: &str,
    url: &Url,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Answer, Error> {
    let line_break = |text: &str| text.contains(['\r', '\n']);
    if line_break(method)
        || headers.iter().any(|(name, value)| line_break(name) || line_break(value))
    {
        return Err(Error::Url("a request's header has a line break in it".into()));
    }
    let mut head = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Dukovo/1\r\n\
         Accept-Encoding: identity\r\nConnection: close\r\nContent-Length: {}\r\n",
        url.path_and_query(),
        host_header(url),
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut connection = connect(url)?;
    connection.write_all(head.as_bytes())?;
    connection.write_all(body)?;
    connection.flush()?;
    let mut reader: BufReader<Box<dyn Read + Send>> =
        BufReader::with_capacity(64 * 1024, Box::new(connection));
    let answer = read_head(&mut reader)?;
    let framing = if method == "HEAD" { Framing::Length(0) } else { answer.framing };
    Ok(Answer { status: answer.status, body: Body::new(reader, framing) })
}

/// Whether `url` may be fetched: any http or https address, or only an
/// https one when `https_only`.
fn allowed(url: &Url, https_only: bool) -> Result<(), Error> {
    if https_only && url.scheme() != "https" {
        return Err(Error::Url(format!("{}: only https:// addresses are used", url.scheme())));
    }
    Ok(())
}

/// [`get`], over TLS only when `https_only`, the first address and every
/// redirect alike, from byte `offset` on: the body, and whether it is a
/// part (206).
fn fetch(url: &Url, https_only: bool, offset: u64) -> Result<(Body, bool), Error> {
    let mut url = url.clone();
    for _ in 0..=REDIRECTS {
        allowed(&url, https_only)?;
        let mut connection = connect(&url)?;
        connection.write_all(request(&url, offset).as_bytes())?;
        connection.flush()?;
        let mut reader: BufReader<Box<dyn Read + Send>> =
            BufReader::with_capacity(64 * 1024, Box::new(connection));
        let head = read_head(&mut reader)?;
        match head.status {
            200 => return Ok((Body::new(reader, head.framing), false)),
            206 if offset > 0 => return Ok((Body::new(reader, head.framing), true)),
            301 | 302 | 303 | 307 | 308 => {
                let location = head.location.ok_or(Error::Status(head.status))?;
                url = url
                    .resolve(&location)
                    .map_err(|err| Error::Url(format!("a redirect: {err}")))?;
            }
            403 | 404 | 410 => return Err(Error::NotFound(head.status)),
            other => return Err(Error::Status(other)),
        }
    }
    Err(Error::Redirects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn head(text: &str) -> io::Result<Head> {
        read_head(&mut BufReader::new(text.as_bytes()))
    }

    fn body(framing: Framing, bytes: &'static [u8]) -> io::Result<Vec<u8>> {
        let reader: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(bytes));
        let mut out = Vec::new();
        Body::new(reader, framing).read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn only_https_is_fetched_when_asked() {
        let plain = Url::parse("http://example.org/install.json").unwrap();
        let secure = Url::parse("https://example.org/install.json").unwrap();
        assert!(allowed(&plain, false).is_ok());
        assert!(allowed(&secure, true).is_ok());
        let refused = allowed(&plain, true).unwrap_err().to_string();
        assert_eq!(refused, "http: only https:// addresses are used");
        // Refused before any connection is made.
        assert!(matches!(get_https(&plain), Err(Error::Url(_))));
    }

    #[test]
    fn heads_say_how_bodies_end() {
        let ok = head("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n").unwrap();
        assert_eq!(ok, Head { status: 200, location: None, framing: Framing::Length(12) });
        let chunked = head(
            "HTTP/1.1 200 OK\r\ncontent-length: 3\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
        )
        .unwrap();
        assert_eq!(chunked.framing, Framing::Chunked { left: 0, started: false, done: false });
        let close = head("HTTP/1.0 200 OK\nServer: x\n\n").unwrap();
        assert_eq!(close.framing, Framing::Close);
        let moved = head("HTTP/1.1 301 Moved\r\nLocation: /b/c.json\r\n\r\n").unwrap();
        assert_eq!(moved.location.as_deref(), Some("/b/c.json"));
        assert!(head("SSH-2.0-OpenSSH\r\n\r\n").is_err());
        assert!(head("HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n").is_err());
        assert!(head("HTTP/1.1 200 OK\r\nno colon\r\n\r\n").is_err());
        assert!(head("HTTP/1.1 200 OK\r\n").is_err());
    }

    #[test]
    fn bodies_end_where_their_framing_says() {
        assert_eq!(body(Framing::Length(5), b"hello world").unwrap(), b"hello");
        assert!(body(Framing::Length(20), b"hello world").is_err());
        assert_eq!(body(Framing::Close, b"hello world").unwrap(), b"hello world");
        let chunked = Framing::Chunked { left: 0, started: false, done: false };
        let text = b"5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nTrailer: x\r\n\r\nafter";
        assert_eq!(body(chunked, text).unwrap(), b"hello world");
        assert!(body(chunked, b"5\r\nhel").is_err());
        assert!(body(chunked, b"5\r\nhelloXX\r\n0\r\n\r\n").is_err());
        assert!(body(chunked, b"zz\r\n").is_err());
    }

    /// Serves each of `answers` to one connection in turn, and gives back
    /// the requests it was sent.
    fn serve(answers: Vec<&'static str>) -> (Url, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                requests.push(request);
                stream.write_all(answer.as_bytes()).unwrap();
            }
            requests
        });
        (url, server)
    }

    #[test]
    fn a_download_goes_on_from_where_it_was_cut() {
        let (url, server) = serve(vec![
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\n\r\ndef",
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabcdef",
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n",
        ]);
        let url = url.resolve("files/abc").unwrap();
        let mut text = String::new();
        let (mut body, part) = get_from(&url, 3).unwrap();
        body.read_to_string(&mut text).unwrap();
        assert_eq!((text.as_str(), part), ("def", true));
        // A server that sends the whole file again.
        let (mut body, part) = get_from(&url, 3).unwrap();
        text.clear();
        body.read_to_string(&mut text).unwrap();
        assert_eq!((text.as_str(), part), ("abcdef", false));
        assert!(matches!(get_from(&url, 6), Err(Error::Status(416))));
        let requests = server.join().unwrap();
        assert!(requests.iter().all(|r| r.contains("\r\nRange: bytes=")), "{requests:?}");
        assert!(requests[0].starts_with("GET /files/abc HTTP/1.1\r\n"));
    }

    #[test]
    fn an_exchange_sends_its_method_headers_and_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let url = Url::parse(&format!("http://{address}/bucket/a%20b?x-id=1")).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let length: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .unwrap()
                .parse()
                .unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 7\r\n\r\ndenied!")
                .unwrap();
            (head, body)
        });
        let mut answer =
            exchange("PUT", &url, &[("x-amz-date", "20130524T000000Z")], b"hello").unwrap();
        let mut text = String::new();
        answer.body.read_to_string(&mut text).unwrap();
        assert_eq!((answer.status, text.as_str()), (403, "denied!"));
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("PUT /bucket/a%20b?x-id=1 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains(&format!("\r\nHost: {address}\r\n")), "{head}");
        assert!(head.contains("\r\nContent-Length: 5\r\n"), "{head}");
        assert!(head.contains("\r\nx-amz-date: 20130524T000000Z\r\n"), "{head}");
        assert_eq!(body, b"hello");
        // A header that would start another is refused before any
        // connection is made.
        assert!(matches!(exchange("GET", &url, &[("a", "b\r\nEvil: 1")], b""), Err(Error::Url(_))));
    }

    #[test]
    fn a_fetch_follows_redirects_and_says_what_is_not_there() {
        let (url, server) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /real/1.root.json?x=1\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n",
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 500 Oops\r\nContent-Length: 0\r\n\r\n",
        ]);
        let mut text = String::new();
        get(&url.resolve("1.root.json").unwrap()).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "{}");
        assert!(matches!(get(&url.resolve("2.root.json").unwrap()), Err(Error::NotFound(404))));
        assert!(matches!(get(&url.resolve("timestamp.json").unwrap()), Err(Error::Status(500))));
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET /1.root.json HTTP/1.1\r\n"), "{}", requests[0]);
        assert!(requests[1].starts_with("GET /real/1.root.json?x=1 HTTP/1.1\r\n"));
        assert!(requests[1].contains(&format!("Host: 127.0.0.1:{}\r\n", url.port().unwrap())));
        assert!(matches!(get(&Url::parse("ftp://example.org/x").unwrap()), Err(Error::Url(_))));
    }

    #[test]
    fn redirects_end() {
        let (url, server) =
            serve(vec!["HTTP/1.1 301 Moved\r\nLocation: /again\r\n\r\n"; REDIRECTS + 1]);
        assert!(matches!(get(&url), Err(Error::Redirects)));
        assert_eq!(server.join().unwrap().len(), REDIRECTS + 1);
    }
}
