// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The SCSI driver against a simulated virtio SCSI adapter: the same PCI
//! configuration space as the block tests, a BAR with three queues, and
//! disks at some of its targets that answer INQUIRY, TEST UNIT READY,
//! READ CAPACITY(10), READ(10), WRITE(10) and SYNCHRONIZE CACHE, and fail
//! any other command with ILLEGAL REQUEST sense data.

use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Mmio, Registers, fake_memory_write};

use crate::scsi::{Address, Answer, Direction, Info, Scsi, response, scsi_status};
use crate::snd_tests::{Buffer, Ring, next_chain, put_used, read_mem};
use crate::tests::{BAR_SIZE, alloc, function};
use crate::{Error, Transport, device_type, feature as common_feature, status};

/// A disk at a target's logical unit, of 512-byte blocks.
struct Lun {
    at: Address,
    blocks: Vec<u8>,
}

#[derive(Default)]
struct State {
    device_select: u32,
    driver_select: u32,
    driver_features: u64,
    status: u8,
    select: u16,
    rings: [Ring; 3],
    sense_size: u32,
    cdb_size: u32,
    luns: Vec<Lun>,
    /// Whether the adapter holds commands until the test answers them.
    holding: bool,
    held: Vec<(u16, Vec<Buffer>)>,
    /// The commands it was sent: where, and the descriptor block's
    /// operation code.
    commands: Vec<(Address, u8)>,
    /// Each chain's buffers, in order: whether the device writes each.
    shapes: Vec<Vec<bool>>,
}

struct Adapter {
    state: Mutex<State>,
}

/// SAM's fixed-format sense data: ILLEGAL REQUEST, invalid command
/// operation code.
const ILLEGAL: [u8; 18] = [0x70, 0, 5, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0x20, 0, 0, 0, 0, 0];

impl Adapter {
    fn new(luns: Vec<Lun>) -> Arc<Adapter> {
        let state = State { luns, sense_size: 96, cdb_size: 32, ..State::default() };
        Arc::new(Adapter { state: Mutex::new(state) })
    }

    /// Takes the commands the driver made available, and answers them
    /// unless it holds them.
    fn take(s: &mut State) {
        while let Some((head, buffers)) = next_chain(&mut s.rings[2]) {
            s.shapes.push(buffers.iter().map(|b| b.2).collect());
            if s.holding {
                s.held.push((head, buffers));
            } else {
                Adapter::answer(s, head, &buffers);
            }
        }
    }

    /// Answers every command held, in order.
    fn release(&self) {
        let mut s = self.state.lock().unwrap();
        s.holding = false;
        for (head, buffers) in std::mem::take(&mut s.held) {
            Adapter::answer(&mut s, head, &buffers);
        }
    }

    fn answer(s: &mut State, head: u16, buffers: &[Buffer]) {
        let (request_at, request_len, writes) = buffers[0];
        assert!(!writes, "the request comes first");
        assert_eq!(request_len, 19 + s.cdb_size);
        let request = read_mem(request_at, request_len as usize);
        assert_eq!(request[0], 1, "the address's first byte");
        assert_eq!(&request[8..16], &u64::from(head).to_le_bytes(), "the tag is the head");
        let at = Address { target: request[1], lun: u16::from_be_bytes([request[2], request[3]]) };
        assert_eq!(at.lun & 0xc000, 0x4000, "flat space addressing");
        let at = Address { lun: at.lun & 0x3fff, ..at };
        let cdb = &request[19..];
        // The data out between the request and the response, the data in
        // after it.
        let response_index = buffers.iter().position(|b| b.2).expect("a response");
        let (response_at, response_len, _) = buffers[response_index];
        assert_eq!(response_len, 12 + s.sense_size);
        let data_out: Vec<_> = buffers[1..response_index].to_vec();
        let data_in: Vec<_> = buffers[response_index + 1..].to_vec();
        assert!(data_in.iter().all(|b| b.2) && data_out.iter().all(|b| !b.2));
        s.commands.push((at, cdb[0]));
        let mut fields = [0u8; 12];
        let mut sense: &[u8] = &[];
        let mut residual = 0u32;
        let mut written = 0u32;
        match s.luns.iter_mut().find(|lun| lun.at == at) {
            None => fields[11] = response::BAD_TARGET,
            Some(lun) => {
                let mut reply: Vec<u8> = Vec::new();
                let block_of = |cdb: &[u8]| {
                    let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]) as usize;
                    let count = u16::from_be_bytes([cdb[7], cdb[8]]) as usize;
                    (lba * 512, count * 512)
                };
                let ok = match cdb[0] {
                    0x00 | 0x35 => true,
                    0x12 => {
                        reply = std::vec![0, 0, 5, 2, 31, 0, 0, 0];
                        reply.extend_from_slice(b"QEMU    QEMU HARDDISK   2.5+");
                        true
                    }
                    0x25 => {
                        let last = (lun.blocks.len() / 512 - 1) as u32;
                        reply.extend_from_slice(&last.to_be_bytes());
                        reply.extend_from_slice(&512u32.to_be_bytes());
                        true
                    }
                    0x28 => {
                        let (from, len) = block_of(cdb);
                        reply = lun.blocks[from..from + len].to_vec();
                        true
                    }
                    0x2a => {
                        let (mut to, _) = block_of(cdb);
                        for &(phys, len, _) in &data_out {
                            let bytes = read_mem(phys, len as usize);
                            lun.blocks[to..to + len as usize].copy_from_slice(&bytes);
                            to += len as usize;
                        }
                        true
                    }
                    _ => false,
                };
                if ok {
                    // The reply spread over the buffers in, as far as
                    // they go; what does not fit is the residual.
                    let room: usize = data_in.iter().map(|b| b.1 as usize).sum();
                    let mut left = &reply[..reply.len().min(room)];
                    for &(phys, len, _) in &data_in {
                        let n = left.len().min(len as usize);
                        fake_memory_write(phys, &left[..n]);
                        left = &left[n..];
                    }
                    written = reply.len().min(room) as u32;
                    residual = (room - written as usize) as u32;
                } else {
                    fields[10] = scsi_status::CHECK_CONDITION;
                    sense = &ILLEGAL;
                }
            }
        }
        fields[0..4].copy_from_slice(&(sense.len() as u32).to_le_bytes());
        fields[4..8].copy_from_slice(&residual.to_le_bytes());
        fake_memory_write(response_at, &fields);
        fake_memory_write(response_at + 12, sense);
        put_used(&mut s.rings[2], head, 12 + sense.len() as u32 + written);
    }
}

const FEATURES: u64 = common_feature::VERSION_1 | common_feature::EVENT_IDX;

impl Registers for Adapter {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let s = self.state.lock().unwrap();
        match (offset, size) {
            (0x04, 4) => (FEATURES >> (32 * s.device_select)) & 0xffff_ffff,
            (0x12, 2) => 3,
            (0x14, 1) => u64::from(s.status),
            (0x15, 1) => 0,
            (0x18, 2) => 64,
            (0x1e, 2) => u64::from(s.select),
            (0x1a, 2) => 0,
            // One request queue, 126 segments, 64 KiB a command, 128
            // commands a logical unit, 8 bytes of events.
            (0x2000, 4) => 1,
            (0x2004, 4) => 126,
            (0x2008, 4) => 128,
            (0x200c, 4) => 128,
            (0x2010, 4) => 8,
            (0x2014, 4) => u64::from(s.sense_size),
            (0x2018, 4) => u64::from(s.cdb_size),
            (0x201c, 2) => 0,
            (0x201e, 2) => 2,
            (0x2020, 4) => 16383,
            _ => 0,
        }
    }

    fn write(&self, offset: usize, _size: usize, value: u64) {
        let mut s = self.state.lock().unwrap();
        let select = usize::from(s.select);
        match offset {
            0x00 => s.device_select = value as u32,
            0x08 => s.driver_select = value as u32,
            0x0c => {
                let shift = 32 * s.driver_select;
                s.driver_features =
                    (s.driver_features & !(0xffff_ffff << shift)) | (value << shift);
            }
            0x14 => {
                let value = value as u8;
                if value & status::FEATURES_OK != 0 && s.driver_features & !FEATURES != 0 {
                    s.status = value & !status::FEATURES_OK;
                } else {
                    s.status = value;
                }
                if value == 0 {
                    s.rings = [Ring::default(); 3];
                    s.sense_size = 96;
                    s.cdb_size = 32;
                }
            }
            0x16 => s.select = value as u16,
            0x18 => s.rings[select].size = value as u16,
            0x20 => s.rings[select].desc = value,
            0x28 => s.rings[select].driver = value,
            0x30 => s.rings[select].device = value,
            0x2014 => s.sense_size = value as u32,
            0x2018 => s.cdb_size = value as u32,
            0x3000 | 0x3004 => panic!("the control and event queues are not used"),
            0x3008 => {
                assert!(s.status & status::DRIVER_OK != 0, "sent before ready");
                Adapter::take(&mut s);
            }
            _ => {}
        }
    }
}

/// Disks at target 0's logical units 0 and 3, and at target 2's 0, of 64
/// blocks each, each block filled with its number and the disk's.
fn disks() -> Vec<Lun> {
    let at = [(0, 0), (0, 3), (2, 0)];
    at.iter()
        .enumerate()
        .map(|(n, &(target, lun))| Lun {
            at: Address { target, lun },
            blocks: (0..64 * 512).map(|i| ((i / 512) as u8) ^ ((n as u8) << 6)).collect(),
        })
        .collect()
}

fn setup() -> (Arc<Adapter>, Scsi) {
    let adapter = Adapter::new(disks());
    // Sizes a driver before changed, which this one sets back.
    adapter.state.lock().unwrap().sense_size = 32;
    let f = function(8, 0x1048);
    let regs = adapter.clone();
    let transport =
        Transport::find(&f, &mut |_, _| Some(Mmio::fake(regs.clone(), BAR_SIZE))).unwrap();
    let scsi = Scsi::new(transport, 0, 64, &mut alloc).unwrap();
    (adapter, scsi)
}

/// Sends `cdb` to `at` and returns how it ended, with `data` (into a
/// page of its own, for a command that answers).
fn command(
    scsi: &mut Scsi,
    at: Address,
    cdb: &[u8],
    data: &[(u64, u32)],
    dir: Direction,
) -> Answer {
    let token = scsi.submit_held(at, cdb, data, dir).unwrap();
    scsi.notify();
    let (done, answer) = scsi.complete().expect("answered at once");
    assert_eq!(done, token);
    answer
}

fn read10(lba: u32, count: u16) -> [u8; 10] {
    let lba = lba.to_be_bytes();
    let count = count.to_be_bytes();
    [0x28, 0, lba[0], lba[1], lba[2], lba[3], 0, count[0], count[1], 0]
}

#[test]
fn finds_the_adapter() {
    assert_eq!(device_type(&function(8, 0x1048)), Some(device_type::SCSI));
    // A transitional one says its type in its subsystem ID.
    assert_eq!(device_type(&function(8, 0x1004)), Some(device_type::SCSI));
}

#[test]
fn reads_its_configuration_and_sets_the_sizes() {
    let (adapter, scsi) = setup();
    // The segments the adapter takes, but no more than fit in the ring
    // without indirect descriptors, which it does not offer.
    assert_eq!(
        scsi.info(),
        Info { max_target: 2, max_lun: 16383, max_segments: 30, max_sectors: 128 }
    );
    let s = adapter.state.lock().unwrap();
    assert_eq!((s.sense_size, s.cdb_size), (96, 32));
    assert_eq!(s.status & status::DRIVER_OK, status::DRIVER_OK);
    // Only the request queue was set up.
    assert_eq!(s.rings[0].size, 0);
    assert_eq!(s.rings[1].size, 0);
    assert_eq!(s.rings[2].size, 64);
}

#[test]
fn needs_a_request_queue() {
    // An adapter of two queues has none for requests.
    struct Two(Arc<Adapter>);
    impl Registers for Two {
        fn read(&self, offset: usize, size: usize) -> u64 {
            if offset == 0x12 { 2 } else { self.0.read(offset, size) }
        }
        fn write(&self, offset: usize, size: usize, value: u64) {
            self.0.write(offset, size, value);
        }
    }
    let regs = Arc::new(Two(Adapter::new(disks())));
    let f = function(8, 0x1048);
    let transport =
        Transport::find(&f, &mut |_, _| Some(Mmio::fake(regs.clone(), BAR_SIZE))).unwrap();
    assert!(matches!(Scsi::new(transport, 0, 64, &mut alloc), Err(Error::NoQueue)));
}

#[test]
fn asks_a_logical_unit_what_it_is() {
    let (adapter, mut scsi) = setup();
    let page = alloc(1).unwrap();
    let at = Address { target: 0, lun: 3 };
    let inquiry = [0x12, 0, 0, 0, 96, 0];
    let answer = command(&mut scsi, at, &inquiry, &[(page.phys(), 96)], Direction::FromDevice);
    assert_eq!((answer.response, answer.status), (response::OK, scsi_status::GOOD));
    // It answered 36 bytes of the 96.
    assert_eq!(answer.residual, 96 - 36);
    let mut got = [0u8; 36];
    page.read(0, &mut got);
    assert_eq!(&got[8..], b"QEMU    QEMU HARDDISK   2.5+");
    let s = adapter.state.lock().unwrap();
    assert_eq!(s.commands, [(at, 0x12)]);
    // The request, the response, then the data in.
    assert_eq!(s.shapes, [std::vec![false, true, true]]);
}

#[test]
fn reads_and_writes_blocks() {
    let (adapter, mut scsi) = setup();
    let pages = [alloc(1).unwrap(), alloc(1).unwrap()];
    let at = Address { target: 2, lun: 0 };
    // Blocks 4 to 11 into two pages.
    let data = [(pages[0].phys(), 4096), (pages[1].phys(), 4096)];
    let answer = command(&mut scsi, at, &read10(4, 16), &data, Direction::FromDevice);
    assert!(answer.good(), "{answer:?}");
    let mut block = [0u8; 512];
    pages[1].read(512, &mut block);
    // Block 13 of the third disk.
    assert!(block.iter().all(|&b| b == 0x0d ^ (2 << 6)));
    // Written to blocks 40 to 47 of the first disk from the first page.
    pages[0].write(0, &[0x5a; 4096]);
    let write = [0x2a, 0, 0, 0, 0, 40, 0, 0, 8, 0];
    let answer =
        command(&mut scsi, Address { target: 0, lun: 0 }, &write, &data[..1], Direction::ToDevice);
    assert!(answer.good());
    let s = adapter.state.lock().unwrap();
    assert!(s.luns[0].blocks[40 * 512..48 * 512].iter().all(|&b| b == 0x5a));
    assert!(s.luns[0].blocks[48 * 512..49 * 512].iter().all(|&b| b == 48));
    // The request, the data out, then the response.
    assert_eq!(s.shapes[1], [false, false, true]);
}

#[test]
fn says_a_target_is_missing() {
    let (_, mut scsi) = setup();
    let answer = command(
        &mut scsi,
        Address { target: 1, lun: 0 },
        &[0, 0, 0, 0, 0, 0],
        &[],
        Direction::FromDevice,
    );
    assert_eq!(answer.response, response::BAD_TARGET);
    assert!(!answer.good());
}

#[test]
fn gives_the_sense_data_of_a_failed_command() {
    let (_, mut scsi) = setup();
    let answer = command(
        &mut scsi,
        Address { target: 0, lun: 0 },
        &[0x5a, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[],
        Direction::FromDevice,
    );
    assert_eq!((answer.response, answer.status), (response::OK, scsi_status::CHECK_CONDITION));
    assert_eq!(answer.sense(), ILLEGAL);
}

#[test]
fn refuses_commands_outside_its_limits() {
    let (_, mut scsi) = setup();
    let page = alloc(1).unwrap();
    let at = Address { target: 0, lun: 0 };
    let data = [(page.phys(), 4096)];
    // No command, a command too long, a target or a unit past the last, a
    // command of more than 128 sectors, more segments than it takes, and
    // an empty buffer.
    let long = [0u8; 33];
    let wide: Vec<(u64, u32)> = (0..31).map(|i| (page.phys() + i * 8, 8)).collect();
    type Case<'a> = (Address, &'a [u8], &'a [(u64, u32)]);
    let cases: [Case<'_>; 7] = [
        (at, &[], &data),
        (at, &long, &data),
        (Address { target: 3, lun: 0 }, &read10(0, 1), &data),
        (Address { target: 0, lun: 16384 }, &read10(0, 1), &data),
        (at, &read10(0, 1), &[(page.phys(), 65536), (page.phys(), 512)]),
        (at, &read10(0, 1), &wide),
        (at, &read10(0, 1), &[(page.phys(), 0)]),
    ];
    for (at, cdb, data) in cases {
        let refused = scsi.submit_held(at, cdb, data, Direction::FromDevice);
        assert_eq!(refused, Err(Error::BadRequest), "{at:?} {}", cdb.len());
    }
}

#[test]
fn holds_as_many_commands_as_fit() {
    let (adapter, mut scsi) = setup();
    adapter.state.lock().unwrap().holding = true;
    let page = alloc(1).unwrap();
    let at = Address { target: 0, lun: 0 };
    // Three descriptors a command in a ring of 64: 21.
    assert_eq!(scsi.room(1), 21);
    let mut tokens = Vec::new();
    while scsi.room(1) > 0 {
        let data = [(page.phys(), 512)];
        tokens.push(scsi.submit_held(at, &read10(0, 1), &data, Direction::FromDevice).unwrap());
    }
    assert_eq!(tokens.len(), 21);
    let data = [(page.phys(), 512)];
    let full = scsi.submit_held(at, &read10(0, 1), &data, Direction::FromDevice);
    assert_eq!(full, Err(Error::Full));
    scsi.notify();
    assert!(scsi.complete().is_none(), "nothing answered yet");
    adapter.release();
    let mut done = Vec::new();
    while let Some((token, answer)) = scsi.complete() {
        assert!(answer.good());
        done.push(token);
    }
    assert_eq!(done, tokens);
    assert_eq!(scsi.room(1), 21);
}

#[test]
fn resets_the_adapter_when_dropped() {
    let (adapter, scsi) = setup();
    drop(scsi);
    assert_eq!(adapter.state.lock().unwrap().status, 0);
}
