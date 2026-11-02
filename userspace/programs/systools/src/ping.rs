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
//! many as were sent), 1 if not, 2 for another failure.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::args::{Arg, Args};
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
            if let Err(e) = socket.send_to(&message, &to) {
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
            Err(e) => {
                tally.errors += 1;
                writeln!(out, "ping: recvmsg: {}", util::strip_os_error(&e))?;
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
    fn round_trips_have_three_figures() {
        assert_eq!(round_trip(0.0512), "0.051 ms");
        assert_eq!(round_trip(1.234), "1.23 ms");
        assert_eq!(round_trip(12.34), "12.3 ms");
        assert_eq!(round_trip(123.4), "123 ms");
    }
}
