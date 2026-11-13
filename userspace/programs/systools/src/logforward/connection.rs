// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A connection from logforward(8) to one syslog server (ADR 0482): TLS
//! (RFC 5425), TCP with each message's length in front (RFC 6587's octet
//! counting), or UDP, a message to a datagram (RFC 5426).
//!
//! Over TLS the server's certificate must chain to a certificate in the
//! destination's `ca` file when it names one, and to no other, or else
//! to the root certificates the system trusts, and must be valid for
//! `server_name`, or else for the host in the address. A certificate in
//! the `ca` file that the server presents as its own is trusted as it is,
//! pinned, whoever signed it: OpenSSL's `req -x509` marks the
//! certificates it makes as authorities, which the chain check refuses
//! from a server. A pinned certificate's name and dates are still
//! checked. Nothing turns the check off. With a client certificate,
//! logforward shows it to a server that asks for one (RFC 5425 §5.3).
//!
//! A connection that fails says which step failed and why, in the words
//! `logforward status` and `logforward test` show; the `log-forward` boot
//! test covers a trusted server, an untrusted one and UDP.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{WebPkiServerVerifier, verify_server_name};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, ProtocolVersion, RootCertStore,
    SignatureScheme,
};

use crate::{logd, tls, util};

/// How long connecting, and a TLS handshake, may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a write may wait on a server that takes nothing.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How much is gathered before it is written out over TCP or TLS: a TLS
/// record's most.
const WRITE_SIZE: usize = 16 << 10;

/// How messages reach a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// TLS over TCP (RFC 5425).
    Tls,
    /// TCP (RFC 6587).
    Tcp,
    /// UDP (RFC 5426).
    Udp,
}

impl Transport {
    /// The transport a setting's `transport` names: `tls`, `tcp` or `udp`.
    pub fn parse(name: &str) -> Option<Transport> {
        match name {
            "tls" => Some(Transport::Tls),
            "tcp" => Some(Transport::Tcp),
            "udp" => Some(Transport::Udp),
            _ => None,
        }
    }

    /// Its name in the settings and in `logforward status`.
    pub fn name(self) -> &'static str {
        match self {
            Transport::Tls => "tls",
            Transport::Tcp => "tcp",
            Transport::Udp => "udp",
        }
    }

    /// The port of an address without one: 6514 for TLS (RFC 5425), 514
    /// for TCP and UDP.
    pub fn default_port(self) -> u16 {
        match self {
            Transport::Tls => 6514,
            Transport::Tcp | Transport::Udp => 514,
        }
    }
}

/// What a destination's settings say of TLS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TlsSettings {
    /// The PEM file of the certificates to trust instead of the system's.
    pub ca: Option<PathBuf>,
    /// The name the server's certificate must be valid for, instead of
    /// the host in the address.
    pub server_name: Option<String>,
    /// The PEM files of the client certificate and of its key.
    pub client: Option<(PathBuf, PathBuf)>,
}

/// An open connection to a syslog server.
pub struct Connection {
    link: Link,
    /// Messages framed but not yet written, over TCP and TLS.
    pending: Vec<u8>,
    /// What the TLS handshake found.
    described: Option<String>,
}

enum Link {
    Tls(Box<tls::Stream>),
    Tcp(TcpStream),
    Udp(UdpSocket),
}

impl Connection {
    /// Connects to `address` (`host`, `host:port`, `[v6]:port` or an
    /// address), on `transport`'s own port when it names none, looking the
    /// name up each time so that a server that moves is followed. Over TLS
    /// it makes the handshake and checks the certificate as `tls` says.
    ///
    /// # Errors
    ///
    /// What failed, for `logforward status`: the name not found, no
    /// address answering, or the handshake refused and why.
    pub fn connect(
        address: &str,
        transport: Transport,
        tls: Option<&TlsSettings>,
    ) -> Result<Connection, String> {
        let (host, port) = util::split_port(address, transport.default_port())?;
        // The TLS settings' files are read first, so that a mistake in
        // them shows whether or not the server answers.
        let client = match transport {
            Transport::Tls => Some(Client::new(tls.unwrap_or(&TlsSettings::default()), host)?),
            Transport::Tcp | Transport::Udp => None,
        };
        let found = (host, port)
            .to_socket_addrs()
            .map_err(|err| format!("cannot find {host}: {}", util::strip_os_error(&err)))?;
        let addresses: Vec<SocketAddr> = found.collect();
        if addresses.is_empty() {
            return Err(format!("{host} has no address"));
        }
        let (link, described) = match client {
            Some(client) => {
                let tcp = tcp(&addresses, host, port)?;
                let (stream, described) = client.handshake(tcp, host, port)?;
                (Link::Tls(Box::new(stream)), Some(described))
            }
            None if transport == Transport::Udp => (Link::Udp(udp(&addresses, host, port)?), None),
            None => (Link::Tcp(tcp(&addresses, host, port)?), None),
        };
        Ok(Connection { link, pending: Vec::new(), described })
    }

    /// Sends one message: after its length and a space over TCP and TLS,
    /// gathered and written out by [`Connection::flush`] or once there is
    /// a TLS record's worth; as one datagram over UDP, at once.
    ///
    /// # Errors
    ///
    /// The socket's, when the server is gone or has taken nothing for 30
    /// seconds.
    pub fn send(&mut self, message: &[u8]) -> io::Result<()> {
        if let Link::Udp(socket) = &self.link {
            return socket.send(message).map(drop);
        }
        self.pending.extend_from_slice(message.len().to_string().as_bytes());
        self.pending.push(b' ');
        self.pending.extend_from_slice(message);
        if self.pending.len() >= WRITE_SIZE {
            self.write_pending()?;
        }
        Ok(())
    }

    /// Writes out every message sent, to the server's kernel; whether the
    /// server has them nobody says (see `position`).
    ///
    /// # Errors
    ///
    /// The socket's, as for [`Connection::send`].
    pub fn flush(&mut self) -> io::Result<()> {
        self.write_pending()?;
        match &mut self.link {
            Link::Tls(stream) => stream.flush(),
            Link::Tcp(stream) => stream.flush(),
            Link::Udp(_) => Ok(()),
        }
    }

    fn write_pending(&mut self) -> io::Result<()> {
        match &mut self.link {
            Link::Tls(stream) => stream.write_all(&self.pending)?,
            Link::Tcp(stream) => stream.write_all(&self.pending)?,
            Link::Udp(_) => {}
        }
        self.pending.clear();
        Ok(())
    }

    /// Whether the server still holds the connection open: it has not
    /// closed it, reset it or sent a TLS alert. A server whose machine
    /// stopped without a word still looks open, until a write fails or
    /// its kernel answers with a reset. UDP connections are always open.
    pub fn open(&mut self) -> bool {
        match &mut self.link {
            Link::Udp(_) => true,
            Link::Tcp(stream) => without_waiting(stream, discard),
            Link::Tls(stream) => {
                let tls::Stream { conn, sock } = stream.as_mut();
                without_waiting(sock, |sock| tls_open(conn, sock))
            }
        }
    }

    /// What the TLS handshake found, for `logforward status` and
    /// `logforward test`: "TLS 1.3, certificate for logs.example.com,
    /// issued by Example CA". `None` over TCP and UDP.
    pub fn describe(&self) -> Option<&str> {
        self.described.as_deref()
    }

    /// Ends the connection as RFC 5425 §4.4 asks: TLS's `close_notify`,
    /// then the socket closed.
    pub fn close(mut self) {
        // A server that has gone cannot be told; the connection ends
        // either way, so neither result matters.
        let _ = self.flush();
        match &mut self.link {
            Link::Tls(stream) => {
                stream.conn.send_close_notify();
                let _ = stream.conn.complete_io(&mut stream.sock);
                let _ = stream.sock.shutdown(Shutdown::Both);
            }
            Link::Tcp(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Link::Udp(_) => {}
        }
    }
}

/// A UDP socket connected to the first of `addresses` one can be.
fn udp(addresses: &[SocketAddr], host: &str, port: u16) -> Result<UdpSocket, String> {
    let mut last = None;
    for address in addresses {
        let local = match address {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        match UdpSocket::bind(local).and_then(|socket| socket.connect(address).map(|()| socket)) {
            Ok(socket) => return Ok(socket),
            Err(err) => last = Some(err),
        }
    }
    Err(cannot_connect(host, port, last))
}

/// A TCP connection to the first of `addresses` that answers, each given
/// [`CONNECT_TIMEOUT`].
fn tcp(addresses: &[SocketAddr], host: &str, port: u16) -> Result<TcpStream, String> {
    let mut last = None;
    for address in addresses {
        match TcpStream::connect_timeout(address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                let timeouts = stream
                    .set_read_timeout(Some(CONNECT_TIMEOUT))
                    .and_then(|()| stream.set_write_timeout(Some(WRITE_TIMEOUT)));
                return match timeouts {
                    Ok(()) => Ok(stream),
                    Err(err) => Err(cannot_connect(host, port, Some(err))),
                };
            }
            Err(err) => last = Some(err),
        }
    }
    Err(cannot_connect(host, port, last))
}

fn cannot_connect(host: &str, port: u16, error: Option<io::Error>) -> String {
    match error {
        Some(err) => format!("cannot connect to {host}:{port}: {}", util::strip_os_error(&err)),
        None => format!("cannot connect to {host}:{port}"),
    }
}

/// A TLS client made from a destination's settings, ready to connect.
struct Client {
    config: Arc<ClientConfig>,
    verifier: Arc<Verifier>,
    /// The name the server's certificate must be valid for.
    name: String,
    /// The `ca` file, for messages.
    ca: Option<PathBuf>,
}

impl Client {
    /// Reads `settings`' files; the certificate is checked for
    /// `server_name`, or else `host`.
    fn new(settings: &TlsSettings, host: &str) -> Result<Client, String> {
        let verifier = Arc::new(Verifier::new(settings.ca.as_deref())?);
        // rustls calls every verifier but its own "dangerous"; this one is
        // rustls's own with pinning added (see Verifier).
        let builder = ClientConfig::builder_with_provider(tls::provider())
            .with_safe_default_protocol_versions()
            .expect(tls::VERSIONS)
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone());
        let config = match &settings.client {
            Some((certificate, key_file)) => {
                let chain = certificates(certificate)?;
                let key = PrivateKeyDer::from_pem_file(key_file).map_err(|err| {
                    format!("cannot read the key in {}: {err}", key_file.display())
                })?;
                builder.with_client_auth_cert(chain, key).map_err(|err| {
                    format!(
                        "cannot use the certificate in {} with the key in {}: {err}",
                        certificate.display(),
                        key_file.display()
                    )
                })?
            }
            None => builder.with_no_client_auth(),
        };
        let name = settings.server_name.as_deref().unwrap_or(host).to_string();
        Ok(Client { config: Arc::new(config), verifier, name, ca: settings.ca.clone() })
    }

    /// Makes the handshake over `tcp`, to `host` at `port`, and describes
    /// what it found.
    fn handshake(
        &self,
        tcp: TcpStream,
        host: &str,
        port: u16,
    ) -> Result<(tls::Stream, String), String> {
        let name = ServerName::try_from(self.name.clone())
            .map_err(|_| format!("'{}' is not a server name", self.name))?;
        let stream = tls::handshake(tcp, name, &self.config).map_err(|err| {
            let why = match err.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
                Some(refused) => refusal(refused, &self.name, self.ca.as_deref()),
                None if err.kind() == io::ErrorKind::UnexpectedEof => {
                    "the server closed the connection during the handshake".to_string()
                }
                None if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
                {
                    format!("no TLS handshake within {} seconds", CONNECT_TIMEOUT.as_secs())
                }
                None => util::strip_os_error(&err),
            };
            format!("TLS with {host}:{port}: {why}")
        })?;
        let version = match stream.conn.protocol_version() {
            Some(ProtocolVersion::TLSv1_3) => "TLS 1.3".to_string(),
            Some(ProtocolVersion::TLSv1_2) => "TLS 1.2".to_string(),
            other => format!("{other:?}"),
        };
        let from = match &self.ca {
            Some(ca) if self.verifier.took_pinned.load(Ordering::Relaxed) => {
                format!("pinned in {}", ca.display())
            }
            _ => {
                let leaf = stream.conn.peer_certificates().and_then(<[_]>::first);
                let issuer = leaf.and_then(|leaf| issuer_name(leaf));
                format!("issued by {}", issuer.as_deref().unwrap_or("an unnamed authority"))
            }
        };
        Ok((stream, format!("{version}, certificate for {}, {from}", self.name)))
    }
}

/// The certificates in PEM file `path`, at least one.
fn certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let cannot =
        |err: rustls::pki_types::pem::Error| format!("cannot read {}: {err}", path.display());
    let found: Vec<_> = CertificateDer::pem_file_iter(path)
        .map_err(cannot)?
        .collect::<Result<_, _>>()
        .map_err(cannot)?;
    if found.is_empty() {
        return Err(format!("{} holds no certificate", path.display()));
    }
    Ok(found)
}

/// Why rustls refused a handshake, in words for `logforward status`.
fn refusal(error: &rustls::Error, name: &str, ca: Option<&Path>) -> String {
    if is_authority_as_server(error) {
        return "its certificate is an authority's, not a server's".into();
    }
    let rustls::Error::InvalidCertificate(certificate) = error else {
        return match error {
            rustls::Error::AlertReceived(alert) => format!("the server refused it ({alert:?})"),
            rustls::Error::NoCertificatesPresented => "the server sent no certificate".into(),
            other => other.to_string(),
        };
    };
    match certificate {
        CertificateError::UnknownIssuer => match ca {
            Some(ca) => format!("its certificate is not from an authority in {}", ca.display()),
            None => "its certificate is not from an authority the system trusts; name the \
                     server's authority in `ca`"
                .into(),
        },
        CertificateError::NotValidForName => format!("its certificate is not for {name}"),
        CertificateError::NotValidForNameContext { presented, .. } => {
            format!("its certificate is not for {name} but for {}", presented.join(", "))
        }
        CertificateError::Expired => "its certificate has expired".into(),
        CertificateError::ExpiredContext { not_after, .. } => {
            format!("its certificate expired on {}", day(*not_after))
        }
        CertificateError::NotValidYet => "its certificate is not valid yet".into(),
        CertificateError::NotValidYetContext { not_before, .. } => {
            format!("its certificate is not valid until {}", day(*not_before))
        }
        CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => {
            "its certificate is not for a server".into()
        }
        CertificateError::BadSignature => "its certificate's signature is wrong".into(),
        other => format!("its certificate is refused: {other:?}"),
    }
}

/// Whether `error` is webpki's refusal of an authority's certificate as a
/// server's own. rustls passes it on as an error of webpki's, which only
/// its name tells apart.
fn is_authority_as_server(error: &rustls::Error) -> bool {
    matches!(
        error,
        rustls::Error::InvalidCertificate(CertificateError::Other(other))
            if format!("{other:?}").contains("CaUsedAsEndEntity")
    )
}

/// `time`'s day, `YYYY-MM-DD`.
fn day(time: UnixTime) -> String {
    let days = i64::try_from(time.as_secs() / 86_400).unwrap_or(i64::MAX);
    let (year, month, day) = logd::civil(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Checks a server's certificate as rustls's own check does, against
/// the roots it was given, and takes one of `pinned` that the server
/// presents as its own.
#[derive(Debug)]
struct Verifier {
    webpki: Arc<WebPkiServerVerifier>,
    /// The certificates of the destination's `ca` file.
    pinned: Vec<CertificateDer<'static>>,
    /// Whether the server's certificate was taken as pinned.
    took_pinned: AtomicBool,
}

impl Verifier {
    /// The check against the certificates in `ca`, and only them, or else
    /// against the system's roots.
    fn new(ca: Option<&Path>) -> Result<Verifier, String> {
        let (roots, pinned) = match ca {
            Some(ca) => {
                let pinned = certificates(ca)?;
                let mut roots = RootCertStore::empty();
                roots.add_parsable_certificates(pinned.iter().cloned());
                (roots, pinned)
            }
            None => (tls::roots(&[], "logforward"), Vec::new()),
        };
        let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), tls::provider())
            .build()
            .map_err(|err| match ca {
                Some(ca) => format!("no certificate in {} can be trusted: {err}", ca.display()),
                None => format!("the system trusts no root certificates: {err}"),
            })?;
        Ok(Verifier { webpki, pinned, took_pinned: AtomicBool::new(false) })
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let chained = self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        let pinned = self.pinned.iter().any(|pin| pin.as_ref() == end_entity.as_ref());
        if chained.is_ok() {
            return chained;
        }
        if !pinned {
            // webpki refuses an authority's certificate as a server's before
            // it looks for the authority. One that signs itself, as `openssl
            // req -x509` makes, is from no authority the server is trusted
            // by, which is what the person needs to hear; refusal() words
            // one that an authority signed.
            return match chained {
                Err(error) if is_authority_as_server(&error) && self_issued(end_entity) => {
                    Err(CertificateError::UnknownIssuer.into())
                }
                other => other,
            };
        }
        verify_server_name(&ParsedCertificate::try_from(end_entity)?, server_name)?;
        let (not_before, not_after) = validity(end_entity).ok_or(CertificateError::BadEncoding)?;
        let now = now.as_secs();
        if now < not_before {
            return Err(CertificateError::NotValidYet.into());
        }
        if now > not_after {
            return Err(CertificateError::Expired.into());
        }
        self.took_pinned.store(true, Ordering::Relaxed);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

/// Runs `check` on `socket` with reads that return at once, and puts the
/// socket back as it was. A socket that cannot be switched counts as
/// closed.
fn without_waiting(socket: &mut TcpStream, check: impl FnOnce(&mut TcpStream) -> bool) -> bool {
    if socket.set_nonblocking(true).is_err() {
        return false;
    }
    let open = check(socket);
    socket.set_nonblocking(false).is_ok() && open
}

/// Reads and drops whatever a TCP syslog server sent, which is nothing
/// as a rule; whether it has closed the connection.
fn discard(stream: &mut TcpStream) -> bool {
    let mut buffer = [0u8; 512];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return false,
            Ok(_) => {}
            Err(err) => return err.kind() == io::ErrorKind::WouldBlock,
        }
    }
}

/// Reads what a TLS server sent (session tickets as a rule) and drops any
/// data; whether it has closed the connection or sent an alert.
fn tls_open(conn: &mut rustls::ClientConnection, sock: &mut TcpStream) -> bool {
    let mut buffer = [0u8; 512];
    loop {
        match conn.read_tls(sock) {
            Ok(0) => return false,
            Ok(_) => {}
            Err(err) => return err.kind() == io::ErrorKind::WouldBlock,
        }
        let Ok(state) = conn.process_new_packets() else { return false };
        if state.peer_has_closed() {
            return false;
        }
        while conn.reader().read(&mut buffer).is_ok_and(|read| read > 0) {}
    }
}

/// DER's tags, as X.509 certificates use them.
const SEQUENCE: u8 = 0x30;
const SET: u8 = 0x31;
const OBJECT_ID: u8 = 0x06;
const VERSION: u8 = 0xa0;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
const BMP_STRING: u8 = 0x1e;
/// The object identifiers of an X.509 name's common name (2.5.4.3) and
/// organization (2.5.4.10).
const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
const ORGANIZATION: &[u8] = &[0x55, 0x04, 0x0a];

/// The first DER element in `der`: its tag, its contents, and what
/// follows it.
fn element(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = der.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = match first {
        0..=0x7f => (usize::from(first), rest),
        0x81..=0x84 => {
            let (len, rest) = rest.split_at_checked(usize::from(first & 0x7f))?;
            (len.iter().fold(0, |len, &byte| len << 8 | usize::from(byte)), rest)
        }
        _ => return None,
    };
    let (contents, rest) = rest.split_at_checked(len)?;
    Some((tag, contents, rest))
}

/// The contents of certificate `der`'s issuer, validity and subject.
fn issuer_validity_subject(der: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let (SEQUENCE, certificate, _) = element(der)? else { return None };
    let (SEQUENCE, signed, _) = element(certificate)? else { return None };
    // The version, when it is not 1, then the serial number and the
    // signature's algorithm come before the issuer.
    let (first, _, mut rest) = element(signed)?;
    if first == VERSION {
        rest = element(rest)?.2;
    }
    rest = element(rest)?.2;
    let (SEQUENCE, issuer, rest) = element(rest)? else { return None };
    let (SEQUENCE, validity, rest) = element(rest)? else { return None };
    let (SEQUENCE, subject, _) = element(rest)? else { return None };
    Some((issuer, validity, subject))
}

/// Whether certificate `der` names itself as its issuer, as one that
/// signs itself does.
fn self_issued(der: &[u8]) -> bool {
    issuer_validity_subject(der).is_some_and(|(issuer, _, subject)| issuer == subject)
}

/// The common name of certificate `der`'s issuer, or else its
/// organization.
fn issuer_name(der: &[u8]) -> Option<String> {
    let (issuer, _, _) = issuer_validity_subject(der)?;
    let (mut common, mut organization) = (None, None);
    let mut names = issuer;
    while let Some((SET, attributes, rest)) = element(names) {
        names = rest;
        let mut attributes = attributes;
        while let Some((SEQUENCE, attribute, rest)) = element(attributes) {
            attributes = rest;
            let Some((OBJECT_ID, kind, value)) = element(attribute) else { continue };
            let Some((tag, text, _)) = element(value) else { continue };
            let text = if tag == BMP_STRING {
                let units: Vec<u16> =
                    text.as_chunks::<2>().0.iter().map(|unit| u16::from_be_bytes(*unit)).collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(text).into_owned()
            };
            if kind == COMMON_NAME {
                common = Some(text);
            } else if kind == ORGANIZATION {
                organization = Some(text);
            }
        }
    }
    common.or(organization)
}

/// When certificate `der` starts and ends, in seconds since the epoch.
pub(crate) fn validity(der: &[u8]) -> Option<(u64, u64)> {
    let (_, validity, _) = issuer_validity_subject(der)?;
    let (start_tag, start, rest) = element(validity)?;
    let (end_tag, end, _) = element(rest)?;
    Some((seconds(start_tag, start)?, seconds(end_tag, end)?))
}

/// Seconds since the epoch of an X.509 time: UTCTime's `YYMMDDHHMMSSZ`,
/// its years from 1950 to 2049 (RFC 5280 §4.1.2.5.1), or
/// GeneralizedTime's `YYYYMMDDHHMMSSZ`. A time before 1970 is taken as
/// its start, which bounds a certificate the same way now.
fn seconds(tag: u8, text: &[u8]) -> Option<u64> {
    let digits = text.strip_suffix(b"Z")?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |at: usize, len: usize| {
        digits[at..at + len].iter().fold(0u32, |n, &digit| n * 10 + u32::from(digit - b'0'))
    };
    let (year, rest) = match (tag, digits.len()) {
        (UTC_TIME, 12) => match number(0, 2) {
            short @ 0..50 => (2000 + short, 2),
            short => (1900 + short, 2),
        },
        (GENERALIZED_TIME, 14) => (number(0, 4), 4),
        _ => return None,
    };
    let (month, day) = (number(rest, 2), number(rest + 2, 2));
    let time = number(rest + 4, 2) * 3600 + number(rest + 6, 2) * 60 + number(rest + 8, 2);
    let days = logd::days(i64::from(year), month, day);
    Some(u64::try_from(days * 86_400 + i64::from(time)).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use rustls::server::WebPkiClientVerifier;
    use rustls::{ServerConfig, ServerConnection, StreamOwned};

    /// tls-test's certificates and keys, made by OpenSSL (its
    /// testdata/make.sh): servers for tls.test and 127.0.0.1 from "Dukovo
    /// test CA", `untrusted` from another authority, `expired` ended in
    /// 2021, `self-signed` an authority's own, and a client certificate.
    fn testdata(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tls-test/testdata").join(name)
    }

    fn trusting(ca: &str) -> TlsSettings {
        TlsSettings { ca: Some(testdata(ca)), server_name: Some("tls.test".into()), client: None }
    }

    /// A TLS server on lo with certificate `name`, asking for a client
    /// certificate from the test authority when `clients` says so; it
    /// hands back what one connection sent it.
    fn tls_server(name: &str, clients: bool) -> (String, JoinHandle<Vec<u8>>) {
        let provider = tls::provider();
        let chain = certificates(&testdata(&format!("{name}.pem"))).unwrap();
        let key = PrivateKeyDer::from_pem_file(testdata(&format!("{name}.key"))).unwrap();
        let builder = ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = if clients {
            let mut roots = RootCertStore::empty();
            roots.add_parsable_certificates(certificates(&testdata("ca.pem")).unwrap());
            let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap();
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let config = Arc::new(builder.with_single_cert(chain, key).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut stream = StreamOwned::new(ServerConnection::new(config).unwrap(), socket);
            let mut received = Vec::new();
            // The handshake refused or the client gone ends it alike.
            let _ = stream.read_to_end(&mut received);
            received
        });
        (address, server)
    }

    #[test]
    fn a_server_whose_certificate_the_ca_file_vouches_for_gets_each_message_counted() {
        let (address, server) = tls_server("rsa-2048", false);
        let mut connection =
            Connection::connect(&address, Transport::Tls, Some(&trusting("ca.pem"))).unwrap();
        connection.send(b"<14>1 first").unwrap();
        connection.send(b"<14>1 second").unwrap();
        connection.flush().unwrap();
        let described = connection.describe().unwrap().to_string();
        assert!(described.starts_with("TLS 1.3, certificate for tls.test"), "{described}");
        assert!(described.ends_with("issued by Dukovo test CA"), "{described}");
        assert!(connection.open());
        connection.close();
        assert_eq!(server.join().unwrap(), b"11 <14>1 first12 <14>1 second");
    }

    #[test]
    fn a_certificate_from_another_authority_is_refused() {
        let (address, server) = tls_server("untrusted", false);
        let refused =
            Connection::connect(&address, Transport::Tls, Some(&trusting("ca.pem"))).err().unwrap();
        assert!(refused.contains("its certificate is not from an authority in"), "{refused}");
        assert!(refused.ends_with("testdata/ca.pem"), "{refused}");
        assert!(server.join().unwrap().is_empty());
    }

    #[test]
    fn a_certificate_that_signs_itself_is_refused_as_from_no_trusted_authority_unless_pinned() {
        let (address, server) = tls_server("self-signed", false);
        let refused =
            Connection::connect(&address, Transport::Tls, Some(&trusting("ca.pem"))).err().unwrap();
        assert!(refused.contains("its certificate is not from an authority in"), "{refused}");
        assert!(server.join().unwrap().is_empty());
        let (address, server) = tls_server("self-signed", false);
        let mut connection =
            Connection::connect(&address, Transport::Tls, Some(&trusting("self-signed.pem")))
                .unwrap();
        let described = connection.describe().unwrap().to_string();
        assert!(described.contains("certificate for tls.test, pinned in"), "{described}");
        connection.send(b"pinned").unwrap();
        connection.close();
        assert_eq!(server.join().unwrap(), b"6 pinned");
    }

    #[test]
    fn an_authoritys_certificate_an_authority_signed_is_refused_as_not_a_servers() {
        // tls-test's intermediate authority, which "Dukovo test CA" signed.
        let verifier = Verifier::new(Some(&testdata("ca.pem"))).unwrap();
        let authority = certificates(&testdata("intermediate.pem")).unwrap().remove(0);
        let name = ServerName::try_from("tls.test").unwrap();
        let refused = verifier
            .verify_server_cert(&authority, &[], &name, &[], UnixTime::now())
            .err()
            .unwrap();
        assert_eq!(
            refusal(&refused, "tls.test", Some(&testdata("ca.pem"))),
            "its certificate is an authority's, not a server's"
        );
        assert!(!self_issued(&authority));
        assert!(self_issued(&certificates(&testdata("self-signed.pem")).unwrap().remove(0)));
    }

    #[test]
    fn a_certificate_for_another_name_is_refused() {
        let (address, server) = tls_server("rsa-2048", false);
        let settings = TlsSettings { server_name: Some("other.test".into()), ..trusting("ca.pem") };
        let refused = Connection::connect(&address, Transport::Tls, Some(&settings)).err().unwrap();
        assert!(refused.contains("its certificate is not for other.test but for"), "{refused}");
        assert!(server.join().unwrap().is_empty());
    }

    #[test]
    fn an_expired_certificate_is_refused() {
        let (address, server) = tls_server("expired", false);
        let refused =
            Connection::connect(&address, Transport::Tls, Some(&trusting("ca.pem"))).err().unwrap();
        assert!(refused.contains("its certificate expired on 2021-01-01"), "{refused}");
        assert!(server.join().unwrap().is_empty());
    }

    #[test]
    fn a_certificate_in_the_ca_file_is_taken_as_pinned_whoever_signed_it() {
        let (address, server) = tls_server("untrusted", false);
        let mut connection =
            Connection::connect(&address, Transport::Tls, Some(&trusting("untrusted.pem")))
                .unwrap();
        let described = connection.describe().unwrap().to_string();
        assert!(described.ends_with("testdata/untrusted.pem"), "{described}");
        assert!(described.contains("certificate for tls.test, pinned in"), "{described}");
        connection.send(b"pinned").unwrap();
        connection.close();
        assert_eq!(server.join().unwrap(), b"6 pinned");
    }

    #[test]
    fn a_pinned_certificate_still_needs_the_name_and_the_dates() {
        let (address, server) = tls_server("untrusted", false);
        let settings =
            TlsSettings { server_name: Some("other.test".into()), ..trusting("untrusted.pem") };
        let refused = Connection::connect(&address, Transport::Tls, Some(&settings)).err().unwrap();
        assert!(refused.contains("is not for other.test"), "{refused}");
        server.join().unwrap();

        let (address, server) = tls_server("expired", false);
        let refused = Connection::connect(&address, Transport::Tls, Some(&trusting("expired.pem")))
            .err()
            .unwrap();
        assert!(refused.contains("its certificate has expired"), "{refused}");
        server.join().unwrap();
    }

    #[test]
    fn a_client_certificate_is_shown_to_a_server_that_asks() {
        let (address, server) = tls_server("rsa-2048", true);
        let settings = TlsSettings {
            client: Some((testdata("client.pem"), testdata("client.key"))),
            ..trusting("ca.pem")
        };
        let mut connection =
            Connection::connect(&address, Transport::Tls, Some(&settings)).unwrap();
        connection.send(b"mutual").unwrap();
        connection.close();
        assert_eq!(server.join().unwrap(), b"6 mutual");
    }

    #[test]
    fn a_missing_ca_file_is_named_before_any_connection() {
        let refused =
            Connection::connect("127.0.0.1:1", Transport::Tls, Some(&trusting("missing.pem")))
                .err()
                .unwrap();
        assert!(
            refused.starts_with("cannot read ") && refused.contains("missing.pem"),
            "{refused}"
        );
    }

    #[test]
    fn over_tcp_each_message_has_its_length_in_front() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            socket.read_to_end(&mut received).unwrap();
            received
        });
        let mut connection = Connection::connect(&address, Transport::Tcp, None).unwrap();
        assert_eq!(connection.describe(), None);
        connection.send("é".as_bytes()).unwrap();
        connection.send(b"").unwrap();
        connection.close();
        assert_eq!(server.join().unwrap(), "2 é0 ".as_bytes());
    }

    #[test]
    fn over_udp_each_message_is_a_datagram_as_it_is() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let mut connection = Connection::connect(&address, Transport::Udp, None).unwrap();
        connection.send(b"<14>1 one").unwrap();
        connection.send(b"<14>1 two").unwrap();
        let mut buffer = [0u8; 64];
        let read = listener.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..read], b"<14>1 one");
        let read = listener.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..read], b"<14>1 two");
        assert!(connection.open());
    }

    #[test]
    fn a_server_that_closes_the_connection_is_seen_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || drop(listener.accept().unwrap()));
        let mut connection = Connection::connect(&address, Transport::Tcp, None).unwrap();
        server.join().unwrap();
        let mut waited = 0;
        while connection.open() && waited < 100 {
            thread::sleep(Duration::from_millis(10));
            waited += 1;
        }
        assert!(!connection.open());
    }

    #[test]
    fn a_tls_server_that_ends_the_session_is_seen_closed() {
        let (address, server) = tls_server("rsa-2048", false);
        let mut connection =
            Connection::connect(&address, Transport::Tls, Some(&trusting("ca.pem"))).unwrap();
        assert!(connection.open());
        // The test server ends its session when its read times out; a
        // shutdown of the client's sending side ends it sooner.
        if let Link::Tls(stream) = &connection.link {
            stream.sock.shutdown(Shutdown::Write).unwrap();
        }
        server.join().unwrap();
        let mut waited = 0;
        while connection.open() && waited < 100 {
            thread::sleep(Duration::from_millis(10));
            waited += 1;
        }
        assert!(!connection.open());
    }

    #[test]
    fn a_port_nobody_listens_on_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let refused =
            Connection::connect(&address.to_string(), Transport::Tcp, None).err().unwrap();
        assert_eq!(
            refused,
            format!("cannot connect to 127.0.0.1:{}: Connection refused", address.port())
        );
    }

    #[test]
    fn transports_have_their_names_and_ports() {
        for (name, transport, port) in [
            ("tls", Transport::Tls, 6514),
            ("tcp", Transport::Tcp, 514),
            ("udp", Transport::Udp, 514),
        ] {
            assert_eq!(Transport::parse(name), Some(transport));
            assert_eq!(transport.name(), name);
            assert_eq!(transport.default_port(), port);
        }
        assert_eq!(Transport::parse("relp"), None);
    }

    #[test]
    fn the_issuer_and_dates_are_read_from_the_certificate() {
        let leaf = certificates(&testdata("expired.pem")).unwrap().remove(0);
        assert_eq!(issuer_name(&leaf).as_deref(), Some("Dukovo test CA"));
        // 2020-01-01 and 2021-01-01, at midnight UTC.
        assert_eq!(validity(&leaf), Some((1_577_836_800, 1_609_459_200)));
        assert_eq!(seconds(UTC_TIME, b"491231235959Z"), Some(2_524_607_999));
        assert_eq!(seconds(UTC_TIME, b"500101000000Z"), Some(0));
        assert_eq!(seconds(GENERALIZED_TIME, b"19500101000000Z"), Some(0));
        assert_eq!(seconds(GENERALIZED_TIME, b"20500101000000Z"), Some(2_524_608_000));
        assert_eq!(seconds(UTC_TIME, b"2001010000Z"), None);
        assert_eq!(element(&[0x30, 0x82, 0x01]), None);
    }
}
