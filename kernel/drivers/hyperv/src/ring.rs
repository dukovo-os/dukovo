// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A channel's two ring buffers and the packets in them.
//!
//! The guest gives the host one block of its memory for a channel: the
//! outbound ring, which the guest writes and the host reads, then the
//! inbound ring, which the host writes and the guest reads. Each ring
//! starts with a page of control fields, of which the guest and the host
//! use a few:
//!
//! - the write index (offset 0), which only the writer moves, and the read
//!   index (4), which only the reader moves, both offsets into the data
//!   that follows the page; the ring is empty when they are equal, so the
//!   writer always leaves at least one byte free;
//! - the interrupt mask (8): a reader that sets it asks not to be
//!   signalled, while it is reading anyway;
//! - the pending send size (12): a writer that found too little room says
//!   how much it needs, and the reader signals it once that much is free,
//!   if the writer set the pending send size feature bit (64) of the ring
//!   to say it does so.
//!
//! A packet is a 16-byte header, its own fields, the data, padding to 8
//! bytes, and then 8 bytes the writer adds: its write index at the
//! packet's start, in the high half, which the reader skips. The header
//! holds the packet's type, the length of the header and the packet's own
//! fields and of the whole packet without the trailer, both in 8-byte
//! units, its flags, and a transaction number that the answer carries
//! back.
//!
//! The writer signals the reader after a write only if the reader has not
//! masked its interrupt and the ring was empty when it began (the reader
//! will find what follows on its own); the reader signals a writer that
//! waits for room once the room it asked for is free.

use radix_hw::{Dma, device_barrier};

use crate::PAGE;

/// A packet with data only (INBAND).
pub const INBAND: u16 = 6;
/// A packet that names a range of the receive buffer (netvsc's).
pub const RECEIVE_BUFFER: u16 = 7;
/// A packet that names the guest's pages the host reads or writes (GPA
/// direct).
pub const PAGES: u16 = 9;
/// The answer to a packet that asked for one (COMPLETION).
pub const COMPLETION: u16 = 0xb;

/// A packet header flag: the host is to answer it with a completion.
pub const ANSWER: u16 = 1;

/// The size of a packet header.
pub const HEADER: usize = 16;
/// The trailer the writer puts after each packet.
const TRAILER: usize = 8;

/// Offsets in a ring's control page.
const WRITE_INDEX: usize = 0;
const READ_INDEX: usize = 4;
const INTERRUPT_MASK: usize = 8;
const PENDING_SEND: usize = 12;
const FEATURES: usize = 64;
/// The feature bit: this ring's writer sets the pending send size.
const FEATURE_PENDING_SEND: u32 = 1;

/// A packet's header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    /// Its type: [`INBAND`], [`RECEIVE_BUFFER`], [`PAGES`] or
    /// [`COMPLETION`].
    pub kind: u16,
    /// The bytes of the header and the type's own fields: where the data
    /// starts.
    pub header_len: usize,
    /// The bytes of the whole packet, without the trailer.
    pub len: usize,
    /// Its flags: [`ANSWER`].
    pub flags: u16,
    /// Its transaction number.
    pub transaction: u64,
}

impl Header {
    fn parse(bytes: &[u8; HEADER]) -> Header {
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let mut transaction = [0u8; 8];
        transaction.copy_from_slice(&bytes[8..16]);
        Header {
            kind: u16_at(0),
            header_len: usize::from(u16_at(2)) * 8,
            len: usize::from(u16_at(4)) * 8,
            flags: u16_at(6),
            transaction: u64::from_le_bytes(transaction),
        }
    }

    /// The bytes of its data, after the header and its own fields.
    pub fn data_len(&self) -> usize {
        self.len.saturating_sub(self.header_len)
    }
}

/// A range of the guest's memory that a [`PAGES`] packet names: `len`
/// bytes from `offset` in the first of `pages`, through the pages in
/// order.
#[derive(Clone, Copy, Debug)]
pub struct Range<'a> {
    /// Its bytes.
    pub len: u32,
    /// Where it starts in its first page.
    pub offset: u32,
    /// Frame numbers: physical addresses shifted right by 12.
    pub pages: &'a [u64],
}

/// Why a packet was not written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    /// The ring has no room for it now.
    NoRoom,
    /// It would never fit the ring.
    TooBig,
}

/// Puts a packet's bytes in the outbound ring, in order, for
/// [`Rings::write_with`].
pub struct Writer<'a> {
    rings: &'a Rings,
    at: usize,
    written: usize,
}

impl Writer<'_> {
    /// Puts `bytes` next.
    pub fn put(&mut self, bytes: &[u8]) {
        self.at = self.rings.put(self.at, bytes);
        self.written += bytes.len();
    }
}

/// A channel's two rings, in one block of memory the host shares.
pub struct Rings {
    memory: Dma,
    /// The bytes of the outbound ring, its control page included.
    outbound: usize,
}

impl Rings {
    /// The rings in `memory`, whose first `outbound_pages` pages are the
    /// outbound ring and the rest the inbound ring, each with its control
    /// page first. `memory` is zeroed; the outbound ring says its writer
    /// sets the pending send size.
    ///
    /// # Panics
    ///
    /// If either ring would have fewer than two pages.
    pub fn new(memory: Dma, outbound_pages: usize) -> Rings {
        let outbound = outbound_pages * PAGE;
        assert!(outbound_pages >= 2 && memory.len() >= outbound + 2 * PAGE, "rings too small");
        memory.write32(FEATURES, FEATURE_PENDING_SEND);
        Rings { memory, outbound }
    }

    /// The memory both rings are in, for its GPADL.
    pub fn memory(&self) -> &Dma {
        &self.memory
    }

    /// How many pages the outbound ring takes.
    pub fn outbound_pages(&self) -> u32 {
        (self.outbound / PAGE) as u32
    }

    /// The bytes of the outbound ring's data.
    fn out_size(&self) -> usize {
        self.outbound - PAGE
    }

    /// The bytes of the inbound ring's data.
    fn in_size(&self) -> usize {
        self.memory.len() - self.outbound - PAGE
    }

    /// The bytes free in the outbound ring now, of which a packet may take
    /// all but one, with its trailer.
    pub fn room(&self) -> usize {
        let size = self.out_size();
        let write = self.memory.read32(WRITE_INDEX) as usize % size;
        let read = self.memory.read32(READ_INDEX) as usize % size;
        if write >= read { size - (write - read) } else { read - write }
    }

    /// Copies `bytes` into the outbound ring's data at `at`, wrapping;
    /// returns where it ended.
    fn put(&self, at: usize, bytes: &[u8]) -> usize {
        let size = self.out_size();
        let first = bytes.len().min(size - at);
        self.memory.write(PAGE + at, &bytes[..first]);
        if first < bytes.len() {
            self.memory.write(PAGE, &bytes[first..]);
        }
        (at + bytes.len()) % size
    }

    /// Writes a packet made of `parts`, in order, padded to 8 bytes.
    /// Returns whether the host must be signalled; see
    /// [`Rings::write_with`].
    pub fn write(&self, parts: &[&[u8]]) -> Result<bool, Full> {
        let len = parts.iter().map(|part| part.len()).sum();
        self.write_with(len, &mut |out| {
            for part in parts {
                out.put(part);
            }
        })
    }

    /// Writes a packet of `len` bytes, which `fill` puts in place, padded
    /// to 8 bytes. Returns whether the host must be signalled. A packet
    /// that finds no room sets the pending send size, so that the host
    /// signals once that much is free; one that is written clears it.
    pub fn write_with(
        &self,
        len: usize,
        fill: &mut dyn FnMut(&mut Writer<'_>),
    ) -> Result<bool, Full> {
        let size = self.out_size();
        let padded = len.next_multiple_of(8);
        let need = padded + TRAILER;
        if need >= size {
            return Err(Full::TooBig);
        }
        if self.room() <= need {
            self.memory.write32(PENDING_SEND, need as u32);
            device_barrier();
            // The host may have read meanwhile, before it saw the size.
            if self.room() <= need {
                return Err(Full::NoRoom);
            }
        }
        self.memory.write32(PENDING_SEND, 0);
        let start = self.memory.read32(WRITE_INDEX) as usize % size;
        let mut out = Writer { rings: self, at: start, written: 0 };
        fill(&mut out);
        debug_assert_eq!(out.written, len, "a packet's parts are not its length");
        out.put(&[0u8; 8][..padded - len]);
        out.put(&((start as u64) << 32).to_le_bytes());
        // The packet is in place before the host can see it.
        device_barrier();
        self.memory.write32(WRITE_INDEX, out.at as u32);
        device_barrier();
        let masked = self.memory.read32(INTERRUPT_MASK) != 0;
        Ok(!masked && self.memory.read32(READ_INDEX) as usize % size == start)
    }

    /// The bytes of the inbound ring's data that wait to be read.
    fn waiting(&self) -> usize {
        let size = self.in_size();
        let base = self.outbound;
        let write = self.memory.read32(base + WRITE_INDEX) as usize % size;
        let read = self.memory.read32(base + READ_INDEX) as usize % size;
        if write >= read { write - read } else { size - read + write }
    }

    /// Copies `out.len()` bytes from the inbound ring's data at `at`,
    /// wrapping.
    fn get(&self, at: usize, out: &mut [u8]) {
        let size = self.in_size();
        let base = self.outbound + PAGE;
        let at = at % size;
        let first = out.len().min(size - at);
        self.memory.read(base + at, &mut out[..first]);
        if first < out.len() {
            let rest = out.len() - first;
            self.memory.read(base, &mut out[first..first + rest]);
        }
    }

    /// The header of the next packet in the inbound ring, if a whole one
    /// is there.
    pub fn peek(&self) -> Option<Header> {
        let waiting = self.waiting();
        if waiting < HEADER {
            return None;
        }
        // The host wrote the packet before it moved its write index.
        device_barrier();
        let read = self.memory.read32(self.outbound + READ_INDEX) as usize;
        let mut bytes = [0u8; HEADER];
        self.get(read, &mut bytes);
        let header = Header::parse(&bytes);
        (header.len >= HEADER && header.header_len >= HEADER && header.len + TRAILER <= waiting)
            .then_some(header)
    }

    /// Copies `out.len()` bytes of the next packet, from byte `offset` of
    /// it, without taking it.
    pub fn peek_bytes(&self, offset: usize, out: &mut [u8]) {
        let read = self.memory.read32(self.outbound + READ_INDEX) as usize;
        self.get(read + offset, out);
    }

    /// Takes the next packet, whose header [`Rings::peek`] gave, off the
    /// inbound ring. Returns whether the host must be signalled: it
    /// waits for room that is now free.
    pub fn take(&self, header: &Header) -> bool {
        let size = self.in_size();
        let base = self.outbound;
        let read = self.memory.read32(base + READ_INDEX) as usize;
        let taken = header.len + TRAILER;
        // Everything is read before the host may write over it.
        device_barrier();
        self.memory.write32(base + READ_INDEX, ((read + taken) % size) as u32);
        device_barrier();
        if self.memory.read32(base + FEATURES) & FEATURE_PENDING_SEND == 0 {
            return false;
        }
        let pending = self.memory.read32(base + PENDING_SEND) as usize;
        if pending == 0 {
            return false;
        }
        let free = size - self.waiting();
        // Signalled only as the room it asked for comes free.
        free > pending && free - taken <= pending
    }

    /// Reads the next packet: its header, and as much of its data as fits
    /// `data`, and takes it off the ring. Returns the header, the bytes of
    /// data copied and whether to signal the host.
    pub fn read(&self, data: &mut [u8]) -> Option<(Header, usize, bool)> {
        let header = self.peek()?;
        let len = header.data_len().min(data.len());
        self.peek_bytes(header.header_len, &mut data[..len]);
        let signal = self.take(&header);
        Some((header, len, signal))
    }

    /// Masks the host's signals for the inbound ring, or unmasks them.
    /// Unmasking returns whether a packet came meanwhile, which then waits
    /// to be read.
    pub fn mask(&self, masked: bool) -> bool {
        self.memory.write32(self.outbound + INTERRUPT_MASK, u32::from(masked));
        device_barrier();
        !masked && self.waiting() > 0
    }
}

/// A packet's header, as written.
fn header(kind: u16, header_len: usize, len: usize, flags: u16, transaction: u64) -> [u8; 16] {
    let mut bytes = [0u8; HEADER];
    bytes[0..2].copy_from_slice(&kind.to_le_bytes());
    bytes[2..4].copy_from_slice(&((header_len / 8) as u16).to_le_bytes());
    bytes[4..6].copy_from_slice(&(len.div_ceil(8) as u16).to_le_bytes());
    bytes[6..8].copy_from_slice(&flags.to_le_bytes());
    bytes[8..16].copy_from_slice(&transaction.to_le_bytes());
    bytes
}

/// Writes an INBAND packet of `data` with transaction number
/// `transaction`, asking for a completion if `answer`. Returns whether to
/// signal the host.
pub fn send(rings: &Rings, data: &[u8], transaction: u64, answer: bool) -> Result<bool, Full> {
    let flags = if answer { ANSWER } else { 0 };
    let head = header(INBAND, HEADER, HEADER + data.len(), flags, transaction);
    rings.write(&[&head, data])
}

/// Writes a COMPLETION packet of `data` that answers the host's packet
/// `transaction`.
pub fn complete(rings: &Rings, data: &[u8], transaction: u64) -> Result<bool, Full> {
    let head = header(COMPLETION, HEADER, HEADER + data.len(), 0, transaction);
    rings.write(&[&head, data])
}

/// The most pages a [`send_pages`] packet names in all: its header must
/// fit 16 bits of 8-byte units, and the rings do not take one much bigger.
pub const MAX_PAGES: usize = 256;

/// Writes a PAGES packet that names `ranges` of the guest's memory, with
/// `data` after them, asking for a completion. Returns whether to signal
/// the host.
pub fn send_pages(
    rings: &Rings,
    ranges: &[Range<'_>],
    data: &[u8],
    transaction: u64,
) -> Result<bool, Full> {
    let pages: usize = ranges.iter().map(|range| range.pages.len()).sum();
    if pages > MAX_PAGES || ranges.len() > MAX_PAGES {
        return Err(Full::TooBig);
    }
    // The header, a reserved word and the count of ranges, then each
    // range's length, offset and frame numbers.
    let header_len = HEADER + 8 + ranges.len() * 8 + pages * 8;
    let len = header_len + data.len();
    let head = header(PAGES, header_len, len, ANSWER, transaction);
    rings.write_with(len, &mut |out| {
        out.put(&head);
        out.put(&0u32.to_le_bytes());
        out.put(&(ranges.len() as u32).to_le_bytes());
        for range in ranges {
            out.put(&range.len.to_le_bytes());
            out.put(&range.offset.to_le_bytes());
            for &page in range.pages {
                out.put(&page.to_le_bytes());
            }
        }
        out.put(data);
    })
}
