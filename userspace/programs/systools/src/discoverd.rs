// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! discoverd(8): what is on the networks this machine is on, and this
//! machine's name on them (ADR 0338).
//!
//! It speaks multicast DNS (RFC 6762) and DNS-SD (RFC 6763) on each card,
//! with mdns-sd: it answers for `NAME.local`, this machine's name, with
//! the addresses of the card each question came on, as Avahi does, and
//! offers `NAME._workstation._tcp.local.`, so that a browser of the
//! network lists it. It finds `.local` names for resolved(8), and the
//! services the network's machines offer, and with WS-Discovery's Probe
//! (on 239.255.255.250, port 3702) the Windows machines, printers and
//! scanners that answer it, for discover(1).
//!
//! Its socket, `/system/live/discoverd/socket`, which a bundle reaches
//! only with the network.local permission (ADR 0337), takes one request
//! a connection, a line:
//!
//! - `resolve NAME`: a line for each address of `NAME.local`, IPv4 first;
//!   none if no machine answers within the time the request may take;
//! - `browse SECONDS [TYPE...]`: for that long, a line for each service
//!   found, of the types named or of every type the network's machines
//!   say they offer: `service`, its name, type, host, port, addresses
//!   and TXT record, by tabs; and one for each WS-Discovery answer:
//!   `device`, its endpoint, types, addresses, and where it describes
//!   itself.
//!
//! Each connection has a thread of its own, CLIENTS_MAX at once; the
//! daemon's own thread answers the network.

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mdns_sd::{HostnameResolutionEvent, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::args::Args;
use crate::util::{self, Fail, fail};

/// discoverd's folder, which a bundle with network.local sees.
pub const FOLDER: &str = "/system/live/discoverd";

/// Where it answers.
pub const SOCKET: &str = "/system/live/discoverd/socket";

/// The service every machine running discoverd offers, as Avahi's
/// `publish-workstation` does: it says the machine is there.
const WORKSTATION: &str = "_workstation._tcp.local.";

/// DNS-SD's question for the types of service a network has (RFC 6763,
/// section 9).
const TYPES: &str = "_services._dns-sd._udp.local.";

/// The longest a browse may take, and a resolve.
const BROWSE_MAX: Duration = Duration::from_secs(30);
const RESOLVE_MAX: Duration = Duration::from_secs(5);

/// The most connections it answers at once.
const CLIENTS_MAX: usize = 32;

/// The longest request line.
const LINE_MAX: usize = 1024;

/// WS-Discovery's group and port, for IPv4.
const WSD_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const WSD_PORT: u16 = 3702;

/// Runs discoverd.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let daemon = ServiceDaemon::new().map_err(|e| fail(format!("mDNS: {e}")))?;
    // Not on lo: it carries no other machine's questions, and its
    // addresses would be offered as this machine's.
    daemon
        .disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6])
        .map_err(|e| fail(format!("mDNS: {e}")))?;
    let name = label(&util::hostname());
    let host = format!("{name}.local.");
    let info = ServiceInfo::new(WORKSTATION, &name, &host, (), 9, None)
        .map_err(|e| fail(format!("{name}: {e}")))?
        .enable_addr_auto();
    daemon.register(info).map_err(|e| fail(format!("offering {name}: {e}")))?;
    let listener = listen().map_err(|e| util::failed(&SOCKET, e))?;
    let clients = Arc::new(AtomicUsize::new(0));
    let me = Arc::new(host);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if clients.fetch_add(1, Ordering::AcqRel) >= CLIENTS_MAX {
            clients.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let (daemon, count, me) = (daemon.clone(), Arc::clone(&clients), Arc::clone(&me));
        let spawned = thread::Builder::new().name("client".into()).spawn(move || {
            let _ = serve(&daemon, &me, &stream);
            count.fetch_sub(1, Ordering::AcqRel);
        });
        if spawned.is_err() {
            clients.fetch_sub(1, Ordering::AcqRel);
        }
    }
    Ok(0)
}

/// The socket, made anew, which anyone may connect to.
fn listen() -> io::Result<UnixListener> {
    fs::create_dir_all(FOLDER)?;
    match fs::remove_file(SOCKET) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(SOCKET)?;
    fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o666))?;
    Ok(listener)
}

/// `hostname` as a DNS label for `.local`: its first label, letters,
/// digits and hyphens, at most 63 of them, or `dukovo` if none are left.
fn label(hostname: &str) -> String {
    let first = hostname.split('.').next().unwrap_or_default();
    let label: String = first
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .take(63)
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() { "dukovo".into() } else { label.into() }
}

/// How long a client has to send its request.
const REQUEST_WAIT: Duration = Duration::from_secs(5);

/// Calls `each` with each line `stream` sends, without its end, until it
/// ends, `each` says to stop, or `wait` passes (TimedOut), by poll: Radix's
/// Unix sockets take no SO_RCVTIMEO. A line is LINE_MAX bytes at most, or
/// cut there; with `cap`, so is all of what is read.
fn lines(
    stream: &UnixStream,
    wait: Duration,
    cap: Option<usize>,
    mut each: impl FnMut(&str) -> bool,
) -> io::Result<()> {
    let until = Instant::now() + wait;
    let mut wanted = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let mut line: Vec<u8> = Vec::new();
    let mut read = 0;
    let mut buffer = [0u8; 4096];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let ms = i32::try_from(left.as_millis().max(1)).unwrap_or(i32::MAX);
        // SAFETY: one pollfd, valid for the call.
        if unsafe { libc::poll(&raw mut wanted, 1, ms) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if wanted.revents == 0 {
            continue;
        }
        let n = match (&*stream).read(&mut buffer) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            break;
        }
        for &byte in &buffer[..n] {
            if byte != b'\n' {
                if line.len() < LINE_MAX {
                    line.push(byte);
                }
                continue;
            }
            if !each(String::from_utf8_lossy(&line).trim_end_matches('\r')) {
                return Ok(());
            }
            line.clear();
        }
        read += n;
        if cap.is_some_and(|cap| read > cap) {
            break;
        }
    }
    if !line.is_empty() {
        each(String::from_utf8_lossy(&line).trim_end_matches('\r'));
    }
    Ok(())
}

/// Answers the request on `stream`.
fn serve(daemon: &ServiceDaemon, me: &str, stream: &UnixStream) -> io::Result<()> {
    let mut line = String::new();
    lines(stream, REQUEST_WAIT, Some(LINE_MAX), |first| {
        line = first.to_string();
        false
    })?;
    let mut words = line.split_whitespace();
    let mut out = io::BufWriter::new(stream);
    match words.next() {
        Some("resolve") => {
            let Some(name) = words.next() else { return writeln!(out, "error: resolve NAME") };
            for address in resolve_here(daemon, me, name, RESOLVE_MAX) {
                writeln!(out, "{address}")?;
            }
        }
        Some("browse") => {
            let seconds = words.next().and_then(|w| w.parse::<f64>().ok()).unwrap_or(3.0);
            let wait = Duration::try_from_secs_f64(seconds).unwrap_or(BROWSE_MAX).min(BROWSE_MAX);
            let types: Vec<String> = words.map(service_type).collect();
            browse(daemon, wait, &types, &mut out)?;
        }
        _ => writeln!(out, "error: resolve NAME, or browse SECONDS [TYPE...]")?,
    }
    out.flush()
}

/// `name` as a DNS-SD type with its domain: `_http._tcp` is
/// `_http._tcp.local.`.
fn service_type(name: &str) -> String {
    let name = name.trim_end_matches('.');
    let local = name
        .get(name.len().saturating_sub(6)..)
        .is_some_and(|end| end.eq_ignore_ascii_case(".local"));
    if local { format!("{name}.") } else { format!("{name}.local.") }
}

/// The addresses of `name` (`printer.local`, its dot or not), IPv4 first:
/// this machine's own if it is this machine's name, as its own questions
/// do not come back to it, or those the machine of that name answers
/// with within `wait`. An IPv6 link-local address is left out: without
/// its interface, which a DNS answer cannot carry, it reaches nothing.
fn resolve_here(daemon: &ServiceDaemon, me: &str, name: &str, wait: Duration) -> Vec<IpAddr> {
    let name = format!("{}.", name.trim_end_matches('.').to_ascii_lowercase());
    if !name.ends_with(".local.") || name.len() > 255 {
        return Vec::new();
    }
    let mut found: BTreeSet<(bool, IpAddr)> = BTreeSet::new();
    if name == me {
        for interface in if_addrs::get_if_addrs().unwrap_or_default() {
            if !interface.is_loopback() {
                found.insert((interface.ip().is_ipv6(), interface.ip()));
            }
        }
    } else if let Ok(events) = daemon.resolve_hostname(&name, Some(millis(wait))) {
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match events.recv_timeout(left) {
                Ok(HostnameResolutionEvent::AddressesFound(_, addresses)) => {
                    found.extend(addresses.iter().map(|a| (a.is_ipv6(), a.to_ip_addr())));
                    break;
                }
                Ok(HostnameResolutionEvent::SearchTimeout(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = daemon.stop_resolve_hostname(&name);
    }
    found.into_iter().map(|(_, a)| a).filter(|a| !link_local(a)).collect()
}

/// Whether `address` is an IPv6 link-local one.
fn link_local(address: &IpAddr) -> bool {
    matches!(address, IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80)
}

/// `wait` in milliseconds, as mdns-sd takes it.
fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

/// Browses for `wait`, writing to `out` each service of `types` found, or
/// of every type the network says it has if none are named, and each
/// device that answers WS-Discovery's probe.
fn browse(
    daemon: &ServiceDaemon,
    wait: Duration,
    types: &[String],
    out: &mut impl Write,
) -> io::Result<()> {
    let until = Instant::now() + wait;
    let probing = thread::Builder::new().name("wsd".into()).spawn(move || probe(until)).ok();
    let mut browsing: Vec<(String, mdns_sd::Receiver<ServiceEvent>)> = Vec::new();
    let mut asked: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let every = types.is_empty();
    let mut wanted: Vec<String> = types.to_vec();
    if every {
        wanted.push(TYPES.into());
        wanted.push(WORKSTATION.into());
    }
    let result = (|| -> io::Result<()> {
        loop {
            for ty in wanted.drain(..) {
                if asked.insert(ty.clone())
                    && let Ok(events) = daemon.browse(&ty)
                {
                    browsing.push((ty, events));
                }
            }
            let mut any = false;
            for (ty, events) in &browsing {
                while let Ok(event) = events.try_recv() {
                    any = true;
                    match event {
                        // The types question answers with types' names.
                        ServiceEvent::ServiceFound(_, name) if ty == TYPES => {
                            wanted.push(service_type(&name));
                        }
                        ServiceEvent::ServiceResolved(service)
                            if seen.insert(service.fullname.clone()) =>
                        {
                            write_service(out, &service)?;
                        }
                        _ => {}
                    }
                }
            }
            if Instant::now() >= until {
                return Ok(());
            }
            if !any {
                thread::sleep(Duration::from_millis(50));
            }
        }
    })();
    for (ty, _) in &browsing {
        let _ = daemon.stop_browse(ty);
    }
    result?;
    if let Some(Ok(devices)) = probing.map(thread::JoinHandle::join) {
        for device in devices {
            writeln!(
                out,
                "device\t{}\t{}\t{}\t{}",
                device.endpoint, device.types, device.from, device.xaddrs
            )?;
        }
    }
    Ok(())
}

/// Writes a line for `service`.
fn write_service(out: &mut impl Write, service: &mdns_sd::ResolvedService) -> io::Result<()> {
    // As `resolve` gives them: IPv4 first, without link-local IPv6 ones.
    let mut addresses: Vec<(bool, IpAddr)> = service
        .addresses
        .iter()
        .map(|a| (a.is_ipv6(), a.to_ip_addr()))
        .filter(|(_, a)| !link_local(a))
        .collect();
    addresses.sort();
    let addresses: Vec<String> = addresses.iter().map(|(_, a)| a.to_string()).collect();
    let txt: Vec<String> = service
        .txt_properties
        .iter()
        .map(|p| format!("{}={}", p.key(), p.val_str()))
        .map(|p| p.replace(['\t', '\n'], " "))
        .collect();
    let instance = service
        .fullname
        .strip_suffix(&format!(".{}", service.ty_domain))
        .unwrap_or(&service.fullname);
    writeln!(
        out,
        "service\t{}\t{}\t{}\t{}\t{}\t{}",
        instance.replace(['\t', '\n'], " "),
        service.ty_domain.trim_end_matches(".local."),
        service.host.trim_end_matches('.'),
        service.port,
        addresses.join(","),
        txt.join(";")
    )
}

/// A device that answered WS-Discovery's probe.
#[derive(Debug, Default, PartialEq, Eq)]
struct Device {
    /// Its endpoint's address, a URN that stays the same.
    endpoint: String,
    /// The types it says it is, as `wsdp:Device pub:Computer`.
    types: String,
    /// Where it describes itself, its XAddrs.
    xaddrs: String,
    /// The address its answer came from.
    from: String,
}

/// Sends WS-Discovery's Probe to its IPv4 group out of each card, twice
/// as the protocol asks for UDP, and gathers the devices that answer
/// until `until`.
fn probe(until: Instant) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return devices };
    let _ = socket.set_multicast_ttl_v4(1);
    let id = message_id();
    let message = probe_message(&id);
    // Out of each card, as Avahi sends: a group has no route of its own.
    let cards: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        })
        .collect();
    let sending = socket2::SockRef::from(&socket);
    for card in cards {
        if sending.set_multicast_if_v4(&card).is_err() {
            continue;
        }
        for _ in 0..2 {
            let _ = socket.send_to(message.as_bytes(), (WSD_GROUP, WSD_PORT));
        }
    }
    let mut buffer = vec![0; 65_535];
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        if left.is_zero() || socket.set_read_timeout(Some(left)).is_err() {
            break;
        }
        let Ok((len, from)) = socket.recv_from(&mut buffer) else { continue };
        let text = String::from_utf8_lossy(&buffer[..len]);
        for mut device in probe_matches(&text, &id) {
            device.from = address_of(from);
            if !devices.iter().any(|d| d.endpoint == device.endpoint) {
                devices.push(device);
            }
        }
    }
    devices
}

/// The address of `from`, without its port.
fn address_of(from: SocketAddr) -> String {
    from.ip().to_string()
}

/// A message identifier: a random UUID's URN.
fn message_id() -> String {
    let mut bytes = [0u8; 16];
    // SAFETY: getrandom writes at most the buffer's length into it.
    let got = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if usize::try_from(got).ok() != Some(bytes.len()) {
        // Unique enough to tell this probe's answers from another's.
        let seed = u128::from(util::now_ns()) ^ (u128::from(std::process::id()) << 64);
        bytes = seed.to_le_bytes();
    }
    // Version 4, variant 1 (RFC 9562).
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// WS-Discovery's Probe (version 2005/04, which Windows answers), for
/// any type, numbered `id`.
fn probe_message(id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\" \
         xmlns:wsa=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" \
         xmlns:wsd=\"http://schemas.xmlsoap.org/ws/2005/04/discovery\">\
         <soap:Header>\
         <wsa:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To>\
         <wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action>\
         <wsa:MessageID>{id}</wsa:MessageID>\
         </soap:Header>\
         <soap:Body><wsd:Probe/></soap:Body>\
         </soap:Envelope>"
    )
}

/// The devices of a ProbeMatches message, `text`, that answers the probe
/// numbered `id`: none for another message.
fn probe_matches(text: &str, id: &str) -> Vec<Device> {
    if !text.contains("ProbeMatches") || element(text, "RelatesTo").map(str::trim) != Some(id) {
        return Vec::new();
    }
    let mut devices = Vec::new();
    let mut rest = text;
    while let Some(at) = find_open(rest, "ProbeMatch") {
        rest = &rest[at..];
        let end = find_close(rest, "ProbeMatch").unwrap_or(rest.len());
        let one = &rest[..end];
        let field =
            |name| element(one, name).map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "));
        let device = Device {
            endpoint: field("Address").unwrap_or_default(),
            types: field("Types").unwrap_or_default(),
            xaddrs: field("XAddrs").unwrap_or_default(),
            from: String::new(),
        };
        if !device.endpoint.is_empty() {
            devices.push(device);
        }
        rest = &rest[end..];
    }
    devices
}

/// Where the first element of local name `name` (any prefix) opens in
/// `text`.
fn find_open(text: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(at) = text[from..].find('<').map(|a| a + from) {
        let tag = &text[at + 1..];
        let local = tag.split([' ', '>', '/']).next().unwrap_or_default();
        let local = local.rsplit(':').next().unwrap_or_default();
        if local == name && !tag.starts_with('/') {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Where the first element of local name `name` in `text` closes: past
/// its closing tag.
fn find_close(text: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(at) = text[from..].find("</").map(|a| a + from) {
        let tag = &text[at + 2..];
        let end = tag.find('>')?;
        if tag[..end].trim_end().rsplit(':').next() == Some(name) {
            return Some(at + 2 + end + 1);
        }
        from = at + 2;
    }
    None
}

/// The text of the first element of local name `name` in `text`.
fn element<'t>(text: &'t str, name: &str) -> Option<&'t str> {
    let at = find_open(text, name)?;
    let start = at + text[at..].find('>')? + 1;
    if text[..start].ends_with("/>") {
        return Some("");
    }
    let end = start + text[start..].find("</")?;
    Some(&text[start..end])
}

/// The addresses discoverd finds for `name`, a `.local` name, within
/// `wait`: none if it finds none; an error if it is not running.
pub fn resolve(name: &str, wait: Duration) -> io::Result<Vec<IpAddr>> {
    let stream = UnixStream::connect(SOCKET)?;
    // No name is so long (a DNS name has 255 bytes at most).
    if name.len() > 255 || name.contains(['\n', '\r']) {
        return Ok(Vec::new());
    }
    let request = format!("resolve {name}\n");
    (&stream).write_all(request.as_bytes())?;
    let mut addresses = Vec::new();
    lines(&stream, wait + Duration::from_millis(500), None, |line| {
        if let Ok(address) = line.trim().parse() {
            addresses.push(address);
        }
        true
    })?;
    Ok(addresses)
}

/// Asks discoverd to browse for `wait`, for `types` or all of them, and
/// calls `each` with each line it gives.
pub fn browse_lines(
    wait: Duration,
    types: &[String],
    mut each: impl FnMut(&str),
) -> io::Result<()> {
    let stream = UnixStream::connect(SOCKET)?;
    let request = format!("browse {} {}\n", wait.as_secs_f64(), types.join(" "));
    (&stream).write_all(request.as_bytes())?;
    lines(&stream, wait + Duration::from_secs(5), None, |line| {
        each(line);
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_come_until_the_end_a_stop_or_the_time() {
        let (mut a, b) = UnixStream::pair().unwrap();
        a.write_all(b"one\r\ntwo\nthree").unwrap();
        drop(a);
        let mut got = Vec::new();
        lines(&b, Duration::from_secs(5), None, |line| {
            got.push(line.to_string());
            true
        })
        .unwrap();
        assert_eq!(got, ["one", "two", "three"]);
        let (mut a, b) = UnixStream::pair().unwrap();
        a.write_all(b"first\nsecond\n").unwrap();
        let mut first = String::new();
        lines(&b, Duration::from_secs(5), Some(LINE_MAX), |line| {
            first = line.to_string();
            false
        })
        .unwrap();
        assert_eq!(first, "first");
        // A client that sends nothing, and keeps its end open.
        let (_a, b) = UnixStream::pair().unwrap();
        let quiet = lines(&b, Duration::from_millis(50), None, |_| true).unwrap_err();
        assert_eq!(quiet.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_hostname_is_a_label_of_its_own() {
        assert_eq!(label("Alice-Laptop.example.com"), "alice-laptop");
        assert_eq!(label("my_box"), "my-box");
        assert_eq!(label("..."), "dukovo");
        assert_eq!(label(&"x".repeat(80)).len(), 63);
    }

    #[test]
    fn types_are_given_their_domain() {
        assert_eq!(service_type("_http._tcp"), "_http._tcp.local.");
        assert_eq!(service_type("_ipp._tcp.local."), "_ipp._tcp.local.");
    }

    #[test]
    fn probe_matches_are_read_for_their_probe_alone() {
        let id = message_id();
        assert!(id.starts_with("urn:uuid:") && id.len() == 45, "{id}");
        assert!(probe_message(&id).contains(&id));
        let answer = format!(
            "<s:Envelope xmlns:s=\"x\"><s:Header><a:RelatesTo>{id}</a:RelatesTo></s:Header>\
             <s:Body><d:ProbeMatches><d:ProbeMatch><a:EndpointReference>\
             <a:Address>urn:uuid:1234</a:Address></a:EndpointReference>\
             <d:Types>wsdp:Device\n pub:Computer</d:Types>\
             <d:XAddrs>http://10.0.0.5:5357/x</d:XAddrs></d:ProbeMatch>\
             <d:ProbeMatch><a:EndpointReference><a:Address>urn:uuid:5678</a:Address>\
             </a:EndpointReference><d:Types/></d:ProbeMatch>\
             </d:ProbeMatches></s:Body></s:Envelope>"
        );
        let devices = probe_matches(&answer, &id);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].endpoint, "urn:uuid:1234");
        assert_eq!(devices[0].types, "wsdp:Device pub:Computer");
        assert_eq!(devices[0].xaddrs, "http://10.0.0.5:5357/x");
        assert_eq!(
            (devices[1].endpoint.as_str(), devices[1].types.as_str()),
            ("urn:uuid:5678", "")
        );
        assert!(probe_matches(&answer, "urn:uuid:other").is_empty());
    }

    #[test]
    fn link_local_addresses_are_told_apart() {
        assert!(link_local(&"fe80::1".parse().unwrap()));
        assert!(!link_local(&"2001:db8::1".parse().unwrap()));
        assert!(!link_local(&"10.0.0.1".parse().unwrap()));
    }
}
