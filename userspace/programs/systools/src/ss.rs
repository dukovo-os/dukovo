// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ss(8): the TCP and UDP sockets, from `/processes/net/tcp` and `udp`,
//! and `tcp6` and `udp6` for IPv6 ones (radix-proc(5), ADR 0332), shown
//! as iproute2's ss shows them (ADR 0190): the connected ones, or with
//! `-l` those that listen and the UDP ones that are bound, or with `-a`
//! all of them; with `-4` or `-6`, of that version alone. Addresses are
//! always numbers, as `-n` shows them.

use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use crate::args::{Arg, Args};
use crate::util::{Fail, fail};

/// Where the kernel lists the sockets.
const TCP: &str = "/processes/net/tcp";
const UDP: &str = "/processes/net/udp";
const TCP6: &str = "/processes/net/tcp6";
const UDP6: &str = "/processes/net/udp6";

/// A socket, as the kernel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Socket {
    tcp: bool,
    /// Linux's number for its state: 0x0a is listening, 0x07 closed (a
    /// UDP socket's, unless it is connected).
    state: u8,
    local: (IpAddr, u16),
    remote: (IpAddr, u16),
    send_queue: u32,
    receive_queue: u32,
}

const LISTEN: u8 = 0x0a;
const CLOSED: u8 = 0x07;
const SYN_RECEIVED: u8 = 0x03;
const TIME_WAIT: u8 = 0x06;

/// A state as ss(8) names it.
fn state_name(state: u8) -> &'static str {
    match state {
        0x01 => "ESTAB",
        0x02 => "SYN-SENT",
        SYN_RECEIVED => "SYN-RECV",
        0x04 => "FIN-WAIT-1",
        0x05 => "FIN-WAIT-2",
        TIME_WAIT => "TIME-WAIT",
        CLOSED => "UNCONN",
        0x08 => "CLOSE-WAIT",
        0x09 => "LAST-ACK",
        LISTEN => "LISTEN",
        0x0b => "CLOSING",
        _ => "UNKNOWN",
    }
}

/// `0F02000A:0050`: an address's bytes, in network order, read as a
/// little-endian number, and a port; an IPv6 one's in four such numbers.
fn endpoint(text: &str) -> Option<(IpAddr, u16)> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |at: usize| {
        let word = address.get(at * 8..at * 8 + 8)?;
        u32::from_str_radix(word, 16).ok().map(u32::to_le_bytes)
    };
    match address.len() {
        8 => Some((IpAddr::V4(Ipv4Addr::from(word(0)?)), port)),
        32 => {
            let mut octets = [0; 16];
            for at in 0..4 {
                octets[at * 4..at * 4 + 4].copy_from_slice(&word(at)?);
            }
            Some((IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        _ => None,
    }
}

/// The sockets the file at `path` lists, of TCP if `tcp`.
fn read_sockets(path: &Path, tcp: bool) -> io::Result<Vec<Socket>> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (send_queue, receive_queue) = fields.get(4)?.split_once(':')?;
            Some(Socket {
                tcp,
                local: endpoint(fields.get(1)?)?,
                remote: endpoint(fields.get(2)?)?,
                state: u8::from_str_radix(fields.get(3)?, 16).ok()?,
                send_queue: u32::from_str_radix(send_queue, 16).ok()?,
                receive_queue: u32::from_str_radix(receive_queue, 16).ok()?,
            })
        })
        .collect())
}

/// Which sockets ss(8) shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    /// The connected ones, as ss(8) shows by default: neither listening
    /// nor closed, nor ending in TIME-WAIT or half open in SYN-RECV.
    Connected,
    Listening,
    All,
}

impl Which {
    fn shows(self, socket: &Socket) -> bool {
        let listening = socket.state == LISTEN || socket.state == CLOSED;
        match self {
            Which::Connected => {
                !listening && socket.state != TIME_WAIT && socket.state != SYN_RECEIVED
            }
            Which::Listening => listening,
            Which::All => true,
        }
    }
}

/// `address:port`, or `address:*` for none; an IPv6 address in
/// brackets, `[::1]:22`, as ss(8) has it.
fn place((address, port): (IpAddr, u16)) -> (String, String) {
    let address = match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => format!("[{address}]"),
    };
    (address, if port == 0 { "*".to_string() } else { port.to_string() })
}

/// Writes `sockets` as ss(8) does: a column of each one's protocol when
/// there are both, its state, its queues and its two ends, each as wide
/// as its widest.
fn show(sockets: &[Socket], both: bool, header: bool, out: &mut dyn Write) -> io::Result<()> {
    let mut rows = Vec::new();
    if header {
        rows.push([
            "Netid".to_string(),
            "State".to_string(),
            "Recv-Q".to_string(),
            "Send-Q".to_string(),
            "Local Address".to_string(),
            "Port".to_string(),
            "Peer Address".to_string(),
            "Port".to_string(),
        ]);
    }
    for socket in sockets {
        let (local, local_port) = place(socket.local);
        let (remote, remote_port) = place(socket.remote);
        rows.push([
            if socket.tcp { "tcp" } else { "udp" }.to_string(),
            state_name(socket.state).to_string(),
            socket.receive_queue.to_string(),
            socket.send_queue.to_string(),
            local,
            local_port,
            remote,
            remote_port,
        ]);
    }
    let mut widths = [0; 8];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    for row in &rows {
        let mut line = String::new();
        if both {
            line += &format!("{:<w$} ", row[0], w = widths[0]);
        }
        line += &format!(
            "{:<w1$} {:<w2$} {:<w3$} {:>w4$}:{:<w5$} {:>w6$}:{}",
            row[1],
            row[2],
            row[3],
            row[4],
            row[5],
            row[6],
            row[7],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
            w5 = widths[5],
            w6 = widths[6],
        );
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(())
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut tcp, mut udp, mut header) = (false, false, true);
    let (mut v4, mut v6) = (false, false);
    let mut which = Which::Connected;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('t') => tcp = true,
            Arg::Short('u') => udp = true,
            Arg::Short('l') => which = Which::Listening,
            Arg::Short('a') => which = Which::All,
            Arg::Short('H') => header = false,
            // Numbers are all it shows.
            Arg::Short('n') => {}
            Arg::Short('4') => v4 = true,
            Arg::Short('6') => v6 = true,
            Arg::Long(name) => match name.as_str() {
                "tcp" => tcp = true,
                "udp" => udp = true,
                "listening" => which = Which::Listening,
                "all" => which = Which::All,
                "no-header" => header = false,
                "numeric" => {}
                "ipv4" => v4 = true,
                "ipv6" => v6 = true,
                _ => return Err(args.unknown(&Arg::Long(name))),
            },
            other => return Err(args.unknown(&other)),
        }
    }
    if !tcp && !udp {
        (tcp, udp) = (true, true);
    }
    if !v4 && !v6 {
        (v4, v6) = (true, true);
    }
    let mut sockets = Vec::new();
    let lists = [
        (tcp && v4, TCP, true),
        (tcp && v6, TCP6, true),
        (udp && v4, UDP, false),
        (udp && v6, UDP6, false),
    ];
    for (wanted, path, is_tcp) in lists {
        if wanted {
            let listed = read_sockets(Path::new(path), is_tcp)
                .map_err(|error| fail(format!("{path}: {error}")))?;
            sockets.extend(listed.into_iter().filter(|socket| which.shows(socket)));
        }
    }
    show(&sockets, tcp && udp, header, &mut io::stdout().lock())?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A listening socket, a connection to it, and a bound UDP socket,
    /// as the kernel lists them.
    const TCP_LIST: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 0
   1: 0F02000A:0016 0202000A:D431 01 00000024:00000000 00:00000000 00000000     0        0 0
   2: 0F02000A:C000 0202000A:0050 06 00000000:00000000 00:00000000 00000000     0        0 0
";
    const UDP_LIST: &str = "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
    0: 00000000:0044 00000000:0000 07 00000000:00000200 00:00000000 00000000     0        0 0
";
    /// An IPv6 listening socket, and a connection to it from ::1.
    const TCP6_LIST: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 0
   1: 00000000000000000000000001000000:0016 00000000000000000000000001000000:C350 01 00000000:00000000 00:00000000 00000000     0        0 0
";

    fn sockets() -> Vec<Socket> {
        // A directory for each call: the tests run at once.
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ss-test-{}-{call}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tcp"), TCP_LIST).unwrap();
        fs::write(dir.join("udp"), UDP_LIST).unwrap();
        let mut sockets = read_sockets(&dir.join("tcp"), true).unwrap();
        sockets.extend(read_sockets(&dir.join("udp"), false).unwrap());
        sockets
    }

    fn shown(which: Which, both: bool) -> String {
        let shown: Vec<Socket> = sockets().into_iter().filter(|s| which.shows(s)).collect();
        let mut out = Vec::new();
        show(&shown, both, true, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn sockets_are_read_as_the_kernel_lists_them() {
        let sockets = sockets();
        assert_eq!(sockets.len(), 4);
        assert_eq!(sockets[1].local, (IpAddr::V4(Ipv4Addr::new(10, 0, 2, 15)), 22));
        assert_eq!(sockets[1].remote, (IpAddr::V4(Ipv4Addr::new(10, 0, 2, 2)), 54321));
        assert_eq!((sockets[1].state, sockets[1].send_queue), (1, 36));
        assert_eq!((sockets[3].tcp, sockets[3].receive_queue), (false, 512));
    }

    #[test]
    fn ipv6_sockets_are_read_and_shown_in_brackets() {
        let dir = std::env::temp_dir().join(format!("ss-test-{}-v6", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tcp6"), TCP6_LIST).unwrap();
        let sockets = read_sockets(&dir.join("tcp6"), true).unwrap();
        assert_eq!(sockets[1].local, (IpAddr::V6(Ipv6Addr::LOCALHOST), 22));
        assert_eq!(sockets[1].remote, (IpAddr::V6(Ipv6Addr::LOCALHOST), 50000));
        let mut out = Vec::new();
        show(&sockets, false, true, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "State  Recv-Q Send-Q Local Address:Port Peer Address:Port\n\
             LISTEN 0      0               [::]:22           [::]:*\n\
             ESTAB  0      0              [::1]:22          [::1]:50000\n"
        );
    }

    #[test]
    fn connections_are_shown_by_default() {
        assert_eq!(
            shown(Which::Connected, false),
            "State Recv-Q Send-Q Local Address:Port Peer Address:Port\n\
             ESTAB 0      36         10.0.2.15:22       10.0.2.2:54321\n"
        );
    }

    #[test]
    fn listening_sockets_and_bound_ones_are_shown_with_l() {
        assert_eq!(
            shown(Which::Listening, true),
            "Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port\n\
             tcp   LISTEN 0      0            0.0.0.0:22        0.0.0.0:*\n\
             udp   UNCONN 512    0            0.0.0.0:68        0.0.0.0:*\n"
        );
        assert_eq!(shown(Which::All, true).lines().count(), 5);
        assert!(shown(Which::All, true).contains("TIME-WAIT"));
    }
}
