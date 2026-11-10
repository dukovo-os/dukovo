// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The synthetic network card (netvsc).
//!
//! The card speaks two protocols, one inside the other. The outer one is
//! the network virtual service protocol (NVSP), whose messages travel in
//! the channel's packets:
//!
//! 1. INIT agrees on a version of NVSP (the newest the host takes, of
//!    6.1, 6, 5, 4, 2 and 1), answered in a completion;
//! 2. from version 2, NDIS_CONFIG says the largest frame, unanswered;
//! 3. NDIS_INIT says which version of NDIS the guest speaks, 6.30 or,
//!    up to NVSP 4, 6.1, unanswered;
//! 4. SEND_RECEIVE_BUFFER names a buffer of the guest's memory, by its
//!    GPADL, that the host is to put received frames in, answered in a
//!    completion.
//!
//! The inner one is Microsoft's Remote NDIS (RNDIS): the guest's RNDIS
//! messages go in RNDIS packets of NVSP, which name the page the message
//! is in; the host's come in the receive buffer, in RECEIVE_BUFFER
//! packets, each naming ranges of the buffer that hold one RNDIS message
//! apiece, which the guest acknowledges with a completion once it has
//! read them all, so that the host may use them again. The guest
//! initializes RNDIS, asks the card's MAC address and whether its link
//! is up, and sets which frames it wants (its own, broadcasts and
//! multicasts). Then each received frame comes as an RNDIS PACKET message
//! in the receive buffer, and the guest sends each frame as one too, from
//! a slot of its own memory that the RNDIS packet of NVSP names, which
//! the host answers with a completion once it has sent it. The host also
//! says when the link goes up or down, in RNDIS INDICATE_STATUS messages.
//!
//! The kernel turns on none of the card's offloads: it sends and takes
//! whole frames with their checksums, as on a wire.

use radix_hw::Dma;
use radix_nic::{Info, Link, MAX_FRAME, MIN_FRAME, Nic, SLOT, Slots, wait_for};

use crate::ring::{self, COMPLETION, INBAND, RECEIVE_BUFFER, Range, Rings};
use crate::{PAGE, Signal};

/// The NVSP versions the kernel speaks, newest first.
pub const VERSIONS: [u32; 6] = [0x6_0001, 0x6_0000, 0x5_0000, 0x4_0000, 0x3_0002, 0x2];

/// NVSP message types.
mod nvs {
    pub const INIT: u32 = 1;
    pub const INIT_RESPONSE: u32 = 2;
    pub const NDIS_INIT: u32 = 100;
    pub const RECEIVE_BUFFER: u32 = 101;
    pub const RECEIVE_BUFFER_RESPONSE: u32 = 102;
    pub const RNDIS: u32 = 107;
    pub const RNDIS_ACK: u32 = 108;
    pub const NDIS_CONFIG: u32 = 125;
    /// A status: success.
    pub const OK: u32 = 1;
    /// The receive buffer's signature, which RECEIVE_BUFFER packets carry.
    pub const RECEIVE_SIGNATURE: u16 = 0xcafe;
    /// An RNDIS packet's channel: a frame.
    pub const DATA: u32 = 0;
    /// An RNDIS packet's channel: a control message.
    pub const CONTROL: u32 = 1;
    /// No send buffer section: the data is in the pages the packet names.
    pub const NO_SECTION: u32 = 0xffff_ffff;
    /// The size the guest's NVSP messages are padded to.
    pub const MESSAGE: usize = 40;
}

/// RNDIS message types and statuses.
mod rndis {
    pub const PACKET: u32 = 1;
    pub const INIT: u32 = 2;
    pub const QUERY: u32 = 4;
    pub const SET: u32 = 5;
    pub const INDICATE_STATUS: u32 = 7;
    /// The bit that marks a completion.
    pub const COMPLETE: u32 = 0x8000_0000;
    pub const STATUS_SUCCESS: u32 = 0;
    pub const MEDIA_CONNECT: u32 = 0x4001_000b;
    pub const MEDIA_DISCONNECT: u32 = 0x4001_000c;
    pub const OID_PERMANENT_ADDRESS: u32 = 0x0101_0101;
    pub const OID_MEDIA_CONNECT_STATUS: u32 = 0x0001_0114;
    pub const OID_LINK_SPEED: u32 = 0x0001_0107;
    pub const OID_PACKET_FILTER: u32 = 0x0001_010e;
    /// The frames the guest takes: its own, every multicast, and
    /// broadcasts.
    pub const FILTER: u32 = 0x1 | 0x4 | 0x8;
    /// The size of a PACKET message's header.
    pub const PACKET_HEADER: usize = 44;
}

/// The receive buffer: 15 MiB, the most NVSP 2 and earlier take, in
/// blocks of 4 MiB or less, as the kernel allocates contiguous memory.
pub const RECEIVE_BUFFER_SIZE: usize = 15 << 20;
/// A block of the receive buffer.
const BLOCK: usize = 4 << 20;
/// The receive buffer's blocks.
const BLOCKS: usize = RECEIVE_BUFFER_SIZE.div_ceil(BLOCK);
/// Frames waiting to be sent, each in a slot of its own.
pub const TRANSMIT: usize = 64;

/// Transaction numbers: a frame's slot is its own, below [`TRANSMIT`].
const TRANSACTION_CONTROL: u64 = 0x1_0000_0000;
const TRANSACTION_NVS: u64 = 0x2_0000_0000;

/// How long the host has to answer a message while the card is set up.
const ANSWER_US: u32 = 5_000_000;

/// The receive buffer, in blocks.
struct Buffer {
    blocks: [Option<Dma>; BLOCKS],
}

impl Buffer {
    /// Copies `out.len()` bytes from `offset`, across blocks.
    fn read(&self, offset: usize, out: &mut [u8]) -> bool {
        if offset.checked_add(out.len()).is_none_or(|end| end > RECEIVE_BUFFER_SIZE) {
            return false;
        }
        let mut done = 0;
        while done < out.len() {
            let at = offset + done;
            let Some(block) = self.blocks[at / BLOCK].as_ref() else { return false };
            let within = at % BLOCK;
            let count = (out.len() - done).min(BLOCK - within);
            block.read(within, &mut out[done..done + count]);
            done += count;
        }
        true
    }

    fn u32_at(&self, offset: usize) -> u32 {
        let mut bytes = [0u8; 4];
        self.read(offset, &mut bytes);
        u32::from_le_bytes(bytes)
    }
}

/// A RECEIVE_BUFFER packet being read, left at the head of the inbound
/// ring until the RNDIS messages in all its ranges are: its header, its
/// count of ranges, and the next to read.
#[derive(Clone, Copy, Default)]
struct Received {
    header: Option<ring::Header>,
    count: usize,
    next: usize,
}

/// The answer to an RNDIS control message, as the receive path found it.
#[derive(Clone, Copy)]
struct ControlAnswer {
    request: u32,
    status: u32,
    /// How long its information is.
    len: usize,
    /// Its first bytes of information.
    info: [u8; 8],
}

/// A synthetic network card, set up and running.
pub struct Netvsc {
    rings: Rings,
    connection: u32,
    signal: Signal,
    receive: Buffer,
    received: Received,
    transmit: Slots,
    /// Which transmit slots hold a frame the host has not sent yet.
    busy: u64,
    /// The page RNDIS control messages are written in.
    control: Dma,
    next_request: u32,
    answer: Option<ControlAnswer>,
    /// The completion of an NVSP message: its first bytes.
    nvs_answer: Option<[u8; 16]>,
    /// Whether to signal the host at the next flush.
    kick: bool,
    mac: [u8; 6],
    link: Link,
    version: u32,
    dropped: u64,
    failed: bool,
}

/// A buffer's pages: the frame number of the `i`th.
pub type Pages<'a> = &'a dyn Fn(usize) -> u64;

/// What [`start`] needs from the kernel.
pub struct Kernel<'a> {
    /// Gives zeroed, contiguous memory the host can reach, in pages.
    pub alloc: &'a mut dyn FnMut(usize) -> Option<Dma>,
    /// Names pages to the host: makes a GPADL of the channel's for a
    /// buffer of the given count of pages, and returns its number.
    pub gpadl: &'a mut dyn FnMut(usize, Pages<'_>) -> Option<u32>,
    /// Waits the given microseconds.
    pub delay: &'a mut dyn FnMut(u32),
}

/// Why the card could not be set up.
pub type Error = radix_nic::Error;

/// Sets up the card on the channel whose rings are `rings` (open), which
/// is signalled through `connection` with `signal`.
#[inline(never)]
pub fn start(
    rings: Rings,
    connection: u32,
    signal: Signal,
    kernel: Kernel<'_>,
) -> Result<Netvsc, Error> {
    let Kernel { alloc, gpadl, delay } = kernel;
    let control = alloc(1).ok_or(Error::NoMemory)?;
    let transmit = Slots::new(TRANSMIT, alloc).ok_or(Error::NoMemory)?;
    let mut blocks = [const { None }; BLOCKS];
    for (i, block) in blocks.iter_mut().enumerate() {
        let size = (RECEIVE_BUFFER_SIZE - i * BLOCK).min(BLOCK);
        *block = Some(alloc(size / PAGE).ok_or(Error::NoMemory)?);
    }
    let mut card = Netvsc {
        rings,
        connection,
        signal,
        receive: Buffer { blocks },
        received: Received::default(),
        transmit,
        busy: 0,
        control,
        next_request: 1,
        answer: None,
        nvs_answer: None,
        kick: false,
        mac: [0; 6],
        link: Link::default(),
        version: 0,
        dropped: 0,
        failed: false,
    };
    card.version = VERSIONS
        .into_iter()
        .find(|&version| card.init(version, delay).is_ok())
        .ok_or(Error::Unsupported("any NVSP version the kernel speaks"))?;
    if card.version >= 0x3_0002 {
        let mut config = nvs_message(nvs::NDIS_CONFIG);
        // The largest frame without its check sequence, and no
        // capabilities: no VLAN tags taken out, no virtual functions.
        config[4..8].copy_from_slice(&((MAX_FRAME - 4) as u32).to_le_bytes());
        card.send_nvs(&config, false)?;
    }
    let mut ndis = nvs_message(nvs::NDIS_INIT);
    let (major, minor) = if card.version > 0x4_0000 { (6u32, 30u32) } else { (6, 1) };
    ndis[4..8].copy_from_slice(&major.to_le_bytes());
    ndis[8..12].copy_from_slice(&minor.to_le_bytes());
    card.send_nvs(&ndis, false)?;

    // The receive buffer, named to the host page by page.
    let firsts = card.receive.blocks.each_ref().map(|block| block.as_ref().map_or(0, Dma::phys));
    let page = |i: usize| (firsts[i * PAGE / BLOCK] + (i * PAGE % BLOCK) as u64) / PAGE as u64;
    let number = gpadl(RECEIVE_BUFFER_SIZE / PAGE, &page)
        .ok_or(Error::Device("the receive buffer's GPADL"))?;
    let mut connect = nvs_message(nvs::RECEIVE_BUFFER);
    connect[4..8].copy_from_slice(&number.to_le_bytes());
    connect[8..10].copy_from_slice(&nvs::RECEIVE_SIGNATURE.to_le_bytes());
    card.send_nvs(&connect, true)?;
    let answer = card.wait_nvs(delay).ok_or(Error::Timeout("the receive buffer"))?;
    if u32_at(&answer, 0) != nvs::RECEIVE_BUFFER_RESPONSE || u32_at(&answer, 4) != nvs::OK {
        return Err(Error::Device("the host refused the receive buffer"));
    }

    // RNDIS: initialize, read the MAC address and the link, and take
    // frames.
    let mut init = [0u8; 24];
    put32(&mut init, 16, 0);
    put32(&mut init, 12, 1);
    put32(&mut init, 20, SLOT as u32);
    card.control(rndis::INIT, &mut init, delay).ok_or(Error::Timeout("RNDIS's initialization"))?;
    let mac =
        card.query(rndis::OID_PERMANENT_ADDRESS, delay).ok_or(Error::Timeout("the MAC address"))?;
    if mac.1 < 6 {
        return Err(Error::Device("no MAC address"));
    }
    card.mac.copy_from_slice(&mac.0[..6]);
    let link = card.query(rndis::OID_MEDIA_CONNECT_STATUS, delay);
    card.link.up = link.is_some_and(|(state, _)| u32_at(&state, 0) == 0);
    if let Some((speed, 4..)) = card.query(rndis::OID_LINK_SPEED, delay) {
        // In units of 100 bits a second.
        card.link.mbps = u32_at(&speed, 0) / 10_000;
    }
    card.link.full_duplex = card.link.up;
    let mut filter = [0u8; 32];
    put32(&mut filter, 12, rndis::OID_PACKET_FILTER);
    put32(&mut filter, 16, 4);
    put32(&mut filter, 20, 20);
    put32(&mut filter, 28, rndis::FILTER);
    let set =
        card.control(rndis::SET, &mut filter, delay).ok_or(Error::Timeout("the packet filter"))?;
    if set.status != rndis::STATUS_SUCCESS {
        return Err(Error::Device("the host refused the packet filter"));
    }
    Ok(card)
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// An NVSP message of type `kind`, padded.
fn nvs_message(kind: u32) -> [u8; nvs::MESSAGE] {
    let mut message = [0u8; nvs::MESSAGE];
    put32(&mut message, 0, kind);
    message
}

/// The NVSP message that carries an RNDIS message on channel `channel`,
/// from the pages its packet names.
fn rndis_carrier(channel: u32) -> [u8; nvs::MESSAGE] {
    let mut message = nvs_message(nvs::RNDIS);
    put32(&mut message, 4, channel);
    put32(&mut message, 8, nvs::NO_SECTION);
    message
}

impl Netvsc {
    /// Asks for NVSP `version`.
    fn init(&mut self, version: u32, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let mut init = nvs_message(nvs::INIT);
        put32(&mut init, 4, version);
        put32(&mut init, 8, version);
        self.send_nvs(&init, true)?;
        let answer = self.wait_nvs(delay).ok_or(Error::Timeout("NVSP's version"))?;
        if u32_at(&answer, 0) == nvs::INIT_RESPONSE && u32_at(&answer, 12) == nvs::OK {
            Ok(())
        } else {
            Err(Error::Unsupported("NVSP version"))
        }
    }

    /// Signals the host's end of the channel.
    fn ring_doorbell(&mut self) {
        (self.signal)(self.connection);
    }

    /// Sends NVSP message `message`, asking for a completion if `answer`.
    fn send_nvs(&mut self, message: &[u8], answer: bool) -> Result<(), Error> {
        self.nvs_answer = None;
        match ring::send(&self.rings, message, TRANSACTION_NVS, answer) {
            Ok(signal) => {
                if signal {
                    self.ring_doorbell();
                }
                Ok(())
            }
            Err(_) => Err(Error::Device("the channel's ring is full")),
        }
    }

    /// Waits for the completion of an NVSP message.
    fn wait_nvs(&mut self, delay: &mut dyn FnMut(u32)) -> Option<[u8; 16]> {
        let mut scratch = [0u8; 0];
        wait_for(delay, ANSWER_US, 100, || {
            self.take(&mut scratch);
            self.nvs_answer.is_some()
        });
        self.nvs_answer.take()
    }

    /// Sends the RNDIS control message of type `kind` whose bytes from
    /// the request number on are `message` (its type, length and number
    /// are filled in), and waits for its answer.
    fn control(
        &mut self,
        kind: u32,
        message: &mut [u8],
        delay: &mut dyn FnMut(u32),
    ) -> Option<ControlAnswer> {
        let request = self.next_request;
        self.next_request = self.next_request.wrapping_add(1).max(1);
        put32(message, 0, kind);
        put32(message, 4, message.len() as u32);
        put32(message, 8, request);
        self.control.write(0, message);
        let page = [self.control.phys() / PAGE as u64];
        let range = Range { len: message.len() as u32, offset: 0, pages: &page };
        let carrier = rndis_carrier(nvs::CONTROL);
        self.answer = None;
        match ring::send_pages(&self.rings, &[range], &carrier, TRANSACTION_CONTROL) {
            Ok(true) => self.ring_doorbell(),
            Ok(false) => {}
            Err(_) => return None,
        }
        let mut scratch = [0u8; 0];
        wait_for(delay, ANSWER_US, 100, || {
            self.take(&mut scratch);
            self.answer.is_some_and(|answer| answer.request == request)
        });
        self.answer.take().filter(|answer| answer.request == request)
    }

    /// Queries `oid`: its first bytes of information, and how many there
    /// are.
    fn query(&mut self, oid: u32, delay: &mut dyn FnMut(u32)) -> Option<([u8; 8], usize)> {
        let mut query = [0u8; 28];
        put32(&mut query, 12, oid);
        put32(&mut query, 20, 20);
        let answer = self.control(rndis::QUERY, &mut query, delay)?;
        (answer.status == rndis::STATUS_SUCCESS).then_some((answer.info, answer.len))
    }

    /// Acknowledges the RECEIVE_BUFFER packet `transaction`, which was
    /// read, so that the host may use its ranges again.
    fn acknowledge(&mut self, transaction: u64) {
        let mut ack = nvs_message(nvs::RNDIS_ACK);
        put32(&mut ack, 4, nvs::OK);
        // The host keeps room for acknowledgements; a few tries cover a
        // ring the frames being sent filled.
        for _ in 0..16 {
            match ring::complete(&self.rings, &ack, transaction) {
                Ok(signal) => {
                    self.kick |= signal;
                    return;
                }
                Err(ring::Full::NoRoom) => self.ring_doorbell(),
                Err(ring::Full::TooBig) => return,
            }
        }
        // The ranges are lost to the host until the card is set up again.
        self.dropped += 1;
    }

    /// Reads the RNDIS message at `offset`, `len` bytes, of the receive
    /// buffer: a frame is copied into `frame` and its length returned;
    /// anything else is taken in.
    fn message(&mut self, offset: usize, len: usize, frame: &mut [u8]) -> Option<usize> {
        if len < 8 {
            return None;
        }
        let kind = self.receive.u32_at(offset);
        let length = (self.receive.u32_at(offset + 4) as usize).min(len);
        match kind {
            rndis::PACKET => {
                let data_at = self.receive.u32_at(offset + 8) as usize + 8;
                let data_len = self.receive.u32_at(offset + 12) as usize;
                if data_at < rndis::PACKET_HEADER
                    || data_at + data_len > length
                    || !(MIN_FRAME..=MAX_FRAME).contains(&data_len)
                    || data_len > frame.len()
                {
                    self.dropped += 1;
                    return None;
                }
                self.receive.read(offset + data_at, &mut frame[..data_len]).then_some(data_len)
            }
            rndis::INDICATE_STATUS => {
                match self.receive.u32_at(offset + 8) {
                    rndis::MEDIA_CONNECT => self.link.up = true,
                    rndis::MEDIA_DISCONNECT => self.link.up = false,
                    _ => {}
                }
                None
            }
            kind if kind & rndis::COMPLETE != 0 && length >= 16 => {
                let request = self.receive.u32_at(offset + 8);
                let status = self.receive.u32_at(offset + 12);
                let (mut at, mut size) = (0, 0);
                if kind == rndis::QUERY | rndis::COMPLETE && length >= 24 {
                    size = self.receive.u32_at(offset + 16) as usize;
                    at = self.receive.u32_at(offset + 20) as usize + 8;
                    if at + size > length {
                        size = 0;
                    }
                }
                let mut info = [0u8; 8];
                let take = size.min(8);
                self.receive.read(offset + at, &mut info[..take]);
                self.answer = Some(ControlAnswer { request, status, len: size, info });
                None
            }
            _ => None,
        }
    }

    /// Takes what the host sent: completions of frames sent and of NVSP
    /// messages, and RNDIS messages in the receive buffer. Returns the
    /// length of the next frame received, copied into `frame`; `None`
    /// when none waits.
    fn take(&mut self, frame: &mut [u8]) -> Option<usize> {
        loop {
            // The ranges of the packet being read first: the length and
            // offset of each, after the buffer's signature and the count.
            if let Some(header) = self.received.header {
                while self.received.next < self.received.count {
                    let mut range = [0u8; 8];
                    self.rings.peek_bytes(ring::HEADER + 8 + self.received.next * 8, &mut range);
                    self.received.next += 1;
                    let (len, offset) = (u32_at(&range, 0), u32_at(&range, 4));
                    if let Some(len) = self.message(offset as usize, len as usize, frame) {
                        return Some(len);
                    }
                }
                self.received = Received::default();
                self.finish(&header);
                self.acknowledge(header.transaction);
                continue;
            }
            let header = self.rings.peek()?;
            match header.kind {
                COMPLETION => self.completion(&header),
                RECEIVE_BUFFER => {
                    let mut fields = [0u8; 8];
                    self.rings.peek_bytes(ring::HEADER, &mut fields);
                    let count = u32_at(&fields, 4) as usize;
                    let fits = ring::HEADER + 8 + count * 8 <= header.header_len;
                    let mut kind = [0u8; 4];
                    self.rings.peek_bytes(header.header_len, &mut kind);
                    if u16::from_le_bytes([fields[0], fields[1]]) == nvs::RECEIVE_SIGNATURE
                        && fits
                        && u32_at(&kind, 0) == nvs::RNDIS
                    {
                        // Read in place, then taken.
                        self.received = Received { header: Some(header), count, next: 0 };
                        continue;
                    }
                    // Not one the driver reads: given back at once.
                    self.dropped += 1;
                    self.finish(&header);
                    self.acknowledge(header.transaction);
                    continue;
                }
                // Notes from the host (its send table, a virtual
                // function), which the kernel does not use.
                INBAND => {}
                _ => self.dropped += 1,
            }
            self.finish(&header);
        }
    }

    /// Takes the packet `header` heads off the inbound ring, signalling
    /// the host if it waits for the room.
    fn finish(&mut self, header: &ring::Header) {
        if self.rings.take(header) {
            self.ring_doorbell();
        }
    }

    /// Takes in the completion `header` heads: a frame's slot comes back,
    /// or an NVSP message is answered.
    fn completion(&mut self, header: &ring::Header) {
        let transaction = header.transaction;
        if transaction < TRANSMIT as u64 {
            self.busy &= !(1 << transaction);
        } else if transaction == TRANSACTION_NVS {
            let mut answer = [0u8; 16];
            let len = header.data_len().min(16);
            self.rings.peek_bytes(header.header_len, &mut answer[..len]);
            self.nvs_answer = Some(answer);
        }
    }

    /// Takes the completions at the head of the inbound ring, up to the
    /// first packet that is not one, which is left for [`Nic::receive`].
    fn take_completions(&mut self) {
        while self.received.header.is_none() {
            let Some(header) = self.rings.peek() else { return };
            if header.kind != COMPLETION {
                return;
            }
            self.completion(&header);
            self.finish(&header);
        }
    }

    /// The NVSP version agreed.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Whether a packet waits in the inbound ring, with the host's
    /// signals turned back on.
    pub fn arm(&self) -> bool {
        self.rings.mask(false)
    }
}

impl Nic for Netvsc {
    fn info(&self) -> Info {
        Info {
            model: "Hyper-V synthetic network card",
            mac: self.mac,
            // Room for this many of the largest frames.
            receive_slots: (RECEIVE_BUFFER_SIZE / SLOT) as u16,
            transmit_slots: TRANSMIT as u16,
        }
    }

    fn link(&self) -> Link {
        self.link
    }

    fn failed(&self) -> bool {
        self.failed
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        if self.failed {
            return None;
        }
        self.take(frame)
    }

    fn can_send(&mut self) -> bool {
        // Completions free slots; frames received stay in the ring.
        self.take_completions();
        !self.failed && (self.busy.count_ones() as usize) < TRANSMIT && self.rings.room() > 256
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if self.failed || !(MIN_FRAME..=MAX_FRAME).contains(&frame.len()) {
            return false;
        }
        let free = (!self.busy).trailing_zeros() as usize;
        if free >= TRANSMIT {
            return false;
        }
        // The RNDIS packet: its header, then the frame.
        let mut header = [0u8; rndis::PACKET_HEADER];
        let total = rndis::PACKET_HEADER + frame.len();
        put32(&mut header, 0, rndis::PACKET);
        put32(&mut header, 4, total as u32);
        put32(&mut header, 8, (rndis::PACKET_HEADER - 8) as u32);
        put32(&mut header, 12, frame.len() as u32);
        self.transmit.write(free, &header);
        self.transmit.write_at(free, rndis::PACKET_HEADER, frame);
        let phys = self.transmit.phys(free);
        let page = [phys / PAGE as u64];
        let range = Range { len: total as u32, offset: (phys % PAGE as u64) as u32, pages: &page };
        let carrier = rndis_carrier(nvs::DATA);
        match ring::send_pages(&self.rings, &[range], &carrier, free as u64) {
            Ok(signal) => {
                self.busy |= 1 << free;
                self.kick |= signal;
                true
            }
            Err(_) => false,
        }
    }

    fn flush(&mut self) {
        if core::mem::take(&mut self.kick) {
            self.ring_doorbell();
        }
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }

    fn stop(&mut self) {
        // The kernel closes the channel; nothing more is sent or taken.
        self.failed = true;
    }
}
