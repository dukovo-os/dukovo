// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The usbredir protocol's wire (version 0.7, as usbredir's
//! `usb-redirection-protocol.md` describes it), from the side of the
//! "usb-host", which has the device: QEMU's `usb-redir` is the
//! "usb-guest", which plugs the device into the machine's USB bus and
//! sends it the machine's transfers.
//!
//! Every packet starts with a header: its type, the length of what
//! follows, and an id, which a reply carries back. All of it is
//! little-endian and packed. Both sides first send a hello with their
//! capabilities; this side asks for those QEMU's xHCI needs (64-bit ids,
//! 32-bit bulk lengths, endpoints' packet sizes) and the device's
//! version in its connect packet, and no others: no filters, bulk
//! streams or buffered bulk input.

use std::io::{self, Read, Write};

/// Packet types (the protocol's enum, in order from 0, data packets from
/// 100).
pub mod kind {
    pub const HELLO: u32 = 0;
    pub const DEVICE_CONNECT: u32 = 1;
    pub const DEVICE_DISCONNECT: u32 = 2;
    pub const RESET: u32 = 3;
    pub const INTERFACE_INFO: u32 = 4;
    pub const EP_INFO: u32 = 5;
    pub const SET_CONFIGURATION: u32 = 6;
    pub const GET_CONFIGURATION: u32 = 7;
    pub const CONFIGURATION_STATUS: u32 = 8;
    pub const SET_ALT_SETTING: u32 = 9;
    pub const GET_ALT_SETTING: u32 = 10;
    pub const ALT_SETTING_STATUS: u32 = 11;
    pub const CANCEL_DATA_PACKET: u32 = 21;
    pub const CONTROL_PACKET: u32 = 100;
    pub const BULK_PACKET: u32 = 101;
}

/// Statuses of a reply.
pub mod status {
    pub const SUCCESS: u8 = 0;
    pub const CANCELLED: u8 = 1;
    pub const INVAL: u8 = 2;
    pub const STALL: u8 = 4;
}

/// Capabilities, as bits of the hello's first word.
mod cap {
    pub const CONNECT_DEVICE_VERSION: u32 = 1 << 1;
    pub const EP_INFO_MAX_PACKET_SIZE: u32 = 1 << 4;
    pub const IDS_64_BITS: u32 = 1 << 5;
    pub const BULK_LENGTH_32_BITS: u32 = 1 << 6;
    /// Those this side has.
    pub const OURS: u32 =
        CONNECT_DEVICE_VERSION | EP_INFO_MAX_PACKET_SIZE | IDS_64_BITS | BULK_LENGTH_32_BITS;
}

/// A packet from QEMU.
#[derive(Debug, PartialEq, Eq)]
pub enum Packet {
    Reset,
    SetConfiguration {
        id: u64,
        configuration: u8,
    },
    GetConfiguration {
        id: u64,
    },
    SetAltSetting {
        id: u64,
        interface: u8,
        alt: u8,
    },
    GetAltSetting {
        id: u64,
        interface: u8,
    },
    /// A control transfer: its setup packet, and the data of one to the
    /// device.
    Control {
        id: u64,
        setup: Setup,
        data: Vec<u8>,
    },
    /// A bulk transfer: to the device (`endpoint` below 0x80) with its
    /// data, or from it, of up to `length` bytes.
    Bulk {
        id: u64,
        endpoint: u8,
        length: u32,
        data: Vec<u8>,
    },
    /// QEMU no longer wants the reply to the data packet `id`.
    Cancel {
        id: u64,
    },
    /// One this side has no use for, of this type.
    Other(u32),
}

/// A control transfer's setup packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

/// An endpoint as the device describes it, for `ep_info`.
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    /// Its address: the direction in bit 7.
    pub address: u8,
    /// USB's transfer types: 0 control, 2 bulk.
    pub kind: u8,
    pub interface: u8,
    pub max_packet: u16,
}

/// The connection to QEMU, once the hellos are exchanged.
pub struct Wire<S> {
    stream: S,
    /// Whether ids are 64 bits, both sides having said so.
    long_ids: bool,
    /// Whether bulk packets' lengths have their high 16 bits.
    long_bulk: bool,
    /// Whether the connect packet carries the device's version.
    device_version: bool,
    /// Whether endpoint information carries packet sizes.
    packet_sizes: bool,
}

/// The size of a hello's version string.
const VERSION_LENGTH: usize = 64;

impl<S: Read + Write> Wire<S> {
    /// Sends this side's hello, reads QEMU's, and keeps the capabilities
    /// both have.
    pub fn hello(mut stream: S, version: &str) -> io::Result<Wire<S>> {
        let mut body = [0u8; VERSION_LENGTH + 4];
        let name = version.as_bytes();
        body[..name.len().min(VERSION_LENGTH - 1)]
            .copy_from_slice(&name[..name.len().min(VERSION_LENGTH - 1)]);
        body[VERSION_LENGTH..].copy_from_slice(&cap::OURS.to_le_bytes());
        // A hello's id is always 32 bits.
        let mut packet = header(kind::HELLO, body.len(), 0, false);
        packet.extend_from_slice(&body);
        stream.write_all(&packet)?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head)?;
        let (kind, length) = (le32(&head[0..4]), le32(&head[4..8]) as usize);
        if kind != kind::HELLO || length < VERSION_LENGTH {
            return Err(invalid("QEMU's first packet is not a hello"));
        }
        let mut theirs = vec![0u8; length];
        stream.read_exact(&mut theirs)?;
        let caps = theirs.get(VERSION_LENGTH..VERSION_LENGTH + 4).map_or(0, le32);
        let both = caps & cap::OURS;
        Ok(Wire {
            stream,
            long_ids: both & cap::IDS_64_BITS != 0,
            long_bulk: both & cap::BULK_LENGTH_32_BITS != 0,
            device_version: both & cap::CONNECT_DEVICE_VERSION != 0,
            packet_sizes: both & cap::EP_INFO_MAX_PACKET_SIZE != 0,
        })
    }

    /// The same connection over `stream`, another handle of it: one
    /// thread reads while another writes.
    pub fn sharing<T>(&self, stream: T) -> Wire<T> {
        Wire {
            stream,
            long_ids: self.long_ids,
            long_bulk: self.long_bulk,
            device_version: self.device_version,
            packet_sizes: self.packet_sizes,
        }
    }

    /// Reads the next packet.
    pub fn read(&mut self) -> io::Result<Packet> {
        let mut head = [0u8; 16];
        let size = if self.long_ids { 16 } else { 12 };
        self.stream.read_exact(&mut head[..size])?;
        let (kind, length) = (le32(&head[0..4]), le32(&head[4..8]) as usize);
        let id = if self.long_ids {
            u64::from_le_bytes(head[8..16].try_into().unwrap())
        } else {
            u64::from(le32(&head[8..12]))
        };
        if length > 64 << 20 {
            return Err(invalid("a packet longer than any transfer"));
        }
        let mut body = vec![0u8; length];
        self.stream.read_exact(&mut body)?;
        let byte = |at: usize| body.get(at).copied().unwrap_or(0);
        let word = |at: usize| u16::from_le_bytes([byte(at), byte(at + 1)]);
        Ok(match kind {
            kind::RESET => Packet::Reset,
            kind::SET_CONFIGURATION => Packet::SetConfiguration { id, configuration: byte(0) },
            kind::GET_CONFIGURATION => Packet::GetConfiguration { id },
            kind::SET_ALT_SETTING => Packet::SetAltSetting { id, interface: byte(0), alt: byte(1) },
            kind::GET_ALT_SETTING => Packet::GetAltSetting { id, interface: byte(0) },
            kind::CANCEL_DATA_PACKET => Packet::Cancel { id },
            kind::CONTROL_PACKET => {
                let setup = Setup {
                    request: byte(1),
                    request_type: byte(2),
                    value: word(4),
                    index: word(6),
                    length: word(8),
                };
                Packet::Control { id, setup, data: body.get(10..).unwrap_or(&[]).to_vec() }
            }
            kind::BULK_PACKET => {
                let size = if self.long_bulk { 10 } else { 8 };
                let high = if self.long_bulk { u32::from(word(8)) } else { 0 };
                let length = u32::from(word(2)) | (high << 16);
                let data = body.get(size..).unwrap_or(&[]).to_vec();
                Packet::Bulk { id, endpoint: byte(0), length, data }
            }
            other => Packet::Other(other),
        })
    }

    /// Says what the device's endpoints are, then its interface, then
    /// that it is there, as the protocol orders them.
    pub fn connect(
        &mut self,
        endpoints: &[Endpoint],
        interface: [u8; 4],
        device: [u16; 3],
    ) -> io::Result<()> {
        self.endpoints(endpoints)?;
        self.interface(interface)?;
        let mut body = vec![2, 0, 0, 0];
        for value in device.iter().take(if self.device_version { 3 } else { 2 }) {
            body.extend(value.to_le_bytes());
        }
        // High speed (2), and the class, subclass and protocol of a
        // device whose interfaces say theirs.
        self.send(kind::DEVICE_CONNECT, 0, &body)
    }

    /// Says the device was unplugged.
    pub fn disconnect(&mut self) -> io::Result<()> {
        self.send(kind::DEVICE_DISCONNECT, 0, &[])
    }

    /// The endpoints' types, intervals, interfaces and packet sizes, at
    /// 0 to 15 for those out and 16 to 31 for those in.
    pub fn endpoints(&mut self, endpoints: &[Endpoint]) -> io::Result<()> {
        let mut types = [0xffu8; 32];
        let intervals = [0u8; 32];
        let mut interfaces = [0u8; 32];
        let mut sizes = [0u16; 32];
        for endpoint in endpoints {
            let number = usize::from(endpoint.address & 0xf);
            let at = if endpoint.address & 0x80 != 0 { 16 + number } else { number };
            types[at] = endpoint.kind;
            interfaces[at] = endpoint.interface;
            sizes[at] = endpoint.max_packet;
        }
        let mut body = Vec::new();
        body.extend(types);
        body.extend(intervals);
        body.extend(interfaces);
        if self.packet_sizes {
            for size in sizes {
                body.extend(size.to_le_bytes());
            }
        }
        self.send(kind::EP_INFO, 0, &body)
    }

    /// The device's one interface: its number, class, subclass and
    /// protocol.
    pub fn interface(&mut self, [number, class, subclass, protocol]: [u8; 4]) -> io::Result<()> {
        let mut body = vec![0u8; 4 + 4 * 32];
        body[..4].copy_from_slice(&1u32.to_le_bytes());
        body[4] = number;
        body[4 + 32] = class;
        body[4 + 64] = subclass;
        body[4 + 96] = protocol;
        self.send(kind::INTERFACE_INFO, 0, &body)
    }

    pub fn configuration_status(
        &mut self,
        id: u64,
        status: u8,
        configuration: u8,
    ) -> io::Result<()> {
        self.send(kind::CONFIGURATION_STATUS, id, &[status, configuration])
    }

    pub fn alt_setting_status(
        &mut self,
        id: u64,
        status: u8,
        interface: u8,
        alt: u8,
    ) -> io::Result<()> {
        self.send(kind::ALT_SETTING_STATUS, id, &[status, interface, alt])
    }

    /// Replies to the control transfer `id` of `setup`: its status, and
    /// the data from the device of one that asked for it.
    pub fn control_reply(
        &mut self,
        id: u64,
        setup: Setup,
        status: u8,
        data: &[u8],
    ) -> io::Result<()> {
        let into_host = setup.request_type & 0x80 != 0;
        let length = if into_host {
            data.len() as u16
        } else if status == status::SUCCESS {
            setup.length
        } else {
            0
        };
        let mut body = vec![setup.request_type & 0x80, setup.request, setup.request_type, status];
        body.extend(setup.value.to_le_bytes());
        body.extend(setup.index.to_le_bytes());
        body.extend(length.to_le_bytes());
        if into_host {
            body.extend_from_slice(data);
        }
        self.send(kind::CONTROL_PACKET, id, &body)
    }

    /// Replies to the bulk transfer `id` on `endpoint`: its status and the
    /// length moved, and the data of one from the device.
    pub fn bulk_reply(
        &mut self,
        id: u64,
        endpoint: u8,
        status: u8,
        length: u32,
        data: &[u8],
    ) -> io::Result<()> {
        let mut body = vec![endpoint, status];
        body.extend((length as u16).to_le_bytes());
        // No stream.
        body.extend(0u32.to_le_bytes());
        if self.long_bulk {
            body.extend(((length >> 16) as u16).to_le_bytes());
        }
        body.extend_from_slice(data);
        self.send(kind::BULK_PACKET, id, &body)
    }

    fn send(&mut self, kind: u32, id: u64, body: &[u8]) -> io::Result<()> {
        let mut packet = header(kind, body.len(), id, self.long_ids);
        packet.extend_from_slice(body);
        self.stream.write_all(&packet)
    }
}

/// A packet's header.
fn header(kind: u32, length: usize, id: u64, long_ids: bool) -> Vec<u8> {
    let mut head = Vec::with_capacity(16);
    head.extend(kind.to_le_bytes());
    head.extend((length as u32).to_le_bytes());
    if long_ids {
        head.extend(id.to_le_bytes());
    } else {
        head.extend((id as u32).to_le_bytes());
    }
    head
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}
