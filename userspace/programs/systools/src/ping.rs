// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ping(8): whether a machine answers, and how soon, as iputils' ping
//! says: ICMP or ICMPv6 echo requests through a ping socket (ADR 0336),
//! which any program may have, with the network's SEND right for a
//! machine beyond the local networks or LOCAL for one on them.
//!
//! It takes iputils' options that a ping socket allows: `-4`, `-6`, `-c`
//! COUNT, `-i` INTERVAL, `-W` TIMEOUT, `-w` DEADLINE, `-s` SIZE, `-t` TTL,
//! `-b` (a broadcast address may be pinged), `-q` and `-n`; it prints what
//! iputils prints, its statistics at the end, or when it is interrupted,
//! and exits as iputils does: 0 if a reply came (with `-w` and `-c`, as
//! many as were sent), 1 if not, 2 for another failure. As iputils' does,
//! it asks for the ICMP errors about its requests (IP_RECVERR, ADR 0345),
//! and says which router or machine sent one: "From 10.0.0.1 icmp_seq=3
//! Time to live exceeded".

use std::fmt::Write as _;
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::args::{Arg, Args};
use crate::traceroute::{self, Heard, ORIGIN_ICMP6};
use crate::util::{self, Fail, fail_with, usage};

/// The bytes of data each request carries, unless `-s` says.
const SIZE: usize = 56;

/// The bytes of an ICMP echo message's header.
const HEADER: usize = 8;

/// The most data a request may carry: an IPv4 datagram's, less its
/// headers.
const SIZE_MAX: usize = 65_507;

/// The shortest interval between requests, as iputils allows anyone who
/// may not flood.
const INTERVAL_MIN: Duration = Duration::from_millis(2);

/// How long it waits for the last replies, unless `-W` says.
const LINGER: Duration = Duration::from_secs(10);

/// Echo requests' and replies' types, for ICMP and ICMPv6.
const REQUEST: u8 = 8;
const REPLY: u8 = 0;
const REQUEST_V6: u8 = 128;
const REPLY_V6: u8 = 129;

/// Whether SIGINT came: ping stops, and says how it went.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// What the command line asks for.
#[derive(Debug)]
struct Options {
    /// Only IPv4, or only IPv6.
    version: Option<bool>,
    count: Option<u64>,
    interval: Duration,
    /// How long to wait for a reply after the last request.
    timeout: Duration,
    /// When to stop, however many replies came.
    deadline: Option<Duration>,
    size: usize,
    ttl: Option<u32>,
    broadcast: bool,
    quiet: bool,
    target: String,
}

/// A number of seconds, as `-i`, `-W` and `-w` take one.
fn seconds(args: &mut Args, what: &str) -> Result<Duration, Fail> {
    let text = args.text()?;
    text.parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
        .ok_or_else(|| usage(format!("invalid {what}: '{text}'")))
}

/// Reads the command line.
fn options(args: &mut Args) -> Result<Options, Fail> {
    let mut options = Options {
        version: None,
        count: None,
        interval: Duration::from_secs(1),
        timeout: LINGER,
        deadline: None,
        size: SIZE,
        ttl: None,
        broadcast: false,
        quiet: false,
        target: String::new(),
    };
    let mut targets = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('4') => options.version = Some(false),
            Arg::Short('6') => options.version = Some(true),
            Arg::Short('c') => {
                let count: u64 = args.number()?;
                if count == 0 {
                    return Err(usage("invalid argument: '0': out of range: 1 <= value".into()));
                }
                options.count = Some(count);
            }
            Arg::Short('i') => {
                let interval = seconds(args, "interval")?;
                if interval < INTERVAL_MIN {
                    return Err(fail_with(
                        2,
                        "cannot flood, minimal interval for user must be >= 2 ms, use -i 0.002 \
                         (or higher)"
                            .into(),
                    ));
                }
                options.interval = interval;
            }
            Arg::Short('W') => options.timeout = seconds(args, "timeout")?,
            Arg::Short('w') => options.deadline = Some(seconds(args, "deadline")?),
            Arg::Short('s') => {
                let size: usize = args.number()?;
                if size > SIZE_MAX {
                    return Err(usage(format!(
                        "invalid argument: '{size}': out of range: 0 <= value <= {SIZE_MAX}"
                    )));
                }
                options.size = size;
            }
            Arg::Short('t') => {
                let ttl: u32 = args.number()?;
                if !(1..=255).contains(&ttl) {
                    return Err(usage(format!(
                        "invalid argument: '{ttl}': out of range: 1 <= value <= 255"
                    )));
                }
                options.ttl = Some(ttl);
            }
            Arg::Short('b') => options.broadcast = true,
            Arg::Short('q') => options.quiet = true,
            // Addresses are shown as numbers already.
            Arg::Short('n') => {}
            Arg::Operand(target) => targets.push(target.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let [target] = <[String; 1]>::try_from(targets)
        .map_err(|_| usage("usage error: Destination address required".into()))?;
    options.target = target;
    Ok(options)
}

/// The address of `target`, a number or a name, of the version asked
/// for if one is.
fn address_of(target: &str, version: Option<bool>) -> Result<IpAddr, Fail> {
    let wanted = |address: &IpAddr| version.is_none_or(|v6| address.is_ipv6() == v6);
    if let Ok(address) = target.parse::<IpAddr>() {
        return if wanted(&address) {
            Ok(address)
        } else {
            Err(fail_with(2, format!("{target}: Address family for hostname not supported")))
        };
    }
    let found = (target, 0)
        .to_socket_addrs()
        .map_err(|e| fail_with(2, format!("{target}: {}", util::strip_os_error(&e))))?;
    found
        .map(|address| address.ip())
        .find(wanted)
        .ok_or_else(|| fail_with(2, format!("{target}: Address family for hostname not supported")))
}

/// What ping has seen so far.
#[derive(Debug, Default)]
struct Tally {
    sent: u64,
    received: u64,
    duplicates: u64,
    errors: u64,
    /// The round trips' times, in milliseconds: the least, the most, the
    /// sum and the sum of their squares.
    least: f64,
    most: f64,
    sum: f64,
    squares: f64,
}

impl Tally {
    /// Counts a reply that took `ms`.
    fn reply(&mut self, ms: f64) {
        if self.received == 0 || ms < self.least {
            self.least = ms;
        }
        self.most = self.most.max(ms);
        self.received += 1;
        self.sum += ms;
        self.squares += ms * ms;
    }

    /// The statistics iputils prints at the end, after `elapsed`.
    fn summary(&self, target: &str, elapsed: Duration) -> String {
        let mut text = format!("\n--- {target} ping statistics ---\n");
        let _ = write!(text, "{} packets transmitted, {} received", self.sent, self.received);
        if self.duplicates > 0 {
            let _ = write!(text, ", +{} duplicates", self.duplicates);
        }
        if self.errors > 0 {
            let _ = write!(text, ", +{} errors", self.errors);
        }
        if self.sent > 0 {
            // As %g prints it: 0, 25, 33.3333.
            let lost = self.sent.saturating_sub(self.received) as f64 * 100.0 / self.sent as f64;
            let lost = (lost * 10_000.0).round() / 10_000.0;
            let _ = write!(text, ", {lost}% packet loss");
        }
        let _ = writeln!(text, ", time {}ms", elapsed.as_millis());
        if self.received > 0 {
            let n = self.received as f64;
            let mean = self.sum / n;
            let deviation = (self.squares / n - mean * mean).max(0.0).sqrt();
            let _ = writeln!(
                text,
                "rtt min/avg/max/mdev = {:.3}/{mean:.3}/{:.3}/{deviation:.3} ms",
                self.least, self.most
            );
        }
        text
    }
}

/// An echo request numbered `seq`, carrying `size` bytes: first the time
/// it went, from `start`, as iputils' carries a timeval, then counting
/// bytes. Its identifier is the socket's, and its checksum is made as it
/// goes.
fn request(v6: bool, seq: u16, size: usize, sent: Duration) -> Vec<u8> {
    let mut message = vec![0u8; HEADER + size];
    message[0] = if v6 { REQUEST_V6 } else { REQUEST };
    message[6..8].copy_from_slice(&seq.to_be_bytes());
    for (i, byte) in message[HEADER..].iter_mut().enumerate() {
        *byte = (i + HEADER) as u8;
    }
    if size >= 8 {
        let nanos = u64::try_from(sent.as_nanos()).unwrap_or(u64::MAX);
        message[HEADER..HEADER + 8].copy_from_slice(&nanos.to_le_bytes());
    }
    message
}

/// A reply's sequence number, and when its request went if it says.
fn reply(v6: bool, message: &[u8]) -> Option<(u16, Option<Duration>)> {
    let kind = if v6 { REPLY_V6 } else { REPLY };
    if message.len() < HEADER || message[0] != kind {
        return None;
    }
    let seq = u16::from_be_bytes([message[6], message[7]]);
    let sent = message
        .get(HEADER..HEADER + 8)
        .and_then(|b| b.try_into().ok())
        .map(|b| Duration::from_nanos(u64::from_le_bytes(b)));
    Some((seq, sent))
}

/// What an ICMP error about a request says, as iputils' ping prints it.
fn error_text(heard: &Heard) -> String {
    let (kind, code, info) = (heard.kind, heard.code, heard.info);
    if heard.origin == ORIGIN_ICMP6 {
        return match kind {
            1 => {
                let why = match code {
                    0 => "No route".to_string(),
                    1 => "Administratively prohibited".into(),
                    2 => "Beyond scope of source address".into(),
                    3 => "Address unreachable".into(),
                    4 => "Port unreachable".into(),
                    code => format!("Unknown code {code}"),
                };
                format!("Destination unreachable: {why}")
            }
            2 if code != 0 => format!("Packet too big: mtu={info}, code={code}"),
            2 => format!("Packet too big: mtu={info}"),
            3 => match code {
                0 => "Time exceeded: Hop limit".into(),
                1 => "Time exceeded: Defragmentation failure".into(),
                code => format!("Time exceeded: code {code}"),
            },
            4 => {
                let what = match code {
                    0 => "Wrong header field ".to_string(),
                    1 => "Unknown header ".into(),
                    2 => "Unknown option ".into(),
                    code => format!("code {code} "),
                };
                format!("Parameter problem: {what}at {info}")
            }
            kind => format!("Unknown icmp type: {kind}"),
        };
    }
    match (kind, code) {
        (3, _) => match code {
            0 => "Destination Net Unreachable".into(),
            1 => "Destination Host Unreachable".into(),
            2 => "Destination Protocol Unreachable".into(),
            3 => "Destination Port Unreachable".into(),
            4 => format!("Frag needed and DF set (mtu = {info})"),
            5 => "Source Route Failed".into(),
            6 => "Destination Net Unknown".into(),
            7 => "Destination Host Unknown".into(),
            8 => "Source Host Isolated".into(),
            9 => "Destination Net Prohibited".into(),
            10 => "Destination Host Prohibited".into(),
            11 => "Destination Net Unreachable for Type of Service".into(),
            12 => "Destination Host Unreachable for Type of Service".into(),
            13 => "Packet filtered".into(),
            14 => "Precedence Violation".into(),
            15 => "Precedence Cutoff".into(),
            code => format!("Dest Unreachable, Bad Code: {code}"),
        },
        (11, 0) => "Time to live exceeded".into(),
        (11, 1) => "Frag reassembly time exceeded".into(),
        (11, code) => format!("Time exceeded, Bad Code: {code}"),
        (12, _) => format!("Parameter problem: pointer = {info}"),
        (kind, _) => format!("Bad ICMP type: {kind}"),
    }
}

/// Takes the ICMP errors that wait for `socket` (MSG_ERRQUEUE) about its
/// requests to `address`, and prints each, as iputils' ping does, unless
/// `quiet`: how many there were.
fn take_errors(
    socket: &Socket,
    v6: bool,
    address: IpAddr,
    quiet: bool,
    out: &mut impl Write,
) -> io::Result<u64> {
    let request = if v6 { REQUEST_V6 } else { REQUEST };
    let mut taken = 0;
    while let Ok(received) = traceroute::receive(socket.as_raw_fd(), true) {
        let Some(heard) = received.heard else { continue };
        let message = &received.data;
        // Another program's, or no error about a request.
        if message.len() < HEADER || message[0] != request || received.name != Some(address) {
            continue;
        }
        taken += 1;
        if !quiet {
            let seq = u16::from_be_bytes([message[6], message[7]]);
            let from = heard.offender.map_or_else(|| "?".to_string(), |from| from.to_string());
            writeln!(out, "From {from} icmp_seq={seq} {}", error_text(&heard))?;
            out.flush()?;
        }
    }
    Ok(taken)
}

/// ping(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let options = options(args)?;
    let address = address_of(&options.target, options.version)?;
    let v6 = address.is_ipv6();
    let (domain, protocol) =
        if v6 { (Domain::IPV6, Protocol::ICMPV6) } else { (Domain::IPV4, Protocol::ICMPV4) };
    let socket = Socket::new(domain, Type::DGRAM, Some(protocol))
        .map_err(|e| fail_with(2, format!("socket: {}", util::strip_os_error(&e))))?;
    if let Some(ttl) = options.ttl {
        let set = if v6 { socket.set_unicast_hops_v6(ttl) } else { socket.set_ttl_v4(ttl) };
        set.map_err(|e| fail_with(2, format!("can't set unicast time-to-live: {e}")))?;
    }
    if options.broadcast && !v6 {
        socket.set_broadcast(true).map_err(|e| fail_with(2, format!("broadcast: {e}")))?;
    }
    // The routers' and the machine's errors about the requests, as
    // iputils asks for them.
    let (level, option) =
        if v6 { (libc::SOL_IPV6, libc::IPV6_RECVERR) } else { (libc::SOL_IP, libc::IP_RECVERR) };
    traceroute::set_int(&socket, level, option, 1)
        .map_err(|e| fail_with(2, format!("setsockopt: {}", util::strip_os_error(&e))))?;
    // SAFETY: all zeros is a valid sigaction: numbers, a signal set and a
    // function address.
    let mut action: libc::sigaction = unsafe { util::zeroed() };
    action.sa_sigaction = on_interrupt as extern "C" fn(libc::c_int) as *const () as usize;
    // SAFETY: the handler only stores to an atomic; without SA_RESTART a
    // wait for a reply ends when SIGINT comes.
    unsafe { libc::sigaction(libc::SIGINT, &raw const action, std::ptr::null_mut()) };
    let to = SockAddr::from(SocketAddr::new(address, 0));
    let mut out = io::stdout().lock();
    if v6 {
        writeln!(out, "PING {} ({address}) {} data bytes", options.target, options.size)?;
    } else {
        let total = options.size + HEADER + 20;
        writeln!(
            out,
            "PING {} ({address}) {}({total}) bytes of data.",
            options.target, options.size
        )?;
    }
    out.flush()?;
    let named = options.target.parse::<IpAddr>().is_err();
    let start = Instant::now();
    let mut tally = Tally::default();
    // Which requests have been answered, by sequence number.
    let mut answered = vec![false; 1 << 16];
    let mut next = start;
    let mut last_sent: Option<Instant> = None;
    let mut buffer = vec![MaybeUninit::<u8>::uninit(); HEADER + SIZE_MAX];
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            break;
        }
        // As iputils' ends: once each request of the count was answered
        // or an error came about it, and with a deadline at the first
        // error.
        if options.count.is_some_and(|c| tally.received + tally.errors >= c)
            || (options.deadline.is_some() && tally.errors > 0)
        {
            break;
        }
        let now = Instant::now();
        if options.deadline.is_some_and(|d| now >= start + d) {
            break;
        }
        let all_sent = options.count.is_some_and(|c| tally.sent >= c);
        if all_sent {
            let all_answered = options.count.is_some_and(|c| tally.received >= c);
            let lingered = last_sent.is_some_and(|t| now >= t + options.timeout);
            if all_answered || (lingered && options.deadline.is_none()) {
                break;
            }
        } else if now >= next {
            let seq = (tally.sent & 0xffff) as u16;
            answered[usize::from(seq)] = false;
            let message = request(v6, seq.wrapping_add(1), options.size, now - start);
            tally.sent += 1;
            last_sent = Some(now);
            next = now + options.interval;
            // An error about an earlier request fails the next send once,
            // as on Linux: its own, which goes again.
            let sent = match socket.send_to(&message, &to) {
                Err(e) if traceroute::left_by_icmp(&e) => {
                    tally.errors += take_errors(&socket, v6, address, options.quiet, &mut out)?;
                    socket.send_to(&message, &to)
                }
                sent => sent,
            };
            if let Err(e) = sent {
                tally.errors += 1;
                writeln!(out, "ping: sendmsg: {}", util::strip_os_error(&e))?;
            }
            continue;
        }
        let mut until = if all_sent { last_sent.unwrap_or(now) + options.timeout } else { next };
        if let Some(deadline) = options.deadline {
            until = until.min(start + deadline);
        }
        let wait = until.saturating_duration_since(now).max(Duration::from_millis(1));
        socket.set_read_timeout(Some(wait))?;
        let (len, from) = match socket.recv_from(&mut buffer) {
            Ok(got) => got,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            // An ICMP error came: it waits in the error queue.
            Err(e) => {
                let taken = take_errors(&socket, v6, address, options.quiet, &mut out)?;
                tally.errors += taken;
                if taken == 0 && !traceroute::left_by_icmp(&e) {
                    tally.errors += 1;
                    writeln!(out, "ping: recvmsg: {}", util::strip_os_error(&e))?;
                }
                continue;
            }
        };
        // SAFETY: recv_from wrote the first `len` bytes.
        let message: Vec<u8> = buffer[..len].iter().map(|b| unsafe { b.assume_init() }).collect();
        let Some((seq, sent)) = reply(v6, &message) else { continue };
        let from = from.as_socket().map_or(address, |a| a.ip());
        let index = usize::from(seq.wrapping_sub(1));
        let duplicate = answered[index];
        let ms = sent.map(|sent| (start.elapsed().saturating_sub(sent)).as_secs_f64() * 1000.0);
        if duplicate {
            tally.duplicates += 1;
        } else {
            answered[index] = true;
            tally.reply(ms.unwrap_or(0.0));
        }
        if !options.quiet {
            let who = if named && from == address {
                format!("{} ({from})", options.target)
            } else {
                from.to_string()
            };
            let time = ms.map_or_default(|ms| format!(" time={}", round_trip(ms)));
            let dup = if duplicate { " (DUP!)" } else { "" };
            writeln!(out, "{len} bytes from {who}: icmp_seq={seq}{time}{dup}")?;
            out.flush()?;
        }
    }
    write!(out, "{}", tally.summary(&options.target, start.elapsed()))?;
    out.flush()?;
    let short = options.deadline.is_some() && options.count.is_some_and(|c| tally.received < c);
    Ok(if tally.received == 0 || short { 1 } else { 0 })
}

/// What one echo request met, for netcheck(8).
pub(crate) enum Echo {
    /// The reply came after this long.
    Answered(Duration),
    /// A router or the machine itself said the request cannot go on, in
    /// ping's words: "Destination Host Unreachable".
    Refused(String),
    /// Nothing came within the time given.
    Silent,
}

/// Sends one echo request of `size` data bytes to `to` and waits `wait`
/// for its reply or the ICMP error about it. The scope of a link-local
/// IPv6 address goes in `to`.
///
/// # Errors
///
/// If the ping socket cannot be made or written to.
pub(crate) fn echo(to: SocketAddr, size: usize, wait: Duration) -> io::Result<Echo> {
    let v6 = to.is_ipv6();
    let (domain, protocol) =
        if v6 { (Domain::IPV6, Protocol::ICMPV6) } else { (Domain::IPV4, Protocol::ICMPV4) };
    let socket = Socket::new(domain, Type::DGRAM, Some(protocol))?;
    let (level, option) =
        if v6 { (libc::SOL_IPV6, libc::IPV6_RECVERR) } else { (libc::SOL_IP, libc::IP_RECVERR) };
    traceroute::set_int(&socket, level, option, 1)?;
    let started = Instant::now();
    let to = SockAddr::from(to);
    let sent = socket.send_to(&request(v6, 1, size, Duration::ZERO), &to);
    if let Err(error) = sent {
        // An ICMP error about an earlier request fails the send; with a
        // fresh socket it is this request's own, which was refused.
        return if traceroute::left_by_icmp(&error) {
            Ok(Echo::Refused(refusal(&socket, v6).unwrap_or_else(|| util::strip_os_error(&error))))
        } else {
            Err(error)
        };
    }
    let mut buffer = vec![MaybeUninit::<u8>::uninit(); HEADER + SIZE_MAX];
    loop {
        let left = wait.saturating_sub(started.elapsed()).max(Duration::from_millis(1));
        socket.set_read_timeout(Some(left))?;
        match socket.recv_from(&mut buffer) {
            Ok((len, _)) => {
                // SAFETY: recv_from wrote the first `len` bytes.
                let message: Vec<u8> =
                    buffer[..len].iter().map(|b| unsafe { b.assume_init() }).collect();
                if reply(v6, &message).is_some() {
                    return Ok(Echo::Answered(started.elapsed()));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
            {
                return Ok(Echo::Silent);
            }
            Err(error) => {
                return Ok(Echo::Refused(
                    refusal(&socket, v6).unwrap_or_else(|| util::strip_os_error(&error)),
                ));
            }
        }
        if started.elapsed() >= wait {
            return Ok(Echo::Silent);
        }
    }
}

/// The ICMP error waiting for `socket`, in ping's words.
fn refusal(socket: &Socket, v6: bool) -> Option<String> {
    let request = if v6 { REQUEST_V6 } else { REQUEST };
    while let Ok(received) = traceroute::receive(socket.as_raw_fd(), true) {
        let Some(heard) = received.heard else { continue };
        if received.data.first() == Some(&request) {
            return Some(error_text(&heard));
        }
    }
    None
}

/// A round trip's time, as iputils shows it: three figures, at most
/// three decimals.
fn round_trip(ms: f64) -> String {
    if ms >= 100.0 {
        format!("{ms:.0} ms")
    } else if ms >= 10.0 {
        format!("{ms:.1} ms")
    } else if ms >= 1.0 {
        format!("{ms:.2} ms")
    } else {
        format!("{ms:.3} ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traceroute::ORIGIN_ICMP;

    #[test]
    fn a_request_carries_its_number_and_time() {
        let message = request(false, 7, 56, Duration::from_millis(5));
        assert_eq!(message.len(), 64);
        assert_eq!(message[0], REQUEST);
        let mut answer = message.clone();
        answer[0] = REPLY;
        assert_eq!(reply(false, &answer), Some((7, Some(Duration::from_millis(5)))));
        assert_eq!(reply(true, &answer), None, "an ICMP reply on an ICMPv6 socket");
        assert_eq!(reply(false, &message), None, "a request is no reply");
        let small = request(true, 1, 0, Duration::ZERO);
        assert_eq!((small.len(), small[0]), (8, REQUEST_V6));
    }

    #[test]
    fn statistics_read_as_iputils_prints_them() {
        let mut tally = Tally { sent: 3, ..Tally::default() };
        tally.reply(1.0);
        tally.reply(3.0);
        let text = tally.summary("b.local", Duration::from_millis(2003));
        assert!(text.contains("--- b.local ping statistics ---"), "{text}");
        assert!(
            text.contains("3 packets transmitted, 2 received, 33.3333% packet loss, time 2003ms"),
            "{text}"
        );
        assert!(text.contains("rtt min/avg/max/mdev = 1.000/2.000/3.000/1.000 ms"), "{text}");
        let none = Tally { sent: 1, ..Tally::default() }.summary("x", Duration::ZERO);
        assert!(none.contains("1 packets transmitted, 0 received, 100% packet loss"), "{none}");
        assert!(!none.contains("rtt"), "{none}");
    }

    #[test]
    fn errors_read_as_iputils_prints_them() {
        let of = |origin, kind, code, info| {
            error_text(&Heard { errno: 0, origin, kind, code, info, offender: None })
        };
        assert_eq!(of(ORIGIN_ICMP, 11, 0, 0), "Time to live exceeded");
        assert_eq!(of(ORIGIN_ICMP, 3, 1, 0), "Destination Host Unreachable");
        assert_eq!(of(ORIGIN_ICMP, 3, 4, 1400), "Frag needed and DF set (mtu = 1400)");
        assert_eq!(of(ORIGIN_ICMP, 12, 0, 20), "Parameter problem: pointer = 20");
        assert_eq!(of(ORIGIN_ICMP6, 3, 0, 0), "Time exceeded: Hop limit");
        assert_eq!(of(ORIGIN_ICMP6, 1, 3, 0), "Destination unreachable: Address unreachable");
        assert_eq!(of(ORIGIN_ICMP6, 2, 0, 1280), "Packet too big: mtu=1280");
        assert_eq!(of(ORIGIN_ICMP6, 4, 1, 6), "Parameter problem: Unknown header at 6");
    }

    #[test]
    fn round_trips_have_three_figures() {
        assert_eq!(round_trip(0.0512), "0.051 ms");
        assert_eq!(round_trip(1.234), "1.23 ms");
        assert_eq!(round_trip(12.34), "12.3 ms");
        assert_eq!(round_trip(123.4), "123 ms");
    }
}
