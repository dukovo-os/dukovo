// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMBus's messages: how the guest connects to the host, learns of the
//! channels it offers, and opens them.
//!
//! The host's messages arrive in the SynIC message page, in VMBus's slot
//! ([`SINT`]): a 16-byte header and up to 240 bytes ([`Slot`]). The guest
//! takes one, empties the slot and, if the hypervisor says another waits
//! behind it, writes the end-of-message register. The guest's messages go
//! to the host with HvPostMessage, whose input block is the connection,
//! the type and the length of the message and its bytes ([`Request`]).
//!
//! Every channel message begins with its type and four reserved bytes:
//!
//! - CONNECT, with the VMBus version the guest asks for and the pages it
//!   gives the host (the interrupt page and two monitor pages), answered
//!   by CONNECT_RESP, which says whether the host speaks that version;
//! - REQUEST_OFFERS, answered by an OFFER for each channel and then
//!   OFFERS_DONE;
//! - GPADL_HEADER and GPADL_BODY, which name the pages of a buffer to the
//!   host under a number the guest chooses, its GPADL, answered by
//!   GPADL_CREATED; GPADL_TEARDOWN, answered by GPADL_TORNDOWN;
//! - OPEN_CHANNEL, which names the GPADL of a channel's two rings and how
//!   many pages the first, the guest's outbound ring, takes, answered by
//!   OPENCHANNEL_RESULT; CLOSE_CHANNEL;
//! - RESCIND_OFFER from the host, when a device goes away, after which
//!   the guest gives the channel back with RELID_RELEASED.

use radix_hw::{Dma, device_barrier};

use crate::Guid;

/// VMBus's synthetic interrupt source.
pub const SINT: u8 = 2;
/// The connection the guest posts its messages to.
pub const MESSAGE_CONNECTION: u32 = 1;
/// The connection a channel's signals go to on Windows Server 2008, which
/// the kernel does not speak: later hosts name one in each offer.
pub const EVENT_CONNECTION: u32 = 2;

/// VMBus 4.0, of Windows 10 and Windows Server 2016 and later.
pub const VERSION_WIN10: u32 = 4 << 16;
/// VMBus 3.0, of Windows 8.1 and Windows Server 2012 R2.
pub const VERSION_WIN8_1: u32 = 3 << 16;
/// VMBus 2.4, of Windows 8 and Windows Server 2012, the oldest the kernel
/// speaks: from it on, each channel has its own bit in the event flags
/// and its own connection to signal.
pub const VERSION_WIN8: u32 = (2 << 16) | 4;
/// The versions the kernel asks for, newest first.
pub const VERSIONS: [u32; 3] = [VERSION_WIN10, VERSION_WIN8_1, VERSION_WIN8];

/// The size of a message slot.
pub const SLOT: usize = 256;
/// The most bytes a message holds.
pub const MESSAGE_DATA: usize = 240;
/// A message slot's type when it is empty.
const MESSAGE_NONE: u32 = 0;
/// A message slot's type for VMBus's messages.
pub const MESSAGE_CHANNEL: u32 = 1;
/// A message slot's flag: the hypervisor holds another message for it.
const MESSAGE_PENDING: u8 = 1;

/// The bytes of a SynIC event flags block: one bit for each channel.
pub const EVENT_FLAGS: usize = 256;

/// Channel message types.
pub mod kind {
    /// The host offers a channel.
    pub const OFFER: u32 = 1;
    /// The host takes a channel back.
    pub const RESCIND_OFFER: u32 = 2;
    /// The guest asks for the channels.
    pub const REQUEST_OFFERS: u32 = 3;
    /// Every channel was offered.
    pub const OFFERS_DONE: u32 = 4;
    /// The guest opens a channel.
    pub const OPEN_CHANNEL: u32 = 5;
    /// The host says whether it opened.
    pub const OPEN_RESULT: u32 = 6;
    /// The guest closes a channel.
    pub const CLOSE_CHANNEL: u32 = 7;
    /// The guest names a buffer's first pages.
    pub const GPADL_HEADER: u32 = 8;
    /// The guest names a buffer's next pages.
    pub const GPADL_BODY: u32 = 9;
    /// The host has the buffer.
    pub const GPADL_CREATED: u32 = 10;
    /// The guest takes a buffer back.
    pub const GPADL_TEARDOWN: u32 = 11;
    /// The host let go of it.
    pub const GPADL_TORNDOWN: u32 = 12;
    /// The guest gives back a channel the host took back.
    pub const RELID_RELEASED: u32 = 13;
    /// The guest connects.
    pub const CONNECT: u32 = 14;
    /// The host says whether it speaks the version asked.
    pub const CONNECT_RESULT: u32 = 15;
    /// The guest disconnects.
    pub const UNLOAD: u32 = 16;
}

/// The most pages a GPADL_HEADER names; the rest go in GPADL_BODY
/// messages.
pub const GPADL_HEADER_PAGES: usize = 26;
/// The most pages a GPADL_BODY names.
pub const GPADL_BODY_PAGES: usize = 28;
/// The bytes of user data an offer and an OPEN_CHANNEL carry.
pub const USER_DATA: usize = 120;

/// A message the hypervisor put in a slot of the message page.
#[derive(Clone, Copy)]
pub struct Slot {
    /// The SynIC message's type: [`MESSAGE_CHANNEL`] for VMBus's.
    pub kind: u32,
    /// How many of `data`'s bytes it holds.
    pub size: u8,
    /// Its bytes.
    pub data: [u8; MESSAGE_DATA],
}

impl Slot {
    /// The message's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.data[..usize::from(self.size).min(MESSAGE_DATA)]
    }
}

/// The message in SINT `sint`'s slot of the message page `page`, if one
/// is there; the slot is left as it is.
pub fn peek(page: &Dma, sint: u8) -> Option<Slot> {
    let at = usize::from(sint) * SLOT;
    let kind = page.read32(at);
    if kind == MESSAGE_NONE {
        return None;
    }
    // The type is read first, and the hypervisor wrote the rest before it.
    device_barrier();
    let size = page.read8(at + 4);
    let mut data = [0u8; MESSAGE_DATA];
    page.read(at + 16, &mut data);
    Some(Slot { kind, size, data })
}

/// Empties SINT `sint`'s slot of the message page `page`. Returns whether
/// the hypervisor holds another message for it, which it delivers once
/// the guest writes the end-of-message register.
pub fn empty(page: &Dma, sint: u8) -> bool {
    let at = usize::from(sint) * SLOT;
    page.write32(at, MESSAGE_NONE);
    // The slot is empty before the flag is looked at: the hypervisor sets
    // it when it finds the slot full.
    device_barrier();
    page.read8(at + 5) & MESSAGE_PENDING != 0
}

/// The channels whose bits are set in SINT `sint`'s block of the event
/// flags page `page`, each taken and cleared; `each` is called with each
/// channel's number. Only channels below `limit` are looked at.
pub fn take_events(page: &Dma, sint: u8, limit: u32, each: &mut dyn FnMut(u32)) {
    let base = usize::from(sint) * EVENT_FLAGS;
    let words = (limit as usize).div_ceil(64).min(EVENT_FLAGS / 8);
    for word in 0..words {
        let mut bits = page.take64(base + word * 8);
        while bits != 0 {
            let bit = bits.trailing_zeros();
            bits &= bits - 1;
            each(word as u32 * 64 + bit);
        }
    }
}

/// A channel the host offers.
#[derive(Clone, Copy, Debug)]
pub struct Offer {
    /// The kind of device, which says what protocol its packets carry.
    pub kind: Guid,
    /// This device among those of its kind.
    pub instance: Guid,
    /// The channel's number, its relid: the guest's name for it in every
    /// message, and its bit in the event flags.
    pub channel: u32,
    /// The connection the guest signals for it.
    pub connection: u32,
    /// Which of a device's channels it is: 0 for its first.
    pub sub_index: u16,
    /// The offer's flags.
    pub flags: u16,
    /// The offer's user data, which some devices read.
    pub user: [u8; USER_DATA],
}

/// What a channel message from the host says.
#[derive(Clone, Copy, Debug)]
pub enum FromHost {
    /// The host speaks the version asked for, or not.
    Connected {
        /// Whether it does.
        accepted: bool,
    },
    /// A channel offered.
    Offer(Offer),
    /// Every channel has been offered.
    OffersDone,
    /// A channel went away.
    Rescinded {
        /// Its number.
        channel: u32,
    },
    /// A GPADL was made, or not.
    GpadlCreated {
        /// The channel it was made for.
        channel: u32,
        /// Its number.
        gpadl: u32,
        /// 0 for success.
        status: u32,
    },
    /// A GPADL was taken down.
    GpadlTornDown {
        /// Its number.
        gpadl: u32,
    },
    /// A channel was opened, or not.
    Opened {
        /// Its number.
        channel: u32,
        /// 0 for success.
        status: u32,
    },
    /// A message the kernel does not take, by type.
    Other(u32),
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn guid_at(data: &[u8], at: usize) -> Guid {
    let mut bytes = [0u8; 16];
    if let Some(b) = data.get(at..at + 16) {
        bytes.copy_from_slice(b);
    }
    Guid(bytes)
}

/// The channel message `data`, which a slot of type [`MESSAGE_CHANNEL`]
/// held; `None` if it is too short for its type.
pub fn parse(data: &[u8]) -> Option<FromHost> {
    if data.len() < 8 {
        return None;
    }
    let need = |len: usize| (data.len() >= len).then_some(());
    Some(match u32_at(data, 0) {
        kind::CONNECT_RESULT => {
            need(9)?;
            FromHost::Connected { accepted: data[8] != 0 }
        }
        kind::OFFER => {
            // The type and instance GUIDs, the latency, the revision, the
            // server context's size, the flags, the MMIO size, the user
            // data, the subchannel index, the channel, the monitor, its
            // flags, and the connection.
            need(196)?;
            let mut user = [0u8; USER_DATA];
            user.copy_from_slice(&data[60..60 + USER_DATA]);
            FromHost::Offer(Offer {
                kind: guid_at(data, 8),
                instance: guid_at(data, 24),
                flags: u16_at(data, 56),
                user,
                sub_index: u16_at(data, 180),
                channel: u32_at(data, 184),
                connection: u32_at(data, 192),
            })
        }
        kind::OFFERS_DONE => FromHost::OffersDone,
        kind::RESCIND_OFFER => {
            need(12)?;
            FromHost::Rescinded { channel: u32_at(data, 8) }
        }
        kind::GPADL_CREATED => {
            need(20)?;
            FromHost::GpadlCreated {
                channel: u32_at(data, 8),
                gpadl: u32_at(data, 12),
                status: u32_at(data, 16),
            }
        }
        kind::GPADL_TORNDOWN => {
            need(12)?;
            FromHost::GpadlTornDown { gpadl: u32_at(data, 8) }
        }
        kind::OPEN_RESULT => {
            need(20)?;
            FromHost::Opened { channel: u32_at(data, 8), status: u32_at(data, 16) }
        }
        other => FromHost::Other(other),
    })
}

/// A message for the host: its bytes, as HvPostMessage carries them.
#[derive(Clone, Copy)]
pub struct Request {
    bytes: [u8; MESSAGE_DATA],
    len: usize,
}

impl Request {
    /// A message of type `kind`, with nothing after its header yet.
    fn new(kind: u32) -> Request {
        let mut request = Request { bytes: [0; MESSAGE_DATA], len: 8 };
        request.bytes[..4].copy_from_slice(&kind.to_le_bytes());
        request
    }

    fn put(&mut self, at: usize, bytes: &[u8]) {
        self.bytes[at..at + bytes.len()].copy_from_slice(bytes);
        self.len = self.len.max(at + bytes.len());
    }

    fn put32(&mut self, at: usize, value: u32) {
        self.put(at, &value.to_le_bytes());
    }

    fn put64(&mut self, at: usize, value: u64) {
        self.put(at, &value.to_le_bytes());
    }

    /// The message's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Writes HvPostMessage's input block for it at `offset` of `block`
    /// (8-byte aligned, 256 bytes): the connection, the message type, its
    /// length and its bytes.
    pub fn write_input(&self, block: &Dma, offset: usize) {
        block.write32(offset, MESSAGE_CONNECTION);
        block.write32(offset + 4, 0);
        block.write32(offset + 8, MESSAGE_CHANNEL);
        block.write32(offset + 12, self.len as u32);
        block.write(offset + 16, self.bytes());
        device_barrier();
    }
}

/// CONNECT: asks for VMBus `version`, giving the host the interrupt page
/// (`interrupt`: the host's event flags for the guest's channels in its
/// second half) and the two monitor pages, and naming the virtual
/// processor `vp` as the one VMBus's messages go to.
pub fn connect(version: u32, interrupt: u64, monitor: [u64; 2], vp: u32) -> Request {
    let mut request = Request::new(kind::CONNECT);
    request.put32(8, version);
    request.put32(12, vp);
    request.put64(16, interrupt);
    request.put64(24, monitor[0]);
    request.put64(32, monitor[1]);
    request
}

/// REQUEST_OFFERS.
pub fn request_offers() -> Request {
    Request::new(kind::REQUEST_OFFERS)
}

/// OPEN_CHANNEL: opens `channel` on the rings of `gpadl`, the first
/// `outbound_pages` of which (header included) are the guest's outbound
/// ring, with its interrupts sent to virtual processor `vp`, and `user`
/// as the device's user data.
pub fn open(channel: u32, gpadl: u32, outbound_pages: u32, vp: u32, user: &[u8]) -> Request {
    let mut request = Request::new(kind::OPEN_CHANNEL);
    request.put32(8, channel);
    // The open's own number, which the result names: the channel's.
    request.put32(12, channel);
    request.put32(16, gpadl);
    request.put32(20, vp);
    request.put32(24, outbound_pages);
    request.put(28, &user[..user.len().min(USER_DATA)]);
    request.len = 28 + USER_DATA;
    request
}

/// CLOSE_CHANNEL.
pub fn close(channel: u32) -> Request {
    let mut request = Request::new(kind::CLOSE_CHANNEL);
    request.put32(8, channel);
    request
}

/// RELID_RELEASED: gives back a rescinded channel's number.
pub fn release(channel: u32) -> Request {
    let mut request = Request::new(kind::RELID_RELEASED);
    request.put32(8, channel);
    request
}

/// GPADL_TEARDOWN.
pub fn gpadl_teardown(channel: u32, gpadl: u32) -> Request {
    let mut request = Request::new(kind::GPADL_TEARDOWN);
    request.put32(8, channel);
    request.put32(12, gpadl);
    request
}

/// The messages that name `count` pages to the host as GPADL `gpadl` of
/// `channel` (a buffer of `count` pages, the frame number of the `i`th
/// being `page(i)`): a GPADL_HEADER with the first
/// [`GPADL_HEADER_PAGES`], then GPADL_BODY messages with up to
/// [`GPADL_BODY_PAGES`] each, handed to `each` in order until it says
/// false. Fails, sending nothing, for a buffer the message cannot
/// describe (over 8190 pages, whose range would not fit 16 bits).
pub fn gpadl(
    channel: u32,
    gpadl: u32,
    count: usize,
    page: &dyn Fn(usize) -> u64,
    each: &mut dyn FnMut(&Request) -> bool,
) -> bool {
    // The range's length: its byte count and offset and the frame
    // numbers, which must fit 16 bits.
    let Ok(range_len) = u16::try_from(8 + count * 8) else { return false };
    let first = count.min(GPADL_HEADER_PAGES);
    let mut header = Request::new(kind::GPADL_HEADER);
    header.put32(8, channel);
    header.put32(12, gpadl);
    header.put(16, &range_len.to_le_bytes());
    // One range.
    header.put(18, &1u16.to_le_bytes());
    header.put32(20, (count * crate::PAGE) as u32);
    header.put32(24, 0);
    for i in 0..first {
        header.put64(28 + i * 8, page(i));
    }
    if !each(&header) {
        return false;
    }
    let mut at = first;
    while at < count {
        let chunk = (count - at).min(GPADL_BODY_PAGES);
        let mut body = Request::new(kind::GPADL_BODY);
        // The message's number, which hosts do not read.
        body.put32(8, 0);
        body.put32(12, gpadl);
        for i in 0..chunk {
            body.put64(16 + i * 8, page(at + i));
        }
        if !each(&body) {
            return false;
        }
        at += chunk;
    }
    true
}
