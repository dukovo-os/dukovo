// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! When a service that says `ready.when = "listening"` is ready (ADR
//! 0470): once the kernel's socket table, `/processes/net/tcp` or `tcp6`
//! (radix-proc(5)), lists a socket listening on one of its ports. The
//! table is read as ss(8) reads it: a header line, then a line a socket,
//! the second field `ADDRESS:PORT` with the port in hexadecimal and the
//! fourth the state, 0A for LISTEN.
//!
//! The table does not say whose a socket is. A service listens only on
//! ports its file grants it (ADR 0183), so a socket on one of them is
//! taken as its own.

/// The state of a listening socket in the table.
const LISTEN: u8 = 0x0a;

/// Whether the socket table `table` lists a socket listening on a port
/// in `ports`, ranges as (first, count).
pub fn listening(table: &str, ports: &[(u32, u32)]) -> bool {
    table.lines().skip(1).filter_map(listening_port).any(|port| {
        let port = u32::from(port);
        ports.iter().any(|&(first, count)| port >= first && port - first < count)
    })
}

/// The port of the socket on `line` of the table, if it listens.
fn listening_port(line: &str) -> Option<u16> {
    let mut fields = line.split_whitespace();
    let local = fields.nth(1)?;
    let state = fields.nth(1)?;
    if u8::from_str_radix(state, 16).ok()? != LISTEN {
        return None;
    }
    let (_, port) = local.rsplit_once(':')?;
    u16::from_str_radix(port, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As Radix writes the table: sshd and a web server listening, a
    /// connection to sshd, and a socket on 8080 closing.
    const TCP: &str = "  sl  local_address rem_address   st tx_queue:rx_queue
   0: 00000000:0016 00000000:0000 0A 00000000:00000000
   1: 00000000:0050 00000000:0000 0A 00000000:00000000
   2: 0F02000A:0016 0202000A:D431 01 00000000:00000000
   3: 0100007F:1F90 0100007F:9C40 08 00000000:00000000
";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue:rx_queue
   0: 00000000000000000000000000000000:1538 00000000000000000000000000000000:0000 0A 00000000:00000000
";

    #[test]
    fn a_listening_socket_on_the_port_is_found() {
        assert!(listening(TCP, &[(80, 1)]));
        assert!(listening(TCP6, &[(5432, 1)]));
    }

    #[test]
    fn other_ports_and_states_are_not() {
        // 8080's socket is closing, and 0x9C40 is a connection's far end.
        assert!(!listening(TCP, &[(8080, 1)]));
        assert!(!listening(TCP, &[(40000, 1)]));
        assert!(!listening(TCP6, &[(80, 1)]));
        assert!(!listening("", &[(80, 1)]));
    }

    #[test]
    fn a_range_holds_its_ports_and_no_more() {
        assert!(listening(TCP, &[(8000, 9), (79, 2)]));
        assert!(!listening(TCP, &[(81, 100)]));
        assert!(!listening(TCP, &[(70, 10)]));
    }

    #[test]
    fn the_header_and_broken_lines_are_skipped() {
        let table = "header 00000000:0050 x 0A\nshort\n 0: 00000000:zz 00000000:0000 0A\n";
        assert!(!listening(table, &[(80, 1)]));
    }
}
