// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The addresses the firewall blocks for a time (ADR 0479): those authd
//! saw guess passwords. A block stops what its address starts, before
//! any rule is asked: new connections and datagrams to the ports it
//! names, or, when it names none, everything, pings too.
//!
//! An address is an IPv4 address, or an IPv6 network of 64 bits, since
//! one machine holds a whole /64. The list is kept in the order of its
//! addresses, which [`Blocks::set`] takes it in, so that [`Blocks::stops`]
//! finds a packet's by a binary search: the IPv4 addresses first, as
//! numbers, then the IPv6 networks.

use smoltcp::wire::IpAddress;

use crate::filter::{Inbound, Protocol};

/// The most addresses the firewall blocks at once.
pub const BLOCKS_MAX: usize = 1024;

/// The most ranges of ports a block names.
pub const BLOCK_PORTS: usize = 4;

/// The bits of an IPv6 address that name its network: the first 64.
const NETWORK_V6: u128 = !((1 << 64) - 1);

/// An address blocked until a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// An IPv4 address, or an IPv6 network, whose last 64 bits are zero.
    pub from: IpAddress,
    /// When it ends, on the clock the firewall is asked by.
    pub until_ns: u64,
    /// The ranges of ports it closes, the first and the last of each:
    /// the first `port_count` of them.
    pub ports: [(u16, u16); BLOCK_PORTS],
    /// How many of `ports` it names: none closes every port, and stops
    /// pings too.
    pub port_count: u8,
}

/// Why [`Blocks::set`] refused a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// More than [`BLOCKS_MAX`].
    TooMany,
    /// A block that is not one: an IPv6 address with any of its last 64
    /// bits set, more than [`BLOCK_PORTS`] ranges, or a range whose first
    /// port is past its last.
    NotABlock,
    /// Addresses out of order, or one twice.
    OutOfOrder,
}

/// Where `address` goes in the list: its version, then its bits, those of
/// an IPv6 address's network alone.
fn place(address: IpAddress) -> (u8, u128) {
    match address {
        IpAddress::Ipv4(address) => (4, u128::from(u32::from(address))),
        IpAddress::Ipv6(address) => (6, u128::from(address) & NETWORK_V6),
    }
}

impl Block {
    /// Whether it is one: see [`Refused::NotABlock`].
    fn is_one(&self) -> bool {
        let network = match self.from {
            IpAddress::Ipv4(_) => true,
            IpAddress::Ipv6(address) => u128::from(address) & !NETWORK_V6 == 0,
        };
        let ports = usize::from(self.port_count) <= BLOCK_PORTS
            && self.ports[..usize::from(self.port_count)].iter().all(|(first, last)| first <= last);
        network && ports
    }

    /// Whether it closes what `packet` starts.
    fn closes(&self, packet: &Inbound) -> bool {
        if self.port_count == 0 {
            return true;
        }
        match packet.protocol {
            Protocol::Ping => false,
            Protocol::Tcp | Protocol::Udp => self.ports[..usize::from(self.port_count)]
                .iter()
                .any(|&(first, last)| (first..=last).contains(&packet.port)),
        }
    }
}

/// The blocked addresses.
pub struct Blocks {
    held: [Option<Block>; BLOCKS_MAX],
    count: usize,
}

impl Blocks {
    /// None.
    // The list is large, larger than a kernel stack: the kernel's is built
    // at compile time, as a static, and only tests build one at run time.
    #[allow(clippy::large_stack_arrays)]
    pub const fn new() -> Blocks {
        Blocks { held: [None; BLOCKS_MAX], count: 0 }
    }

    /// Blocks `blocks`, in place of those it had, if each is one and they
    /// are in order: by version, IPv4 first, then by address.
    ///
    /// # Errors
    ///
    /// Why it refused them, when it did; it then keeps those it had.
    pub fn set(&mut self, blocks: &[Block]) -> Result<(), Refused> {
        if blocks.len() > BLOCKS_MAX {
            return Err(Refused::TooMany);
        }
        if !blocks.iter().all(Block::is_one) {
            return Err(Refused::NotABlock);
        }
        if !blocks.windows(2).all(|pair| place(pair[0].from) < place(pair[1].from)) {
            return Err(Refused::OutOfOrder);
        }
        // A place at a time: the list is too large for a stack frame.
        for (at, held) in self.held.iter_mut().enumerate() {
            *held = blocks.get(at).copied();
        }
        self.count = blocks.len();
        Ok(())
    }

    /// How many addresses are blocked, counting those whose time is up.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The block in place `at`, in order, if there is one.
    pub fn get(&self, at: usize) -> Option<Block> {
        self.held.get(at).copied().flatten()
    }

    /// Whether a block stops `packet` at `now_ns`: one of its address,
    /// whose time is not up, that closes what it starts.
    pub fn stops(&self, packet: &Inbound, now_ns: u64) -> bool {
        let wanted = place(packet.from);
        let held = &self.held[..self.count];
        let found = held.binary_search_by(|block| {
            // Every place up to the count holds a block.
            block.map_or((0, 0), |block| place(block.from)).cmp(&wanted)
        });
        found
            .ok()
            .and_then(|at| held[at])
            .is_some_and(|block| now_ns < block.until_ns && block.closes(packet))
    }
}

impl Default for Blocks {
    fn default() -> Blocks {
        Blocks::new()
    }
}

#[cfg(test)]
mod tests {
    use smoltcp::wire::{Ipv4Address, Ipv6Address};

    use super::*;

    const SECOND: u64 = 1_000_000_000;

    fn v4(address: [u8; 4]) -> IpAddress {
        IpAddress::Ipv4(Ipv4Address::from(address))
    }

    fn v6(address: u128) -> IpAddress {
        IpAddress::Ipv6(Ipv6Address::from(address))
    }

    fn block(from: IpAddress, until_ns: u64, ports: &[(u16, u16)]) -> Block {
        let mut held = [(0, 0); BLOCK_PORTS];
        held[..ports.len()].copy_from_slice(ports);
        Block { from, until_ns, ports: held, port_count: ports.len() as u8 }
    }

    fn packet(protocol: Protocol, from: IpAddress, port: u16) -> Inbound {
        Inbound { protocol, from, from_port: 40000, to: v4([192, 0, 2, 1]), port }
    }

    #[test]
    fn a_block_stops_its_address_until_its_time_is_up() {
        let mut blocks = Blocks::new();
        let guesser = v4([203, 0, 113, 9]);
        blocks.set(&[block(guesser, 10 * SECOND, &[])]).unwrap();
        assert!(blocks.stops(&packet(Protocol::Tcp, guesser, 22), SECOND));
        assert!(blocks.stops(&packet(Protocol::Ping, guesser, 0), SECOND));
        // Not another address, and not once its time is up.
        assert!(!blocks.stops(&packet(Protocol::Tcp, v4([203, 0, 113, 10]), 22), SECOND));
        assert!(!blocks.stops(&packet(Protocol::Tcp, guesser, 22), 10 * SECOND));
    }

    #[test]
    fn a_block_of_ports_closes_those_alone_and_lets_pings_in() {
        let mut blocks = Blocks::new();
        let guesser = v4([203, 0, 113, 9]);
        blocks.set(&[block(guesser, 10 * SECOND, &[(22, 22), (9090, 9090), (137, 139)])]).unwrap();
        for port in [22, 9090, 137, 138, 139] {
            assert!(blocks.stops(&packet(Protocol::Tcp, guesser, port), 0), "{port}");
            assert!(blocks.stops(&packet(Protocol::Udp, guesser, port), 0), "{port}");
        }
        for port in [21, 23, 80, 443, 136, 140] {
            assert!(!blocks.stops(&packet(Protocol::Tcp, guesser, port), 0), "{port}");
        }
        assert!(!blocks.stops(&packet(Protocol::Ping, guesser, 0), 0));
    }

    #[test]
    fn an_ipv6_block_is_of_the_whole_network_of_64_bits() {
        let mut blocks = Blocks::new();
        let network = 0x2001_0db8_0004_0007_u128 << 64;
        blocks.set(&[block(v6(network), 10 * SECOND, &[])]).unwrap();
        for host in [1, 0xffff, 0x1234_5678_9abc_def0] {
            assert!(blocks.stops(&packet(Protocol::Tcp, v6(network | host), 22), 0));
        }
        let next = network + (1 << 64);
        assert!(!blocks.stops(&packet(Protocol::Tcp, v6(next | 1), 22), 0));
        // A block names the network, with its host bits zero.
        let host = block(v6(network | 1), 10 * SECOND, &[]);
        assert_eq!(blocks.set(&[host]), Err(Refused::NotABlock));
    }

    #[test]
    fn many_blocks_are_found_among_each_other() {
        let mut list = Vec::new();
        for n in 0..500_u32 {
            list.push(block(IpAddress::Ipv4(Ipv4Address::from(0x0a00_0000 + n * 3)), SECOND, &[]));
        }
        for n in 0..500_u128 {
            list.push(block(v6((0x2001_0db8_0000_0000 + n * 5) << 64), SECOND, &[]));
        }
        let mut blocks = Blocks::new();
        blocks.set(&list).unwrap();
        assert_eq!(blocks.len(), 1000);
        for n in 0..1500_u32 {
            let ipv4 = IpAddress::Ipv4(Ipv4Address::from(0x0a00_0000 + n));
            assert_eq!(blocks.stops(&packet(Protocol::Tcp, ipv4, 22), 0), n % 3 == 0);
        }
        for n in 0..2500_u128 {
            let ipv6 = v6(((0x2001_0db8_0000_0000 + n) << 64) | 7);
            assert_eq!(blocks.stops(&packet(Protocol::Tcp, ipv6, 22), 0), n % 5 == 0);
        }
    }

    #[test]
    fn a_list_out_of_order_or_too_long_is_refused_and_the_old_one_kept() {
        let mut blocks = Blocks::new();
        let (a, b) = (v4([192, 0, 2, 7]), v4([198, 51, 100, 7]));
        blocks.set(&[block(a, SECOND, &[])]).unwrap();
        let ipv6 = v6(0x2001_0db8_u128 << 96);
        assert_eq!(
            blocks.set(&[block(b, SECOND, &[]), block(a, SECOND, &[])]),
            Err(Refused::OutOfOrder)
        );
        assert_eq!(
            blocks.set(&[block(ipv6, SECOND, &[]), block(a, SECOND, &[])]),
            Err(Refused::OutOfOrder)
        );
        assert_eq!(
            blocks.set(&[block(a, SECOND, &[]), block(a, 2 * SECOND, &[])]),
            Err(Refused::OutOfOrder)
        );
        let too_many = vec![block(a, SECOND, &[]); BLOCKS_MAX + 1];
        assert_eq!(blocks.set(&too_many), Err(Refused::TooMany));
        let backwards = block(b, SECOND, &[(23, 22)]);
        assert_eq!(blocks.set(&[backwards]), Err(Refused::NotABlock));
        let mut five = block(b, SECOND, &[]);
        five.port_count = 5;
        assert_eq!(blocks.set(&[five]), Err(Refused::NotABlock));
        // The block from before stands.
        assert_eq!(blocks.len(), 1);
        assert!(blocks.stops(&packet(Protocol::Tcp, a, 22), 0));
        // An empty list lifts every block.
        blocks.set(&[]).unwrap();
        assert!(blocks.is_empty());
        assert!(!blocks.stops(&packet(Protocol::Tcp, a, 22), 0));
    }
}
