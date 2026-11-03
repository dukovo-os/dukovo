// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dig(1) and host(1): DNS's questions, asked of a name server and its
//! answers shown, as BIND's dig and host show them (ADR 0341).
//!
//! Both ask the server `@SERVER` (dig) or the second operand (host)
//! names, else the first `nameserver` of /etc/resolv.conf, which is
//! resolved(8)'s 127.0.0.53: over UDP, and over TCP when the answer was
//! cut short or `+tcp` (`-T`) says, waiting `+time` seconds `+tries`
//! times. The messages are netproto's, and the records are shown as zone
//! files write them (netproto::present). dig shows the whole answer, its
//! header, EDNS pseudo-section and sections, unless its `+` options say
//! less; host says what each record means, in words.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use netproto::dns::{self, Edns, Record};
use netproto::present;

use crate::args::Args;
use crate::util::{self, Fail, fail_with, usage};

/// The port name servers listen on.
const PORT: u16 = 53;

/// The UDP size offered, as BIND's dig offers it since the 2020 DNS flag
/// day.
const BUFSIZE: u16 = 1232;

/// The server asked when resolv.conf names none: resolved's.
const RESOLVED: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 53);

/// Where the C library and these tools find the name servers.
const RESOLV_CONF: &str = "/etc/resolv.conf";

/// dig's exit statuses: a usage mistake, no reply, and another failure.
const USAGE: i32 = 1;
const NO_REPLY: i32 = 9;
const INTERNAL: i32 = 10;

/// How a question is asked.
#[derive(Clone, Debug)]
pub struct Asking {
    /// The server, by name or address, and the port.
    pub server: Option<String>,
    pub port: u16,
    /// Only IPv4 or only IPv6, for the server's address.
    pub version: Option<bool>,
    /// Over TCP from the start.
    pub tcp: bool,
    /// How long to wait for each reply, and how many times to ask.
    pub time: Duration,
    pub tries: u32,
    /// The header's flags: RD, AD, CD.
    pub flags: u16,
    /// The OPT record's settings, or none.
    pub edns: Option<Edns>,
}

impl Default for Asking {
    fn default() -> Asking {
        Asking {
            server: None,
            port: PORT,
            version: None,
            tcp: false,
            time: Duration::from_secs(5),
            tries: 3,
            flags: dns::RD | dns::AD,
            edns: Some(Edns { udp: BUFSIZE, dnssec: false }),
        }
    }
}

/// An answer, and how it came.
pub struct Reply {
    pub message: Vec<u8>,
    pub server: SocketAddr,
    pub tcp: bool,
    pub took: Duration,
}

/// Why a question has no answer.
pub enum Failure {
    /// The name cannot be asked for: a label is too long, or the name.
    Name(String),
    /// The server's name has no address.
    Server(String),
    /// No answer came from any of the server's addresses.
    NoReply,
}

/// The servers resolv.conf names, in order.
fn resolv_conf_servers() -> Vec<IpAddr> {
    let text = fs::read_to_string(RESOLV_CONF).unwrap_or_default();
    let servers: Vec<IpAddr> = text
        .lines()
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
        .collect();
    if servers.is_empty() { vec![RESOLVED.into()] } else { servers }
}

/// The server's addresses, by `asking`'s name or resolv.conf's.
fn server_addresses(asking: &Asking) -> Result<Vec<SocketAddr>, Failure> {
    let addresses: Vec<SocketAddr> = match &asking.server {
        Some(name) => {
            let bare = name.trim_start_matches('[').trim_end_matches(']');
            match bare.parse::<IpAddr>() {
                Ok(ip) => vec![SocketAddr::new(ip, asking.port)],
                Err(_) => (name.as_str(), asking.port)
                    .to_socket_addrs()
                    .map_err(|_| Failure::Server(name.clone()))?
                    .collect(),
            }
        }
        None => {
            resolv_conf_servers().into_iter().map(|ip| SocketAddr::new(ip, asking.port)).collect()
        }
    };
    let wanted: Vec<SocketAddr> = addresses
        .into_iter()
        .filter(|a| asking.version.is_none_or(|v6| a.is_ipv6() == v6))
        .collect();
    if wanted.is_empty() {
        return Err(Failure::Server(asking.server.clone().unwrap_or_else(|| RESOLV_CONF.into())));
    }
    Ok(wanted)
}

/// A query's number, at random.
fn query_id() -> u16 {
    let mut id = [0u8; 2];
    // SAFETY: getrandom writes at most the buffer's two bytes.
    let got = unsafe { libc::getrandom(id.as_mut_ptr().cast(), id.len(), 0) };
    if got != 2 {
        return (util::now_ns() & 0xffff) as u16;
    }
    u16::from_ne_bytes(id)
}

/// Asks `server` over UDP, waiting `time` for an answer with its number.
fn over_udp(server: SocketAddr, message: &[u8], time: Duration) -> io::Result<Vec<u8>> {
    let local: SocketAddr = if server.is_ipv6() {
        "[::]:0".parse().expect("an address")
    } else {
        "0.0.0.0:0".parse().expect("an address")
    };
    let socket = UdpSocket::bind(local)?;
    socket.connect(server)?;
    socket.send(message)?;
    let id = dns::header(message).map_or(0, |h| h.id);
    let deadline = Instant::now() + time;
    let mut buf = vec![0; 65_535];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        socket.set_read_timeout(Some(left))?;
        let len = match socket.recv(&mut buf) {
            Ok(len) => len,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
            }
            Err(e) => return Err(e),
        };
        if dns::header(&buf[..len]).is_some_and(|h| h.id == id && h.flags & dns::QR != 0) {
            return Ok(buf[..len].to_vec());
        }
    }
}

/// Asks `server` over TCP, the message after its length (RFC 7766).
fn over_tcp(server: SocketAddr, message: &[u8], time: Duration) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&server, time)?;
    stream.set_read_timeout(Some(time))?;
    stream.set_write_timeout(Some(time))?;
    let mut framed = (message.len() as u16).to_be_bytes().to_vec();
    framed.extend_from_slice(message);
    stream.write_all(&framed)?;
    let mut len = [0; 2];
    stream.read_exact(&mut len)?;
    let mut answer = vec![0; usize::from(u16::from_be_bytes(len))];
    stream.read_exact(&mut answer)?;
    Ok(answer)
}

/// Asks the question `message` as `asking` says, of each of the server's
/// addresses in turn, `tries` times in all: over UDP, then over TCP if
/// the answer was cut short (saying so on standard output when `say`).
pub fn exchange(message: &[u8], asking: &Asking, say: bool) -> Result<Reply, Failure> {
    let servers = server_addresses(asking)?;
    for attempt in 0..asking.tries.max(1) as usize {
        let server = servers[attempt % servers.len()];
        let start = Instant::now();
        let mut tcp = asking.tcp;
        let result = if tcp {
            over_tcp(server, message, asking.time)
        } else {
            over_udp(server, message, asking.time).and_then(|answer| {
                if dns::header(&answer).is_some_and(|h| h.flags & dns::TC != 0) {
                    if say {
                        println!(";; Truncated, retrying in TCP mode.");
                    }
                    tcp = true;
                    over_tcp(server, message, asking.time)
                } else {
                    Ok(answer)
                }
            })
        };
        match result {
            Ok(message) => return Ok(Reply { message, server, tcp, took: start.elapsed() }),
            Err(e) => {
                if say {
                    println!(
                        ";; communications error to {}: {}",
                        server_text(server),
                        error_text(&e)
                    );
                }
            }
        }
    }
    Err(Failure::NoReply)
}

/// A server's address as dig writes it: `ADDRESS#PORT`.
pub fn server_text(server: SocketAddr) -> String {
    format!("{}#{}", server.ip(), server.port())
}

fn error_text(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "timed out".into(),
        io::ErrorKind::ConnectionRefused => "connection refused".into(),
        _ => util::strip_os_error(error).to_lowercase(),
    }
}

/// The name a reverse lookup asks for `address`: in in-addr.arpa or
/// ip6.arpa, nibble by nibble.
pub fn reverse_name(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa.")
        }
        IpAddr::V6(v6) => {
            let mut name = String::new();
            for byte in v6.octets().iter().rev() {
                let _ = write!(name, "{:x}.{:x}.", byte & 0xf, byte >> 4);
            }
            name + "ip6.arpa."
        }
    }
}

/// A name as asked for, ending in a dot.
pub fn absolute(name: &str) -> String {
    if name.ends_with('.') { name.to_string() } else { format!("{name}.") }
}

/// Builds the query for `name`, or says why it cannot be one.
pub fn build(name: &str, qtype: u16, qclass: u16, asking: &Asking) -> Result<Vec<u8>, Failure> {
    let labels = name.strip_suffix('.').unwrap_or(name);
    if labels.split('.').any(|l| l.len() > 63) {
        return Err(Failure::Name(format!("'{name}' is not a legal name (label too long)")));
    }
    if labels.split('.').any(str::is_empty) && !labels.is_empty() {
        return Err(Failure::Name(format!("'{name}' is not a legal name (empty label)")));
    }
    dns::query_with(name, qtype, qclass, query_id(), asking.flags, asking.edns)
        .ok_or_else(|| Failure::Name(format!("'{name}' is not a legal name (ran out of space)")))
}

/// What dig's `+` options say to show.
#[derive(Clone, Debug)]
struct Show {
    cmd: bool,
    comments: bool,
    question: bool,
    answer: bool,
    authority: bool,
    additional: bool,
    stats: bool,
    short: bool,
    ttl: bool,
    class: bool,
    identify: bool,
}

impl Default for Show {
    fn default() -> Show {
        Show {
            cmd: true,
            comments: true,
            question: true,
            answer: true,
            authority: true,
            additional: true,
            stats: true,
            short: false,
            ttl: true,
            class: true,
            identify: false,
        }
    }
}

/// One question on dig's command line.
#[derive(Clone, Debug)]
struct Query {
    name: String,
    qtype: Option<u16>,
    qclass: Option<u16>,
}

/// Appends tabs to `line` until it reaches column `column`, at least one.
fn tab_to(line: &mut String, column: usize) {
    loop {
        line.push('\t');
        let width = line.rsplit('\n').next().map_or(0, |l| {
            l.chars().fold(0, |w, c| if c == '\t' { (w / 8 + 1) * 8 } else { w + 1 })
        });
        if width >= column {
            break;
        }
    }
}

/// A record as dig's sections show it: owner, TTL, class, type and data,
/// at tab stops.
fn record_line(message: &[u8], record: &Record, show: &Show) -> String {
    let owner = present::name_at(message, record.owner).map_or_else(|| "?".into(), |(n, _)| n);
    let mut line = owner;
    if show.ttl {
        tab_to(&mut line, 24);
        let _ = write!(line, "{}", record.ttl);
    }
    if show.class {
        tab_to(&mut line, 32);
        line.push_str(&present::class_name(record.class));
    }
    tab_to(&mut line, 40);
    line.push_str(&present::type_name(record.rtype));
    tab_to(&mut line, 48);
    line.push_str(&present::record_data(message, record));
    line
}

/// The OPT pseudo-section: EDNS's version, flags and size, and each
/// option.
fn opt_section(message: &[u8], opt: &Record) -> String {
    let version = (opt.ttl >> 16) & 0xff;
    let flags = if opt.ttl & u32::from(dns::DO) != 0 { " do" } else { "" };
    let mut out = format!(
        ";; OPT PSEUDOSECTION:\n; EDNS: version: {version}, flags:{flags}; udp: {}\n",
        opt.class
    );
    let (mut at, end) = opt.data;
    while at + 4 <= end {
        let code = u16::from_be_bytes([message[at], message[at + 1]]);
        let len = usize::from(u16::from_be_bytes([message[at + 2], message[at + 3]]));
        let Some(data) = message.get(at + 4..at + 4 + len).filter(|_| at + 4 + len <= end) else {
            break;
        };
        let hex = present::hex(data);
        match code {
            3 => {
                let text: String = data
                    .iter()
                    .map(|&b| if b.is_ascii_graphic() { char::from(b) } else { '.' })
                    .collect();
                let _ = writeln!(out, "; NSID: {hex} (\"{text}\")");
            }
            10 => {
                let _ = writeln!(out, "; COOKIE: {}", hex.to_lowercase());
            }
            15 if data.len() >= 2 => {
                let info = u16::from_be_bytes([data[0], data[1]]);
                let extra = String::from_utf8_lossy(&data[2..]);
                if extra.is_empty() {
                    let _ = writeln!(out, "; EDE: {info}");
                } else {
                    let _ = writeln!(out, "; EDE: {info}: ({extra})");
                }
            }
            _ => {
                let _ = writeln!(out, "; OPT={code}: {hex}");
            }
        }
        at += 4 + len;
    }
    out
}

/// The header's flags as dig names them, in its order.
fn flag_names(flags: u16) -> String {
    [
        (dns::QR, "qr"),
        (1 << 10, "aa"),
        (dns::TC, "tc"),
        (dns::RD, "rd"),
        (dns::RA, "ra"),
        (dns::AD, "ad"),
        (dns::CD, "cd"),
    ]
    .iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, name)| *name)
    .collect::<Vec<_>>()
    .join(" ")
}

/// Shows `reply` as dig does.
fn show_reply(out: &mut String, reply: &Reply, show: &Show) -> Result<(), String> {
    let message = &reply.message;
    let header = dns::header(message).ok_or("the answer is too short to read")?;
    let records = dns::records(message).ok_or("the answer does not hold what its header says")?;
    if show.short {
        for record in records.iter().filter(|r| r.section == 0) {
            let data = present::record_data(message, record);
            if show.identify {
                let _ = writeln!(
                    out,
                    "{data} from server {} in {} ms.",
                    reply.server.ip(),
                    reply.took.as_millis()
                );
            } else {
                let _ = writeln!(out, "{data}");
            }
        }
        return Ok(());
    }
    let opcode = (header.flags >> 11) & 0xf;
    let rcode = u16::try_from(
        u32::from(header.rcode())
            | records.iter().find(|r| r.rtype == dns::OPT).map_or(0, |o| (o.ttl >> 24) << 4),
    )
    .unwrap_or(0);
    if show.comments {
        let _ = writeln!(out, ";; Got answer:");
        let _ = writeln!(
            out,
            ";; ->>HEADER<<- opcode: {}, status: {}, id: {}",
            present::opcode_name(opcode),
            present::rcode_name(rcode),
            header.id
        );
        let _ = writeln!(
            out,
            ";; flags: {}; QUERY: {}, ANSWER: {}, AUTHORITY: {}, ADDITIONAL: {}",
            flag_names(header.flags),
            header.questions,
            header.answers,
            header.authorities,
            header.additionals
        );
        if header.flags & dns::RD != 0 && header.flags & dns::RA == 0 {
            let _ = writeln!(out, ";; WARNING: recursion requested but not available");
        }
        let _ = writeln!(out);
        if let Some(opt) = records.iter().find(|r| r.rtype == dns::OPT) {
            out.push_str(&opt_section(message, opt));
        }
    }
    if show.question {
        if show.comments {
            let _ = writeln!(out, ";; QUESTION SECTION:");
        }
        if let Some(question) = dns::question(message) {
            let mut line = format!(";{}", absolute(&question.name));
            tab_to(&mut line, 32);
            line.push_str(&present::class_name(question.qclass));
            tab_to(&mut line, 40);
            line.push_str(&present::type_name(question.qtype));
            let _ = writeln!(out, "{line}");
        }
        let _ = writeln!(out);
    }
    for (section, wanted, title) in [
        (0, show.answer, "ANSWER"),
        (1, show.authority, "AUTHORITY"),
        (2, show.additional, "ADDITIONAL"),
    ] {
        let in_section: Vec<&Record> =
            records.iter().filter(|r| r.section == section && r.rtype != dns::OPT).collect();
        if !wanted || in_section.is_empty() {
            continue;
        }
        if show.comments {
            let _ = writeln!(out, ";; {title} SECTION:");
        }
        for record in in_section {
            let _ = writeln!(out, "{}", record_line(message, record, show));
        }
        let _ = writeln!(out);
    }
    if show.stats {
        let now = util::now_ns() / 1_000_000_000;
        let _ = writeln!(out, ";; Query time: {} msec", reply.took.as_millis());
        let _ = writeln!(
            out,
            ";; SERVER: {}({}) ({})",
            server_text(reply.server),
            reply.server.ip(),
            if reply.tcp { "TCP" } else { "UDP" }
        );
        let _ =
            writeln!(out, ";; WHEN: {}", util::local_time(now as i64, "%a %b %d %H:%M:%S %Z %Y"));
        let _ = writeln!(out, ";; MSG SIZE  rcvd: {}", message.len());
        let _ = writeln!(out);
    }
    Ok(())
}

/// A `+option` of dig's: its name without `+` and `no`, whether it was
/// negated, and its `=value`.
fn plus_option(text: &str) -> (String, bool, Option<String>) {
    let (name, value) = match text.split_once('=') {
        Some((n, v)) => (n, Some(v.to_string())),
        None => (text, None),
    };
    match name.strip_prefix("no") {
        Some(rest) if !rest.is_empty() && !matches!(name, "nsid" | "nssearch") => {
            (rest.to_string(), true, value)
        }
        _ => (name.to_string(), false, value),
    }
}

/// Whether `full` is `name` or an abbreviation of it at least `least`
/// letters long, as dig takes `+short` as `+sho`.
fn abbreviates(full: &str, name: &str, least: usize) -> bool {
    name.len() >= least && full.starts_with(name)
}

/// Runs dig.
pub fn dig(args: &mut Args) -> Result<i32, Fail> {
    let tokens: Vec<String> =
        args.rest().into_iter().map(|t: OsString| t.to_string_lossy().into_owned()).collect();
    let mut asking = Asking::default();
    let mut show = Show::default();
    let mut queries: Vec<Query> = Vec::new();
    let mut iter = tokens.iter().peekable();
    let value = |iter: &mut std::iter::Peekable<std::slice::Iter<String>>, option: &str| {
        iter.next()
            .cloned()
            .ok_or_else(|| fail_with(USAGE, format!("option requires an argument -- '{option}'")))
    };
    while let Some(token) = iter.next() {
        if let Some(server) = token.strip_prefix('@') {
            asking.server = Some(server.to_string());
        } else if let Some(option) = token.strip_prefix('+') {
            let (name, no, val) = plus_option(option);
            let on = !no;
            let number = |what: &str| -> Result<u64, Fail> {
                val.as_deref()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| fail_with(USAGE, format!("Invalid option: +{what}")))
            };
            match name.as_str() {
                n if abbreviates("short", n, 3) => show.short = on,
                n if abbreviates("tcp", n, 2) || abbreviates("vc", n, 2) => asking.tcp = on,
                n if abbreviates("recurse", n, 3) => {
                    asking.flags = if on { asking.flags | dns::RD } else { asking.flags & !dns::RD }
                }
                n if abbreviates("adflag", n, 2) => {
                    asking.flags = if on { asking.flags | dns::AD } else { asking.flags & !dns::AD }
                }
                n if abbreviates("cdflag", n, 2) => {
                    asking.flags = if on { asking.flags | dns::CD } else { asking.flags & !dns::CD }
                }
                n if abbreviates("dnssec", n, 2) => {
                    if on {
                        let udp = asking.edns.map_or(BUFSIZE, |e| e.udp);
                        asking.edns = Some(Edns { udp, dnssec: true });
                    } else if let Some(edns) = &mut asking.edns {
                        edns.dnssec = false;
                    }
                }
                n if abbreviates("edns", n, 2) => {
                    asking.edns = if on { Some(Edns { udp: BUFSIZE, dnssec: false }) } else { None }
                }
                n if abbreviates("bufsize", n, 1) => {
                    let udp = u16::try_from(number("bufsize")?)
                        .map_err(|_| fail_with(USAGE, "Invalid option: +bufsize".into()))?;
                    let dnssec = asking.edns.is_some_and(|e| e.dnssec);
                    asking.edns = Some(Edns { udp, dnssec });
                }
                n if abbreviates("time", n, 2) => {
                    asking.time = Duration::from_secs(number("time")?.max(1));
                }
                n if abbreviates("tries", n, 2) => asking.tries = number("tries")?.max(1) as u32,
                n if abbreviates("retry", n, 3) => asking.tries = number("retry")? as u32 + 1,
                n if abbreviates("cmd", n, 2) => show.cmd = on,
                n if abbreviates("comments", n, 3) => show.comments = on,
                n if abbreviates("question", n, 2) => show.question = on,
                n if abbreviates("answer", n, 2) => show.answer = on,
                n if abbreviates("authority", n, 2) => show.authority = on,
                n if abbreviates("additional", n, 3) => show.additional = on,
                n if abbreviates("stats", n, 2) => show.stats = on,
                n if abbreviates("ttlid", n, 4) => show.ttl = on,
                n if abbreviates("class", n, 2) => show.class = on,
                n if abbreviates("identify", n, 1) => show.identify = on,
                "all" => {
                    show.comments = on;
                    show.question = on;
                    show.answer = on;
                    show.authority = on;
                    show.additional = on;
                    show.stats = on;
                    show.cmd = on;
                }
                n if abbreviates("multiline", n, 2) || abbreviates("search", n, 3) => {}
                n if abbreviates("trace", n, 2) || abbreviates("nssearch", n, 3) => {
                    return Err(fail_with(USAGE, format!("+{n} is not supported yet")));
                }
                _ => return Err(fail_with(USAGE, format!("Invalid option: +{option}"))),
            }
        } else if token.len() >= 2 && token.starts_with('-') {
            let flag = &token[1..2];
            let joined = (token.len() > 2).then(|| token[2..].to_string());
            let take = |iter: &mut std::iter::Peekable<std::slice::Iter<String>>| {
                joined.clone().map_or_else(|| value(iter, flag), Ok)
            };
            match flag {
                "4" => asking.version = Some(false),
                "6" => asking.version = Some(true),
                "h" => {
                    print!("{}", DIG_HELP);
                    return Ok(0);
                }
                "v" => {
                    eprintln!("dig {} (Dukovo)", env!("CARGO_PKG_VERSION"));
                    return Ok(0);
                }
                "p" => {
                    let port = take(&mut iter)?;
                    asking.port = port
                        .parse()
                        .map_err(|_| fail_with(USAGE, format!("invalid port number '{port}'")))?;
                }
                "t" => {
                    let name = take(&mut iter)?;
                    let qtype = present::type_by_name(&name)
                        .ok_or_else(|| fail_with(USAGE, format!("invalid type: {name}")))?;
                    match queries.last_mut() {
                        Some(q) if q.qtype.is_none() => q.qtype = Some(qtype),
                        _ => queries.push(Query {
                            name: String::new(),
                            qtype: Some(qtype),
                            qclass: None,
                        }),
                    }
                }
                "c" => {
                    let name = take(&mut iter)?;
                    let qclass = present::class_by_name(&name)
                        .ok_or_else(|| fail_with(USAGE, format!("invalid class: {name}")))?;
                    match queries.last_mut() {
                        Some(q) => q.qclass = Some(qclass),
                        None => queries.push(Query {
                            name: String::new(),
                            qtype: None,
                            qclass: Some(qclass),
                        }),
                    }
                }
                "q" => {
                    let name = take(&mut iter)?;
                    push_name(&mut queries, name);
                }
                "x" => {
                    let text = take(&mut iter)?;
                    let address: IpAddr = text
                        .parse()
                        .map_err(|_| fail_with(USAGE, format!("Invalid IP address {text}")))?;
                    push_name(&mut queries, reverse_name(address));
                    if let Some(q) = queries.last_mut() {
                        q.qtype.get_or_insert(dns::PTR);
                    }
                }
                _ => return Err(fail_with(USAGE, format!("Invalid option: {token}"))),
            }
        } else if let Some(qtype) = present::type_by_name(token)
            .filter(|_| queries.last().is_some_and(|q| !q.name.is_empty() && q.qtype.is_none()))
        {
            queries.last_mut().expect("a query").qtype = Some(qtype);
        } else if let Some(qclass) = present::class_by_name(token)
            .filter(|_| queries.last().is_some_and(|q| !q.name.is_empty() && q.qclass.is_none()))
        {
            queries.last_mut().expect("a query").qclass = Some(qclass);
        } else {
            push_name(&mut queries, token.clone());
        }
    }
    if queries.is_empty() {
        // No name: the root's name servers, as dig asks.
        queries.push(Query { name: ".".into(), qtype: Some(dns::NS), qclass: None });
    }
    let mut status = 0;
    let mut stdout = io::stdout().lock();
    for query in &queries {
        let name = if query.name.is_empty() { ".".to_string() } else { absolute(&query.name) };
        let qtype = query.qtype.unwrap_or(dns::A);
        let qclass = query.qclass.unwrap_or(dns::IN);
        let mut out = String::new();
        if show.cmd && !show.short {
            let _ = writeln!(
                out,
                "\n; <<>> dig {} (Dukovo) <<>> {}",
                env!("CARGO_PKG_VERSION"),
                tokens.join(" ")
            );
            let _ = writeln!(out, ";; global options: +cmd");
        }
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
        out.clear();
        let message = match build(&name, qtype, qclass, &asking) {
            Ok(message) => message,
            Err(Failure::Name(text)) => {
                eprintln!("dig: {text}");
                return Ok(INTERNAL);
            }
            Err(_) => unreachable!("building asks no server"),
        };
        match exchange(&message, &asking, !show.short) {
            Ok(reply) => {
                if let Err(text) = show_reply(&mut out, &reply, &show) {
                    let _ = writeln!(out, ";; Warning: {text}");
                    status = status.max(INTERNAL);
                }
            }
            Err(Failure::Server(server)) => {
                eprintln!("dig: couldn't get address for '{server}': not found");
                return Ok(INTERNAL);
            }
            Err(Failure::NoReply) => {
                if show.short {
                    let _ = writeln!(out, ";; connection timed out; no servers could be reached");
                } else {
                    let _ = writeln!(out, ";; no servers could be reached\n");
                }
                status = NO_REPLY;
            }
            Err(Failure::Name(text)) => {
                eprintln!("dig: {text}");
                return Ok(INTERNAL);
            }
        }
        let _ = stdout.write_all(out.as_bytes());
    }
    Ok(status)
}

/// A name operand: a new query, or the name of one a `-t` or `-c` began.
fn push_name(queries: &mut Vec<Query>, name: String) {
    match queries.last_mut() {
        Some(q) if q.name.is_empty() => q.name = name,
        _ => queries.push(Query { name, qtype: None, qclass: None }),
    }
}

const DIG_HELP: &str = "\
Usage:  dig [@server] [-p port] [-4|-6] [-t type] [-c class] [-x addr]
            [-q name] [name] [type] [class] [+option...]
Asks a name server about a name, and shows its answer.
  @server      the server to ask: a name or an address; else resolv.conf's
  -p port      its port, 53 unless given
  -4, -6       only its IPv4 or IPv6 address
  -t type      what to ask for: A unless given (ANY, MX, TXT, TYPE65...)
  -c class     the class: IN unless given (CH, HS)
  -x addr      an address, looked up backwards for its name
  +[no]tcp     over TCP, not UDP           +[no]short   the answers' data alone
  +[no]recurse ask the server to recurse   +[no]dnssec  ask for signatures
  +[no]cdflag  checking disabled           +[no]adflag  authentic data asked
  +[no]edns    with EDNS's OPT record      +bufsize=N   the UDP size offered
  +time=T      seconds to wait for each    +tries=N     how many times to ask
  +[no]cmd +[no]comments +[no]question +[no]answer +[no]authority
  +[no]additional +[no]stats +[no]all +[no]ttlid +[no]class +[no]identify
";

/// What host says a record of type `rtype` is, before its data.
fn host_words(rtype: u16) -> String {
    match rtype {
        dns::A => "has address".into(),
        dns::AAAA => "has IPv6 address".into(),
        dns::MX => "mail is handled by".into(),
        dns::CNAME => "is an alias for".into(),
        dns::NS => "name server".into(),
        dns::PTR => "domain name pointer".into(),
        dns::TXT => "descriptive text".into(),
        dns::SOA => "has SOA record".into(),
        other => format!("has {} record", present::type_name(other)),
    }
}

/// Runs host.
pub fn host(args: &mut Args) -> Result<i32, Fail> {
    use crate::args::Arg;
    let mut asking = Asking {
        time: Duration::from_secs(2),
        tries: 2,
        edns: None,
        flags: dns::RD,
        ..Asking::default()
    };
    let mut qtype = None;
    let mut qclass = dns::IN;
    let mut verbose = false;
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('4') => asking.version = Some(false),
            Arg::Short('6') => asking.version = Some(true),
            Arg::Short('a') => {
                verbose = true;
                qtype = Some(dns::ANY);
            }
            Arg::Short('v' | 'd') => verbose = true,
            Arg::Short('T') => asking.tcp = true,
            Arg::Short('r') => asking.flags &= !dns::RD,
            Arg::Short('t') => {
                let name = args.text()?;
                qtype = Some(
                    present::type_by_name(&name)
                        .ok_or_else(|| fail_with(USAGE, format!("invalid type: {name}")))?,
                );
            }
            Arg::Short('c') => {
                let name = args.text()?;
                qclass = present::class_by_name(&name)
                    .ok_or_else(|| fail_with(USAGE, format!("invalid class: {name}")))?;
            }
            Arg::Short('W') => asking.time = Duration::from_secs(args.number::<u64>()?.max(1)),
            Arg::Short('w') => asking.time = Duration::from_secs(u64::from(u32::MAX)),
            Arg::Short('R') => asking.tries = args.number::<u32>()?.max(1),
            Arg::Short('p') => asking.port = args.number()?,
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let (name, server) =
        match operands.as_slice() {
            [name] => (name.clone(), None),
            [name, server] => (name.clone(), Some(server.clone())),
            _ => return Err(usage(
                "usage: host [-aCdrTv46] [-c class] [-t type] [-W time] [-R number] name [server]"
                    .into(),
            )),
        };
    asking.server.clone_from(&server);
    let mut stdout = io::stdout().lock();
    if let Some(server) = &server {
        let addresses = server_addresses(&asking)
            .map_err(|_| fail_with(1, format!("couldn't get address for '{server}': not found")))?;
        let _ = writeln!(
            stdout,
            "Using domain server:\nName: {server}\nAddress: {}\nAliases: \n",
            server_text(addresses[0])
        );
    }
    let (asked, types) = match name.parse::<IpAddr>() {
        Ok(address) => (reverse_name(address), vec![qtype.unwrap_or(dns::PTR)]),
        Err(_) => {
            (absolute(&name), qtype.map_or_else(|| vec![dns::A, dns::AAAA, dns::MX], |t| vec![t]))
        }
    };
    let mut found = false;
    for (i, &rtype) in types.iter().enumerate() {
        let message = match build(&asked, rtype, qclass, &asking) {
            Ok(message) => message,
            Err(Failure::Name(text)) => return Err(fail_with(1, text)),
            Err(_) => unreachable!("building asks no server"),
        };
        if verbose {
            let _ = writeln!(stdout, "Trying \"{}\"", asked.trim_end_matches('.'));
        }
        let reply = match exchange(&message, &asking, false) {
            Ok(reply) => reply,
            Err(Failure::NoReply) => {
                let _ = writeln!(stdout, ";; connection timed out; no servers could be reached");
                return Ok(1);
            }
            Err(Failure::Server(server)) => {
                return Err(fail_with(
                    1,
                    format!("couldn't get address for '{server}': not found"),
                ));
            }
            Err(Failure::Name(text)) => return Err(fail_with(1, text)),
        };
        let header = dns::header(&reply.message)
            .ok_or_else(|| fail_with(1, "the answer is too short to read".into()))?;
        let records = dns::records(&reply.message).unwrap_or_default();
        if header.rcode() != dns::NOERROR {
            if i == 0 {
                let _ = writeln!(
                    stdout,
                    "Host {} not found: {}({})",
                    asked.trim_end_matches('.'),
                    header.rcode(),
                    present::rcode_name(header.rcode())
                );
                return Ok(1);
            }
            continue;
        }
        if verbose {
            let mut out = String::new();
            let show = Show { cmd: false, stats: false, ..Show::default() };
            let _ = show_reply(&mut out, &reply, &show);
            let _ = write!(stdout, "{out}");
            let _ = writeln!(
                stdout,
                "Received {} bytes from {} in {} ms",
                reply.message.len(),
                server_text(reply.server),
                reply.took.as_millis()
            );
            found |= header.answers > 0;
            continue;
        }
        for record in records.iter().filter(|r| r.section == 0) {
            // A CNAME is said once, with the first type asked for.
            if record.rtype == dns::CNAME && i > 0 {
                continue;
            }
            if record.rtype != rtype && record.rtype != dns::CNAME && rtype != dns::ANY {
                continue;
            }
            let owner = present::name_at(&reply.message, record.owner)
                .map_or_else(|| "?".into(), |(n, _)| n);
            let _ = writeln!(
                stdout,
                "{} {} {}",
                owner.trim_end_matches('.'),
                host_words(record.rtype),
                present::record_data(&reply.message, record)
            );
            found = true;
        }
    }
    if !found && qtype.is_some() && !verbose {
        let _ = writeln!(
            stdout,
            "{} has no {} record",
            asked.trim_end_matches('.'),
            present::type_name(types[0])
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_looked_up_backwards() {
        assert_eq!(reverse_name("192.0.2.5".parse().unwrap()), "5.2.0.192.in-addr.arpa.");
        let v6 = reverse_name("2001:db8::1".parse().unwrap());
        assert!(v6.starts_with("1.0.0.0.0.0.0.0") && v6.ends_with(".8.b.d.0.1.0.0.2.ip6.arpa."));
    }

    #[test]
    fn plus_options_read_as_dig_reads_them() {
        assert_eq!(plus_option("noall"), ("all".into(), true, None));
        assert_eq!(plus_option("time=3"), ("time".into(), false, Some("3".into())));
        assert_eq!(plus_option("nsid"), ("nsid".into(), false, None));
        assert!(abbreviates("short", "sho", 3) && !abbreviates("short", "sh", 3));
    }

    #[test]
    fn records_line_up_at_dig_tab_stops() {
        let mut m = vec![0, 1, 0x81, 0x80, 0, 0, 0, 1, 0, 0, 0, 0];
        m.extend_from_slice(b"\x07example\x03com\x00\x00\x01\x00\x01\x00\x00\x0e\x10\x00\x04");
        m.extend_from_slice(&[192, 0, 2, 1]);
        let records = dns::records(&m).unwrap();
        let line = record_line(&m, &records[0], &Show::default());
        assert_eq!(line, "example.com.\t\t3600\tIN\tA\t192.0.2.1");
        let mut q = String::from(";example.com.");
        tab_to(&mut q, 32);
        assert_eq!(q, ";example.com.\t\t\t");
        assert_eq!(flag_names(dns::QR | dns::RD | dns::RA), "qr rd ra");
    }
}
