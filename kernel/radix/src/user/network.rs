// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The methods of the network capability (Phase 12, radix-network(2),
//! ADR 0183): making network sockets, deriving a capability for fewer
//! ports or rights, as Spark does for each service, and with the control
//! right setting the interfaces up (ADR 0190), opening tunnels for the
//! programs that carry their packets (ADR 0339), and opening packet
//! captures (ADR 0346).
//!
//! What a network capability allows is checked again where it is used:
//! by the socket methods that take its slot (`files`), not by the socket
//! it made, which is a file like any other.

use radix_abi::errors::{File, Kernel};
use radix_abi::network::{
    self as abi, AddLinkArgs, AddLinkResults, AddressFamily, ConfigureArgs, ConfigureIpv6Args,
    InterfaceAddress, InterfaceAddressIpv6, Ipv6Flags, Ipv6Settings, LinkKind, OpenCaptureArgs,
    OpenTunnelArgs, PortsArgs, PortsResults, RemoveLinkArgs, RestrictArgs, SetMasterArgs,
    SocketArgs, SocketType,
};
use radix_cap::{Capability, Derivation, ObjectKind, Rights, SlotRef};
use radix_hal::UserMode;
use radix_net::{Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr, ipv6};

use super::memory::cap_rights;
use super::methods::{Answer, answer};
use super::{Result, UserRef, cap_error, error};
use crate::Arch;
use crate::caps;
use crate::fs::{capture, inet, tunnel};
use crate::net::links::{self, LinkError, Role};
use crate::net::{self, ConfigureError};

/// Calls the method `label` on `cap`, a network capability in `user`'s
/// slot `slot`, kept out of line, as [`super::methods::call`] says.
#[inline(never)]
pub fn method(
    user: &UserRef,
    slot: u32,
    cap: &Capability,
    label: u32,
    args: &[u64; 8],
) -> Result<Answer> {
    let (base, len) = cap.range().ok_or(error(Kernel::WrongKind))?;
    match label {
        abi::SOCKET => {
            let a = SocketArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::NONE).map_err(cap_error)?;
            let rights = cap.rights();
            let sends = rights.contains(Rights::SEND) || rights.contains(Rights::LOCAL);
            // A ping socket only sends, and takes the replies to what it sent.
            if !sends && (a.kind == SocketType::Ping || !rights.contains(Rights::RECEIVE)) {
                return Err(Kernel::MissingRights.status(u32::from(Rights::SEND.bits())));
            }
            let kind = match a.kind {
                SocketType::Stream => inet::Kind::Stream,
                SocketType::Datagram => inet::Kind::Datagram,
                SocketType::Ping => inet::Kind::Ping,
            };
            let v6 = a.family == AddressFamily::Ipv6;
            inet::make(user, a.to, kind, v6, a.flags)?;
            answer(label, 0, [0; 8])
        }
        abi::RESTRICT => {
            let a = RestrictArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::COPY).map_err(cap_error)?;
            let offset = u64::from(a.first).checked_sub(base).ok_or(error(Kernel::OutOfRange))?;
            let how = Derivation {
                rights: cap_rights(a.rights),
                badge: None,
                range: Some((offset, u64::from(a.count))),
            };
            let (from, to) = (SlotRef::new(user.cspace, slot), SlotRef::new(user.cspace, a.to));
            caps::derive(from, to, how).map_err(cap_error)?;
            answer(label, 0, [0; 8])
        }
        abi::PORTS => {
            PortsArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::NONE).map_err(cap_error)?;
            // A range of ports is at most the 65,536 of the network.
            let results = PortsResults { first: base as u32, count: len as u32 };
            answer(label, PortsResults::WORDS, results.encode())
        }
        abi::CONFIGURE => {
            let a = ConfigureArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            let (addresses, count) = addresses_in(a.addresses.address, a.addresses.len)?;
            let router = router_in(a.router.address, a.router.len)?;
            net::configure(a.interface, &addresses[..count], router).map_err(configure_error)?;
            answer(label, 0, [0; 8])
        }
        abi::CONFIGURE_IPV6 => {
            let a = ConfigureIpv6Args::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            let change = settings_in(a.settings.address, a.settings.len)?;
            let (addresses, count) = addresses_ipv6_in(a.addresses.address, a.addresses.len)?;
            let router = router_ipv6_in(a.router.address, a.router.len)?;
            net::configure_ipv6(a.interface, change, &addresses[..count], router)
                .map_err(configure_error)?;
            answer(label, 0, [0; 8])
        }
        abi::ADD_LINK => {
            let a = AddLinkArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            let mut name = [0u8; 16];
            let len = usize::try_from(a.name.len).ok().filter(|&len| len < name.len());
            let len = len.ok_or(error(Kernel::InvalidArgument))?;
            copy_in(&mut name[..len], a.name.address, a.name.len)?;
            let role = match a.kind {
                LinkKind::Bridge => Role::Bridge,
                LinkKind::Tunnel => Role::Tunnel,
                LinkKind::Vlan => {
                    let parent = number_of(a.parent)?;
                    let id = u16::try_from(a.vlan).map_err(|_| error(Kernel::InvalidArgument))?;
                    Role::Vlan { parent, id }
                }
            };
            let number = links::add(role, &name[..len]).map_err(link_error)?;
            // At most MAX_INTERFACES.
            let results = AddLinkResults { interface: number as u32 + 1 };
            answer(label, AddLinkResults::WORDS, results.encode())
        }
        abi::SET_MASTER => {
            let a = SetMasterArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            let port = number_of(a.interface)?;
            let bridge = match a.bridge {
                0 => None,
                bridge => Some(number_of(bridge)?),
            };
            links::set_master(port, bridge).map_err(link_error)?;
            answer(label, 0, [0; 8])
        }
        abi::REMOVE_LINK => {
            let a = RemoveLinkArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            links::remove(number_of(a.interface)?).map_err(link_error)?;
            answer(label, 0, [0; 8])
        }
        abi::OPEN_TUNNEL => {
            let a = OpenTunnelArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            tunnel::open(user, number_of(a.interface)?, a.to, a.flags)?;
            answer(label, 0, [0; 8])
        }
        abi::OPEN_CAPTURE => {
            let a = OpenCaptureArgs::decode(args)?;
            cap.check(ObjectKind::Network, Rights::CONTROL).map_err(cap_error)?;
            let interface = match a.interface {
                0 => None,
                index => Some(number_of(index)?),
            };
            capture::open(user, interface, a.snap, a.buffer, a.to, a.flags)?;
            answer(label, 0, [0; 8])
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}

/// The kernel's number for the interface Linux numbers `index`
/// (`ifindex`), if there is one.
fn number_of(index: u32) -> Result<usize> {
    net::by_index(index).map(|interface| interface.number).ok_or(File::NoDevice.status(0))
}

/// What `add_link`, `set_master` and `remove_link` fail with.
fn link_error(err: LinkError) -> radix_abi::Status {
    match err {
        LinkError::Name | LinkError::Vlan => error(Kernel::InvalidArgument),
        LinkError::Taken => error(Kernel::Occupied),
        LinkError::NoDevice => File::NoDevice.status(0),
        LinkError::NotSupported => error(Kernel::NotSupported),
        LinkError::NoRoom => error(Kernel::NoRoom),
        LinkError::NoMemory => error(Kernel::NoMemory),
    }
}

/// What `configure` and `configure_ipv6` fail with when `net` would not.
fn configure_error(err: ConfigureError) -> radix_abi::Status {
    match err {
        ConfigureError::NoInterface => File::NoDevice.status(0),
        ConfigureError::Loopback => error(Kernel::NotSupported),
        ConfigureError::Off => Kernel::NotSupported.status(1),
    }
}

/// Copies the caller's `len` bytes at `address` into `bytes`, which must
/// be as long.
fn copy_in(bytes: &mut [u8], address: u64, len: u64) -> Result<()> {
    if len != bytes.len() as u64 {
        return Err(error(Kernel::InvalidArgument));
    }
    if !radix_ipc::range::user_bytes(address, len) || !Arch::copy_from_user(bytes, address) {
        return Err(error(Kernel::BadAddress));
    }
    Ok(())
}

/// What `configure_ipv6` changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ipv6Change {
    /// How the interface makes its addresses: `None` for IPv6 off.
    Settings(Option<ipv6::Settings>),
    /// Only the addresses it was given and its router.
    GivenOnly,
}

/// `configure_ipv6`'s settings, an Ipv6Settings structure.
fn settings_in(address: u64, len: u64) -> Result<Ipv6Change> {
    let mut bytes = [0u8; Ipv6Settings::SIZE];
    copy_in(&mut bytes, address, len)?;
    let flags = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let flags = Ipv6Flags::from_bits(flags).ok_or(error(Kernel::InvalidArgument))?;
    if bytes[4..8] != [0; 4] {
        return Err(error(Kernel::InvalidArgument));
    }
    if flags.contains(Ipv6Flags::GIVEN_ONLY) {
        return Ok(Ipv6Change::GivenOnly);
    }
    if !flags.contains(Ipv6Flags::ENABLED) {
        return Ok(Ipv6Change::Settings(None));
    }
    let mut secret = [0u8; 16];
    secret.copy_from_slice(&bytes[8..24]);
    Ok(Ipv6Change::Settings(Some(ipv6::Settings {
        secret,
        autoconf: flags.contains(Ipv6Flags::AUTOCONF),
        temporary: flags.contains(Ipv6Flags::TEMPORARY),
    })))
}

/// The most IPv6 addresses `configure_ipv6` takes.
const ADDRESSES_IPV6_MAX: usize = abi::ADDRESSES_IPV6_MAX as usize;

/// `configure_ipv6`'s addresses: InterfaceAddressIpv6 structures, each a
/// unicast address that is not link-local and a prefix of 1 to 128, with
/// its reserved bytes zero.
fn addresses_ipv6_in(address: u64, len: u64) -> Result<([Ipv6Cidr; ADDRESSES_IPV6_MAX], usize)> {
    let size = InterfaceAddressIpv6::SIZE as u64;
    if !len.is_multiple_of(size) || len / size > abi::ADDRESSES_IPV6_MAX {
        return Err(error(Kernel::InvalidArgument));
    }
    let count = (len / size) as usize;
    let mut bytes = [0u8; InterfaceAddressIpv6::SIZE * ADDRESSES_IPV6_MAX];
    let bytes = &mut bytes[..InterfaceAddressIpv6::SIZE * count];
    copy_in(bytes, address, len)?;
    let mut addresses = [Ipv6Cidr::new(Ipv6Address::UNSPECIFIED, 0); ADDRESSES_IPV6_MAX];
    for (at, entry) in bytes.as_chunks::<{ InterfaceAddressIpv6::SIZE }>().0.iter().enumerate() {
        let ip = unicast_ipv6(&entry[..16])?;
        let prefix = entry[16];
        if !(1..=128).contains(&prefix)
            || ipv6::link_local(&ip)
            || entry[17..].iter().any(|&byte| byte != 0)
        {
            return Err(error(Kernel::InvalidArgument));
        }
        addresses[at] = Ipv6Cidr::new(ip, prefix);
    }
    Ok((addresses, count))
}

/// `configure_ipv6`'s router: none for 0 bytes, else a unicast address's
/// 16, link-local as routers' are or not.
fn router_ipv6_in(address: u64, len: u64) -> Result<Option<Ipv6Address>> {
    if len == 0 {
        return Ok(None);
    }
    let mut bytes = [0u8; 16];
    copy_in(&mut bytes, address, len)?;
    unicast_ipv6(&bytes).map(Some)
}

/// `bytes`, an IPv6 address in network byte order, if it is one a host
/// may have: not ::, a multicast or the loopback address, or an
/// IPv4-mapped one.
fn unicast_ipv6(bytes: &[u8]) -> Result<Ipv6Address> {
    let mut octets = [0u8; 16];
    octets.copy_from_slice(bytes);
    let ip = Ipv6Address::from(octets);
    if ip.is_unspecified() || ip.is_multicast() || ip.is_loopback() || ip.to_ipv4_mapped().is_some()
    {
        return Err(error(Kernel::InvalidArgument));
    }
    Ok(ip)
}

/// The most addresses `configure` takes.
const ADDRESSES_MAX: usize = abi::ADDRESSES_MAX as usize;

/// The `len` bytes at the caller's `address`, read as `configure`'s
/// addresses: InterfaceAddress structures, each a unicast address and a
/// prefix of at most 32, with its reserved bytes zero.
fn addresses_in(address: u64, len: u64) -> Result<([Ipv4Cidr; ADDRESSES_MAX], usize)> {
    let size = InterfaceAddress::SIZE as u64;
    if !len.is_multiple_of(size) || len / size > abi::ADDRESSES_MAX {
        return Err(error(Kernel::InvalidArgument));
    }
    let count = (len / size) as usize;
    let mut bytes = [0u8; InterfaceAddress::SIZE * ADDRESSES_MAX];
    let bytes = &mut bytes[..InterfaceAddress::SIZE * count];
    if !radix_ipc::range::user_bytes(address, len) || !Arch::copy_from_user(bytes, address) {
        return Err(error(Kernel::BadAddress));
    }
    let mut addresses = [Ipv4Cidr::new(Ipv4Address::UNSPECIFIED, 0); ADDRESSES_MAX];
    for (at, entry) in bytes.as_chunks::<{ InterfaceAddress::SIZE }>().0.iter().enumerate() {
        let ip = unicast(&entry[..4])?;
        let prefix = entry[4];
        if prefix > 32 || entry[5..].iter().any(|&byte| byte != 0) {
            return Err(error(Kernel::InvalidArgument));
        }
        addresses[at] = Ipv4Cidr::new(ip, prefix);
    }
    Ok((addresses, count))
}

/// The `len` bytes at the caller's `address`, read as `configure`'s
/// router: none for 0 bytes, else a unicast address's 4.
fn router_in(address: u64, len: u64) -> Result<Option<Ipv4Address>> {
    match len {
        0 => Ok(None),
        4 => {
            let mut bytes = [0u8; 4];
            if !radix_ipc::range::user_bytes(address, len)
                || !Arch::copy_from_user(&mut bytes, address)
            {
                return Err(error(Kernel::BadAddress));
            }
            unicast(&bytes).map(Some)
        }
        _ => Err(error(Kernel::InvalidArgument)),
    }
}

/// `bytes`, an IPv4 address in network byte order, if it is one a host
/// may have: not 0.0.0.0, a broadcast, multicast or loopback address.
fn unicast(bytes: &[u8]) -> Result<Ipv4Address> {
    let ip = Ipv4Address::new(bytes[0], bytes[1], bytes[2], bytes[3]);
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || ip.is_loopback() {
        return Err(error(Kernel::InvalidArgument));
    }
    Ok(ip)
}
