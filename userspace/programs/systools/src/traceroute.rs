// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! traceroute(8): the way to a machine, a router at a time, as Dmitry
//! Butskoy's traceroute 2.1, the one Linux distributions ship, finds and
//! prints it (ADR 0345). Each probe goes one hop further than the last
//! (a time to live of one more), and the router where it runs out sends
//! back an ICMP "time exceeded", which the kernel gives the probe's
//! socket (IP_RECVERR, read with MSG_ERRQUEUE), as it gives the
//! machine's "port unreachable" when a probe gets there: sockets any
//! program may have, with the network's SEND right.
//!
//! Its default method sends UDP datagrams to ports from 33434 on, one
//! more for each probe, each from a socket of its own; `-U` sends them
//! all to one port, 53 unless `-p` says; `-I` sends ICMP echo requests
//! through one ping socket (ADR 0336), told apart by their sequence
//! numbers, and the machine's echo reply ends the way. It sends up to
//! `-N` probes at once, 16, and waits for each up to `-w` MAX seconds,
//! 5, or HERE times as long as another of its hop took, 3, or NEAR times
//! as long as one of a later hop, 10; it prints each hop's probes in
//! order as they come or time out, and stops after the hop the machine
//! answered at, or that said it cannot be reached (`!H`, `!N`, `!X`...).
//! It takes `-4`, `-6`, `-n`, `-f` FIRST, `-m` MAX, `-q` NQUERIES, `-z`
//! SENDWAIT, `-s` SOURCE and `-i` INTERFACE, and a packet's length after
//! the host; traceroute6 is traceroute -6.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::mem;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail_with, usage};

/// The first port the default method's probes go to.
const PORT: u16 = 33434;

/// The port `-U`'s probes go to, unless `-p` says.
const UDP_PORT: u16 = 53;

/// The hops tried, unless `-m` says, and the most.
const HOPS: u32 = 30;
const HOPS_MAX: u32 = 255;

/// The probes sent to each hop, unless `-q` says, and the most.
const QUERIES: usize = 3;
const QUERIES_MAX: usize = 10;

/// The probes sent at once, unless `-N` says.
const SIMULTANEOUS: usize = 16;

/// How long to wait for a probe, unless `-w` says: at most MAX seconds,
/// HERE times as long as another probe of its hop took, NEAR times as
/// long as one of a later hop.
const WAIT: f64 = 5.0;
const HERE: f64 = 3.0;
const NEAR: f64 = 10.0;

/// Added to a round trip before it is multiplied, as Butskoy's adds its
/// timer's precision.
const PRECISION: f64 = 0.001;

/// The bytes of data a probe carries, unless the packet length says:
/// 60-byte packets by IPv4, 80-byte ones by IPv6.
const DATA: usize = 32;

/// The longest packet it sends.
const PACKET_MAX: usize = 65_000;

/// The bytes of a UDP header and an ICMP echo message's.
const HEADER: usize = 8;

/// ICMP's and ICMPv6's types of echo request and reply.
const ECHO: u8 = 8;
const ECHO_REPLY: u8 = 0;
const ECHO_V6: u8 = 128;
const ECHO_REPLY_V6: u8 = 129;

/// Linux's ee_origin values for an ICMP and an ICMPv6 error.
pub const ORIGIN_ICMP: u8 = 2;
pub const ORIGIN_ICMP6: u8 = 3;

/// How probes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    /// UDP to a port one more for each probe.
    Udp,
    /// UDP to one port (`-U`).
    UdpPort,
    /// ICMP echo requests (`-I`).
    Icmp,
}

/// What the command line asks for.
#[derive(Debug)]
struct Options {
    /// Only IPv4, or only IPv6.
    version: Option<bool>,
    numeric: bool,
    method: Method,
    first: u32,
    max: u32,
    queries: usize,
    simultaneous: usize,
    /// MAX, HERE and NEAR.
    wait: (f64, f64, f64),
    /// The least time between two probes.
    sendwait: Duration,
    port: Option<u16>,
    source: Option<IpAddr>,
    interface: Option<String>,
    target: String,
    length: Option<usize>,
}

/// `-w`'s MAX[,HERE,NEAR], with `,` or `/` between them.
fn wait_of(text: &str) -> Option<(f64, f64, f64)> {
    let mut values = [WAIT, HERE, NEAR];
    let mut parts = text.split([',', '/']);
    for value in &mut values {
        let Some(part) = parts.next() else { break };
        *value = part.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some((values[0], values[1], values[2]))
}

/// `-z`'s SENDWAIT: milliseconds above 10, seconds otherwise.
fn sendwait_of(text: &str) -> Option<Duration> {
    let value = text.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)?;
    let seconds = if value > 10.0 { value / 1000.0 } else { value };
    Duration::try_from_secs_f64(seconds).ok()
}

/// A number of `what` from `min` to `max`.
fn ranged<T>(args: &mut Args, what: &str, min: T, max: T) -> Result<T, Fail>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let text = args.text()?;
    match text.parse::<T>() {
        Ok(value) if value >= min && value <= max => Ok(value),
        _ => Err(usage(format!("{what} must be {min}..{max}, not '{text}'"))),
    }
}

/// What `--help` prints.
const HELP: &str = "Usage: traceroute [ -46InU ] [ -f first_ttl ] [ -i device ] [ -m max_ttl ] \
[ -N squeries ] [ -p port ] [ -q nqueries ] [ -s src_addr ] [ -w MAX,HERE,NEAR ] [ -z sendwait ] \
host [ packetlen ]\n";

/// Reads the command line; traceroute6 asks for IPv6. `None` for
/// `--help`, which it prints.
fn options(args: &mut Args, v6: bool) -> Result<Option<Options>, Fail> {
    let mut options = Options {
        version: v6.then_some(true),
        numeric: false,
        method: Method::Udp,
        first: 1,
        max: HOPS,
        queries: QUERIES,
        simultaneous: SIMULTANEOUS,
        wait: (WAIT, HERE, NEAR),
        sendwait: Duration::ZERO,
        port: None,
        source: None,
        interface: None,
        target: String::new(),
        length: None,
    };
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        let long = |name: &str| matches!(&arg, Arg::Long(long) if long == name);
        match arg {
            Arg::Short('4') => options.version = Some(false),
            Arg::Short('6') => options.version = Some(true),
            Arg::Short('n') => options.numeric = true,
            Arg::Short('I') => options.method = Method::Icmp,
            Arg::Short('U') => options.method = Method::UdpPort,
            Arg::Short('f') => options.first = ranged(args, "first hop", 1, HOPS_MAX)?,
            Arg::Short('m') => options.max = ranged(args, "max hops", 1, HOPS_MAX)?,
            Arg::Short('q') => options.queries = ranged(args, "queries", 1, QUERIES_MAX)?,
            Arg::Short('N') => options.simultaneous = ranged(args, "sim queries", 1, 1000)?,
            Arg::Short('p') => options.port = Some(ranged(args, "port", 1, u16::MAX)?),
            Arg::Short('w') => {
                let text = args.text()?;
                options.wait = wait_of(&text)
                    .ok_or_else(|| usage(format!("Cannot handle `-w' option with arg `{text}'")))?;
            }
            Arg::Short('z') => {
                let text = args.text()?;
                options.sendwait = sendwait_of(&text)
                    .ok_or_else(|| usage(format!("Cannot handle `-z' option with arg `{text}'")))?;
            }
            Arg::Short('s') => {
                let text = args.text()?;
                options.source =
                    Some(text.parse().map_err(|_| {
                        usage(format!("Cannot handle `-s' option with arg `{text}'"))
                    })?);
            }
            Arg::Short('i') => options.interface = Some(args.text()?),
            Arg::Long(_) if long("icmp") => options.method = Method::Icmp,
            Arg::Long(_) if long("udp") => options.method = Method::UdpPort,
            Arg::Long(_) if long("first") => {
                options.first = ranged(args, "first hop", 1, HOPS_MAX)?;
            }
            Arg::Long(_) if long("max-hops") => {
                options.max = ranged(args, "max hops", 1, HOPS_MAX)?;
            }
            Arg::Long(_) if long("queries") => {
                options.queries = ranged(args, "queries", 1, QUERIES_MAX)?;
            }
            Arg::Long(_) if long("sim-queries") => {
                options.simultaneous = ranged(args, "sim queries", 1, 1000)?;
            }
            Arg::Long(_) if long("port") => {
                options.port = Some(ranged(args, "port", 1, u16::MAX)?);
            }
            Arg::Long(_) if long("wait") => {
                let text = args.text()?;
                options.wait = wait_of(&text)
                    .ok_or_else(|| usage(format!("Cannot handle `-w' option with arg `{text}'")))?;
            }
            Arg::Long(_) if long("sendwait") => {
                let text = args.text()?;
                options.sendwait = sendwait_of(&text)
                    .ok_or_else(|| usage(format!("Cannot handle `-z' option with arg `{text}'")))?;
            }
            Arg::Long(_) if long("source") => {
                let text = args.text()?;
                options.source =
                    Some(text.parse().map_err(|_| {
                        usage(format!("Cannot handle `-s' option with arg `{text}'"))
                    })?);
            }
            Arg::Long(_) if long("interface") => options.interface = Some(args.text()?),
            Arg::Long(_) if long("help") => {
                print!("{HELP}");
                return Ok(None);
            }
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    if options.first > options.max {
        return Err(usage("first hop out of range".into()));
    }
    let mut operands = operands.into_iter();
    options.target =
        operands.next().ok_or_else(|| usage("Specify \"host\" missing argument.".into()))?;
    if let Some(length) = operands.next() {
        let parsed = length.parse::<usize>().ok().filter(|&length| length <= PACKET_MAX);
        options.length = Some(parsed.ok_or_else(|| {
            usage(format!("Cannot handle \"packetlen\" cmdline arg `{length}' on position 2"))
        })?);
    }
    if let Some(extra) = operands.next() {
        return Err(usage(format!("Extra arg `{extra}' (position 3, argc 3)")));
    }
    Ok(Some(options))
}

/// The address of `target`, of the version asked for if one is: IPv4's
/// when a name has both, as Butskoy's chooses.
fn address_of(target: &str, version: Option<bool>) -> Result<IpAddr, Fail> {
    let wanted = |address: &IpAddr| version.is_none_or(|v6| address.is_ipv6() == v6);
    let not_known = || {
        fail_with(
            2,
            format!(
                "{target}: Name or service not known\nCannot handle \"host\" cmdline arg \
                 `{target}' on position 1 (argc 1)"
            ),
        )
    };
    if let Ok(address) = target.parse::<IpAddr>() {
        return if wanted(&address) { Ok(address) } else { Err(not_known()) };
    }
    let found: Vec<IpAddr> = (target, 0)
        .to_socket_addrs()
        .map_err(|_| not_known())?
        .map(|address| address.ip())
        .filter(wanted)
        .collect();
    found.iter().find(|address| address.is_ipv4()).or(found.first()).copied().ok_or_else(not_known)
}

/// The probe an echo message's sequence number names.
fn echo_probe(message: &[u8]) -> Option<usize> {
    let seq = u16::from_be_bytes(message.get(6..8)?.try_into().ok()?);
    usize::from(seq).checked_sub(1)
}

/// Linux's struct sock_extended_err, which an IP_RECVERR or IPV6_RECVERR
/// message holds, then the offender's address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard {
    pub errno: u32,
    pub origin: u8,
    pub kind: u8,
    pub code: u8,
    pub info: u32,
    /// Who sent it.
    pub offender: Option<IpAddr>,
}

/// The bytes of a struct sock_extended_err.
const EXTENDED_ERROR: usize = 16;

/// The address of a struct sockaddr_in or sockaddr_in6 in `bytes`, an
/// IPv4-mapped one as IPv4.
fn address_in(bytes: &[u8]) -> Option<IpAddr> {
    let family = i32::from(u16::from_ne_bytes([*bytes.first()?, *bytes.get(1)?]));
    if family == libc::AF_INET {
        let ip: [u8; 4] = bytes.get(4..8)?.try_into().ok()?;
        Some(IpAddr::from(ip))
    } else if family == libc::AF_INET6 {
        let ip: [u8; 16] = bytes.get(8..24)?.try_into().ok()?;
        Some(IpAddr::from(ip).to_canonical())
    } else {
        None
    }
}

/// The error an IP_RECVERR or IPV6_RECVERR message's `data` tells of.
fn heard(data: &[u8]) -> Option<Heard> {
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_ne_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    Some(Heard {
        errno: word(0)?,
        origin: *data.get(4)?,
        kind: *data.get(5)?,
        code: *data.get(6)?,
        info: word(8)?,
        offender: data.get(EXTENDED_ERROR..).and_then(address_in),
    })
}

/// What an ICMP or ICMPv6 error says of a probe, as Butskoy's prints it:
/// `None` for a time exceeded on the way, which is no end; otherwise the
/// probe went as far as it goes, and the text after its time (empty for
/// a machine that was reached, "!H" for a host unreachable...).
fn note(heard: &Heard) -> Option<String> {
    let (kind, code, info) = (heard.kind, heard.code, heard.info);
    let text = match heard.origin {
        ORIGIN_ICMP => match (kind, code) {
            (11, 0) => return None,
            (3, 0 | 6 | 8 | 11) => "!N".into(),
            (3, 1 | 7 | 12) => "!H".into(),
            (3, 9 | 10 | 13) => "!X".into(),
            (3, 3) => String::new(),
            (3, 2) => "!P".into(),
            (3, 4) => format!("!F-{info}"),
            (3, 5) => "!S".into(),
            (3, 14) => "!V".into(),
            (3, 15) => "!C".into(),
            (3, code) => format!("!<{code}>"),
            (kind, code) => format!("!<{kind}-{code}>"),
        },
        ORIGIN_ICMP6 => match (kind, code) {
            (3, 0) => return None,
            (1, 0) => "!N".into(),
            (1, 2 | 3) => "!H".into(),
            (1, 1) => "!X".into(),
            (1, 4) => String::new(),
            (1, code) => format!("!<{code}>"),
            (2, _) => format!("!F-{info}"),
            (kind, code) => format!("!<{kind}-{code}>"),
        },
        _ => format!("!<{kind}-{code}>"),
    };
    Some(text)
}

/// One probe.
#[derive(Debug, Default)]
struct Probe {
    sent: Option<Instant>,
    received: Option<Instant>,
    /// Answered, or waited for long enough.
    done: bool,
    /// Its hop is the last.
    last: bool,
    /// Who answered it.
    from: Option<IpAddr>,
    /// What the answer said, as Butskoy's prints it.
    note: String,
    /// Its own socket, for a UDP method.
    socket: Option<Socket>,
}

/// How long to wait for `probes[at]`: the most `wait` allows, or less
/// once another probe of its hop (of `queries`) or a later one has been
/// answered, as Butskoy's waits.
fn timeout(probes: &[Probe], at: usize, queries: usize, wait: (f64, f64, f64)) -> f64 {
    let (most, here, near) = wait;
    let took = |probe: &Probe| match (probe.done, probe.sent, probe.received) {
        (true, Some(sent), Some(received)) => Some(received.duration_since(sent).as_secs_f64()),
        _ => None,
    };
    if here > 0.0 {
        let hop = at - at % queries;
        if let Some(took) = probes[hop..(hop + queries).min(probes.len())].iter().find_map(took) {
            return ((took + PRECISION) * here).min(most);
        }
    }
    if near > 0.0 {
        let later = probes[at + 1..].iter().take_while(|probe| probe.sent.is_some());
        if let Some(took) = later.filter_map(took).next() {
            return ((took + PRECISION) * near).min(most);
        }
    }
    most
}

/// What `probes[at]`, of `queries` a hop, prints: its hop's number first
/// if it is the hop's first, then who answered (unless the hop's last
/// answered probe was answered by the same), how long it took and what
/// the answer said; `name` gives what an address is shown as.
fn line_of(
    probes: &[Probe],
    at: usize,
    queries: usize,
    name: &mut dyn FnMut(IpAddr) -> String,
) -> String {
    let probe = &probes[at];
    let place = at % queries;
    let mut text = String::new();
    if place == 0 {
        let _ = write!(text, "\n{:2} ", at / queries + 1);
    }
    match probe.from {
        None => text.push_str(" *"),
        Some(from) => {
            let before = probes[at - place..at].iter().rev().find_map(|probe| probe.from);
            if before != Some(from) {
                text.push(' ');
                text.push_str(&name(from));
            }
        }
    }
    if let (Some(sent), Some(received)) = (probe.sent, probe.received) {
        let ms = received.duration_since(sent).as_secs_f64() * 1000.0;
        let _ = write!(text, "  {ms:.3} ms");
    }
    if !probe.note.is_empty() {
        let _ = write!(text, " {}", probe.note);
    }
    text
}

/// What one read from a socket gave.
pub struct Received {
    pub data: Vec<u8>,
    /// Who sent it, or where the packet went for an error.
    pub name: Option<IpAddr>,
    pub heard: Option<Heard>,
}

/// Reads the next datagram from `fd`, or with `errors` its next error
/// (MSG_ERRQUEUE), without waiting.
pub fn receive(fd: RawFd, errors: bool) -> io::Result<Received> {
    let mut data = vec![0u8; PACKET_MAX + 512];
    // SAFETY: all zeros is a valid sockaddr_storage.
    let mut name: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut control = [0u64; 64];
    let mut iov = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
    // SAFETY: all zeros is a valid msghdr: null pointers and lengths.
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_name = (&raw mut name).cast();
    message.msg_namelen = mem::size_of_val(&name) as libc::socklen_t;
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = mem::size_of_val(&control) as _;
    let flags = libc::MSG_DONTWAIT | if errors { libc::MSG_ERRQUEUE } else { 0 };
    // SAFETY: the message's buffers are this function's, of the sizes it
    // gives.
    let length = unsafe { libc::recvmsg(fd, &raw mut message, flags) };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    data.truncate(length as usize);
    let mut heard_error = None;
    // SAFETY: the kernel filled `control` with whole control messages, as
    // msg_controllen says, which CMSG_FIRSTHDR and CMSG_NXTHDR walk.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&raw const message);
        while !cmsg.is_null() {
            let (level, kind) = ((*cmsg).cmsg_level, (*cmsg).cmsg_type);
            if (level == libc::SOL_IP && kind == libc::IP_RECVERR)
                || (level == libc::SOL_IPV6 && kind == libc::IPV6_RECVERR)
            {
                let start = libc::CMSG_DATA(cmsg);
                let header = start as usize - cmsg as usize;
                let length = ((*cmsg).cmsg_len as usize).saturating_sub(header);
                heard_error = heard(std::slice::from_raw_parts(start, length));
            }
            cmsg = libc::CMSG_NXTHDR(&raw const message, cmsg);
        }
    }
    // SAFETY: `name` is a sockaddr_storage the kernel wrote
    // msg_namelen bytes of.
    let bytes = unsafe {
        std::slice::from_raw_parts((&raw const name).cast::<u8>(), message.msg_namelen as usize)
    };
    Ok(Received { data, name: address_in(bytes), heard: heard_error })
}

/// Sets an int option of `socket`.
pub fn set_int(socket: &Socket, level: i32, name: i32, value: i32) -> io::Result<()> {
    // SAFETY: an int option, of an int's size.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (&raw const value).cast(),
            mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Binds `socket` to interface `name` (SO_BINDTODEVICE).
fn bind_device(socket: &Socket, name: &str) -> io::Result<()> {
    // SAFETY: a name of `name.len()` bytes.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.len() as libc::socklen_t,
        )
    };
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Whether `error` is one an ICMP error left for the socket's next call,
/// which a send that fails with it has taken.
pub fn left_by_icmp(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(
            libc::EHOSTUNREACH
                | libc::ENETUNREACH
                | libc::ECONNREFUSED
                | libc::EPROTO
                | libc::EMSGSIZE
                | libc::EHOSTDOWN
                | libc::ENONET
                | libc::ENOPROTOOPT
                | libc::EACCES
                | libc::EOPNOTSUPP
        )
    )
}

/// A trace under way.
struct Trace {
    options: Options,
    address: IpAddr,
    v6: bool,
    probes: Vec<Probe>,
    /// The ping socket, for `-I`.
    icmp: Option<Socket>,
    /// What each probe carries after its header.
    data: Vec<u8>,
    next_port: u16,
    /// Names found for addresses.
    names: HashMap<IpAddr, String>,
}

impl Trace {
    /// A socket for a probe, or `-I`'s, with what the options ask for.
    fn socket(&self, icmp: bool) -> Result<Socket, Fail> {
        let domain = if self.v6 { Domain::IPV6 } else { Domain::IPV4 };
        let protocol = match (icmp, self.v6) {
            (true, false) => Protocol::ICMPV4,
            (true, true) => Protocol::ICMPV6,
            (false, _) => Protocol::UDP,
        };
        let socket = Socket::new(domain, Type::DGRAM, Some(protocol))
            .map_err(|e| fail_with(1, format!("socket: {}", util::strip_os_error(&e))))?;
        let broken = |what: &str, e: io::Error| {
            fail_with(1, format!("{what}: {}", util::strip_os_error(&e)))
        };
        socket.set_nonblocking(true).map_err(|e| broken("fcntl", e))?;
        let (level, option) = if self.v6 {
            (libc::SOL_IPV6, libc::IPV6_RECVERR)
        } else {
            (libc::SOL_IP, libc::IP_RECVERR)
        };
        set_int(&socket, level, option, 1).map_err(|e| broken("setsockopt IP_RECVERR", e))?;
        if let Some(interface) = &self.options.interface {
            bind_device(&socket, interface).map_err(|e| broken("setsockopt SO_BINDTODEVICE", e))?;
        }
        if let Some(source) = self.options.source {
            socket
                .bind(&SockAddr::from(SocketAddr::new(source, 0)))
                .map_err(|e| broken("bind", e))?;
        }
        Ok(socket)
    }

    /// Sets the hop limit `ttl` of `socket`.
    fn set_ttl(&self, socket: &Socket, ttl: u32) -> Result<(), Fail> {
        let set = if self.v6 { socket.set_unicast_hops_v6(ttl) } else { socket.set_ttl_v4(ttl) };
        set.map_err(|e| fail_with(1, format!("setsockopt IP_TTL: {}", util::strip_os_error(&e))))
    }

    /// Sends `probes[at]` with a hop limit of `ttl`.
    fn send(&mut self, at: usize, ttl: u32) -> Result<(), Fail> {
        let sent = match self.options.method {
            Method::Udp | Method::UdpPort => {
                let socket = self.socket(false)?;
                self.set_ttl(&socket, ttl)?;
                let port = match self.options.method {
                    Method::UdpPort => self.options.port.unwrap_or(UDP_PORT),
                    _ => {
                        let port = self.next_port;
                        self.next_port = self.next_port.checked_add(1).unwrap_or(1);
                        port
                    }
                };
                let to = SockAddr::from(SocketAddr::new(self.address, port));
                socket
                    .connect(&to)
                    .map_err(|e| fail_with(1, format!("connect: {}", util::strip_os_error(&e))))?;
                let sent = socket.send(&self.data);
                self.probes[at].socket = Some(socket);
                sent
            }
            Method::Icmp => {
                let mut message = vec![0u8; HEADER];
                message[0] = if self.v6 { ECHO_V6 } else { ECHO };
                let seq = (at + 1) as u16;
                message[6..8].copy_from_slice(&seq.to_be_bytes());
                message.extend_from_slice(&self.data);
                let socket = self.icmp.as_ref().expect("-I's socket");
                self.set_ttl(socket, ttl)?;
                let to = SockAddr::from(SocketAddr::new(self.address, 0));
                // An error that came for an earlier probe fails the next
                // send once, as on Linux: it is the earlier one's.
                match socket.send_to(&message, &to) {
                    Err(e) if left_by_icmp(&e) => socket.send_to(&message, &to),
                    sent => sent,
                }
            }
        };
        match sent {
            Ok(_) => {}
            // What the error queue will say more of, as Butskoy's has it.
            Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => {}
            Err(e) if left_by_icmp(&e) && self.options.method != Method::Icmp => {}
            Err(e) => return Err(fail_with(1, format!("send: {}", util::strip_os_error(&e)))),
        }
        self.probes[at].sent = Some(Instant::now());
        Ok(())
    }

    /// Gives `probes[at]` the answer `received` that came at `when`, if
    /// it is not done.
    fn answer(&mut self, at: usize, received: &Received, when: Instant) {
        let Some(probe) = self.probes.get_mut(at) else { return };
        if probe.done || probe.sent.is_none() {
            return;
        }
        match &received.heard {
            Some(heard) => {
                if let Some(note) = note(heard) {
                    probe.last = true;
                    probe.note = note;
                }
                probe.from = heard.offender.or(received.name);
            }
            // The machine answered.
            None => {
                probe.last = true;
                probe.from = received.name;
            }
        }
        probe.received = Some(when);
        probe.done = true;
        probe.socket = None;
    }

    /// Reads what `-I`'s socket has: errors first, whose taking leaves
    /// the next for the socket's next call, then echo replies.
    fn read_icmp(&mut self, fd: RawFd) {
        let reply = if self.v6 { ECHO_REPLY_V6 } else { ECHO_REPLY };
        for _ in 0..256 {
            while let Ok(received) = receive(fd, true) {
                let when = Instant::now();
                if let Some(at) = echo_probe(&received.data) {
                    self.answer(at, &received, when);
                }
            }
            match receive(fd, false) {
                Ok(received) => {
                    let when = Instant::now();
                    if received.data.first() == Some(&reply)
                        && let Some(at) = echo_probe(&received.data)
                    {
                        self.answer(at, &received, when);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                // An error left for the socket's next call: the queue
                // holds it.
                Err(_) => {}
            }
        }
    }

    /// Waits until `until` for answers to the probes from `start` to
    /// `end`, and takes those that come.
    fn wait(&mut self, start: usize, end: usize, until: Instant) -> Result<(), Fail> {
        let mut fds = Vec::new();
        let mut owners = Vec::new();
        if let Some(socket) = &self.icmp {
            fds.push(libc::pollfd { fd: socket.as_raw_fd(), events: libc::POLLIN, revents: 0 });
            owners.push(None);
        }
        for (at, probe) in self.probes.iter().enumerate().take(end).skip(start) {
            if let (false, Some(socket)) = (probe.done, &probe.socket) {
                fds.push(libc::pollfd { fd: socket.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                owners.push(Some(at));
            }
        }
        let wait = until.saturating_duration_since(Instant::now());
        let ms = wait.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32;
        // SAFETY: `fds` is a vector of pollfds of its length.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        if ready < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(fail_with(1, format!("poll: {}", util::strip_os_error(&e))));
        }
        for (entry, owner) in fds.iter().zip(owners) {
            if entry.revents == 0 {
                continue;
            }
            match owner {
                None => self.read_icmp(entry.fd),
                Some(at) => {
                    let errors = entry.revents & libc::POLLERR != 0;
                    if let Ok(received) = receive(entry.fd, errors) {
                        self.answer(at, &received, Instant::now());
                    }
                }
            }
        }
        Ok(())
    }

    /// Prints `probes[at]`: an address as its number with `-n`, else as
    /// its name and its number.
    fn print(&mut self, at: usize, out: &mut impl Write) -> io::Result<()> {
        let queries = self.options.queries;
        let mut names = mem::take(&mut self.names);
        let numeric = self.options.numeric;
        let text = line_of(&self.probes, at, queries, &mut |address| {
            if numeric {
                return address.to_string();
            }
            let name = names.entry(address).or_insert_with(|| reverse(address));
            format!("{name} ({address})")
        });
        self.names = names;
        out.write_all(text.as_bytes())?;
        out.flush()
    }

    /// Traces the way, printing it.
    fn run(&mut self, out: &mut impl Write) -> Result<(), Fail> {
        let queries = self.options.queries;
        let (sendwait, simultaneous, wait) =
            (self.options.sendwait, self.options.simultaneous, self.options.wait);
        let mut start = (self.options.first as usize - 1) * queries;
        let mut end = self.probes.len();
        let mut last_send: Option<Instant> = None;
        while start < end {
            let now = Instant::now();
            let mut wake: Option<Instant> = None;
            let mut outstanding = 0;
            let mut at = start;
            while at < end {
                if at == start
                    && !self.probes[at].done
                    && let Some(sent) = self.probes[at].sent
                    && sent + Duration::from_secs_f64(timeout(&self.probes, at, queries, wait))
                        <= now
                {
                    let probe = &mut self.probes[at];
                    probe.done = true;
                    probe.socket = None;
                }
                if self.probes[at].done {
                    if at == start {
                        self.print(at, out)?;
                        start += 1;
                    }
                    if self.probes[at].last {
                        end = end.min((at / queries + 1) * queries);
                    }
                    at += 1;
                    continue;
                }
                if self.probes[at].sent.is_none() {
                    if let Some(last) = last_send
                        && now < last + sendwait
                    {
                        wake = Some(last + sendwait);
                        break;
                    }
                    self.send(at, (at / queries + 1) as u32)?;
                    last_send = Some(Instant::now());
                }
                outstanding += 1;
                if outstanding >= simultaneous {
                    break;
                }
                at += 1;
            }
            if start >= end {
                break;
            }
            let probe = &self.probes[start];
            if let (false, Some(sent)) = (probe.done, probe.sent) {
                let expire =
                    sent + Duration::from_secs_f64(timeout(&self.probes, start, queries, wait));
                wake = Some(wake.map_or(expire, |wake| wake.min(expire)));
            }
            let until = wake.unwrap_or(now);
            self.wait(start, end, until)?;
        }
        writeln!(out)?;
        out.flush()?;
        Ok(())
    }
}

/// The name of `address`, or its number if it has none.
fn reverse(address: IpAddr) -> String {
    let name = SockAddr::from(SocketAddr::new(address, 0));
    let mut host = [0 as libc::c_char; 1025];
    // SAFETY: the address is a sockaddr of its length; the host buffer is
    // of the length given, and no service is asked for.
    let result = unsafe {
        libc::getnameinfo(
            name.as_ptr().cast(),
            name.len(),
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            0,
        )
    };
    if result != 0 {
        return address.to_string();
    }
    // SAFETY: getnameinfo wrote a NUL-terminated name.
    unsafe { CStr::from_ptr(host.as_ptr()) }.to_string_lossy().into_owned()
}

/// traceroute(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    run(args, false)
}

/// traceroute6(8): traceroute -6.
pub fn main6(args: &mut Args) -> Result<i32, Fail> {
    run(args, true)
}

fn run(args: &mut Args, v6: bool) -> Result<i32, Fail> {
    let Some(options) = options(args, v6)? else { return Ok(0) };
    let address = address_of(&options.target, options.version)?;
    let v6 = address.is_ipv6();
    let ip = if v6 { 40 } else { 20 };
    let data = match options.length {
        Some(length) => length.saturating_sub(ip + HEADER),
        None => DATA,
    };
    let length = ip + HEADER + data;
    let probes = (0..options.max as usize * options.queries).map(|_| Probe::default()).collect();
    let next_port = options.port.unwrap_or(PORT);
    let mut trace = Trace {
        address,
        v6,
        probes,
        icmp: None,
        data: (0..data).map(|i| 0x40 + (i & 0x3f) as u8).collect(),
        next_port,
        names: HashMap::new(),
        options,
    };
    if trace.options.method == Method::Icmp {
        trace.icmp = Some(trace.socket(true)?);
    }
    let mut out = io::stdout().lock();
    write!(
        out,
        "traceroute to {} ({address}), {} hops max, {length} byte packets",
        trace.options.target, trace.options.max
    )?;
    out.flush()?;
    trace.run(&mut out)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe answered by `from` in `ms`, or by none.
    fn answered(from: Option<[u8; 4]>, ms: u64, note: &str) -> Probe {
        let sent = Instant::now();
        Probe {
            sent: Some(sent),
            received: from.map(|_| sent + Duration::from_millis(ms)),
            done: true,
            last: false,
            from: from.map(IpAddr::from),
            note: note.into(),
            socket: None,
        }
    }

    #[test]
    fn errors_read_as_linux_writes_them() {
        let mut data = vec![0u8; EXTENDED_ERROR + 16];
        data[0..4].copy_from_slice(&(libc::EHOSTUNREACH as u32).to_ne_bytes());
        data[4] = ORIGIN_ICMP;
        data[5] = 11;
        data[8..12].copy_from_slice(&7u32.to_ne_bytes());
        data[16..18].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
        data[20..24].copy_from_slice(&[10, 77, 1, 1]);
        assert_eq!(
            heard(&data).unwrap(),
            Heard {
                errno: libc::EHOSTUNREACH as u32,
                origin: ORIGIN_ICMP,
                kind: 11,
                code: 0,
                info: 7,
                offender: Some(IpAddr::from([10, 77, 1, 1])),
            }
        );
        assert_eq!(heard(&data[..10]), None, "too short");
        // An IPv4-mapped offender, as an IPv6 socket hears an IPv4 error.
        let mut six = vec![0u8; EXTENDED_ERROR + 28];
        six[4] = ORIGIN_ICMP;
        six[16..18].copy_from_slice(&(libc::AF_INET6 as u16).to_ne_bytes());
        six[24..40]
            .copy_from_slice(&"::ffff:10.77.2.1".parse::<std::net::Ipv6Addr>().unwrap().octets());
        assert_eq!(heard(&six).unwrap().offender, Some(IpAddr::from([10, 77, 2, 1])));
    }

    #[test]
    fn notes_read_as_butskoys() {
        let of = |origin, kind, code, info| {
            note(&Heard { errno: 0, origin, kind, code, info, offender: None })
        };
        assert_eq!(of(ORIGIN_ICMP, 11, 0, 0), None, "time exceeded on the way");
        assert_eq!(of(ORIGIN_ICMP, 3, 3, 0).as_deref(), Some(""), "reached");
        assert_eq!(of(ORIGIN_ICMP, 3, 1, 0).as_deref(), Some("!H"));
        assert_eq!(of(ORIGIN_ICMP, 3, 0, 0).as_deref(), Some("!N"));
        assert_eq!(of(ORIGIN_ICMP, 3, 13, 0).as_deref(), Some("!X"));
        assert_eq!(of(ORIGIN_ICMP, 3, 2, 0).as_deref(), Some("!P"));
        assert_eq!(of(ORIGIN_ICMP, 3, 4, 1400).as_deref(), Some("!F-1400"));
        assert_eq!(of(ORIGIN_ICMP, 3, 99, 0).as_deref(), Some("!<99>"));
        assert_eq!(of(ORIGIN_ICMP, 12, 0, 0).as_deref(), Some("!<12-0>"));
        assert_eq!(of(ORIGIN_ICMP6, 3, 0, 0), None);
        assert_eq!(of(ORIGIN_ICMP6, 1, 4, 0).as_deref(), Some(""));
        assert_eq!(of(ORIGIN_ICMP6, 1, 3, 0).as_deref(), Some("!H"));
        assert_eq!(of(ORIGIN_ICMP6, 1, 0, 0).as_deref(), Some("!N"));
        assert_eq!(of(ORIGIN_ICMP6, 2, 0, 1280).as_deref(), Some("!F-1280"));
    }

    #[test]
    fn hops_print_as_butskoys() {
        let probes = vec![
            answered(Some([10, 77, 1, 1]), 1, ""),
            answered(Some([10, 77, 1, 1]), 2, ""),
            answered(None, 0, ""),
            answered(None, 0, ""),
            answered(Some([10, 77, 2, 1]), 3, ""),
            answered(Some([10, 77, 2, 9]), 4, "!H"),
        ];
        let mut numbers = |address: IpAddr| address.to_string();
        let text: String =
            (0..probes.len()).map(|at| line_of(&probes, at, 3, &mut numbers)).collect();
        assert_eq!(
            text,
            "\n 1  10.77.1.1  1.000 ms  2.000 ms *\n 2  * 10.77.2.1  3.000 ms 10.77.2.9  4.000 ms !H"
        );
    }

    #[test]
    fn waits_shorten_once_others_answer() {
        let wait = (WAIT, HERE, NEAR);
        let mut probes: Vec<Probe> = (0..6).map(|_| Probe::default()).collect();
        assert_eq!(timeout(&probes, 0, 3, wait), WAIT);
        // A later hop's answer in 1 ms: ten times its time, and a little.
        probes[3] = answered(Some([1, 1, 1, 1]), 1, "");
        probes[0].sent = Some(Instant::now());
        probes[1].sent = Some(Instant::now());
        probes[2].sent = Some(Instant::now());
        let near = timeout(&probes, 0, 3, wait);
        assert!((near - 0.02).abs() < 0.0005, "{near}");
        // Its own hop's, three times.
        probes[1] = answered(Some([1, 1, 1, 1]), 2, "");
        let here = timeout(&probes, 0, 3, wait);
        assert!((here - 0.009).abs() < 0.0005, "{here}");
        assert_eq!(timeout(&probes, 0, 3, (0.005, 3.0, 10.0)), 0.005, "never past MAX");
    }

    #[test]
    fn waits_and_sendwaits_read_as_butskoys() {
        assert_eq!(wait_of("5"), Some((5.0, HERE, NEAR)));
        assert_eq!(wait_of("1,2,4"), Some((1.0, 2.0, 4.0)));
        assert_eq!(wait_of("1/0"), Some((1.0, 0.0, NEAR)));
        assert_eq!(wait_of("1,2,3,4"), None);
        assert_eq!(wait_of("x"), None);
        assert_eq!(sendwait_of("0.5"), Some(Duration::from_millis(500)));
        assert_eq!(sendwait_of("50"), Some(Duration::from_millis(50)));
    }
}
