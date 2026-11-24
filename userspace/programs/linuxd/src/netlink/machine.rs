// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's interfaces, addresses and routes, as netlink lists them.
//!
//! [`Machine`] is what `answer` encodes from; [`Hardware`] reads it from
//! the kernel's files, and the tests give a fixed one. The files are
//! Dukovo's own text: `/kernel/class/net/<interface>/` (radix-sysfs(5))
//! for the interfaces and their addresses, and `/processes/net/dev`,
//! `route` and `ipv6_route` (radix-proc(5)) for the counters and the
//! routes, in the formats of Linux's `/proc/net`. Each parser here takes
//! the text and returns what it holds, so that the formats are tested
//! without the files; a line a parser does not understand is left out.
//!
//! When an interface or a route is missing from a list, read the file it
//! comes from by hand and compare it with the parser's tests.

use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

/// IFF_BROADCAST, IFF_RUNNING and IFF_LOWER_UP, the interface flags this
/// module reads or adds.
const IFF_BROADCAST: u32 = 0x2;
const IFF_RUNNING: u32 = 0x40;
const IFF_LOWER_UP: u32 = 0x1_0000;

/// IF_OPER_UNKNOWN, IF_OPER_DOWN and IF_OPER_UP, `IFLA_OPERSTATE`.
const IF_OPER_UNKNOWN: u8 = 0;
const IF_OPER_DOWN: u8 = 2;
const IF_OPER_UP: u8 = 6;

/// IFA_F_TEMPORARY, IFA_F_DEPRECATED, IFA_F_TENTATIVE and
/// IFA_F_PERMANENT.
const IFA_F_TEMPORARY: u32 = 0x01;
const IFA_F_DEPRECATED: u32 = 0x20;
const IFA_F_TENTATIVE: u32 = 0x40;
const IFA_F_PERMANENT: u32 = 0x80;

/// RT_SCOPE_UNIVERSE, RT_SCOPE_LINK and RT_SCOPE_HOST.
const RT_SCOPE_UNIVERSE: u8 = 0;
const RT_SCOPE_LINK: u8 = 253;
const RT_SCOPE_HOST: u8 = 254;

/// RTPROT_KERNEL, RTPROT_BOOT and RTPROT_RA.
const RTPROT_KERNEL: u8 = 2;
const RTPROT_BOOT: u8 = 3;
const RTPROT_RA: u8 = 9;

/// IPv6 routes' RTF_GATEWAY and RTF_ADDRCONF, as `ipv6_route` writes
/// them.
const RTF_GATEWAY: u32 = 0x2;
const RTF_ADDRCONF: u32 = 0x4_0000;

/// The longest name an interface has, without the NUL (IFNAMSIZ - 1).
const NAME_MAX: usize = 15;

/// The counters of `/processes/net/dev`, named as Linux's
/// `struct rtnl_link_stats64` names them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames received.
    pub rx_packets: u64,
    /// Frames sent.
    pub tx_packets: u64,
    /// Bytes received.
    pub rx_bytes: u64,
    /// Bytes sent.
    pub tx_bytes: u64,
    /// Receive errors.
    pub rx_errors: u64,
    /// Send errors.
    pub tx_errors: u64,
    /// Received frames dropped.
    pub rx_dropped: u64,
    /// Frames dropped on sending.
    pub tx_dropped: u64,
    /// Multicast frames received.
    pub multicast: u64,
    /// Collisions.
    pub collisions: u64,
    /// Receive FIFO overruns.
    pub rx_fifo_errors: u64,
    /// Receive framing errors.
    pub rx_frame_errors: u64,
    /// Send FIFO underruns.
    pub tx_fifo_errors: u64,
    /// Carrier losses while sending.
    pub tx_carrier_errors: u64,
    /// Compressed frames received.
    pub rx_compressed: u64,
    /// Compressed frames sent.
    pub tx_compressed: u64,
}

impl Stats {
    /// The 24 counters of `struct rtnl_link_stats64`, in its order; the
    /// ones Radix does not count are 0.
    pub fn counters(&self) -> [u64; 24] {
        [
            self.rx_packets,
            self.tx_packets,
            self.rx_bytes,
            self.tx_bytes,
            self.rx_errors,
            self.tx_errors,
            self.rx_dropped,
            self.tx_dropped,
            self.multicast,
            self.collisions,
            0,
            0,
            0,
            self.rx_frame_errors,
            self.rx_fifo_errors,
            0,
            0,
            self.tx_carrier_errors,
            self.tx_fifo_errors,
            0,
            0,
            self.rx_compressed,
            self.tx_compressed,
            0,
        ]
    }
}

/// A network interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Its number, from 1.
    pub index: u32,
    /// Its name, at most 15 bytes.
    pub name: String,
    /// `ifi_type`: ARPHRD_ETHER, ARPHRD_LOOPBACK or ARPHRD_NONE.
    pub kind: u16,
    /// `ifi_flags`, IFF_RUNNING and IFF_LOWER_UP included while the
    /// interface is not down.
    pub flags: u32,
    /// The longest packet it sends.
    pub mtu: u32,
    /// Its hardware address; empty for an interface that has none.
    pub address: Vec<u8>,
    /// The address that reaches every card on its network; empty for an
    /// interface with no hardware address.
    pub broadcast: Vec<u8>,
    /// IFLA_OPERSTATE: 0 unknown, 2 down, 6 up.
    pub operstate: u8,
    /// What it sent and received.
    pub stats: Stats,
}

/// An address of an interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// The number of the interface it is on.
    pub index: u32,
    /// The address itself; its type says the family.
    pub address: IpAddr,
    /// The bits of the network's prefix.
    pub prefix_len: u8,
    /// RT_SCOPE_HOST for a loopback address, RT_SCOPE_LINK for a
    /// link-local one, else RT_SCOPE_UNIVERSE.
    pub scope: u8,
    /// IFA_F_ flags.
    pub flags: u32,
    /// An IPv4 address's broadcast address, where the interface has one.
    pub broadcast: Option<Ipv4Addr>,
    /// An IPv4 address's label, which is its interface's name.
    pub label: Option<String>,
    /// Seconds it is preferred for; `u32::MAX` for always.
    pub preferred: u32,
    /// Seconds it is valid for; `u32::MAX` for always.
    pub valid: u32,
}

/// A route of the main table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// The network it leads to; its type says the family.
    pub destination: IpAddr,
    /// The bits of the network's prefix; 0 for a default route.
    pub prefix_len: u8,
    /// The router it goes through, if it goes through one.
    pub gateway: Option<IpAddr>,
    /// The number of the interface it leaves by.
    pub index: u32,
    /// Its metric, where it has one.
    pub metric: Option<u32>,
    /// The address the machine sends from on it, where known.
    pub source: Option<IpAddr>,
    /// RTPROT_KERNEL, RTPROT_BOOT or RTPROT_RA.
    pub protocol: u8,
    /// RT_SCOPE_LINK for a network on the interface, else
    /// RT_SCOPE_UNIVERSE.
    pub scope: u8,
}

/// The machine's network, as it is when asked.
pub trait Machine: Send + Sync {
    /// The interfaces, by number.
    fn links(&self) -> Vec<Link>;
    /// Every interface's addresses, IPv4 and IPv6, in the order of the
    /// interfaces' files.
    fn addresses(&self) -> Vec<Address>;
    /// The routes of the main table.
    fn routes(&self) -> Vec<Route>;
}

/// The machine's network, read from the kernel's files.
pub struct Hardware {
    class: PathBuf,
    net: PathBuf,
}

impl Hardware {
    /// Reads interfaces from the folders of `class` (`/kernel/class/net`)
    /// and counters and routes from the files of `net`
    /// (`/processes/net`).
    pub fn new(class: impl Into<PathBuf>, net: impl Into<PathBuf>) -> Hardware {
        Hardware { class: class.into(), net: net.into() }
    }

    /// The text of `file` of interface `name`, if it can be read.
    fn field(&self, name: &str, file: &str) -> Option<String> {
        fs::read_to_string(self.class.join(name).join(file)).ok()
    }
}

impl Machine for Hardware {
    fn links(&self) -> Vec<Link> {
        // A link with no counters is still listed.
        let counters = read(&self.net.join("dev")).map_or_default(|text| parse_stats(&text));
        let Ok(entries) = fs::read_dir(&self.class) else {
            return Vec::new();
        };
        let mut links: Vec<Link> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter_map(|name| {
                let stats = counters.get(&name).copied().unwrap_or_default();
                parse_link(&name, |file| self.field(&name, file), stats)
            })
            .collect();
        links.sort_by_key(|link| link.index);
        links
    }

    fn addresses(&self) -> Vec<Address> {
        let mut addresses = Vec::new();
        for link in self.links() {
            let ipv4 = self.field(&link.name, "ipv4").unwrap_or_default();
            let ipv6 = self.field(&link.name, "ipv6").unwrap_or_default();
            addresses.extend(parse_ipv4(&link, &ipv4));
            addresses.extend(parse_ipv6(&link, &ipv6));
        }
        addresses
    }

    fn routes(&self) -> Vec<Route> {
        let ipv4 = read(&self.net.join("route")).unwrap_or_default();
        let ipv6 = read(&self.net.join("ipv6_route")).unwrap_or_default();
        let links = self.links();
        let addresses = self.addresses();
        let mut routes = parse_routes(&ipv4, &links, &addresses);
        routes.extend(parse_ipv6_routes(&ipv6, &links));
        routes
    }
}

/// The text of the file at `path`, if it can be read.
fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// The interface called `name`, from its files: `field` reads one. An
/// interface with no readable number is not one.
pub(super) fn parse_link(
    name: &str,
    field: impl Fn(&str) -> Option<String>,
    stats: Stats,
) -> Option<Link> {
    if name.is_empty() || name.len() > NAME_MAX || name.contains('/') {
        return None;
    }
    let index: u32 = field("ifindex")?.trim().parse().ok().filter(|&index| index > 0)?;
    let operstate = match field("operstate").as_deref().map(str::trim) {
        Some("up") => IF_OPER_UP,
        Some("down") => IF_OPER_DOWN,
        _ => IF_OPER_UNKNOWN,
    };
    let mut flags = field("flags").and_then(|text| parse_hex(&text)).unwrap_or(0);
    if operstate != IF_OPER_DOWN {
        flags |= IFF_RUNNING | IFF_LOWER_UP;
    }
    Some(Link {
        index,
        name: name.to_string(),
        kind: field("type").and_then(|text| text.trim().parse().ok()).unwrap_or(0),
        flags,
        mtu: field("mtu").and_then(|text| text.trim().parse().ok()).unwrap_or(0),
        address: field("address").map_or_default(|text| parse_mac(&text)),
        broadcast: field("broadcast").map_or_default(|text| parse_mac(&text)),
        operstate,
        stats,
    })
}

/// A number in hexadecimal, with or without `0x`.
fn parse_hex(text: &str) -> Option<u32> {
    let text = text.trim();
    u32::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).ok()
}

/// A MAC address as `52:54:00:12:34:56`; no bytes for any other text,
/// which is what a tunnel's empty file holds.
fn parse_mac(text: &str) -> Vec<u8> {
    let bytes: Option<Vec<u8>> = text
        .trim()
        .split(':')
        .map(|part| {
            let digits = part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_hexdigit());
            u8::from_str_radix(part, 16).ok().filter(|_| digits)
        })
        .collect();
    bytes.filter(|bytes| bytes.len() == 6).unwrap_or_default()
}

/// The counters of each interface in `/processes/net/dev`, by name.
pub(super) fn parse_stats(text: &str) -> HashMap<String, Stats> {
    let mut counters = HashMap::new();
    for line in text.lines() {
        // The two header lines have no `name:` that a line of numbers
        // follows.
        let Some((name, numbers)) = line.split_once(':') else {
            continue;
        };
        let numbers: Option<Vec<u64>> =
            numbers.split_whitespace().map(|number| number.parse().ok()).collect();
        let Some(
            &[
                rx_bytes,
                rx_packets,
                rx_errors,
                rx_dropped,
                rx_fifo_errors,
                rx_frame_errors,
                rx_compressed,
                multicast,
                tx_bytes,
                tx_packets,
                tx_errors,
                tx_dropped,
                tx_fifo_errors,
                collisions,
                tx_carrier_errors,
                tx_compressed,
            ],
        ) = numbers.as_deref()
        else {
            continue;
        };
        counters.insert(
            name.trim().to_string(),
            Stats {
                rx_packets,
                tx_packets,
                rx_bytes,
                tx_bytes,
                rx_errors,
                tx_errors,
                rx_dropped,
                tx_dropped,
                multicast,
                collisions,
                rx_fifo_errors,
                rx_frame_errors,
                tx_fifo_errors,
                tx_carrier_errors,
                rx_compressed,
                tx_compressed,
            },
        );
    }
    counters
}

/// The scope Linux gives an IPv4 address.
fn scope_v4(address: Ipv4Addr) -> u8 {
    if address.is_loopback() {
        RT_SCOPE_HOST
    } else if address.is_link_local() {
        RT_SCOPE_LINK
    } else {
        RT_SCOPE_UNIVERSE
    }
}

/// The scope Linux gives an IPv6 address.
fn scope_v6(address: Ipv6Addr) -> u8 {
    if address.is_loopback() {
        RT_SCOPE_HOST
    } else if address.is_unicast_link_local() {
        RT_SCOPE_LINK
    } else {
        RT_SCOPE_UNIVERSE
    }
}

/// An address and prefix as `10.0.2.15/24`.
fn parse_cidr<A: std::str::FromStr>(text: &str, bits: u8) -> Option<(A, u8)> {
    let (address, prefix) = text.split_once('/')?;
    let prefix = prefix.parse().ok().filter(|&prefix| prefix <= bits)?;
    Some((address.parse().ok()?, prefix))
}

/// The addresses of `link` in its `ipv4` file, a CIDR on each line.
pub(super) fn parse_ipv4(link: &Link, text: &str) -> Vec<Address> {
    text.lines()
        .filter_map(|line| {
            let (address, prefix_len) = parse_cidr::<Ipv4Addr>(line.trim(), 32)?;
            // Linux gives no broadcast address to a /31 or a /32, nor to
            // an interface that has none.
            let broadcast = (link.flags & IFF_BROADCAST != 0 && prefix_len < 31).then(|| {
                let mask = u32::MAX << (32 - u32::from(prefix_len));
                Ipv4Addr::from(u32::from(address) | !mask)
            });
            Some(Address {
                index: link.index,
                address: IpAddr::V4(address),
                prefix_len,
                scope: scope_v4(address),
                flags: IFA_F_PERMANENT,
                broadcast,
                label: Some(link.name.clone()),
                preferred: u32::MAX,
                valid: u32::MAX,
            })
        })
        .collect()
}

/// The addresses of `link` in its `ipv6` file: the lines that start with
/// `address`, as `address 2001:db8::5/64 temporary preferred 86000 valid
/// 172000 tentative`.
pub(super) fn parse_ipv6(link: &Link, text: &str) -> Vec<Address> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "address" {
                return None;
            }
            let (address, prefix_len) = parse_cidr::<Ipv6Addr>(words.next()?, 128)?;
            let kind = words.next()?;
            let (mut preferred, mut valid, mut tentative) = (u32::MAX, u32::MAX, false);
            while let Some(word) = words.next() {
                match word {
                    "preferred" => preferred = seconds(words.next()?)?,
                    "valid" => valid = seconds(words.next()?)?,
                    "tentative" => tentative = true,
                    _ => {}
                }
            }
            let mut flags = match kind {
                "temporary" => IFA_F_TEMPORARY,
                "link-local" | "given" => IFA_F_PERMANENT,
                _ => 0,
            };
            if tentative {
                flags |= IFA_F_TENTATIVE;
            }
            if preferred == 0 {
                flags |= IFA_F_DEPRECATED;
            }
            Some(Address {
                index: link.index,
                address: IpAddr::V6(address),
                prefix_len,
                scope: scope_v6(address),
                flags,
                broadcast: None,
                label: None,
                preferred,
                valid,
            })
        })
        .collect()
}

/// A lifetime as the `ipv6` file writes it: seconds, or `forever`.
fn seconds(word: &str) -> Option<u32> {
    if word == "forever" { Some(u32::MAX) } else { word.parse().ok() }
}

/// The number of the interface called `name`.
fn index_of(links: &[Link], name: &str) -> Option<u32> {
    links.iter().find(|link| link.name == name).map(|link| link.index)
}

/// The IPv4 routes of `/processes/net/route`, Linux's format: a header
/// line, then for each route the interface, destination, gateway, flags,
/// reference count, use, metric and mask, in hexadecimal for all but the
/// counts, an address's bytes in order as a little-endian number. A route
/// that is on an interface goes out from that interface's first address
/// in its network.
pub(super) fn parse_routes(text: &str, links: &[Link], addresses: &[Address]) -> Vec<Route> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [name, destination, gateway, _, _, _, metric, mask, ..] = fields[..] else {
                return None;
            };
            let index = index_of(links, name)?;
            let destination = hex_v4(destination)?;
            let gateway = hex_v4(gateway)?;
            let mask = u32::from(hex_v4(mask)?);
            // A netmask is ones, then zeros.
            if mask.leading_ones() + mask.trailing_zeros() != 32 {
                return None;
            }
            let prefix_len = u8::try_from(mask.leading_ones()).ok()?;
            let via = (!gateway.is_unspecified()).then_some(IpAddr::V4(gateway));
            let source = if via.is_some() {
                None
            } else {
                addresses.iter().filter(|address| address.index == index).find_map(|address| {
                    match address.address {
                        IpAddr::V4(local) if u32::from(local) & mask == u32::from(destination) => {
                            Some(IpAddr::V4(local))
                        }
                        _ => None,
                    }
                })
            };
            Some(Route {
                destination: IpAddr::V4(destination),
                prefix_len,
                gateway: via,
                index,
                metric: metric.parse().ok().filter(|&metric| metric != 0),
                source,
                protocol: if via.is_some() { RTPROT_BOOT } else { RTPROT_KERNEL },
                scope: if via.is_some() { RT_SCOPE_UNIVERSE } else { RT_SCOPE_LINK },
            })
        })
        .collect()
}

/// An IPv4 address as `/processes/net/route` writes it: eight hex digits,
/// its bytes in order as a little-endian number.
fn hex_v4(text: &str) -> Option<Ipv4Addr> {
    let number = u32::from_str_radix(text, 16).ok()?;
    Some(Ipv4Addr::from(number.to_le_bytes()))
}

/// The IPv6 routes of `/processes/net/ipv6_route`, Linux's format with no
/// header: for each route the destination (32 hex digits) and its prefix
/// length, the source and its prefix length, the next hop, the metric,
/// the reference count, the use count, the flags (each in hex) and the
/// interface.
pub(super) fn parse_ipv6_routes(text: &str, links: &[Link]) -> Vec<Route> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [destination, prefix_len, _, _, next_hop, metric, _, _, flags, name] = fields[..]
            else {
                return None;
            };
            let destination = hex_v6(destination)?;
            let prefix_len = u8::from_str_radix(prefix_len, 16).ok().filter(|&bits| bits <= 128)?;
            let next_hop = hex_v6(next_hop)?;
            let metric = u32::from_str_radix(metric, 16).ok()?;
            let flags = u32::from_str_radix(flags, 16).ok()?;
            let via = (flags & RTF_GATEWAY != 0 && !next_hop.is_unspecified())
                .then_some(IpAddr::V6(next_hop));
            let protocol = if flags & RTF_ADDRCONF != 0 {
                RTPROT_RA
            } else if via.is_some() {
                RTPROT_BOOT
            } else {
                RTPROT_KERNEL
            };
            Some(Route {
                destination: IpAddr::V6(destination),
                prefix_len,
                gateway: via,
                index: index_of(links, name)?,
                metric: Some(metric),
                source: None,
                protocol,
                scope: RT_SCOPE_UNIVERSE,
            })
        })
        .collect()
}

/// An IPv6 address as `/processes/net/ipv6_route` writes it: 32 hex
/// digits, its bytes in order.
fn hex_v6(text: &str) -> Option<Ipv6Addr> {
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(Ipv6Addr::from(bytes))
}

/// A machine with a fixed network, for the tests.
#[cfg(test)]
#[derive(Default)]
pub(super) struct Fixed {
    pub(super) links: Vec<Link>,
    pub(super) addresses: Vec<Address>,
    pub(super) routes: Vec<Route>,
}

#[cfg(test)]
impl Machine for Fixed {
    fn links(&self) -> Vec<Link> {
        self.links.clone()
    }

    fn addresses(&self) -> Vec<Address> {
        self.addresses.clone()
    }

    fn routes(&self) -> Vec<Route> {
        self.routes.clone()
    }
}

#[cfg(test)]
impl Fixed {
    /// What Dukovo shows on a machine with one card at 10.0.2.15 and a
    /// router at 10.0.2.2: `lo` and `net0`.
    pub(super) fn qemu() -> Fixed {
        let lo = parse_link("lo", lo_file, Stats::default()).unwrap();
        let net0 = parse_link("net0", net0_file, net0_stats()).unwrap();
        let mut addresses = parse_ipv4(&lo, "127.0.0.1/8\n");
        addresses
            .extend(parse_ipv6(&lo, "address ::1/128 given preferred forever valid forever\n"));
        addresses.extend(parse_ipv4(&net0, "10.0.2.15/24\n"));
        addresses.extend(parse_ipv6(
            &net0,
            "address fe80::5054:ff:fe12:3456/64 link-local preferred forever valid forever\n\
             address 2001:db8::5/64 temporary preferred 86000 valid 172000\n",
        ));
        let links = vec![lo, net0];
        let mut routes = parse_routes(ROUTE, &links, &addresses);
        routes.extend(parse_ipv6_routes(IPV6_ROUTE, &links));
        Fixed { links, addresses, routes }
    }
}

/// `/processes/net/route` of `Fixed::qemu`.
#[cfg(test)]
pub(super) const ROUTE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
net0\t0002000A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
net0\t00000000\t0202000A\t0003\t0\t0\t0\t00000000\t0\t0\t0\n";

/// `/processes/net/ipv6_route` of `Fixed::qemu`.
#[cfg(test)]
pub(super) const IPV6_ROUTE: &str = "fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000000 00000000 00000001     net0\n\
20010db8000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000000 00000000 00000001     net0\n\
00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000000 00000000 00050003     net0\n";

#[cfg(test)]
fn lo_file(file: &str) -> Option<String> {
    Some(
        match file {
            "ifindex" => "1\n",
            "address" | "broadcast" => "00:00:00:00:00:00\n",
            "mtu" => "65536\n",
            "type" => "772\n",
            "flags" => "0x9\n",
            "operstate" => "unknown\n",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
fn net0_file(file: &str) -> Option<String> {
    Some(
        match file {
            "ifindex" => "2\n",
            "address" => "52:54:00:12:34:56\n",
            "broadcast" => "ff:ff:ff:ff:ff:ff\n",
            "mtu" => "1500\n",
            "type" => "1\n",
            "flags" => "0x1003\n",
            "operstate" => "up\n",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
fn net0_stats() -> Stats {
    Stats {
        rx_bytes: 1000,
        rx_packets: 10,
        tx_bytes: 2000,
        tx_packets: 20,
        tx_dropped: 3,
        ..Stats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_read_from_its_files() {
        let net0 = parse_link("net0", net0_file, net0_stats()).unwrap();
        assert_eq!(net0.index, 2);
        assert_eq!(net0.kind, 1);
        assert_eq!(net0.mtu, 1500);
        assert_eq!(net0.address, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        assert_eq!(net0.broadcast, [0xff; 6]);
        assert_eq!(net0.operstate, IF_OPER_UP);
        // IFF_UP, IFF_BROADCAST and IFF_MULTICAST, and the two that say
        // the link is up.
        assert_eq!(net0.flags, 0x1003 | IFF_RUNNING | IFF_LOWER_UP);
    }

    #[test]
    fn a_loopback_runs_though_its_state_is_unknown() {
        let lo = parse_link("lo", lo_file, Stats::default()).unwrap();
        assert_eq!(lo.operstate, IF_OPER_UNKNOWN);
        assert_eq!(lo.flags, 0x9 | IFF_RUNNING | IFF_LOWER_UP);
        assert_eq!(lo.address, [0; 6]);
    }

    #[test]
    fn a_link_that_is_down_is_neither_running_nor_lower_up() {
        let files = |file: &str| match file {
            "operstate" => Some("down\n".to_string()),
            _ => net0_file(file),
        };
        let link = parse_link("net0", files, Stats::default()).unwrap();
        assert_eq!(link.operstate, IF_OPER_DOWN);
        assert_eq!(link.flags, 0x1003);
    }

    #[test]
    fn a_tunnel_has_no_hardware_address() {
        let files = |file: &str| match file {
            "address" | "broadcast" => Some("\n".to_string()),
            "type" => Some("65534\n".to_string()),
            _ => net0_file(file),
        };
        let link = parse_link("wg0", files, Stats::default()).unwrap();
        assert!(link.address.is_empty() && link.broadcast.is_empty());
        assert_eq!(link.kind, 65534);
    }

    #[test]
    fn what_is_not_an_interface_is_left_out() {
        assert!(parse_link("net0", |_| None, Stats::default()).is_none());
        let zero = |_: &str| Some("0\n".to_string());
        assert!(parse_link("net0", zero, Stats::default()).is_none());
        assert!(parse_link("", net0_file, Stats::default()).is_none());
        assert!(parse_link("sixteen-chars-xx", net0_file, Stats::default()).is_none());
        assert!(parse_link("a/b", net0_file, Stats::default()).is_none());
    }

    #[test]
    fn a_mac_address_has_six_bytes_of_two_digits() {
        assert_eq!(parse_mac("52:54:00:12:34:56\n"), [0x52, 0x54, 0, 0x12, 0x34, 0x56]);
        assert!(parse_mac("52:54:00:12:34\n").is_empty());
        assert!(parse_mac("52:54:0:12:34:56\n").is_empty());
        assert!(parse_mac("zz:54:00:12:34:56\n").is_empty());
        assert!(parse_mac("\n").is_empty());
    }

    #[test]
    fn counters_are_read_by_interface_name() {
        let text = "Inter-|   Receive                                                |  Transmit\n \
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    \
 lo:     100       1    0    0    0     0          0         0      200       2    0    0    0     0       0          0\n  \
net0:    1000      10    1    2    3     4          5         6     2000      20    7    8    9    10      11         12\n\
 odd: 1 2 3\n";
        let counters = parse_stats(text);
        assert_eq!(counters.len(), 2);
        assert_eq!(counters["lo"].rx_bytes, 100);
        assert_eq!(counters["lo"].tx_packets, 2);
        let net0 = counters["net0"];
        assert_eq!(net0.rx_bytes, 1000);
        assert_eq!(net0.rx_packets, 10);
        assert_eq!(net0.rx_errors, 1);
        assert_eq!(net0.rx_dropped, 2);
        assert_eq!(net0.rx_fifo_errors, 3);
        assert_eq!(net0.rx_frame_errors, 4);
        assert_eq!(net0.rx_compressed, 5);
        assert_eq!(net0.multicast, 6);
        assert_eq!(net0.tx_bytes, 2000);
        assert_eq!(net0.tx_packets, 20);
        assert_eq!(net0.tx_errors, 7);
        assert_eq!(net0.tx_dropped, 8);
        assert_eq!(net0.tx_fifo_errors, 9);
        assert_eq!(net0.collisions, 10);
        assert_eq!(net0.tx_carrier_errors, 11);
        assert_eq!(net0.tx_compressed, 12);
    }

    #[test]
    fn stats_are_laid_out_as_linuxs_are() {
        let counters = net0_stats().counters();
        assert_eq!(counters[0], 10, "rx_packets");
        assert_eq!(counters[1], 20, "tx_packets");
        assert_eq!(counters[2], 1000, "rx_bytes");
        assert_eq!(counters[3], 2000, "tx_bytes");
        assert_eq!(counters[7], 3, "tx_dropped");
    }

    #[test]
    fn an_ipv4_address_has_its_scope_and_broadcast() {
        let net0 = parse_link("net0", net0_file, Stats::default()).unwrap();
        let found = parse_ipv4(&net0, "10.0.2.15/24\n169.254.3.4/16\n192.0.2.1/32\n10.0.0.1/31\n");
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].address, IpAddr::V4(Ipv4Addr::new(10, 0, 2, 15)));
        assert_eq!(found[0].prefix_len, 24);
        assert_eq!(found[0].scope, RT_SCOPE_UNIVERSE);
        assert_eq!(found[0].broadcast, Some(Ipv4Addr::new(10, 0, 2, 255)));
        assert_eq!(found[0].label.as_deref(), Some("net0"));
        assert_eq!(found[1].scope, RT_SCOPE_LINK);
        assert_eq!(found[1].broadcast, Some(Ipv4Addr::new(169, 254, 255, 255)));
        assert_eq!(found[2].broadcast, None);
        assert_eq!(found[3].broadcast, None);
    }

    #[test]
    fn a_loopback_address_is_on_the_host_and_has_no_broadcast() {
        let lo = parse_link("lo", lo_file, Stats::default()).unwrap();
        let found = parse_ipv4(&lo, "127.0.0.1/8\n");
        assert_eq!(found[0].scope, RT_SCOPE_HOST);
        assert_eq!(found[0].broadcast, None);
    }

    #[test]
    fn what_is_not_an_address_is_left_out() {
        let net0 = parse_link("net0", net0_file, Stats::default()).unwrap();
        assert!(parse_ipv4(&net0, "10.0.2.15\n10.0.2.15/33\nfoo/8\n\n").is_empty());
        assert!(
            parse_ipv6(&net0, "router fe80::1 lifetime 1800\nmtu 1500\ndhcp managed\n").is_empty()
        );
        assert!(parse_ipv6(&net0, "address ::1/129 given\naddress nonsense\naddress\n").is_empty());
    }

    #[test]
    fn an_ipv6_address_has_what_its_line_says() {
        let net0 = parse_link("net0", net0_file, Stats::default()).unwrap();
        let text = "address fe80::1/64 link-local preferred forever valid forever\n\
                    address 2001:db8::5/64 temporary preferred 86000 valid 172000\n\
                    address 2001:db8::6/64 stable preferred 0 valid 100 tentative\n\
                    router fe80::1 lifetime 1800\n\
                    dns 2001:db8::53 lifetime 600\n";
        let found = parse_ipv6(&net0, text);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].scope, RT_SCOPE_LINK);
        assert_eq!(found[0].flags, IFA_F_PERMANENT);
        assert_eq!((found[0].preferred, found[0].valid), (u32::MAX, u32::MAX));
        assert_eq!(found[1].scope, RT_SCOPE_UNIVERSE);
        assert_eq!(found[1].flags, IFA_F_TEMPORARY);
        assert_eq!((found[1].preferred, found[1].valid), (86000, 172000));
        assert_eq!(found[2].flags, IFA_F_TENTATIVE | IFA_F_DEPRECATED);
        assert_eq!(found[2].label, None);
    }

    #[test]
    fn the_connected_route_leaves_from_the_interfaces_address() {
        let machine = Fixed::qemu();
        let routes = parse_routes(ROUTE, &machine.links, &machine.addresses);
        assert_eq!(routes.len(), 2);
        let connected = &routes[0];
        assert_eq!(connected.destination, IpAddr::V4(Ipv4Addr::new(10, 0, 2, 0)));
        assert_eq!(connected.prefix_len, 24);
        assert_eq!(connected.gateway, None);
        assert_eq!(connected.index, 2);
        assert_eq!(connected.source, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 2, 15))));
        assert_eq!((connected.protocol, connected.scope), (RTPROT_KERNEL, RT_SCOPE_LINK));
        assert_eq!(connected.metric, None);
    }

    #[test]
    fn the_default_route_goes_through_its_router() {
        let machine = Fixed::qemu();
        let routes = parse_routes(ROUTE, &machine.links, &machine.addresses);
        let default = &routes[1];
        assert_eq!(default.destination, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(default.prefix_len, 0);
        assert_eq!(default.gateway, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 2, 2))));
        assert_eq!(default.source, None);
        assert_eq!((default.protocol, default.scope), (RTPROT_BOOT, RT_SCOPE_UNIVERSE));
    }

    #[test]
    fn a_route_that_is_not_one_is_left_out() {
        let machine = Fixed::qemu();
        let bad = "Iface\tDestination\n\
                   net0\t0002000A\t00000000\t0001\t0\t0\t0\t00FF00FF\t0\t0\t0\n\
                   gone0\t0002000A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
                   net0\tzzzz\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
                   net0\t0002000A\t00000000\t0001\n";
        assert!(parse_routes(bad, &machine.links, &machine.addresses).is_empty());
        assert!(parse_ipv6_routes("not a route\n1 2 3\n", &machine.links).is_empty());
    }

    #[test]
    fn ipv6_routes_say_whether_a_router_advertised_them() {
        let machine = Fixed::qemu();
        let routes = parse_ipv6_routes(IPV6_ROUTE, &machine.links);
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].destination, "fe80::".parse::<IpAddr>().unwrap());
        assert_eq!(routes[0].prefix_len, 64);
        assert_eq!(routes[0].metric, Some(256));
        assert_eq!((routes[0].protocol, routes[0].gateway), (RTPROT_KERNEL, None));
        let default = &routes[2];
        assert_eq!(default.prefix_len, 0);
        assert_eq!(default.metric, Some(1024));
        assert_eq!(default.gateway, Some("fe80::1".parse().unwrap()));
        assert_eq!(default.protocol, RTPROT_RA);
        assert_eq!(default.index, 2);
    }

    /// A folder of its own under the system's, removed with it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("linuxd-netlink-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // The folder is in the temporary folder; a leftover does no
            // harm, so a failure to remove it is not worth a panic.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_kernels_files_make_the_same_machine_as_the_fixed_one() {
        let scratch = Scratch::new("hardware");
        let class = scratch.0.join("class");
        let net = scratch.0.join("net");
        fs::create_dir_all(&net).unwrap();
        for (name, files) in [("lo", lo_file as fn(&str) -> Option<String>), ("net0", net0_file)] {
            let folder = class.join(name);
            fs::create_dir_all(&folder).unwrap();
            for file in ["ifindex", "address", "broadcast", "mtu", "type", "flags", "operstate"] {
                fs::write(folder.join(file), files(file).unwrap()).unwrap();
            }
        }
        fs::write(class.join("lo/ipv4"), "127.0.0.1/8\n").unwrap();
        fs::write(class.join("lo/ipv6"), "address ::1/128 given preferred forever valid forever\n")
            .unwrap();
        fs::write(class.join("net0/ipv4"), "10.0.2.15/24\n").unwrap();
        fs::write(
            class.join("net0/ipv6"),
            "address fe80::5054:ff:fe12:3456/64 link-local preferred forever valid forever\n\
             address 2001:db8::5/64 temporary preferred 86000 valid 172000\n\
             router fe80::1 lifetime 1800\n",
        )
        .unwrap();
        // A file in the folder of interfaces is not one.
        fs::write(class.join("bonding_masters"), "").unwrap();
        fs::write(
            net.join("dev"),
            "Inter-|   Receive |  Transmit\n face |bytes\n \
 net0: 1000 10 0 0 0 0 0 0 2000 20 0 3 0 0 0 0\n",
        )
        .unwrap();
        fs::write(net.join("route"), ROUTE).unwrap();
        fs::write(net.join("ipv6_route"), IPV6_ROUTE).unwrap();

        let hardware = Hardware::new(&class, &net);
        let fixed = Fixed::qemu();
        assert_eq!(hardware.links(), fixed.links);
        assert_eq!(hardware.addresses(), fixed.addresses);
        assert_eq!(hardware.routes(), fixed.routes);
    }

    #[test]
    fn a_machine_with_no_files_has_nothing_to_list() {
        let hardware = Hardware::new("/nonexistent/class", "/nonexistent/net");
        assert!(hardware.links().is_empty());
        assert!(hardware.addresses().is_empty());
        assert!(hardware.routes().is_empty());
    }
}
