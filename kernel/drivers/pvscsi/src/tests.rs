// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver against a simulated PVSCSI adapter, which acts as FreeBSD's
//! driver expects VMware's to: it takes commands and their words through
//! its registers, says how big it took each ring in the rings' state, and
//! on a doorbell carries every waiting request to a disk at target 0 of
//! 64 blocks, answering each on the completion ring and raising its
//! interrupt.

use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers, fake_memory, fake_memory_write};

use crate::{
    Address, CONTEXTS, Direction, Error, MAX_SEGMENTS, Message, PAGE, Pvscsi, REGISTERS,
    host_status, is_read_write, scsi_status,
};

const COMMAND: usize = 0x0000;
const COMMAND_DATA: usize = 0x0004;
const COMMAND_STATUS: usize = 0x0008;
const INTERRUPT_STATUS: usize = 0x100c;
const INTERRUPT_MASK: usize = 0x2010;
const KICK_OTHER: usize = 0x3014;
const KICK_READ_WRITE: usize = 0x4018;

const ADAPTER_RESET: u32 = 1;
const SETUP_RINGS: u32 = 3;
const SETUP_MESSAGE_RING: u32 = 8;
const GET_MAX_TARGETS: u32 = 11;

/// The simulated disk's blocks.
const BLOCKS: usize = 64;

fn alloc(pages: usize) -> Option<Dma> {
    Some(Dma::fake(pages * PAGE))
}

fn read(phys: u64, len: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; len];
    fake_memory(phys, &mut out);
    out
}

fn u32_at(phys: u64) -> u32 {
    u32::from_le_bytes(read(phys, 4).try_into().unwrap())
}

fn u64_at(phys: u64) -> u64 {
    u64::from_le_bytes(read(phys, 8).try_into().unwrap())
}

fn put32(phys: u64, value: u32) {
    fake_memory_write(phys, &value.to_le_bytes());
}

/// A request as the adapter read it.
#[derive(Clone, Debug)]
struct Seen {
    target: u8,
    lun: u8,
    cdb: Vec<u8>,
    flags: u32,
}

#[derive(Default)]
struct State {
    /// Whether the adapter knows SETUP_MSG_RING, and takes its rings.
    messages: bool,
    takes_rings: bool,
    targets: u32,
    command: u32,
    words: Vec<u32>,
    status: u32,
    resets: usize,
    mask: u32,
    raised: u32,
    /// The rings' state page, and each ring's first page and size.
    state: u64,
    requests: Vec<u64>,
    completions: Vec<u64>,
    message_pages: Vec<u64>,
    disk: Vec<u8>,
    seen: Vec<Seen>,
    kicks: (usize, usize),
    /// Whether requests wait for a test to answer them.
    holding: bool,
}

struct Adapter(Mutex<State>);

impl Adapter {
    fn new(messages: bool) -> Arc<Adapter> {
        Arc::new(Adapter(Mutex::new(State {
            messages,
            takes_rings: true,
            targets: 16,
            disk: (0..BLOCKS * 512).map(|n| (n / 512) as u8).collect(),
            ..State::default()
        })))
    }

    fn regs(self: &Arc<Adapter>) -> Mmio {
        Mmio::fake(self.clone(), 0x8000)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }

    /// Sends a message that a disk came or went.
    fn send_message(&self, kind: u32, target: u32, lun: u8) {
        let mut s = self.state();
        let produced = u32_at(s.state + 128);
        let at = s.message_pages[0] + u64::from(produced % 32) * 128;
        put32(at, kind);
        put32(at + 4, 0);
        put32(at + 8, target);
        fake_memory_write(at + 12, &[0, lun, 0, 0, 0, 0, 0, 0]);
        put32(s.state + 128, produced + 1);
        s.raised |= 0b0100;
    }

    /// Answers every request waiting.
    fn serve(&self) {
        let mut s = self.state();
        let produced = u32_at(s.state);
        let mut consumed = u32_at(s.state + 4);
        while consumed != produced {
            let page = (consumed as usize % 256) / 32;
            let at = s.requests[page] + ((consumed as usize % 32) * 128) as u64;
            consumed += 1;
            put32(s.state + 4, consumed);
            let answer = carry(&mut s, at);
            let done = u32_at(s.state + 12);
            let page = (done as usize % 256) / 128;
            let slot = s.completions[page] + ((done as usize % 128) * 32) as u64;
            fake_memory_write(slot, &u64_at(at).to_le_bytes());
            fake_memory_write(slot + 8, &answer.0.to_le_bytes());
            put32(slot + 16, answer.1);
            fake_memory_write(slot + 20, &answer.2.to_le_bytes());
            fake_memory_write(slot + 22, &answer.3.to_le_bytes());
            put32(s.state + 12, done + 1);
            s.raised |= 0b0001;
        }
    }
}

/// Carries the request at `at` to the disk: how much it moved, its sense
/// data's length, and the adapter's and the target's statuses.
fn carry(s: &mut State, at: u64) -> (u64, u32, u16, u16) {
    let flags = u32_at(at + 36);
    let cdb_len = read(at + 56, 1)[0] as usize;
    let cdb = read(at + 40, cdb_len);
    let lun = read(at + 57, 8);
    let target = read(at + 67, 1)[0];
    s.seen.push(Seen { target, lun: lun[1], cdb: cdb.clone(), flags });
    if target != 0 || lun[1] != 0 {
        return (0, 0, host_status::SELECTION_TIMEOUT, 0);
    }
    // Its runs: one, or a list of them.
    let (address, length) = (u64_at(at + 8), u64_at(at + 16));
    let runs: Vec<(u64, usize)> = if flags & 1 != 0 {
        let mut runs = Vec::new();
        let mut left = length;
        let mut n = 0;
        while left > 0 {
            let run = (u64_at(address + n * 16), u32_at(address + n * 16 + 8) as usize);
            left -= run.1 as u64;
            runs.push(run);
            n += 1;
        }
        runs
    } else if length > 0 {
        std::vec![(address, length as usize)]
    } else {
        Vec::new()
    };
    match cdb[0] {
        // TEST UNIT READY
        0x00 => (0, 0, 0, scsi_status::GOOD),
        // READ(10) and WRITE(10)
        0x28 | 0x2a => {
            let lba = u32::from_be_bytes(cdb[2..6].try_into().unwrap()) as usize;
            let mut offset = lba * 512;
            for (phys, len) in runs {
                if cdb[0] == 0x28 {
                    fake_memory_write(phys, &s.disk[offset..offset + len]);
                } else {
                    let data = read(phys, len);
                    s.disk[offset..offset + len].copy_from_slice(&data);
                }
                offset += len;
            }
            // Like some adapters, it leaves the count at zero for a
            // command that ended well: the driver takes it as all moved.
            (0, 0, 0, scsi_status::GOOD)
        }
        // Anything else: ILLEGAL REQUEST, INVALID COMMAND OPERATION CODE.
        _ => {
            let sense = [0x70, 0, 0x05, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0x20, 0x00];
            fake_memory_write(u64_at(at + 24), &sense);
            (0, sense.len() as u32, 0, scsi_status::CHECK_CONDITION)
        }
    }
}

impl Registers for Adapter {
    fn read(&self, offset: usize, _size: usize) -> u64 {
        let s = self.state();
        u64::from(match offset {
            COMMAND_STATUS => s.status,
            INTERRUPT_STATUS => s.raised,
            INTERRUPT_MASK => s.mask,
            _ => 0,
        })
    }

    fn write(&self, offset: usize, _size: usize, value: u64) {
        let value = value as u32;
        match offset {
            COMMAND => {
                let mut s = self.state();
                s.command = value;
                s.words.clear();
                s.status = match value {
                    ADAPTER_RESET => {
                        s.resets += 1;
                        0
                    }
                    GET_MAX_TARGETS => s.targets,
                    SETUP_MESSAGE_RING if !s.messages => u32::MAX,
                    _ => 0,
                };
            }
            COMMAND_DATA => {
                let mut s = self.state();
                s.words.push(value);
                let page = |words: &[u32], at: usize| {
                    (u64::from(words[at]) | (u64::from(words[at + 1]) << 32)) * PAGE as u64
                };
                if s.command == SETUP_RINGS && s.words.len() == 132 {
                    let words = s.words.clone();
                    s.state = page(&words, 2);
                    s.requests = (0..words[0] as usize).map(|p| page(&words, 4 + 2 * p)).collect();
                    s.completions =
                        (0..words[1] as usize).map(|p| page(&words, 68 + 2 * p)).collect();
                    if s.takes_rings {
                        put32(s.state + 8, (words[0] as usize * 32).trailing_zeros());
                        put32(s.state + 20, (words[1] as usize * 128).trailing_zeros());
                    }
                } else if s.command == SETUP_MESSAGE_RING && s.words.len() == 34 {
                    let words = s.words.clone();
                    s.message_pages =
                        (0..words[0] as usize).map(|p| page(&words, 2 + 2 * p)).collect();
                    put32(s.state + 136, (words[0] as usize * 32).trailing_zeros());
                }
            }
            INTERRUPT_STATUS => self.state().raised &= !value,
            INTERRUPT_MASK => self.state().mask = value,
            KICK_OTHER | KICK_READ_WRITE => {
                let holding = {
                    let mut s = self.state();
                    if offset == KICK_OTHER {
                        s.kicks.0 += 1;
                    } else {
                        s.kicks.1 += 1;
                    }
                    s.holding
                };
                if !holding {
                    self.serve();
                }
            }
            _ => {}
        }
    }
}

fn read10(lba: u32, blocks: u16) -> [u8; 10] {
    let mut cdb = [0x28, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    cdb[2..6].copy_from_slice(&lba.to_be_bytes());
    cdb[7..9].copy_from_slice(&blocks.to_be_bytes());
    cdb
}

fn write10(lba: u32, blocks: u16) -> [u8; 10] {
    let mut cdb = read10(lba, blocks);
    cdb[0] = 0x2a;
    cdb
}

const DISK: Address = Address { target: 0, lun: 0 };

#[test]
fn the_adapter_is_reset_and_given_its_rings() {
    let adapter = Adapter::new(true);
    let pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    let s = adapter.state();
    assert_eq!(s.resets, 1);
    assert_eq!((s.requests.len(), s.completions.len(), s.message_pages.len()), (8, 2, 1));
    // The pages are each ring's own, in order.
    assert_eq!(s.requests[1], s.requests[0] + PAGE as u64);
    assert_eq!(pvscsi.targets(), 16);
    assert!(pvscsi.has_messages());
    // No interrupt until asked.
    assert_eq!(s.mask, 0);
    drop(s);
    pvscsi.enable_interrupts();
    assert_eq!(adapter.state().mask, 0b1111);
    pvscsi.disable_interrupts();
    assert_eq!(adapter.state().mask, 0);
}

#[test]
fn an_adapter_without_a_message_ring_interrupts_for_completions_alone() {
    let adapter = Adapter::new(false);
    adapter.state().targets = u32::MAX;
    let pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    assert!(!pvscsi.has_messages());
    // An adapter that does not say has 16 targets.
    assert_eq!(pvscsi.targets(), 16);
    assert!(adapter.state().message_pages.is_empty());
    pvscsi.enable_interrupts();
    assert_eq!(adapter.state().mask, 0b0011);
}

#[test]
fn an_adapter_that_does_not_take_its_rings_is_refused() {
    let adapter = Adapter::new(true);
    adapter.state().takes_rings = false;
    assert_eq!(
        Pvscsi::new(adapter.regs(), &mut alloc).err(),
        Some(Error::Device("the adapter did not take its rings"))
    );
    let small = Mmio::fake(adapter.clone(), REGISTERS - 4);
    assert_eq!(Pvscsi::new(small, &mut alloc).err(), Some(Error::Registers));
    let mut none = |_: usize| None;
    assert_eq!(Pvscsi::new(adapter.regs(), &mut none).err(), Some(Error::OutOfMemory));
}

#[test]
fn reads_and_writes_go_through_one_run_or_a_list() {
    let adapter = Adapter::new(true);
    let mut pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    // A read of blocks 3 and 4 into one run.
    let buffer = Dma::fake(PAGE);
    let context = pvscsi
        .submit(DISK, &read10(3, 2), &[(buffer.phys(), 1024)], Direction::FromDevice)
        .unwrap();
    pvscsi.kick(true);
    let (done, answer) = pvscsi.complete().unwrap();
    assert_eq!(done, context);
    assert!(answer.good());
    assert_eq!((answer.moved, answer.asked), (1024, 1024));
    let mut data = [0u8; 1024];
    buffer.read(0, &mut data);
    assert!(data[..512].iter().all(|&b| b == 3) && data[512..].iter().all(|&b| b == 4));
    // A write of three blocks from three runs, through the list.
    let pages = [Dma::fake(PAGE), Dma::fake(PAGE), Dma::fake(PAGE)];
    for (n, page) in pages.iter().enumerate() {
        page.write(0, &[0xa0 + n as u8; 512]);
    }
    let runs: Vec<(u64, u32)> = pages.iter().map(|page| (page.phys(), 512)).collect();
    pvscsi.submit(DISK, &write10(10, 3), &runs, Direction::ToDevice).unwrap();
    pvscsi.kick(true);
    assert!(pvscsi.complete().unwrap().1.good());
    let s = adapter.state();
    assert_eq!(&s.disk[10 * 512..10 * 512 + 2], &[0xa0, 0xa0]);
    assert_eq!(s.disk[12 * 512 + 511], 0xa2);
    // The flags: a read to the host in one run, a write to the device
    // through a list.
    assert_eq!(s.seen[0].flags, 1 << 3);
    assert_eq!(s.seen[1].flags, (1 << 4) | 1);
    assert_eq!(s.kicks, (0, 2));
}

#[test]
fn a_command_that_fails_brings_its_sense_data() {
    let adapter = Adapter::new(true);
    let mut pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    // TEST UNIT READY moves nothing and goes on the other doorbell.
    pvscsi.submit(DISK, &[0; 6], &[], Direction::None).unwrap();
    pvscsi.kick(is_read_write(0x00));
    assert!(pvscsi.complete().unwrap().1.good());
    assert_eq!(adapter.state().seen[0].flags, 1 << 2);
    // An operation the disk does not know.
    pvscsi.submit(DISK, &[0xc5, 0, 0, 0, 0, 0], &[], Direction::None).unwrap();
    pvscsi.kick(false);
    let (_, answer) = pvscsi.complete().unwrap();
    assert!(!answer.good());
    assert_eq!(answer.scsi_status, scsi_status::CHECK_CONDITION);
    assert_eq!(answer.sense()[2], 0x05);
    assert_eq!(answer.sense()[12], 0x20);
    assert_eq!(adapter.state().kicks, (2, 0));
    // Nothing at target 3.
    let nowhere = Address { target: 3, lun: 0 };
    pvscsi.submit(nowhere, &[0; 6], &[], Direction::None).unwrap();
    pvscsi.kick(false);
    let (_, answer) = pvscsi.complete().unwrap();
    assert_eq!(answer.host_status, host_status::SELECTION_TIMEOUT);
    assert!(answer.sense().is_empty());
    let seen = adapter.state().seen.last().cloned().unwrap();
    assert_eq!((seen.target, seen.lun, seen.cdb.len()), (3, 0, 6));
}

#[test]
fn every_context_can_be_in_flight_and_comes_back() {
    let adapter = Adapter::new(true);
    adapter.state().holding = true;
    let mut pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    let buffer = Dma::fake(PAGE);
    let mut contexts = Vec::new();
    for n in 0..CONTEXTS {
        assert_eq!(pvscsi.room(), CONTEXTS - n);
        let run = [(buffer.phys(), 512)];
        contexts.push(pvscsi.submit(DISK, &read10(0, 1), &run, Direction::FromDevice).unwrap());
    }
    assert_eq!(pvscsi.room(), 0);
    let run = [(buffer.phys(), 512)];
    assert_eq!(pvscsi.submit(DISK, &read10(0, 1), &run, Direction::FromDevice), Err(Error::Full));
    contexts.sort_unstable();
    contexts.dedup();
    assert_eq!(contexts.len(), CONTEXTS);
    adapter.serve();
    let mut done = 0;
    while let Some((_, answer)) = pvscsi.complete() {
        assert!(answer.good());
        done += 1;
    }
    assert_eq!(done, CONTEXTS);
    assert_eq!(pvscsi.room(), CONTEXTS);
    // The rings wrap: another round goes through.
    adapter.state().holding = false;
    for _ in 0..3 {
        pvscsi.submit(DISK, &read10(1, 1), &run, Direction::FromDevice).unwrap();
        pvscsi.kick(true);
        assert!(pvscsi.complete().unwrap().1.good());
    }
}

#[test]
fn commands_it_cannot_carry_are_refused() {
    let adapter = Adapter::new(true);
    let mut pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    let long = [0u8; 17];
    assert_eq!(pvscsi.submit(DISK, &long, &[], Direction::None), Err(Error::BadRequest));
    assert_eq!(pvscsi.submit(DISK, &[], &[], Direction::None), Err(Error::BadRequest));
    let runs = std::vec![(0x1000u64, 512u32); MAX_SEGMENTS + 1];
    assert_eq!(
        pvscsi.submit(DISK, &write10(0, 33), &runs, Direction::ToDevice),
        Err(Error::BadRequest)
    );
    assert_eq!(pvscsi.room(), CONTEXTS);
}

#[test]
fn messages_say_disks_came_and_went_and_interrupts_are_cleared() {
    let adapter = Adapter::new(true);
    let mut pvscsi = Pvscsi::new(adapter.regs(), &mut alloc).unwrap();
    assert_eq!(pvscsi.message(), None);
    adapter.send_message(0, 4, 0);
    adapter.send_message(1, 2, 1);
    adapter.send_message(7, 0, 0);
    assert_eq!(pvscsi.acknowledge(), 0b0100);
    assert_eq!(pvscsi.acknowledge(), 0);
    assert_eq!(pvscsi.message(), Some(Message::Added(Address { target: 4, lun: 0 })));
    assert_eq!(pvscsi.message(), Some(Message::Removed(Address { target: 2, lun: 1 })));
    assert_eq!(pvscsi.message(), Some(Message::Other(7)));
    assert_eq!(pvscsi.message(), None);
    // A completion raises the other bits.
    pvscsi.submit(DISK, &[0; 6], &[], Direction::None).unwrap();
    pvscsi.kick(false);
    assert_eq!(pvscsi.acknowledge(), 0b0001);
    assert!(pvscsi.complete().unwrap().1.good());
    // A reset drops what was in flight.
    adapter.state().holding = true;
    pvscsi.submit(DISK, &[0; 6], &[], Direction::None).unwrap();
    pvscsi.reset();
    assert_eq!(adapter.state().resets, 2);
    assert_eq!(pvscsi.room(), CONTEXTS - 1);
}

#[test]
fn reads_and_writes_are_told_from_other_commands() {
    for opcode in [0x08, 0x0a, 0x28, 0x2a, 0xa8, 0xaa, 0x88, 0x8a] {
        assert!(is_read_write(opcode), "{opcode:#x}");
    }
    for opcode in [0x00, 0x12, 0x25, 0x35, 0x9e, 0xa0] {
        assert!(!is_read_write(opcode), "{opcode:#x}");
    }
}
