// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Whom admind answers (ADR 0514). The firewall opens a running service's
//! port to everyone (ADR 0334), so admind checks each connection's
//! address as it accepts it, before a byte of TLS: with `reach =
//! "private"`, the default, it answers this machine, its own cards'
//! networks, and the address ranges no one reaches from the internet:
//! loopback, link-local, RFC 1918's, RFC 6598's shared range (where
//! Tailscale and Headscale put machines) and RFC 4193's unique local
//! IPv6 addresses. An IPv4 address carried in IPv6 is taken as itself.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::admin_settings::Reach;

/// A network of one of the machine's cards: an address and its prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    pub ip: IpAddr,
    pub prefix: u8,
}

impl Network {
    /// Whether `ip` is in it.
    pub fn holds(&self, ip: IpAddr) -> bool {
        match (self.ip, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let bits = u32::from(self.prefix.min(32));
                let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let bits = u32::from(self.prefix.min(128));
                let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// `ip` as itself: an IPv4 address carried in IPv6 as its IPv4.
pub fn plain(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Whether `ip` is in a range no one reaches from the internet.
pub(super) fn private_range(ip: IpAddr) -> bool {
    match plain(ip) {
        IpAddr::V4(v4) => {
            let shared = Network { ip: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0)), prefix: 10 };
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || shared.holds(v4.into())
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6 == Ipv6Addr::LOCALHOST || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

/// Whether admind answers a connection from `ip`, with `reach`, on a
/// machine whose cards are on the networks `own`.
pub fn answers(reach: Reach, ip: IpAddr, own: &[Network]) -> bool {
    match reach {
        Reach::Any => true,
        Reach::Private => {
            let ip = plain(ip);
            private_range(ip) || own.iter().any(|network| network.holds(ip))
        }
    }
}

/// The networks of the machine's cards now.
pub fn own_networks() -> Vec<Network> {
    let cards = if_addrs::get_if_addrs().unwrap_or_default();
    cards
        .into_iter()
        .map(|card| match card.addr {
            if_addrs::IfAddr::V4(v4) => Network { ip: IpAddr::V4(v4.ip), prefix: v4.prefixlen },
            if_addrs::IfAddr::V6(v6) => Network { ip: IpAddr::V6(v6.ip), prefix: v6.prefixlen },
        })
        .collect()
}

/// The name of the card that holds `ip`, if one does.
pub fn card_of(ip: IpAddr) -> Option<String> {
    let ip = plain(ip);
    let cards = if_addrs::get_if_addrs().ok()?;
    cards.into_iter().find(|card| plain(card.ip()) == ip).map(|card| card.name)
}

/// The address admind counts a connection's limits by: an IPv4 address,
/// or an IPv6 address's /64, which one machine usually holds whole.
pub fn counted(ip: IpAddr) -> IpAddr {
    match plain(ip) {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn private_networks_are_answered_and_the_internet_is_not() {
        let own = [Network { ip: ip("203.0.113.10"), prefix: 24 }];
        for near in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.25",
            "169.254.3.4",
            "100.64.0.1",
            "100.127.255.254",
            "::1",
            "fd00::5",
            "fc12::1",
            "fe80::1",
            "::ffff:192.168.1.25",
            "203.0.113.99",
            "::ffff:203.0.113.200",
        ] {
            assert!(answers(Reach::Private, ip(near), &own), "{near}");
        }
        for far in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "192.169.0.1",
            "203.0.114.1",
            "2001:db8::1",
            "::ffff:8.8.8.8",
            "fec0::1",
        ] {
            assert!(!answers(Reach::Private, ip(far), &own), "{far}");
            assert!(answers(Reach::Any, ip(far), &own), "{far}");
        }
    }

    #[test]
    fn networks_hold_their_addresses() {
        let v6 = Network { ip: ip("2001:db8:1:2::10"), prefix: 64 };
        assert!(v6.holds(ip("2001:db8:1:2:ffff::1")));
        assert!(!v6.holds(ip("2001:db8:1:3::1")));
        assert!(!v6.holds(ip("10.0.0.1")));
        assert!(Network { ip: ip("10.0.0.1"), prefix: 0 }.holds(ip("8.8.8.8")));
        assert!(Network { ip: ip("10.0.0.1"), prefix: 32 }.holds(ip("10.0.0.1")));
        assert!(!Network { ip: ip("10.0.0.1"), prefix: 32 }.holds(ip("10.0.0.2")));
    }

    #[test]
    fn a_machine_is_counted_by_its_ipv4_address_or_its_ipv6_64() {
        assert_eq!(counted(ip("192.0.2.7")), ip("192.0.2.7"));
        assert_eq!(counted(ip("::ffff:192.0.2.7")), ip("192.0.2.7"));
        assert_eq!(counted(ip("2001:db8:1:2:3:4:5:6")), ip("2001:db8:1:2::"));
    }
}
