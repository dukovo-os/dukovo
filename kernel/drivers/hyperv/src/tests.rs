// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests against a simulated host: it reads the guest's rings and writes
//! its answers through the fake physical addresses, as Hyper-V does
//! through the guest's memory.

use std::cell::{Cell, RefCell};

use radix_hw::{Dma, fake_memory, fake_memory_write};
use radix_nic::Nic;

use crate::guid::{self, Guid};
use crate::ic::{Action, Ic, Service};
use crate::keyboard::{self, Message};
use crate::ring::{self, ANSWER, COMPLETION, Full, INBAND, PAGES, Range, Rings};
use crate::storage::{self, Direction};
use crate::vmbus::{self, FromHost};
use crate::{PAGE, network};

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

fn put16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(data: &mut [u8], at: usize, value: u64) {
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

// GUIDs.

#[test]
fn guids_read_and_write_as_windows_does() {
    let storage = guid::STORAGE;
    // The first three fields little-endian, the rest in order.
    assert_eq!(storage.0[..4], [0xd9, 0x63, 0x61, 0xba]);
    assert_eq!(storage.0[4..6], [0xa1, 0x04]);
    assert_eq!(storage.0[6..8], [0x29, 0x4d]);
    assert_eq!(storage.0[8..], [0xb6, 0x05, 0x72, 0xe2, 0xff, 0xb1, 0xdc, 0x7f]);
    assert_eq!(storage.to_string(), "ba6163d9-04a1-4d29-b605-72e2ffb1dc7f");
    assert_eq!(Guid::parse("BA6163D9-04A1-4D29-B605-72E2FFB1DC7F"), storage);
    assert_eq!(guid::name(&guid::NETWORK), Some("the synthetic network card"));
    assert_eq!(guid::name(&Guid::default()), None);
}

#[test]
#[should_panic(expected = "36 characters")]
fn a_short_guid_is_refused() {
    Guid::parse("ba6163d9-04a1-4d29-b605");
}

// VMBus's messages.

/// The message page's slot and the event flags' block for VMBus's SINT.
const VMBUS_SLOT: usize = vmbus::SINT as usize * vmbus::SLOT;
const VMBUS_EVENTS: usize = vmbus::SINT as usize * vmbus::EVENT_FLAGS;

#[test]
fn a_message_is_taken_from_its_slot() {
    let page = Dma::fake(PAGE);
    assert!(vmbus::peek(&page, vmbus::SINT).is_none());
    page.write32(VMBUS_SLOT, vmbus::MESSAGE_CHANNEL);
    // Its size, and the flag that another waits.
    page.write8(VMBUS_SLOT + 4, 12);
    page.write8(VMBUS_SLOT + 5, 1);
    page.write(VMBUS_SLOT + 16, &[4, 0, 0, 0, 0, 0, 0, 0, 9, 9, 9, 9]);
    // Another SINT's slot is not VMBus's.
    assert!(vmbus::peek(&page, 0).is_none());
    let slot = vmbus::peek(&page, vmbus::SINT).unwrap();
    assert_eq!(slot.kind, vmbus::MESSAGE_CHANNEL);
    assert_eq!(slot.bytes().len(), 12);
    assert!(matches!(vmbus::parse(slot.bytes()), Some(FromHost::OffersDone)));
    // Emptying it says another waits, and leaves it empty.
    assert!(vmbus::empty(&page, vmbus::SINT));
    assert!(vmbus::peek(&page, vmbus::SINT).is_none());
    page.write8(VMBUS_SLOT + 5, 0);
    assert!(!vmbus::empty(&page, vmbus::SINT));
}

#[test]
fn event_flags_are_taken_once() {
    let page = Dma::fake(PAGE);
    page.set64(VMBUS_EVENTS, 1 << 3 | 1 << 63);
    page.set64(VMBUS_EVENTS + 8, 1 << 1);
    // Another SINT's flags are left alone.
    page.set64(0, 1);
    let mut seen = Vec::new();
    vmbus::take_events(&page, vmbus::SINT, 64, &mut |channel| seen.push(channel));
    assert_eq!(seen, [3, 63]);
    // Channels at or past the limit wait.
    assert_eq!(page.read64(VMBUS_EVENTS + 8), 1 << 1);
    seen.clear();
    vmbus::take_events(&page, vmbus::SINT, 128, &mut |channel| seen.push(channel));
    assert_eq!(seen, [65]);
    seen.clear();
    vmbus::take_events(&page, vmbus::SINT, 2048, &mut |channel| seen.push(channel));
    assert!(seen.is_empty());
    assert_eq!(page.read64(0), 1);
}

/// An OFFER of a network card, as the host writes it.
fn offer_message() -> [u8; 196] {
    let mut data = [0u8; 196];
    put32(&mut data, 0, vmbus::kind::OFFER);
    data[8..24].copy_from_slice(&guid::NETWORK.0);
    data[24..40].copy_from_slice(&Guid::parse("00112233-4455-6677-8899-aabbccddeeff").0);
    put16(&mut data, 56, 0x2000);
    data[60] = 0xaa;
    data[179] = 0xbb;
    put16(&mut data, 180, 1);
    put32(&mut data, 184, 14);
    put32(&mut data, 192, 0x2_000e);
    data
}

#[test]
fn an_offer_reads() {
    let data = offer_message();
    let Some(FromHost::Offer(offer)) = vmbus::parse(&data) else { panic!("not an offer") };
    assert_eq!(offer.kind, guid::NETWORK);
    assert_eq!(offer.instance.to_string(), "00112233-4455-6677-8899-aabbccddeeff");
    assert_eq!(offer.flags, 0x2000);
    assert_eq!((offer.user[0], offer.user[119]), (0xaa, 0xbb));
    assert_eq!(offer.sub_index, 1);
    assert_eq!(offer.channel, 14);
    assert_eq!(offer.connection, 0x2_000e);
    // One byte short is not an offer.
    assert!(vmbus::parse(&data[..195]).is_none());
}

#[test]
fn the_hosts_other_messages_read() {
    let message = |kind: u32, fields: &[u32]| {
        let mut data = vec![0u8; 8 + fields.len() * 4];
        put32(&mut data, 0, kind);
        for (i, &field) in fields.iter().enumerate() {
            put32(&mut data, 8 + i * 4, field);
        }
        data
    };
    let parse = |data: &[u8]| vmbus::parse(data).unwrap();
    assert!(matches!(
        parse(&message(vmbus::kind::CONNECT_RESULT, &[1])),
        FromHost::Connected { accepted: true }
    ));
    assert!(matches!(
        parse(&message(vmbus::kind::CONNECT_RESULT, &[0])),
        FromHost::Connected { accepted: false }
    ));
    assert!(matches!(
        parse(&message(vmbus::kind::RESCIND_OFFER, &[14])),
        FromHost::Rescinded { channel: 14 }
    ));
    assert!(matches!(
        parse(&message(vmbus::kind::GPADL_CREATED, &[14, 0xe1e10, 0])),
        FromHost::GpadlCreated { channel: 14, gpadl: 0xe1e10, status: 0 }
    ));
    assert!(matches!(
        parse(&message(vmbus::kind::GPADL_TORNDOWN, &[0xe1e10])),
        FromHost::GpadlTornDown { gpadl: 0xe1e10 }
    ));
    assert!(matches!(
        parse(&message(vmbus::kind::OPEN_RESULT, &[14, 14, 0xc000_0001])),
        FromHost::Opened { channel: 14, status: 0xc000_0001 }
    ));
    assert!(matches!(parse(&message(99, &[])), FromHost::Other(99)));
    // Too short for its type, or for any.
    assert!(vmbus::parse(&message(vmbus::kind::GPADL_CREATED, &[14, 1])).is_none());
    assert!(vmbus::parse(&[1, 0, 0, 0]).is_none());
}

#[test]
fn the_guests_messages_are_laid_out() {
    let connect = vmbus::connect(vmbus::VERSION_WIN10, 0x1000, [0x2000, 0x3000], 3);
    let bytes = connect.bytes();
    assert_eq!(bytes.len(), 40);
    assert_eq!(u32_at(bytes, 0), vmbus::kind::CONNECT);
    assert_eq!(u32_at(bytes, 8), 4 << 16);
    assert_eq!(u32_at(bytes, 12), 3);
    assert_eq!((u64_at(bytes, 16), u64_at(bytes, 24), u64_at(bytes, 32)), (0x1000, 0x2000, 0x3000));

    let open = vmbus::open(14, 0xe1e10, 4, 0, &[1, 2, 3]);
    let bytes = open.bytes();
    assert_eq!(bytes.len(), 148);
    assert_eq!(u32_at(bytes, 0), vmbus::kind::OPEN_CHANNEL);
    assert_eq!((u32_at(bytes, 8), u32_at(bytes, 12)), (14, 14));
    assert_eq!((u32_at(bytes, 16), u32_at(bytes, 20), u32_at(bytes, 24)), (0xe1e10, 0, 4));
    assert_eq!(bytes[28..32], [1, 2, 3, 0]);

    assert_eq!(vmbus::request_offers().bytes(), [3, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(vmbus::close(14).bytes(), [7, 0, 0, 0, 0, 0, 0, 0, 14, 0, 0, 0]);
    assert_eq!(vmbus::release(14).bytes(), [13, 0, 0, 0, 0, 0, 0, 0, 14, 0, 0, 0]);
    let teardown = vmbus::gpadl_teardown(14, 0xe1e10);
    assert_eq!(teardown.bytes().len(), 16);
    assert_eq!(u32_at(teardown.bytes(), 12), 0xe1e10);

    // HvPostMessage's input: the connection, the type, the length, the
    // message.
    let block = Dma::fake(PAGE);
    vmbus::close(14).write_input(&block, 256);
    assert_eq!(block.read32(256), vmbus::MESSAGE_CONNECTION);
    assert_eq!(block.read32(264), vmbus::MESSAGE_CHANNEL);
    assert_eq!(block.read32(268), 12);
    assert_eq!(block.read32(272), vmbus::kind::CLOSE_CHANNEL);
    assert_eq!(block.read32(280), 14);
}

/// The messages that name `count` pages, numbered from 1000.
fn gpadl_messages(count: usize) -> Option<Vec<Vec<u8>>> {
    let mut messages = Vec::new();
    let page = |i: usize| 1000 + i as u64;
    vmbus::gpadl(14, 0xe1e10, count, &page, &mut |message| {
        messages.push(message.bytes().to_vec());
        true
    })
    .then_some(messages)
}

#[test]
fn a_gpadl_names_every_page_in_order() {
    let messages = gpadl_messages(100).unwrap();
    // A header with 26 pages, then bodies of 28, 28 and 18.
    let lens: Vec<usize> = messages.iter().map(Vec::len).collect();
    assert_eq!(lens, [28 + 26 * 8, 16 + 28 * 8, 16 + 28 * 8, 16 + 18 * 8]);
    let header = &messages[0];
    assert_eq!(u32_at(header, 0), vmbus::kind::GPADL_HEADER);
    assert_eq!((u32_at(header, 8), u32_at(header, 12)), (14, 0xe1e10));
    // The range: its length, one range, its bytes and its offset.
    assert_eq!((u16_at(header, 16), u16_at(header, 18)), (8 + 100 * 8, 1));
    assert_eq!((u32_at(header, 20), u32_at(header, 24)), (100 * PAGE as u32, 0));
    let mut pages: Vec<u64> = (0..26).map(|i| u64_at(header, 28 + i * 8)).collect();
    for body in &messages[1..] {
        assert_eq!(u32_at(body, 0), vmbus::kind::GPADL_BODY);
        assert_eq!(u32_at(body, 12), 0xe1e10);
        pages.extend((16..body.len()).step_by(8).map(|at| u64_at(body, at)));
    }
    assert_eq!(pages, (1000..1100).collect::<Vec<u64>>());
}

#[test]
fn a_gpadl_has_its_limits() {
    // A header alone, then one body more.
    assert_eq!(gpadl_messages(26).unwrap().len(), 1);
    assert_eq!(gpadl_messages(27).unwrap().len(), 2);
    // The receive buffer's 3840 pages fit; past 8190 nothing is sent.
    assert_eq!(gpadl_messages(3840).unwrap().len(), 1 + (3840 - 26usize).div_ceil(28));
    assert!(gpadl_messages(8190).is_some());
    let mut sent = 0;
    assert!(!vmbus::gpadl(14, 1, 8191, &|i| i as u64, &mut |_| {
        sent += 1;
        true
    }));
    assert_eq!(sent, 0);
    // A message that cannot be posted stops the rest.
    let mut sent = 0;
    assert!(!vmbus::gpadl(14, 1, 100, &|i| i as u64, &mut |_| {
        sent += 1;
        sent < 2
    }));
    assert_eq!(sent, 2);
}

// Rings.

/// The host's end of a channel's rings: it reads the guest's outbound
/// ring and writes its inbound ring through their physical addresses.
struct HostRings {
    base: u64,
    /// The outbound ring's bytes, control page included.
    outbound: usize,
    len: usize,
}

impl HostRings {
    fn new(rings: &Rings) -> HostRings {
        HostRings {
            base: rings.memory().phys(),
            outbound: rings.outbound_pages() as usize * PAGE,
            len: rings.memory().len(),
        }
    }

    fn read32(&self, at: usize) -> u32 {
        let mut bytes = [0u8; 4];
        fake_memory(self.base + at as u64, &mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn write32(&self, at: usize, value: u32) {
        fake_memory_write(self.base + at as u64, &value.to_le_bytes());
    }

    /// Copies `out.len()` bytes of the ring whose control page is at
    /// `ring` and whose data is `size` bytes, from `at`, wrapping.
    fn copy_out(&self, ring: usize, size: usize, at: usize, out: &mut [u8]) {
        for (i, byte) in out.iter_mut().enumerate() {
            let mut one = [0u8];
            fake_memory(self.base + (ring + PAGE + (at + i) % size) as u64, &mut one);
            *byte = one[0];
        }
    }

    /// The guest's next packet, without its trailer, taken off the
    /// outbound ring.
    fn take(&self) -> Option<Vec<u8>> {
        let size = self.outbound - PAGE;
        let (write, read) = (self.read32(0) as usize, self.read32(4) as usize);
        if write == read {
            return None;
        }
        let mut head = [0u8; 16];
        self.copy_out(0, size, read, &mut head);
        let len = usize::from(u16_at(&head, 4)) * 8;
        let mut packet = vec![0u8; len];
        self.copy_out(0, size, read, &mut packet);
        let mut trailer = [0u8; 8];
        self.copy_out(0, size, read + len, &mut trailer);
        assert_eq!(
            u64_at(&trailer, 0),
            (read as u64) << 32,
            "the trailer holds where the packet began"
        );
        self.write32(4, ((read + len + 8) % size) as u32);
        Some(packet)
    }

    /// Writes `packet`, padded to 8 bytes, and its trailer into the
    /// inbound ring.
    fn put(&self, packet: &[u8]) {
        let ring = self.outbound;
        let size = self.len - ring - PAGE;
        let write = self.read32(ring) as usize;
        let mut bytes = packet.to_vec();
        bytes.resize(packet.len().next_multiple_of(8), 0);
        bytes.extend(((write as u64) << 32).to_le_bytes());
        for (i, byte) in bytes.iter().enumerate() {
            fake_memory_write(self.base + (ring + PAGE + (write + i) % size) as u64, &[*byte]);
        }
        self.write32(ring, ((write + bytes.len()) % size) as u32);
    }
}

/// A packet of `kind` with its own `fields` (a multiple of 8 bytes) and
/// `data`.
fn packet(kind: u16, fields: &[u8], data: &[u8], flags: u16, transaction: u64) -> Vec<u8> {
    let header_len = 16 + fields.len();
    let len = (header_len + data.len()).next_multiple_of(8);
    let mut bytes = vec![0u8; 16];
    put16(&mut bytes, 0, kind);
    put16(&mut bytes, 2, (header_len / 8) as u16);
    put16(&mut bytes, 4, (len / 8) as u16);
    put16(&mut bytes, 6, flags);
    put64(&mut bytes, 8, transaction);
    bytes.extend_from_slice(fields);
    bytes.extend_from_slice(data);
    bytes.resize(len, 0);
    bytes
}

/// Rings of two pages each: 4096 bytes of data each way.
fn small_rings() -> (Rings, HostRings) {
    let rings = Rings::new(Dma::fake(4 * PAGE), 2);
    let host = HostRings::new(&rings);
    (rings, host)
}

#[test]
fn a_packet_reaches_the_host() {
    let (rings, host) = small_rings();
    assert_eq!(rings.room(), 4096);
    // The ring was empty: the host is signalled.
    assert_eq!(ring::send(&rings, b"hello", 7, true), Ok(true));
    let packet = host.take().unwrap();
    assert_eq!(u16_at(&packet, 0), INBAND);
    // A header of 2 units, a packet of 3 (16 + 5, padded).
    assert_eq!((u16_at(&packet, 2), u16_at(&packet, 4)), (2, 3));
    assert_eq!(u16_at(&packet, 6), ANSWER);
    assert_eq!(u64_at(&packet, 8), 7);
    assert_eq!(&packet[16..21], b"hello");
    // A packet behind one the host has not read is not signalled.
    assert_eq!(ring::complete(&rings, b"one", 8), Ok(true));
    assert_eq!(ring::complete(&rings, b"two", 9), Ok(false));
    assert_eq!(u16_at(&host.take().unwrap(), 0), COMPLETION);
    // Nor is one the host asked not to be signalled for.
    host.take().unwrap();
    host.write32(8, 1);
    assert_eq!(ring::send(&rings, b"masked", 10, false), Ok(false));
}

#[test]
fn packets_wrap_round_the_outbound_ring() {
    let (rings, host) = small_rings();
    // 72 bytes each, with the trailer: the ring's end falls inside them.
    for round in 0..200u64 {
        let data = [round as u8; 48];
        assert_eq!(ring::send(&rings, &data, round, false), Ok(true));
        let packet = host.take().unwrap();
        assert_eq!(u64_at(&packet, 8), round);
        assert_eq!(packet[16..], data);
    }
    assert_eq!(rings.room(), 4096);
}

#[test]
fn a_full_ring_asks_to_be_signalled() {
    let (rings, host) = small_rings();
    let mut sent = 0;
    while ring::send(&rings, &[1; 48], sent, false).is_ok() {
        sent += 1;
    }
    // 56 packets of 72 bytes fit, leaving one byte or more free.
    assert_eq!(sent, 56);
    assert_eq!(ring::send(&rings, &[1; 48], 99, false), Err(Full::NoRoom));
    // The pending send size: the packet and its trailer.
    assert_eq!(host.read32(12), 72);
    host.take().unwrap();
    assert!(ring::send(&rings, &[1; 48], 99, false).is_ok());
    assert_eq!(host.read32(12), 0);
    // A packet that could never fit.
    assert_eq!(ring::send(&rings, &[0; 4096], 1, false), Err(Full::TooBig));
    // The guest says it sets the pending send size.
    assert_eq!(host.read32(64), 1);
}

#[test]
fn packets_wrap_round_the_inbound_ring() {
    let (rings, host) = small_rings();
    let mut data = [0u8; 64];
    for round in 0..200u64 {
        host.put(&packet(COMPLETION, &[], &[round as u8; 48], 0, round));
        let (header, len, signal) = rings.read(&mut data).unwrap();
        assert_eq!((header.kind, header.transaction, header.data_len()), (COMPLETION, round, 48));
        assert_eq!(data[..len], [round as u8; 48]);
        assert!(!signal);
    }
    assert!(rings.read(&mut data).is_none());
}

#[test]
fn a_packet_is_read_only_whole() {
    let (rings, host) = small_rings();
    // Its header says 64 bytes, but the host moved its index past only
    // 32 of them.
    let whole = packet(INBAND, &[], &[5; 48], 0, 1);
    let ring = host.outbound;
    for (i, byte) in whole.iter().enumerate() {
        fake_memory_write(host.base + (ring + PAGE + i) as u64, &[*byte]);
    }
    host.write32(ring, 32);
    assert!(rings.peek().is_none());
    host.write32(ring, 72);
    assert_eq!(rings.peek().unwrap().len, 64);
}

#[test]
fn reading_signals_a_host_that_waits_for_room() {
    let (rings, host) = small_rings();
    let ring = host.outbound;
    // 50 packets of 72 bytes: 496 bytes free.
    for round in 0..50 {
        host.put(&packet(INBAND, &[], &[0; 48], 0, round));
    }
    let mut data = [0u8; 48];
    // Without the feature bit, the host is never signalled.
    host.write32(ring + 12, 600);
    assert!(!rings.read(&mut data).unwrap().2);
    host.write32(ring + 64, 1);
    // 640 free after this one, from 568: the 600 asked for came free.
    assert!(rings.read(&mut data).unwrap().2);
    // Already free before.
    assert!(!rings.read(&mut data).unwrap().2);
    host.write32(ring + 12, 0);
    assert!(!rings.read(&mut data).unwrap().2);
}

#[test]
fn masking_tells_of_a_packet_that_came_meanwhile() {
    let (rings, host) = small_rings();
    assert!(!rings.mask(true));
    assert_eq!(host.read32(host.outbound + 8), 1);
    assert!(!rings.mask(false));
    assert!(!rings.mask(true));
    host.put(&packet(INBAND, &[], b"late", 0, 1));
    assert!(rings.mask(false));
    assert_eq!(host.read32(host.outbound + 8), 0);
}

#[test]
fn a_pages_packet_names_its_ranges() {
    let (rings, host) = small_rings();
    let ranges = [
        Range { len: 100, offset: 12, pages: &[5, 6] },
        Range { len: 4096, offset: 0, pages: &[9] },
    ];
    assert_eq!(ring::send_pages(&rings, &ranges, b"carrier!", 3), Ok(true));
    let packet = host.take().unwrap();
    assert_eq!(u16_at(&packet, 0), PAGES);
    // The header, the reserved word and the count, two ranges' lengths
    // and offsets, three pages: 64 bytes.
    assert_eq!(u16_at(&packet, 2), 8);
    assert_eq!((u16_at(&packet, 6), u64_at(&packet, 8)), (ANSWER, 3));
    assert_eq!((u32_at(&packet, 16), u32_at(&packet, 20)), (0, 2));
    assert_eq!((u32_at(&packet, 24), u32_at(&packet, 28)), (100, 12));
    assert_eq!((u64_at(&packet, 32), u64_at(&packet, 40)), (5, 6));
    assert_eq!((u32_at(&packet, 48), u32_at(&packet, 52)), (4096, 0));
    assert_eq!(u64_at(&packet, 56), 9);
    assert_eq!(&packet[64..72], b"carrier!");
    let many = vec![1u64; ring::MAX_PAGES + 1];
    let too_many = [Range { len: 1, offset: 0, pages: &many }];
    assert_eq!(ring::send_pages(&rings, &too_many, &[], 4), Err(Full::TooBig));
}

#[test]
#[should_panic(expected = "rings too small")]
fn rings_need_two_pages_each() {
    Rings::new(Dma::fake(3 * PAGE), 2);
}

// Storage.

#[test]
fn a_scsi_command_is_laid_out() {
    let read10 = [0x28, 0, 0, 0, 0, 8, 0, 0, 1, 0];
    let packet = storage::execute(1, 2, &read10, Direction::In, 512);
    assert_eq!((u32_at(&packet, 0), u32_at(&packet, 4)), (storage::operation::EXECUTE_SRB, 1));
    assert_eq!(u16_at(&packet, 12), 52);
    assert_eq!((packet[18], packet[19], packet[20], packet[21]), (1, 2, 10, 20));
    assert_eq!((packet[22], u32_at(&packet, 24)), (1, 512));
    assert_eq!(packet[28..38], read10);
    assert_eq!((u32_at(&packet, 52), u32_at(&packet, 56)), (0x48, 60));
    let write = storage::execute(0, 0, &[0x2a; 10], Direction::Out, 4096);
    assert_eq!((write[22], u32_at(&write, 52)), (0, 0x88));
    let none = storage::execute(0, 0, &[0; 6], Direction::None, 0);
    assert_eq!((none[22], u32_at(&none, 52)), (2, 0x08));
    // A command longer than 16 bytes is cut.
    assert_eq!(storage::execute(0, 0, &[7; 20], Direction::None, 0)[20], 16);
}

#[test]
fn the_controller_is_set_up_in_order() {
    let operation = |packet: [u8; storage::PACKET]| u32_at(&packet, 0);
    assert_eq!(operation(storage::begin()), 7);
    assert_eq!(operation(storage::query_properties()), 10);
    assert_eq!(operation(storage::end()), 8);
    let query = storage::query_version(storage::VERSIONS[0]);
    assert_eq!((u32_at(&query, 0), u16_at(&query, 12)), (9, 0x0602));
    let mut answer = [0u8; storage::PACKET];
    put32(&mut answer, 0, storage::operation::COMPLETE_IO);
    put16(&mut answer, 16, 3);
    put32(&mut answer, 24, 0x20_0000);
    assert_eq!(
        storage::properties(&answer),
        storage::Properties { max_channels: 3, max_transfer: 0x20_0000 }
    );
}

#[test]
fn a_reply_reads() {
    let mut data = [0u8; storage::PACKET];
    put32(&mut data, 0, storage::operation::COMPLETE_IO);
    // An error with sense data, CHECK CONDITION, and the sense: not ready.
    data[14] = storage::srb_status::ERROR | storage::AUTOSENSE_VALID;
    data[15] = 2;
    data[28..31].copy_from_slice(&[0x70, 0, 2]);
    let reply = storage::parse(&data).unwrap();
    assert_eq!((reply.operation, reply.status), (1, 0));
    assert_eq!(reply.srb_status, storage::srb_status::ERROR);
    assert!(reply.sense_valid);
    assert_eq!((reply.scsi_status, reply.sense[2]), (2, 2));
    // The queue frozen flag is dropped too.
    data[14] = storage::srb_status::SUCCESS | 0x40;
    assert_eq!(storage::parse(&data).unwrap().srb_status, storage::srb_status::SUCCESS);
    assert!(!storage::parse(&data).unwrap().sense_valid);
    assert!(storage::parse(&data[..11]).is_none());
    assert_eq!(storage::status_name(storage::srb_status::INVALID_LUN), "no such logical unit");
}

// The keyboard.

#[test]
fn keys_turn_into_set_1_bytes() {
    assert_eq!(keyboard::protocol_request(), [1, 0, 0, 0, 0, 0, 1, 0]);
    assert_eq!(
        keyboard::parse(&[2, 0, 0, 0, 1, 0, 0, 0]),
        Some(Message::Protocol { accepted: true })
    );
    assert_eq!(
        keyboard::parse(&[2, 0, 0, 0, 0, 0, 0, 0]),
        Some(Message::Protocol { accepted: false })
    );
    let event = |code: u16, flags: u32| {
        let mut data = [0u8; 12];
        put32(&mut data, 0, 3);
        put16(&mut data, 4, code);
        put32(&mut data, 8, flags);
        keyboard::parse(&data).unwrap()
    };
    // A pressed, A let go, right Ctrl let go, Pause.
    assert_eq!(event(0x1e, 0), Message::Key { bytes: [0x1e, 0], len: 1 });
    assert_eq!(event(0x1e, 2), Message::Key { bytes: [0x9e, 0], len: 1 });
    assert_eq!(event(0x1d, 2 | 4), Message::Key { bytes: [0xe0, 0x9d], len: 2 });
    assert_eq!(event(0x1d, 8), Message::Key { bytes: [0xe1, 0x1d], len: 2 });
    // A character from the host's clipboard is dropped.
    assert_eq!(event(u16::from(b'a'), 1), Message::Other);
    assert_eq!(keyboard::parse(&[3, 0, 0, 0, 0x1e]), None);
    assert_eq!(keyboard::parse(&[9, 0, 0, 0]), Some(Message::Other));
}

// The integration services.

/// A request of type `kind` whose data is `data`, in a buffer of 64
/// bytes or more, and its length.
fn ic_request(kind: u16, data: &[u8]) -> (Vec<u8>, usize) {
    let len = 28 + data.len();
    let mut message = vec![0u8; len.max(64)];
    put16(&mut message, 12, kind);
    put16(&mut message, 18, data.len() as u16);
    // A transaction.
    message[25] = 0x01;
    message[28..len].copy_from_slice(data);
    (message, len)
}

/// NEGOTIATE with `frameworks` and `messages` as (major, minor).
fn negotiate(frameworks: &[(u16, u16)], messages: &[(u16, u16)]) -> (Vec<u8>, usize) {
    let mut data = vec![0u8; 8];
    put16(&mut data, 0, frameworks.len() as u16);
    put16(&mut data, 2, messages.len() as u16);
    for &(major, minor) in frameworks.iter().chain(messages) {
        // Major in the low half, minor in the high.
        data.extend(major.to_le_bytes());
        data.extend(minor.to_le_bytes());
    }
    ic_request(0, &data)
}

#[test]
fn versions_are_agreed() {
    let mut time = Ic::new(Service::TimeSync);
    let (mut message, len) = negotiate(&[(1, 0), (3, 0)], &[(1, 0), (3, 0), (4, 0), (5, 0)]);
    let (answer, action) = time.handle(&mut message, len).unwrap();
    assert_eq!(action, Action::Negotiated { framework: (3, 0), message: (4, 0) });
    // One of each, in 16 bytes of data.
    assert_eq!(answer, 44);
    assert_eq!((u16_at(&message, 28), u16_at(&message, 30)), (1, 1));
    assert_eq!((u32_at(&message, 36), u32_at(&message, 40)), (3, 4));
    assert_eq!(u16_at(&message, 18), 16);
    assert_eq!((u32_at(&message, 20), message[25]), (0, 5));

    let mut heartbeat = Ic::new(Service::Heartbeat);
    let (mut message, len) = negotiate(&[(3, 0)], &[(3, 0), (4, 0)]);
    let (_, action) = heartbeat.handle(&mut message, len).unwrap();
    assert_eq!(action, Action::Negotiated { framework: (3, 0), message: (3, 0) });

    // A host that speaks only newer versions is refused.
    let (mut message, len) = negotiate(&[(5, 0)], &[(5, 0)]);
    let (_, action) = heartbeat.handle(&mut message, len).unwrap();
    assert_eq!(action, Action::NoVersion);
    assert_eq!(u32_at(&message, 20), 0x8000_4005);
    // A list longer than the message.
    let (mut message, _) = negotiate(&[(3, 0)], &[(3, 0)]);
    put16(&mut message, 30, 9);
    assert_eq!(heartbeat.handle(&mut message, 44).unwrap().1, Action::NoVersion);
}

#[test]
fn heartbeats_are_answered() {
    let mut heartbeat = Ic::new(Service::Heartbeat);
    let (mut message, len) = ic_request(1, &41u64.to_le_bytes());
    assert_eq!(heartbeat.handle(&mut message, len), Some((len, Action::None)));
    assert_eq!(u64_at(&message, 28), 42);
    assert_eq!(message[25], 5);
    // Too short to answer.
    assert_eq!(heartbeat.handle(&mut message, 20), None);
    assert_eq!(heartbeat.handle(&mut [0u8; 30], 30), None);
}

#[test]
fn the_host_may_ask_to_shut_down() {
    let mut shutdown = Ic::new(Service::Shutdown);
    let request = |flags: u32| {
        let mut data = [0u8; 12];
        put32(&mut data, 8, flags);
        ic_request(3, &data)
    };
    for flags in [0, 1] {
        let (mut message, len) = request(flags);
        assert_eq!(shutdown.handle(&mut message, len).unwrap().1, Action::PowerOff);
        assert_eq!(u32_at(&message, 20), 0);
    }
    // A reboot, which the kernel does not do for the host.
    let (mut message, len) = request(2);
    assert_eq!(shutdown.handle(&mut message, len).unwrap().1, Action::None);
    assert_eq!(u32_at(&message, 20), 0x8000_4005);
    // A heartbeat on the shutdown channel asks nothing.
    let (mut message, len) = ic_request(1, &[0; 8]);
    assert_eq!(shutdown.handle(&mut message, len).unwrap().1, Action::None);
}

#[test]
fn the_hosts_clock_is_read() {
    // 1970 in 100 ns steps since 1601, and a second.
    let host_time = 116_444_736_000_000_000u64 + 10_000_000;
    let mut time = Ic::new(Service::TimeSync);
    let (mut message, len) = negotiate(&[(3, 0)], &[(4, 0)]);
    time.handle(&mut message, len).unwrap();
    // Version 4: the time, the reference count, the flags.
    let sample = |flags: u8, at: usize| {
        let mut data = vec![0u8; at - 28 + 8];
        put64(&mut data, 0, host_time);
        data[at - 28] = flags;
        ic_request(4, &data)
    };
    let (mut message, len) = sample(1, 44);
    assert_eq!(
        time.handle(&mut message, len).unwrap().1,
        Action::Time { unix_ns: 1_000_000_000, sync: true }
    );
    let (mut message, len) = sample(2, 44);
    assert_eq!(
        time.handle(&mut message, len).unwrap().1,
        Action::Time { unix_ns: 1_000_000_000, sync: false }
    );
    let (mut message, len) = sample(0, 44);
    assert_eq!(time.handle(&mut message, len).unwrap().1, Action::None);

    // Version 3: the time, the guest's, the round trip, the flags.
    let mut old = Ic::new(Service::TimeSync);
    let (mut message, len) = negotiate(&[(3, 0)], &[(3, 0)]);
    old.handle(&mut message, len).unwrap();
    let (mut message, len) = sample(1, 52);
    assert_eq!(
        old.handle(&mut message, len).unwrap().1,
        Action::Time { unix_ns: 1_000_000_000, sync: true }
    );
    // Too short for its flags.
    let (mut message, _) = sample(1, 52);
    assert_eq!(old.handle(&mut message, 52), None);
}

// The network card, against a simulated host.

/// The connection the card's channel is signalled through.
const CONNECTION: u32 = 0x2_000e;
/// The number the host gives the receive buffer's GPADL.
const RECEIVE_GPADL: u32 = 0xe1e10;
/// The host's MAC address for the card.
const MAC: [u8; 6] = [0x00, 0x15, 0x5d, 0x01, 0x02, 0x03];

thread_local! {
    /// How many times this test's card signalled the host.
    static SIGNALS: Cell<u32> = const { Cell::new(0) };
}

fn signal(connection: u32) -> u16 {
    assert_eq!(connection, CONNECTION);
    SIGNALS.with(|signals| signals.set(signals.get() + 1));
    0
}

/// A Hyper-V host's network card, as the guest sees it through its
/// channel.
struct Host {
    rings: HostRings,
    /// The NVSP version it takes.
    version: u32,
    /// The receive buffer's frame numbers, once the guest named them.
    buffer: Vec<u64>,
    /// Where in the receive buffer it writes next.
    next: usize,
    next_transaction: u64,
    /// RECEIVE_BUFFER packets the guest has not acknowledged.
    unacknowledged: Vec<u64>,
    /// What the guest set up.
    mtu: u32,
    ndis: (u32, u32),
    filter: u32,
    /// The frames the guest sent.
    frames: Vec<Vec<u8>>,
    /// Frames sent whose completion the host holds back, when it does.
    hold: Option<Vec<u64>>,
}

impl Host {
    fn new(rings: &Rings, version: u32) -> Host {
        Host {
            rings: HostRings::new(rings),
            version,
            buffer: Vec::new(),
            // The first message crosses a page of the buffer.
            next: PAGE - 24,
            next_transaction: 0x100,
            unacknowledged: Vec::new(),
            mtu: 0,
            ndis: (0, 0),
            filter: 0,
            frames: Vec::new(),
            hold: None,
        }
    }

    /// Takes in every packet the guest sent, answering each.
    fn serve(&mut self) {
        while let Some(packet) = self.rings.take() {
            self.packet(&packet);
        }
    }

    fn complete(&self, data: &[u8], transaction: u64) {
        self.rings.put(&packet(COMPLETION, &[], data, 0, transaction));
    }

    fn packet(&mut self, packet: &[u8]) {
        let header_len = usize::from(u16_at(packet, 2)) * 8;
        let transaction = u64_at(packet, 8);
        let message = &packet[header_len..];
        match u16_at(packet, 0) {
            INBAND => match u32_at(message, 0) {
                // INIT: the version asked, from the least to the most.
                1 => {
                    assert_eq!(u16_at(packet, 6), ANSWER);
                    assert_eq!(u32_at(message, 4), u32_at(message, 8));
                    let ok = u32::from(u32_at(message, 8) == self.version);
                    let mut answer = [0u8; 16];
                    put32(&mut answer, 0, 2);
                    put32(&mut answer, 12, ok);
                    self.complete(&answer, transaction);
                }
                125 => self.mtu = u32_at(message, 4),
                100 => self.ndis = (u32_at(message, 4), u32_at(message, 8)),
                // The receive buffer: its GPADL and its signature.
                101 => {
                    assert_eq!((u32_at(message, 4), u16_at(message, 8)), (RECEIVE_GPADL, 0xcafe));
                    assert_eq!(self.buffer.len(), network::RECEIVE_BUFFER_SIZE / PAGE);
                    let mut answer = [0u8; 16];
                    put32(&mut answer, 0, 102);
                    put32(&mut answer, 4, 1);
                    self.complete(&answer, transaction);
                }
                other => panic!("NVSP message {other}"),
            },
            PAGES => {
                // One range: its length, offset and page; then the NVSP
                // message that carries the RNDIS one.
                assert_eq!(u32_at(packet, 20), 1);
                let (len, offset, page) =
                    (u32_at(packet, 24), u32_at(packet, 28), u64_at(packet, 32));
                assert_eq!(u32_at(message, 0), 107);
                assert_eq!(u32_at(message, 8), 0xffff_ffff);
                let mut rndis = vec![0u8; len as usize];
                fake_memory(page * PAGE as u64 + u64::from(offset), &mut rndis);
                assert_eq!(u32_at(&rndis, 4), len, "the RNDIS message's length");
                let ack = [108, 0, 0, 0, 1, 0, 0, 0];
                if u32_at(message, 4) == 0 {
                    self.frame(&rndis);
                    match self.hold.as_mut() {
                        Some(held) => held.push(transaction),
                        None => self.complete(&ack, transaction),
                    }
                } else {
                    self.complete(&ack, transaction);
                    self.control(&rndis);
                }
            }
            COMPLETION => {
                // The guest is done with a RECEIVE_BUFFER packet.
                assert_eq!((u32_at(message, 0), u32_at(message, 4)), (108, 1));
                let at = self.unacknowledged.iter().position(|&t| t == transaction);
                self.unacknowledged.remove(at.expect("an acknowledgement of a packet sent"));
            }
            other => panic!("packet type {other}"),
        }
    }

    /// A frame the guest sent, in an RNDIS PACKET message.
    fn frame(&mut self, rndis: &[u8]) {
        assert_eq!(u32_at(rndis, 0), 1);
        let at = u32_at(rndis, 8) as usize + 8;
        let len = u32_at(rndis, 12) as usize;
        assert_eq!(at, 44);
        self.frames.push(rndis[at..at + len].to_vec());
    }

    /// An RNDIS control message, answered through the receive buffer.
    fn control(&mut self, rndis: &[u8]) {
        let kind = u32_at(rndis, 0);
        let request = u32_at(rndis, 8);
        let mut answer = vec![0u8; 16];
        put32(&mut answer, 0, kind | 0x8000_0000);
        put32(&mut answer, 8, request);
        match kind {
            // INIT: version 1.0 and the transfer size the guest gave.
            2 => {
                assert_eq!((u32_at(rndis, 12), u32_at(rndis, 16)), (1, 0));
                answer.resize(52, 0);
                put32(&mut answer, 16, 1);
                put32(&mut answer, 32, 1);
                put32(&mut answer, 36, u32_at(rndis, 20));
            }
            // QUERY: the information after a 24-byte answer.
            4 => {
                let info: Vec<u8> = match u32_at(rndis, 12) {
                    0x0101_0101 => MAC.to_vec(),
                    0x0001_0114 => 0u32.to_le_bytes().to_vec(),
                    // 10 Gbit/s, in units of 100 bits a second.
                    0x0001_0107 => 100_000_000u32.to_le_bytes().to_vec(),
                    _ => Vec::new(),
                };
                answer.resize(24, 0);
                put32(&mut answer, 16, info.len() as u32);
                put32(&mut answer, 20, 16);
                answer.extend(info);
            }
            // SET: the packet filter.
            5 => {
                assert_eq!(u32_at(rndis, 12), 0x0001_010e);
                let at = u32_at(rndis, 20) as usize + 8;
                assert_eq!(u32_at(rndis, 16), 4);
                self.filter = u32_at(rndis, at);
            }
            other => panic!("RNDIS message {other}"),
        }
        let len = answer.len() as u32;
        put32(&mut answer, 4, len);
        self.deliver(&[answer]);
    }

    /// Writes `data` at `offset` of the receive buffer, page by page.
    fn write_buffer(&self, offset: usize, data: &[u8]) {
        for (i, byte) in data.iter().enumerate() {
            let at = offset + i;
            fake_memory_write(self.buffer[at / PAGE] * PAGE as u64 + (at % PAGE) as u64, &[*byte]);
        }
    }

    /// Puts `messages` in the receive buffer and hands them to the guest
    /// in one RECEIVE_BUFFER packet.
    fn deliver(&mut self, messages: &[Vec<u8>]) {
        let mut fields = vec![0u8; 8];
        put16(&mut fields, 0, 0xcafe);
        put32(&mut fields, 4, messages.len() as u32);
        for message in messages {
            if self.next + message.len() > network::RECEIVE_BUFFER_SIZE {
                self.next = 0;
            }
            self.write_buffer(self.next, message);
            fields.extend((message.len() as u32).to_le_bytes());
            fields.extend((self.next as u32).to_le_bytes());
            // Each range starts 8-byte aligned.
            self.next += message.len().next_multiple_of(8);
        }
        let mut carrier = [0u8; 16];
        put32(&mut carrier, 0, 107);
        let transaction = self.next_transaction;
        self.next_transaction += 1;
        self.unacknowledged.push(transaction);
        self.rings.put(&packet(ring::RECEIVE_BUFFER, &fields, &carrier, 0, transaction));
    }
}

/// An RNDIS PACKET message that carries `frame`.
fn rndis_frame(frame: &[u8]) -> Vec<u8> {
    let mut message = vec![0u8; 44];
    put32(&mut message, 0, 1);
    put32(&mut message, 4, (44 + frame.len()) as u32);
    put32(&mut message, 8, 36);
    put32(&mut message, 12, frame.len() as u32);
    message.extend_from_slice(frame);
    message
}

/// A frame of `len` bytes, each its own.
fn frame(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect()
}

/// Sets up a card against a host that takes NVSP `version`.
fn card(version: u32) -> (Result<network::Netvsc, network::Error>, RefCell<Host>) {
    let rings = Rings::new(Dma::fake(8 * PAGE), 4);
    let host = RefCell::new(Host::new(&rings, version));
    let mut alloc = |pages: usize| Some(Dma::fake(pages * PAGE));
    let mut gpadl = |count: usize, page: &dyn Fn(usize) -> u64| {
        host.borrow_mut().buffer = (0..count).map(page).collect();
        Some(RECEIVE_GPADL)
    };
    let mut delay = |_us: u32| host.borrow_mut().serve();
    let kernel = network::Kernel { alloc: &mut alloc, gpadl: &mut gpadl, delay: &mut delay };
    let card = network::start(rings, CONNECTION, signal, kernel);
    (card, host)
}

#[test]
fn the_network_card_is_set_up() {
    let (card, host) = card(0x5_0000);
    let card = card.unwrap();
    let mut host = host.into_inner();
    // The host takes NVSP 5, the third the guest asks for.
    assert_eq!(card.version(), 0x5_0000);
    assert_eq!(host.mtu, 1514);
    assert_eq!(host.ndis, (6, 30));
    assert_eq!(host.filter, 0xd);
    let info = card.info();
    assert_eq!(info.mac, MAC);
    let link = card.link();
    assert!(link.up && link.full_duplex);
    assert_eq!(link.mbps, 10_000);
    // Every answer the host sent through the receive buffer was
    // acknowledged.
    host.serve();
    assert!(host.unacknowledged.is_empty());
    assert!(!card.arm());
}

#[test]
fn an_old_host_gets_ndis_6_1() {
    let (card, host) = card(0x2);
    assert_eq!(card.unwrap().version(), 0x2);
    // No NDIS_CONFIG before NVSP 3.2.
    assert_eq!(host.borrow().mtu, 0);
    assert_eq!(host.borrow().ndis, (6, 1));
}

#[test]
fn a_host_without_a_version_in_common_is_left() {
    let (card, _) = card(0x7_0000);
    assert!(matches!(card, Err(network::Error::Unsupported(_))));
}

#[test]
fn frames_come_through_the_receive_buffer() {
    let (card, host) = card(0x6_0001);
    let mut card = card.unwrap();
    let mut host = host.into_inner();
    host.serve();
    let (first, second) = (frame(60, 1), frame(1514, 2));
    let mut disconnect = vec![0u8; 20];
    put32(&mut disconnect, 0, 7);
    put32(&mut disconnect, 4, 20);
    put32(&mut disconnect, 8, 0x4001_000c);
    host.deliver(&[rndis_frame(&first), rndis_frame(&second), disconnect]);
    assert!(card.arm());
    let mut buffer = [0u8; 2048];
    assert_eq!(card.receive(&mut buffer), Some(60));
    assert_eq!(buffer[..60], first);
    assert_eq!(card.receive(&mut buffer), Some(1514));
    assert_eq!(buffer[..1514], second);
    // The packet is acknowledged once all three were read.
    host.serve();
    assert_eq!(host.unacknowledged.len(), 1);
    assert_eq!(card.receive(&mut buffer), None);
    assert!(!card.link().up);
    host.serve();
    assert!(host.unacknowledged.is_empty());

    // A frame whose length runs past its message is dropped, and its
    // packet acknowledged still.
    let mut bad = rndis_frame(&frame(100, 3));
    put32(&mut bad, 12, 200);
    host.deliver(&[bad]);
    assert_eq!(card.receive(&mut buffer), None);
    assert_eq!(card.dropped(), 1);
    host.serve();
    assert!(host.unacknowledged.is_empty());
}

#[test]
fn frames_are_sent_from_their_slots() {
    let (card, host) = card(0x6_0001);
    let mut card = card.unwrap();
    let mut host = host.into_inner();
    host.serve();
    let before = SIGNALS.with(Cell::get);
    let sent = frame(342, 9);
    assert!(card.can_send());
    assert!(card.send(&sent));
    // The host is signalled at the flush, once.
    card.flush();
    card.flush();
    assert_eq!(SIGNALS.with(Cell::get), before + 1);
    host.serve();
    assert_eq!(host.frames, [sent]);
    // Too short or too long for a frame.
    assert!(!card.send(&[0; 13]));
    assert!(!card.send(&[0; 1519]));

    // Every slot busy until the host says it sent their frames.
    host.hold = Some(Vec::new());
    for i in 0..network::TRANSMIT {
        assert!(card.can_send(), "frame {i}");
        assert!(card.send(&frame(64, i as u8)), "frame {i}");
        host.serve();
    }
    assert!(!card.can_send());
    assert!(!card.send(&frame(64, 0)));
    for transaction in host.hold.take().unwrap() {
        host.complete(&[108, 0, 0, 0, 1, 0, 0, 0], transaction);
    }
    assert!(card.can_send());
    assert!(card.send(&frame(64, 0)));
    assert_eq!(host.frames.len(), 1 + network::TRANSMIT);

    // A stopped card neither sends nor receives.
    card.stop();
    assert!(card.failed());
    assert!(!card.can_send());
    assert!(!card.send(&frame(64, 0)));
}
