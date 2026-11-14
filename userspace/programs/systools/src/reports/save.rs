// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Saving a report that is not sent from this computer (ADR 0532): to a
//! file, as `report --save FILE` and the preview's `F` do, and to a phone,
//! as `report --save` and `P` do, with a download that works once.
//!
//! **To a file.** The bytes the person saw, in a file only they read (mode
//! 0600), written under another name in the same folder, synced, renamed
//! to the name asked for and the folder synced, so that a stick pulled out
//! at any moment holds the whole file or none. A file that is there is not
//! replaced.
//!
//! **To a phone.** The report is served over HTTPS, by rustls on this
//! computer's addresses, to the first request for one address:
//! `https://ADDRESS:PORT/TOKEN/report.txt`, with a token of 128 random bits.
//! The certificate is made for this download alone, valid for the
//! computer's own addresses and for days, not years, so the phone warns of
//! it; the warning is the phone's way of saying it does not know it, and
//! the fingerprint shown beside the QR code is what to compare. One
//! connection is served at a time. The first GET of the address gets the
//! bytes, as an attachment; every request after it for the address gets
//! 410 Gone, any other address 404, and the listener closes after the
//! first download, or when ten minutes have passed. A download that broke
//! before its last byte does not count: the phone may ask again.
//!
//! A person's own program that listens is open to this computer alone
//! until a rule of the firewall lets others in (firewall(8)), so the phone
//! reaches the download only when an administrator has allowed its port;
//! the screen says how. `--port` gives the port, so that the rule can be
//! made before the download is started.
//!
//! When a phone does not get the report: run `report --save --port N`, and
//! `firewall` as an administrator to see whether the port is dropped.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ECDSA_P256_SHA256, SanType};
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::qr;
use crate::util::{self, Fail, fail};
use crate::{logd, tls};

/// How long a download waits for the phone.
pub const LIMIT: Duration = Duration::from_secs(600);

/// How long the server waits for a phone's next bytes of a request.
const CONNECTION_WAIT: Duration = Duration::from_secs(5);

/// How long a phone has for a whole request, however slowly it sends.
const REQUEST_WAIT: Duration = Duration::from_secs(15);

/// How often the server looks at the clock while no one connects.
const TICK: Duration = Duration::from_millis(200);

/// The most bytes of a request the server reads: a request of a phone's
/// browser fits in a few hundred.
const HEAD_MAX: usize = 8192;

/// The most connections queued behind a download that are told it is gone.
const QUEUED_MAX: usize = 8;

/// The file name a phone saves the report under.
pub const FILE_NAME: &str = "report.txt";

/// Saves `bytes` as the file `path`, as the module's text says.
///
/// # Errors
///
/// If `path` is there already, or the folder, the file or its sync fails.
pub fn to_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the file exists already: name another, or remove it first",
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "there is no file name"))?;
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let partial = folder.join(format!(".{}.{}.part", name.to_string_lossy(), std::process::id()));
    let renamed = write_synced(&partial, bytes).and_then(|()| fs::rename(&partial, path));
    if renamed.is_err() {
        // The first error is the one to report, not a second that the
        // partial file was never made.
        let _ = fs::remove_file(&partial);
    }
    renamed?;
    sync_folder(folder)
}

/// Writes `bytes` to a new file `path` that only its owner reads, and
/// syncs it.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Syncs `folder`, so that the new name is on the disk. A filesystem that
/// has no folders to sync refuses with these kinds; its file is synced
/// already.
fn sync_folder(folder: &Path) -> io::Result<()> {
    match File::open(folder).and_then(|folder| folder.sync_all()) {
        Err(err)
            if matches!(err.kind(), io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput) =>
        {
            Ok(())
        }
        done => done,
    }
}

/// The certificate and key of one download.
pub struct Certificate {
    der: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

/// Seconds in a day.
const DAY: u64 = 86_400;

/// The UTC date `seconds` after 1970 falls on, as a certificate's limits
/// are given: year, month and day.
fn date(seconds: u64) -> (i32, u8, u8) {
    let (year, month, day) = logd::civil(i64::try_from(seconds / DAY).unwrap_or(0));
    (
        i32::try_from(year).unwrap_or(1970),
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

impl Certificate {
    /// A self-signed certificate, with an ECDSA key on P-256, for the IP
    /// addresses `addresses`, valid from a day before `now` to two days
    /// after it: a phone's clock may be a little off, and the download
    /// lasts ten minutes.
    ///
    /// # Errors
    ///
    /// Why the certificate could not be made.
    pub fn new(addresses: &[IpAddr], now: SystemTime) -> Result<Certificate, String> {
        let made = |err: rcgen::Error| format!("cannot make the download's certificate: {err}");
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(made)?;
        params.subject_alt_names = addresses.iter().map(|ip| SanType::IpAddress(*ip)).collect();
        params.distinguished_name.push(DnType::CommonName, "Dukovo problem report");
        let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        let (from, until) = (date(seconds.saturating_sub(DAY)), date(seconds + 2 * DAY));
        params.not_before = rcgen::date_time_ymd(from.0, from.1, from.2);
        params.not_after = rcgen::date_time_ymd(until.0, until.1, until.2);
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(made)?;
        let certificate = params.self_signed(&key).map_err(made)?;
        Ok(Certificate {
            der: certificate.der().clone(),
            key: PrivatePkcs8KeyDer::from(key.serialize_der()),
        })
    }

    /// The certificate's SHA-256 fingerprint, as a phone's certificate
    /// page shows it: pairs of capital hexadecimal digits with colons.
    pub fn fingerprint(&self) -> String {
        let hash = digest(&SHA256, self.der.as_ref());
        let pairs: Vec<String> = hash.as_ref().iter().map(|byte| format!("{byte:02X}")).collect();
        pairs.join(":")
    }

    /// A TLS server's settings that show this certificate, to TLS 1.2 and
    /// 1.3 clients, as phones are.
    ///
    /// # Errors
    ///
    /// If the key and the certificate do not go together.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, rustls::Error> {
        let config = ServerConfig::builder_with_provider(tls::provider())
            .with_safe_default_protocol_versions()
            .expect(tls::VERSIONS)
            .with_no_client_auth()
            .with_single_cert(vec![self.der.clone()], PrivateKeyDer::Pkcs8(self.key.clone_key()))?;
        Ok(Arc::new(config))
    }
}

/// A token for a download's address: 128 random bits as 32 hexadecimal
/// digits.
///
/// # Errors
///
/// If the system gives no random numbers.
pub fn token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "the system gave no random numbers for the download's address".to_string())?;
    Ok(hex(&bytes))
}

/// `bytes` as lowercase hexadecimal digits.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing to a string cannot fail.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Whether a phone on the local network could reach `address`: not the
/// loopback or unspecified address, and not a link-local one, which needs
/// the interface named as well.
fn is_reachable(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local(),
        IpAddr::V6(v6) => {
            !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

/// `addresses` with the IPv4 ones first, each once: a phone's network has
/// IPv4 for certain, IPv6 sometimes.
fn ipv4_first(mut addresses: Vec<IpAddr>) -> Vec<IpAddr> {
    addresses.sort_by_key(IpAddr::is_ipv6);
    let mut seen = Vec::new();
    addresses.retain(|address| {
        let new = !seen.contains(address);
        seen.push(*address);
        new
    });
    addresses
}

/// This computer's addresses a phone could reach, IPv4 first.
fn reachable_addresses() -> Vec<IpAddr> {
    let found = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .iter()
        .map(if_addrs::Interface::ip)
        .filter(is_reachable)
        .collect();
    ipv4_first(found)
}

/// The address a phone opens: `https://192.168.1.20:43517/TOKEN/report.txt`,
/// with an IPv6 address in brackets.
fn url(address: IpAddr, port: u16, token: &str) -> String {
    match address {
        IpAddr::V4(v4) => format!("https://{v4}:{port}/{token}/{FILE_NAME}"),
        IpAddr::V6(v6) => format!("https://[{v6}]:{port}/{token}/{FILE_NAME}"),
    }
}

/// What the server gives out, and the address it gives it at.
pub struct Offer {
    /// The path of the one address that has the report, `/TOKEN/report.txt`.
    pub path: String,
    /// The report.
    pub body: Vec<u8>,
}

/// How a download ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A phone took the report.
    Downloaded,
    /// No one did within the time.
    TimedOut,
}

/// What a request gets.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Download,
    BadRequest,
    NotFound,
    MethodNotAllowed,
    Gone,
}

/// The answer to the request whose first lines are `head`: the report for
/// the first GET of `path`, 410 for any later request for it, 404 for any
/// other address. `taken` is whether the report was given out already.
fn answer_for(head: &[u8], path: &str, taken: bool) -> Answer {
    let line = head.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let Ok(line) = std::str::from_utf8(line) else { return Answer::BadRequest };
    let mut words = line.trim_end().split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Answer::BadRequest;
    };
    if !version.starts_with("HTTP/1.") {
        return Answer::BadRequest;
    }
    if target.split('?').next() != Some(path) {
        Answer::NotFound
    } else if taken {
        Answer::Gone
    } else if method != "GET" {
        Answer::MethodNotAllowed
    } else {
        Answer::Download
    }
}

/// The bytes of the response to `answer`: the head, then the report for a
/// download, or a line of words for the rest.
fn response(answer: &Answer, body: &[u8]) -> Vec<u8> {
    let (status, extra, message): (&str, &str, &[u8]) = match answer {
        Answer::Download => {
            ("200 OK", "Content-Disposition: attachment; filename=\"report.txt\"\r\n", body)
        }
        Answer::BadRequest => ("400 Bad Request", "", b"Bad request.\n"),
        Answer::NotFound => ("404 Not Found", "", b"Not found.\n"),
        Answer::MethodNotAllowed => ("405 Method Not Allowed", "Allow: GET\r\n", b"Use GET.\n"),
        Answer::Gone => ("410 Gone", "", b"This download was used and is gone.\n"),
    };
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n{extra}\
         Content-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n",
        message.len()
    )
    .into_bytes();
    bytes.extend_from_slice(message);
    bytes
}

/// Waits up to `wait` for a connection to come to `listener`.
fn wait_for_connection(listener: &TcpListener, wait: Duration) -> io::Result<()> {
    let mut fd = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one pollfd, of a descriptor the listener owns.
    let ready = unsafe { libc::poll(&raw mut fd, 1, ms) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

/// Reads a request's head, up to the empty line that ends it.
fn read_head(stream: &mut impl Read, now: &dyn Fn() -> Instant) -> io::Result<Vec<u8>> {
    let until = now() + REQUEST_WAIT;
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() > HEAD_MAX || now() >= until {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a request too long or slow"));
        }
        let length = stream.read(&mut chunk)?;
        if length == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&chunk[..length]);
    }
    Ok(head)
}

/// Serves `tcp`, a connection of the listener, one request: whether it got
/// the report whole.
fn talk(
    tcp: TcpStream,
    tls: &Arc<ServerConfig>,
    offer: &Offer,
    taken: bool,
    now: &dyn Fn() -> Instant,
) -> io::Result<bool> {
    // A socket a listener accepts takes the listener's blocking mode on
    // some systems, as on the BSDs; this one is read with timeouts.
    tcp.set_nonblocking(false)?;
    tcp.set_read_timeout(Some(CONNECTION_WAIT))?;
    tcp.set_write_timeout(Some(CONNECTION_WAIT))?;
    let connection = ServerConnection::new(tls.clone()).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(connection, tcp);
    let answer = answer_for(&read_head(&mut stream, now)?, &offer.path, taken);
    stream.write_all(&response(&answer, &offer.body))?;
    stream.conn.send_close_notify();
    stream.flush()?;
    // The phone's own close, read before this one, keeps the end of the
    // response from being lost to a reset that unread bytes cause.
    let _ = stream.sock.shutdown(Shutdown::Write);
    let _ = stream.sock.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = io::copy(&mut (&stream.sock).take(HEAD_MAX as u64), &mut io::sink());
    Ok(answer == Answer::Download)
}

/// Tells the connections that queued behind a download that it is gone.
fn tell_queued(
    listener: &TcpListener,
    tls: &Arc<ServerConfig>,
    offer: &Offer,
    now: &dyn Fn() -> Instant,
) {
    for _ in 0..QUEUED_MAX {
        let Ok((tcp, _)) = listener.accept() else { return };
        // A phone that went away mid-request is one the download is gone
        // for anyway.
        let _ = talk(tcp, tls, offer, true, now);
    }
}

/// Serves `offer` on `listener` until a phone has the report or `now`
/// reaches `until`, one connection at a time. A connection that breaks, or
/// stops sending, is only that connection's end.
///
/// # Errors
///
/// If the listener fails.
pub fn serve(
    listener: &TcpListener,
    tls: &Arc<ServerConfig>,
    offer: &Offer,
    until: Instant,
    now: &dyn Fn() -> Instant,
) -> io::Result<Outcome> {
    listener.set_nonblocking(true)?;
    loop {
        let left = until.saturating_duration_since(now());
        if left.is_zero() {
            return Ok(Outcome::TimedOut);
        }
        wait_for_connection(listener, left.min(TICK))?;
        match listener.accept() {
            Ok((tcp, _)) => {
                if talk(tcp, tls, offer, false, now).unwrap_or(false) {
                    tell_queued(listener, tls, offer, now);
                    return Ok(Outcome::Downloaded);
                }
            }
            Err(err)
                if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
            }
            Err(err) => return Err(err),
        }
    }
}

/// The lines the screen shows while the download waits: how to reach it,
/// and what to expect. `qr` is the code's lines, if the terminal has room
/// for it. The code comes last, so that on a small screen it is what
/// stays in view.
fn screen_lines(
    urls: &[String],
    fingerprint: &str,
    port: u16,
    qr: Option<&[String]>,
) -> Vec<String> {
    let mut lines = vec![
        "Download on a phone".to_string(),
        String::new(),
        "Scan the code with the phone's camera, or type the address. The phone warns".into(),
        "that it does not know the certificate: this computer made it a moment ago.".into(),
        "Check that the fingerprint below is the one the phone shows, and go on.".into(),
        String::new(),
    ];
    lines.extend(urls.iter().map(|url| format!("  {url}")));
    lines.push(String::new());
    lines.push("Certificate fingerprint (SHA-256):".into());
    let (first, second) = fingerprint.split_at(fingerprint.len() / 2);
    lines.push(format!("  {first}"));
    lines.push(format!("  {}", second.trim_start_matches(':')));
    lines.push(String::new());
    lines.push("If the phone cannot connect, the firewall drops it. In another terminal:".into());
    lines.push(format!("  elevate firewall allow tcp {port}"));
    lines.push("and, when it is done, take the rule away with elevate firewall remove.".into());
    lines.push(String::new());
    match qr {
        Some(code) => lines.extend(code.iter().cloned()),
        None => lines.push("The terminal has no room for the code: type the address.".into()),
    }
    lines.push(String::new());
    lines.push(format!(
        "Waiting for one download ({} minutes). Press Ctrl+C to stop.",
        LIMIT.as_secs() / 60
    ));
    lines
}

/// Offers `text` to a phone, as the module's text says, and waits for its
/// one download. `port` is the port to listen on, or any free one.
///
/// # Errors
///
/// If this computer has no address a phone could reach, the port cannot be
/// listened on, or the certificate cannot be made.
pub fn download(text: &str, port: Option<u16>) -> Result<Outcome, Fail> {
    let mut addresses = reachable_addresses();
    let Some(first) = addresses.first().copied() else {
        return Err(fail(
            "this computer has no network address a phone could reach: connect it to the \
             network the phone is on first"
                .into(),
        ));
    };
    // One listener serves one family, IPv4 when the computer has it.
    addresses.retain(|address| address.is_ipv4() == first.is_ipv4());
    let any: IpAddr =
        if first.is_ipv4() { Ipv4Addr::UNSPECIFIED.into() } else { Ipv6Addr::UNSPECIFIED.into() };
    let wanted = SocketAddr::new(any, port.unwrap_or(0));
    let listener = TcpListener::bind(wanted)
        .map_err(|err| util::failed(&format!("cannot listen on {wanted}"), err))?;
    let port = listener.local_addr()?.port();
    let certificate = Certificate::new(&addresses, SystemTime::now()).map_err(fail)?;
    let config = certificate.server_config().map_err(|err| fail(err.to_string()))?;
    let token = token().map_err(fail)?;
    let offer = Offer { path: format!("/{token}/{FILE_NAME}"), body: text.as_bytes().to_vec() };
    let urls: Vec<String> = addresses.iter().take(3).map(|a| url(*a, port, &token)).collect();
    let (columns, rows) = util::terminal_size(1).unwrap_or((80, 24));
    // The blank line and the waiting line go below the code.
    let code =
        qr::encode(&urls[0]).and_then(|code| qr::lines(&code, columns, rows.saturating_sub(3)));
    for line in screen_lines(&urls, &certificate.fingerprint(), port, code.as_deref()) {
        println!("{line}");
    }
    let _ = io::stdout().flush();
    Ok(serve(&listener, &config, &offer, Instant::now() + LIMIT, &Instant::now)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};

    const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    /// A folder of the test's own, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("report-save-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_is_written_whole_for_its_owner_alone_with_no_partial_left() {
        let dir = scratch("file");
        let path = dir.join("report.txt");
        to_file(&path, b"Dukovo problem report\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"Dukovo problem report\n");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["report.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_there_is_not_replaced() {
        let dir = scratch("exists");
        let path = dir.join("report.txt");
        fs::write(&path, b"mine").unwrap();
        let error = to_file(&path, b"report").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"mine");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_is_missing_fails_and_leaves_nothing() {
        let dir = scratch("missing");
        let error = to_file(&dir.join("nowhere").join("report.txt"), b"report").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(date(0), (1970, 1, 1));
        assert_eq!(date(365 * DAY), (1971, 1, 1));
        assert_eq!(date(951_782_400), (2000, 2, 29));
        assert_eq!(date(951_782_400 + DAY - 1), (2000, 2, 29));
    }

    #[test]
    fn a_token_is_128_random_bits_as_hex() {
        let (first, second) = (token().unwrap(), token().unwrap());
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_ne!(first, second);
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn a_fingerprint_is_pairs_of_capital_hex_digits() {
        let one = Certificate::new(&[LOCAL], SystemTime::now()).unwrap();
        let other = Certificate::new(&[LOCAL], SystemTime::now()).unwrap();
        let fingerprint = one.fingerprint();
        assert_eq!(fingerprint.len(), 32 * 3 - 1);
        assert!(fingerprint.split(':').all(|pair| {
            pair.len() == 2
                && pair.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        }));
        assert_ne!(fingerprint, other.fingerprint());
    }

    #[test]
    fn addresses_a_phone_cannot_use_are_left_out() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        assert!(is_reachable(&ip("192.168.1.20")));
        assert!(is_reachable(&ip("fd00::20")));
        assert!(is_reachable(&ip("2001:db8::1")));
        for text in ["127.0.0.1", "0.0.0.0", "169.254.3.4", "::1", "::", "fe80::1", "febf::1"] {
            assert!(!is_reachable(&ip(text)), "{text}");
        }
    }

    #[test]
    fn ipv4_comes_first_and_each_address_once() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        let sorted = ipv4_first(vec![
            ip("fd00::20"),
            ip("192.168.1.20"),
            ip("fd00::20"),
            ip("10.0.0.2"),
            ip("192.168.1.20"),
        ]);
        assert_eq!(sorted, [ip("192.168.1.20"), ip("10.0.0.2"), ip("fd00::20")]);
    }

    #[test]
    fn an_address_has_the_port_and_the_token_and_ipv6_is_bracketed() {
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            url("192.168.1.20".parse().unwrap(), 43517, token),
            "https://192.168.1.20:43517/0123456789abcdef0123456789abcdef/report.txt"
        );
        assert_eq!(
            url("fd00::20".parse().unwrap(), 8443, token),
            "https://[fd00::20]:8443/0123456789abcdef0123456789abcdef/report.txt"
        );
    }

    #[test]
    fn only_the_first_get_of_the_address_gets_the_report() {
        let get = |target: &str| format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n");
        let path = "/abc/report.txt";
        let cases: [(String, bool, Answer); 12] = [
            (get(path), false, Answer::Download),
            (get("/abc/report.txt?x=1"), false, Answer::Download),
            (get(path), true, Answer::Gone),
            (format!("POST {path} HTTP/1.1\r\n\r\n"), false, Answer::MethodNotAllowed),
            (format!("POST {path} HTTP/1.1\r\n\r\n"), true, Answer::Gone),
            (get("/"), false, Answer::NotFound),
            (get("/abd/report.txt"), false, Answer::NotFound),
            (get("/abc/report.txt/"), true, Answer::NotFound),
            (get("/abc"), false, Answer::NotFound),
            ("GET\r\n\r\n".into(), false, Answer::BadRequest),
            (format!("GET {path} SPDY/3\r\n\r\n"), false, Answer::BadRequest),
            (format!("GET {path} HTTP/1.1 extra\r\n\r\n"), false, Answer::BadRequest),
        ];
        for (head, taken, answer) in cases {
            assert_eq!(answer_for(head.as_bytes(), path, taken), answer, "{head:?} taken={taken}");
        }
        assert_eq!(answer_for(b"GET \xff HTTP/1.1\r\n\r\n", path, false), Answer::BadRequest);
    }

    #[test]
    fn a_response_has_its_length_and_never_lets_a_browser_guess() {
        let bytes = response(&Answer::Download, b"report\n");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 7\r\n"));
        assert!(text.contains("Content-Disposition: attachment; filename=\"report.txt\"\r\n"));
        assert!(text.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(text.ends_with("\r\n\r\nreport\n"));
        let gone = String::from_utf8(response(&Answer::Gone, b"report\n")).unwrap();
        assert!(gone.starts_with("HTTP/1.1 410 Gone\r\n"));
        assert!(!gone.contains("report\n"));
        let wrong = String::from_utf8(response(&Answer::MethodNotAllowed, b"")).unwrap();
        assert!(wrong.contains("Allow: GET\r\n"));
    }

    #[test]
    fn the_screen_says_how_to_reach_the_download_and_what_to_check() {
        let urls = vec!["https://192.168.1.20:43517/t/report.txt".to_string()];
        let finger = "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99";
        let code = vec!["▀▀".to_string()];
        let lines = screen_lines(&urls, finger, 43517, Some(&code));
        let text = lines.join("\n");
        assert!(text.contains("  https://192.168.1.20:43517/t/report.txt"));
        assert!(text.contains("▀▀"));
        assert!(text.contains("elevate firewall allow tcp 43517"));
        assert!(text.contains("10 minutes"));
        let shown: String = lines
            .iter()
            .filter(|l| l.starts_with("  ") && l.contains(':'))
            .map(String::as_str)
            .collect();
        assert!(shown.contains("AA:BB:CC:DD") && shown.contains("88:99"));
        assert!(lines.iter().all(|line| line.chars().count() <= 78 || line.starts_with("  https")));
        assert_eq!(lines[lines.len() - 3], "▀▀");
        let none = screen_lines(&urls, finger, 43517, None).join("\n");
        assert!(none.contains("no room for the code"));
    }

    /// A server of one download on a free port of this computer, and its
    /// certificate.
    struct Server {
        address: SocketAddr,
        certificate: Certificate,
        path: String,
        done: JoinHandle<io::Result<Outcome>>,
    }

    fn start(until: Duration) -> Server {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let certificate = Certificate::new(&[LOCAL], SystemTime::now()).unwrap();
        let config = certificate.server_config().unwrap();
        let path = format!("/{}/{FILE_NAME}", token().unwrap());
        let offer = Offer { path: path.clone(), body: b"Dukovo problem report\n".to_vec() };
        let until = Instant::now() + until;
        let done = thread::spawn(move || serve(&listener, &config, &offer, until, &Instant::now));
        Server { address, certificate, path, done }
    }

    /// What a request got: the status line and the body.
    fn ask(server: &Server, name: IpAddr, request: &str) -> io::Result<(String, Vec<u8>)> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(server.certificate.der.clone()).unwrap();
        let config = tls::verified(roots, &[]);
        let mut stream = tls::connect(
            server.address,
            rustls::pki_types::ServerName::IpAddress(name.into()),
            &config,
            Duration::from_secs(5),
        )?;
        tls::send(&mut stream, request.as_bytes())?;
        let mut answer = Vec::new();
        // A server that closes without TLS's goodbye ends the read in an
        // error, and what came before it is still the answer.
        let _ = stream.read_to_end(&mut answer);
        let text = String::from_utf8_lossy(&answer).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head.lines().next().unwrap_or_default().to_string();
        Ok((status, body.as_bytes().to_vec()))
    }

    fn get(server: &Server, path: &str) -> (String, Vec<u8>) {
        ask(server, LOCAL, &format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
            .unwrap()
    }

    #[test]
    fn the_report_goes_once_to_a_client_that_checks_the_certificate() {
        let server = start(Duration::from_secs(60));
        let (status, body) = get(&server, "/wrong/report.txt");
        assert_eq!(
            (status.as_str(), body.as_slice()),
            ("HTTP/1.1 404 Not Found", &b"Not found.\n"[..])
        );
        let (status, _) =
            ask(&server, LOCAL, &format!("POST {} HTTP/1.1\r\n\r\n", server.path)).unwrap();
        assert_eq!(status, "HTTP/1.1 405 Method Not Allowed");
        let (status, body) = get(&server, &server.path);
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert_eq!(body, b"Dukovo problem report\n");
        assert_eq!(server.done.join().unwrap().unwrap(), Outcome::Downloaded);
    }

    #[test]
    fn a_certificate_for_another_address_is_refused() {
        let server = start(Duration::from_secs(60));
        let other: IpAddr = "192.0.2.1".parse().unwrap();
        let refused = ask(&server, other, "GET / HTTP/1.1\r\n\r\n");
        assert!(refused.is_err());
        // The refusal was one connection's: the real download still works.
        assert_eq!(get(&server, &server.path).0, "HTTP/1.1 200 OK");
        assert_eq!(server.done.join().unwrap().unwrap(), Outcome::Downloaded);
    }

    #[test]
    fn a_connection_that_breaks_does_not_end_the_download() {
        let server = start(Duration::from_secs(60));
        // A port scanner's connection and a phone that quit mid-handshake.
        drop(TcpStream::connect(server.address).unwrap());
        let mut half = TcpStream::connect(server.address).unwrap();
        half.write_all(&[0x16, 0x03, 0x01, 0x00]).unwrap();
        drop(half);
        assert_eq!(get(&server, &server.path).0, "HTTP/1.1 200 OK");
        assert_eq!(server.done.join().unwrap().unwrap(), Outcome::Downloaded);
    }

    #[test]
    fn connections_queued_behind_the_download_are_told_it_is_gone() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let certificate = Certificate::new(&[LOCAL], SystemTime::now()).unwrap();
        let config = certificate.server_config().unwrap();
        let offer = Offer { path: "/t/report.txt".into(), body: b"report".to_vec() };
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate.der.clone()).unwrap();
        let client = thread::spawn(move || {
            let config = tls::verified(roots, &[]);
            let name = rustls::pki_types::ServerName::IpAddress(LOCAL.into());
            let mut stream = tls::connect(address, name, &config, Duration::from_secs(5)).unwrap();
            tls::send(&mut stream, b"GET /t/report.txt HTTP/1.1\r\n\r\n").unwrap();
            let mut answer = Vec::new();
            let _ = stream.read_to_end(&mut answer);
            String::from_utf8_lossy(&answer).into_owned()
        });
        wait_for_connection(&listener, Duration::from_secs(5)).unwrap();
        tell_queued(&listener, &config, &offer, &Instant::now);
        let answer = client.join().unwrap();
        assert!(answer.starts_with("HTTP/1.1 410 Gone\r\n"), "{answer}");
        assert!(!answer.contains("report\n") && !answer.ends_with("report"));
    }

    #[test]
    fn no_download_in_the_time_ends_the_wait() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let certificate = Certificate::new(&[LOCAL], SystemTime::now()).unwrap();
        let config = certificate.server_config().unwrap();
        let offer = Offer { path: "/t/report.txt".into(), body: Vec::new() };
        // A clock that moves five minutes at every look: ten minutes pass
        // in two, with no need to wait for them.
        let start = Instant::now();
        let looks = Cell::new(0u32);
        let clock = || {
            looks.set(looks.get() + 1);
            start + Duration::from_secs(300) * looks.get()
        };
        let outcome = serve(&listener, &config, &offer, start + LIMIT, &clock).unwrap();
        assert_eq!(outcome, Outcome::TimedOut);
        assert!(looks.get() <= 4, "{} looks", looks.get());
    }

    #[test]
    fn the_wait_is_ten_minutes() {
        assert_eq!(LIMIT, Duration::from_mins(10));
    }
}
