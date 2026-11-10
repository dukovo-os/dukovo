// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver against a simulated MANA function, which acts as FreeBSD's
//! mana(4) expects the card and the host's driver to: it sets the
//! hardware channel up from the shared memory message with the host's
//! events, answers each request as its send queue's doorbell rings, with
//! the answer in the posted buffer and a completion for each queue, takes
//! frames from the data send queue as its doorbell rings and writes
//! frames into the posted receive buffers when a test delivers one, each
//! completion and event with its owner bits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers, fake_memory, fake_memory_write};
use radix_nic::{Error, Interrupt, Nic, PAGE};

use crate::gdma::establish_message;
use crate::{DEVICE, Mana, VENDOR, identify, start};

const MAC: [u8; 6] = [0x00, 0x0d, 0x3a, 0x12, 0x34, 0x56];
/// Where the simulated function puts its doorbell pages and its shared
/// memory, and the pages it gives the hardware channel and the device.
const DOORBELLS: usize = 0x2000;
const DOORBELL_SIZE: usize = 0x1000;
const SHM: usize = 0x100;
const BAR: usize = DOORBELLS + 8 * DOORBELL_SIZE;
const HWC_DOORBELL: u32 = 1;
const MANA_DOORBELL: u32 = 3;
/// The hardware channel's queues.
const HWC_EQ: u32 = 7;
const HWC_CQ: u32 = 8;
const HWC_RQ: u32 = 9;
const HWC_SQ: u32 = 10;
const HWC_KEY: u32 = 0x66;
/// The MANA device: instance 1, its protection domain and memory key.
const MANA_ID: u32 = 2 | 1 << 16;
const PDID: u32 = 0x55;
const KEY: u32 = 0x77;
const VPORT: u64 = 0x1234_5678_9abc;
/// The data queues' numbers, and the work queue objects' handles.
const DATA_EQ: u32 = 20;
const SQ: u32 = 30;
const TX_CQ: u32 = 31;
const RQ: u32 = 40;
const RX_CQ: u32 = 41;
const TX_OBJECT: u64 = 0xa1;
const RX_OBJECT: u64 = 0xa2;

const QUERY_DEV_CONFIG: u32 = 0x2_0001;
const CONFIG_VPORT_TX: u32 = 0x2_0003;
const CREATE_WQ_OBJ: u32 = 0x2_0004;
const DESTROY_WQ_OBJ: u32 = 0x2_0005;
const FENCE_RQ: u32 = 0x2_0006;
const CONFIG_VPORT_RX: u32 = 0x2_0007;
const QUERY_VPORT_CONFIG: u32 = 0x2_0008;

fn alloc(pages: usize) -> Option<Dma> {
    Some(Dma::fake(pages * PAGE))
}

fn no_delay(_: u32) {}

fn read(phys: u64, len: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; len];
    fake_memory(phys, &mut out);
    out
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// A queue the card writes entries into, with their owner bits.
#[derive(Clone, Copy, Debug)]
struct Writer {
    base: u64,
    entries: usize,
    entry: usize,
    count: usize,
    /// Whether the driver asked for an event, or an interrupt, at the
    /// next entry.
    armed: bool,
}

impl Writer {
    fn new(base: u64, len: usize, entry: usize) -> Writer {
        Writer { base, entries: len / entry, entry, count: 0, armed: false }
    }

    /// Writes an entry: `data` first, its info word (with the owner bits)
    /// last.
    fn write(&mut self, data: &[u8], info: u32) {
        let at = self.base + ((self.count % self.entries) * self.entry) as u64;
        let owner = ((self.count / self.entries + 1) & 7) as u32;
        let mut entry = std::vec![0u8; self.entry];
        entry[..data.len()].copy_from_slice(data);
        let last = self.entry - 4;
        put32(&mut entry, last, info | owner << 29);
        fake_memory_write(at, &entry);
        self.count += 1;
    }
}

/// A work queue the card reads requests from, as far as its doorbell
/// says.
#[derive(Clone, Copy, Debug)]
struct Reader {
    base: u64,
    len: usize,
    /// Basic units taken, and posted.
    taken: usize,
    posted: usize,
}

/// A work request as the card reads it.
#[derive(Debug)]
struct Wqe {
    oob: Vec<u32>,
    address: u64,
    key: u32,
    len: u32,
}

impl Reader {
    fn new(base: u64, len: usize) -> Reader {
        Reader { base, len, taken: 0, posted: 0 }
    }

    fn bytes(&self, at: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| read(self.base + ((at + i) % self.len) as u64, 1)[0]).collect()
    }

    /// The next request, if one is posted.
    fn take(&mut self) -> Option<Wqe> {
        if self.taken >= self.posted {
            return None;
        }
        let at = self.taken * 32;
        let header = self.bytes(at, 8);
        let flags = u32_at(&header, 4);
        assert_eq!(flags & 0xff, 1, "one scatter-gather entry");
        let oob_len = ((flags >> 8) & 0x7) as usize * 4;
        let oob = self.bytes(at + 8, oob_len);
        let sge = self.bytes(at + 8 + oob_len, 16);
        let units = (8 + oob_len + 16).div_ceil(32);
        self.taken += units;
        Some(Wqe {
            oob: oob.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect(),
            address: u64_at(&sge, 0),
            key: u32_at(&sge, 8),
            len: u32_at(&sge, 12),
        })
    }
}

/// A frame the card sent, with its send request's out-of-band data.
#[derive(Debug)]
struct Sent {
    frame: Vec<u8>,
    oob: Vec<u32>,
}

#[derive(Default)]
struct State {
    smc: [u32; 8],
    /// The shared memory message that set the channel up.
    established: Option<[u8; 32]>,
    destroyed: bool,
    hwc_eq: Option<Writer>,
    hwc_cq: Option<Writer>,
    hwc_rq: Option<Reader>,
    hwc_sq: Option<Reader>,
    /// The requests, by code, in order.
    requests: Vec<u32>,
    /// Requests answered with an error, or never.
    refuse: Option<u32>,
    silent: Option<u32>,
    /// The devices LIST_DEVICES gives.
    no_mana: bool,
    vport_offset: u16,
    regions: HashMap<u64, (u64, usize)>,
    next_region: u64,
    destroyed_regions: Vec<u64>,
    eq: Option<Writer>,
    eq_vector: Option<u32>,
    tx_cq: Option<Writer>,
    rx_cq: Option<Writer>,
    sq: Option<Reader>,
    rq: Option<Reader>,
    /// Whether frames sent are completed.
    hold: bool,
    sent: Vec<Sent>,
    steering: Vec<(bool, u64)>,
    destroyed_objects: Vec<(u32, u64)>,
    disabled: Vec<u32>,
    deregistered: bool,
    /// Interrupts raised.
    interrupts: usize,
}

struct Card(Mutex<State>);

impl Card {
    fn new() -> Arc<Card> {
        let state = State { next_region: 0x1000, ..State::default() };
        Arc::new(Card(Mutex::new(state)))
    }

    fn mmio(self: &Arc<Self>) -> Mmio {
        Mmio::fake(self.clone(), BAR)
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.0.lock().unwrap())
    }

    /// Writes `frame` into the next receive buffer, as the card does with
    /// a frame from the wire, with a completion of type `kind`.
    fn deliver_as(&self, frame: &[u8], kind: u32) {
        self.with(|s| {
            let wqe = s.rq.as_mut().unwrap().take().expect("a receive buffer");
            assert_eq!(wqe.key, KEY);
            assert!(frame.len() <= wqe.len as usize);
            fake_memory_write(wqe.address, frame);
            let mut data = [0u8; 60];
            put32(&mut data, 0, kind);
            put32(&mut data, 8, frame.len() as u32);
            s.rx_cq.as_mut().unwrap().write(&data, RQ);
            s.raise(RX_CQ);
        });
    }

    fn deliver(&self, frame: &[u8]) {
        self.deliver_as(frame, 1);
    }
}

impl State {
    /// A completion on queue `cq` raises an event if it was armed, and
    /// the event an interrupt if the event queue was.
    fn raise(&mut self, cq: u32) {
        let armed = match cq {
            TX_CQ => self.tx_cq.as_mut().map(|q| core::mem::take(&mut q.armed)),
            _ => self.rx_cq.as_mut().map(|q| core::mem::take(&mut q.armed)),
        };
        if armed == Some(true) {
            let eq = self.eq.as_mut().unwrap();
            let mut data = [0u8; 12];
            put32(&mut data, 0, cq);
            eq.write(&data, 3);
            if core::mem::take(&mut eq.armed) {
                self.interrupts += 1;
            }
        }
    }

    fn smc(&mut self) {
        let header = self.smc[7];
        assert_eq!(header & 0x8000_0000, 0, "the driver writes no owner bit");
        match header & 7 {
            1 => {
                let mut message = [0u8; 32];
                for (i, word) in self.smc.iter().enumerate() {
                    message[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                self.established = Some(message);
                let page = |i: usize| {
                    let mut low = [0u8; 8];
                    low[..6].copy_from_slice(&message[i * 6..i * 6 + 6]);
                    let high = u64::from(u16_at(&message, 24) >> (i * 4) & 0xf);
                    (u64::from_le_bytes(low) | high << 48) << 12
                };
                self.hwc_eq = Some(Writer::new(page(0), PAGE, 16));
                self.hwc_cq = Some(Writer::new(page(1), PAGE, 64));
                self.hwc_rq = Some(Reader::new(page(2), PAGE));
                self.hwc_sq = Some(Reader::new(page(3), PAGE));
                self.smc[7] = 1 | 1 << 7;
                let eq = self.hwc_eq.as_mut().unwrap();
                let mut event = |kind: u32, detail: u32| {
                    let mut data = [0u8; 12];
                    put32(&mut data, 0, detail);
                    eq.write(&data, kind);
                };
                event(129, HWC_EQ | HWC_DOORBELL << 16);
                for (kind, value) in [
                    (1, HWC_CQ),
                    (2, HWC_RQ),
                    (3, HWC_SQ),
                    (4, 1),
                    (5, 0x1000),
                    (6, 0x1000),
                    (7, 64),
                    (8, 0x11),
                    (9, HWC_KEY),
                ] {
                    event(130, kind << 24 | value);
                }
                event(131, 0);
            }
            2 => {
                self.destroyed = true;
                self.smc[7] = 2 | 1 << 7;
            }
            other => panic!("shared memory message {other}"),
        }
    }

    fn doorbell(&mut self, offset: usize, value: u64) {
        let page = ((offset - DOORBELLS) / DOORBELL_SIZE) as u32;
        let within = (offset - DOORBELLS) % DOORBELL_SIZE;
        let tail = (value >> 32) as u32;
        match within {
            0x000 | 0x400 => {
                let id = value as u32 & 0xff_ffff;
                let units = (tail / 32) as usize;
                match (page, id) {
                    (HWC_DOORBELL, HWC_SQ) => {
                        self.hwc_sq.as_mut().unwrap().posted = units;
                        while let Some(wqe) = self.hwc_sq.as_mut().unwrap().take() {
                            self.request(&wqe);
                        }
                    }
                    (HWC_DOORBELL, HWC_RQ) => self.hwc_rq.as_mut().unwrap().posted = units,
                    (MANA_DOORBELL, SQ) => {
                        self.sq.as_mut().unwrap().posted = units;
                        self.transmit();
                    }
                    (MANA_DOORBELL, RQ) => self.rq.as_mut().unwrap().posted = units,
                    other => panic!("work queue doorbell {other:?}"),
                }
            }
            0x800 => {
                let id = value as u32 & 0xff_ffff;
                let arm = value >> 63 == 1;
                match (page, id) {
                    (HWC_DOORBELL, HWC_CQ) => assert!(!arm, "the channel is polled"),
                    (MANA_DOORBELL, TX_CQ) => self.tx_cq.as_mut().unwrap().armed = arm,
                    (MANA_DOORBELL, RX_CQ) => self.rx_cq.as_mut().unwrap().armed = arm,
                    other => panic!("completion queue doorbell {other:?}"),
                }
            }
            0xff8 => {
                let id = value as u32 & 0xffff;
                let arm = value >> 63 == 1;
                match (page, id) {
                    (HWC_DOORBELL, HWC_EQ) => assert!(!arm, "the channel is polled"),
                    (MANA_DOORBELL, DATA_EQ) => self.eq.as_mut().unwrap().armed = arm,
                    other => panic!("event queue doorbell {other:?}"),
                }
            }
            other => panic!("doorbell at {other:#x}"),
        }
    }

    /// Takes the frames posted on the data send queue.
    fn transmit(&mut self) {
        while let Some(wqe) = self.sq.as_mut().unwrap().take() {
            assert_eq!(wqe.key, KEY);
            let frame = read(wqe.address, wqe.len as usize);
            self.sent.push(Sent { frame, oob: wqe.oob });
            if !self.hold {
                self.complete_send();
            }
        }
    }

    fn complete_send(&mut self) {
        let mut data = [0u8; 60];
        put32(&mut data, 0, 32);
        self.tx_cq.as_mut().unwrap().write(&data, SQ | 1 << 24);
        self.raise(TX_CQ);
    }

    /// A request on the hardware channel: its answer goes into the posted
    /// buffer, with a completion for the send and one for the answer.
    fn request(&mut self, wqe: &Wqe) {
        assert_eq!(wqe.key, HWC_KEY);
        assert_eq!(wqe.oob.len(), 6);
        assert_eq!(wqe.oob[4], HWC_CQ, "the request names the channel's completion queue");
        assert_eq!(wqe.oob[5], HWC_SQ, "the request names the channel's send queue");
        let request = read(wqe.address, wqe.len as usize);
        assert_eq!(u32_at(&request, 12) as usize, request.len(), "the request's size");
        let code = u32_at(&request, 4);
        self.requests.push(code);
        // The send's completion.
        self.hwc_cq.as_mut().unwrap().write(&[0u8; 60], HWC_SQ | 1 << 24);
        if self.silent == Some(code) {
            return;
        }
        // A refused request changes nothing.
        let mut answer = if self.refuse == Some(code) {
            let mut refused = std::vec![0u8; 32];
            put32(&mut refused, 24, 0xc000_0001);
            refused
        } else {
            self.answer(code, &request)
        };
        put32(&mut answer, 4, code);
        answer[8] = if code == QUERY_DEV_CONFIG { 2 } else { 1 };
        answer[10..12].copy_from_slice(&request[10..12]);
        let len = answer.len() as u32;
        put32(&mut answer, 12, len);
        put32(&mut answer, 16, u32_at(&request, 32));
        put32(&mut answer, 20, u32_at(&request, 36));
        let buffer = self.hwc_rq.as_mut().unwrap().take().expect("an answer buffer");
        assert_eq!(buffer.key, HWC_KEY);
        assert!(answer.len() <= buffer.len as usize);
        fake_memory_write(buffer.address, &answer);
        let mut data = [0u8; 60];
        put32(&mut data, 24, len);
        self.hwc_cq.as_mut().unwrap().write(&data, HWC_RQ);
    }

    /// The answer to request `code`, its header left for the caller.
    fn answer(&mut self, code: u32, request: &[u8]) -> Vec<u8> {
        let device = u32_at(request, 32);
        let mut answer = std::vec![0u8; 32];
        match code {
            // VERIFY_VF_DRIVER_VERSION: protocol 1, from an OS of its own.
            1 => {
                assert_eq!((u64_at(request, 40), u64_at(request, 48)), (1, 1));
                assert_eq!(u32_at(request, 96), 0x30);
                assert_eq!(&request[128..134], b"Dukovo");
                answer.resize(72, 0);
                put64(&mut answer, 32, 1);
            }
            // QUERY_MAX_RESOURCES.
            2 => {
                answer.resize(72, 0);
                for at in [36, 40, 44, 48] {
                    put32(&mut answer, at, 16);
                }
                put32(&mut answer, 68, 2);
            }
            // LIST_DEVICES: the channel, then MANA.
            3 => {
                answer.resize(296, 0);
                put32(&mut answer, 32, 2);
                put32(&mut answer, 40, 1);
                put32(&mut answer, 44, if self.no_mana { 3 } else { MANA_ID });
            }
            // REGISTER_DEVICE.
            4 => {
                assert_eq!(device, MANA_ID);
                answer.resize(44, 0);
                put32(&mut answer, 32, PDID);
                put32(&mut answer, 36, KEY);
                put32(&mut answer, 40, MANA_DOORBELL);
            }
            // DEREGISTER_DEVICE.
            5 => {
                assert_eq!(device, MANA_ID);
                self.deregistered = true;
            }
            // GENERATE_TEST_EQE.
            10 => {
                let queue = u32_at(request, 40);
                let eq = match queue {
                    HWC_EQ => {
                        assert_eq!(device, 1);
                        self.hwc_eq.as_mut().unwrap()
                    }
                    DATA_EQ => {
                        assert_eq!(device, MANA_ID);
                        self.eq.as_mut().unwrap()
                    }
                    other => panic!("test event on queue {other}"),
                };
                eq.write(&[0u8; 12], 64);
            }
            // CREATE_QUEUE: the event queue, on its region.
            12 => {
                assert_eq!(device, MANA_ID);
                assert_eq!(u32_at(request, 40), 4);
                assert_eq!((u32_at(request, 48), u32_at(request, 52)), (PDID, MANA_DOORBELL));
                let (base, len) = self.region(u64_at(request, 56));
                assert_eq!(u32_at(request, 68) as usize, len);
                assert_eq!(u32_at(request, 72), 3);
                self.eq_vector = Some(u32_at(request, 76));
                self.eq = Some(Writer::new(base, len, 16));
                answer.resize(36, 0);
                put32(&mut answer, 32, DATA_EQ);
            }
            // DISABLE_QUEUE.
            13 => {
                assert_eq!((u32_at(request, 40), u32_at(request, 48)), (4, 1));
                self.disabled.push(u32_at(request, 44));
            }
            // CREATE_DMA_REGION: whole pages, in order.
            25 => {
                let len = u64_at(request, 40) as usize;
                let pages = u32_at(request, 56) as usize;
                assert_eq!(u32_at(request, 48), 0);
                assert_eq!(pages * PAGE, len);
                assert_eq!(u32_at(request, 60) as usize, pages);
                assert!(len.is_power_of_two());
                let first = u64_at(request, 64);
                for page in 0..pages {
                    assert_eq!(u64_at(request, 64 + page * 8), first + (page * PAGE) as u64);
                }
                let handle = self.next_region;
                self.next_region += 1;
                self.regions.insert(handle, (first, len));
                answer.resize(40, 0);
                put64(&mut answer, 32, handle);
            }
            // DESTROY_DMA_REGION.
            27 => {
                let handle = u64_at(request, 40);
                assert!(self.regions.remove(&handle).is_some());
                self.destroyed_regions.push(handle);
            }
            QUERY_DEV_CONFIG => {
                assert_eq!(answer.len(), 32);
                assert_eq!(request[24], 2, "version 2 of the answer");
                assert_eq!((u32_at(request, 72), u32_at(request, 76)), (0, 1));
                answer.resize(80, 0);
                answer[64] = 1;
                answer[72..74].copy_from_slice(&1514u16.to_le_bytes());
            }
            QUERY_VPORT_CONFIG => {
                assert_eq!(u32_at(request, 40), 0);
                answer.resize(64, 0);
                put32(&mut answer, 32, 1);
                put32(&mut answer, 36, 1);
                put32(&mut answer, 40, 64);
                answer[48..54].copy_from_slice(&MAC);
                put64(&mut answer, 56, VPORT);
            }
            CONFIG_VPORT_TX => {
                assert_eq!(u64_at(request, 40), VPORT);
                assert_eq!((u32_at(request, 48), u32_at(request, 52)), (PDID, MANA_DOORBELL));
                answer.resize(36, 0);
                answer[32..34].copy_from_slice(&self.vport_offset.to_le_bytes());
                answer[34] = 1;
            }
            CREATE_WQ_OBJ => {
                assert_eq!(u64_at(request, 40), VPORT);
                assert_eq!(u32_at(request, 84), DATA_EQ, "the completions' event queue");
                let kind = u32_at(request, 48);
                let (wq, wq_len) = self.region(u64_at(request, 56));
                let (cq, cq_len) = self.region(u64_at(request, 64));
                assert_eq!(u32_at(request, 72) as usize, wq_len);
                assert_eq!(u32_at(request, 76) as usize, cq_len);
                answer.resize(48, 0);
                if kind == 1 {
                    self.sq = Some(Reader::new(wq, wq_len));
                    self.tx_cq = Some(Writer::new(cq, cq_len, 64));
                    put32(&mut answer, 32, SQ);
                    put32(&mut answer, 36, TX_CQ);
                    put64(&mut answer, 40, TX_OBJECT);
                } else {
                    assert_eq!(kind, 2);
                    self.rq = Some(Reader::new(wq, wq_len));
                    self.rx_cq = Some(Writer::new(cq, cq_len, 64));
                    put32(&mut answer, 32, RQ);
                    put32(&mut answer, 36, RX_CQ);
                    put64(&mut answer, 40, RX_OBJECT);
                }
            }
            DESTROY_WQ_OBJ => {
                self.destroyed_objects.push((u32_at(request, 40), u64_at(request, 48)));
            }
            CONFIG_VPORT_RX => {
                assert_eq!(u64_at(request, 40), VPORT);
                assert_eq!((u16_at(request, 48), u16_at(request, 50)), (64, 112));
                assert_eq!(request.len(), 112 + 64 * 8);
                let on = u32_at(request, 52) == 1;
                assert_eq!(u32_at(request, 56), 0, "no hashing across queues");
                assert_eq!(request[60], 1);
                let default = u64_at(request, 64);
                if request[62] == 1 {
                    for entry in 0..64 {
                        assert_eq!(u64_at(request, 112 + entry * 8), default);
                    }
                }
                self.steering.push((on, default));
            }
            FENCE_RQ => {
                assert_eq!(u64_at(request, 40), RX_OBJECT);
                let mut data = [0u8; 60];
                put32(&mut data, 0, 3);
                self.rx_cq.as_mut().unwrap().write(&data, RQ);
            }
            other => panic!("request {other:#x}"),
        }
        answer
    }

    /// The memory a DMA region names, which the queue made on it takes.
    fn region(&mut self, handle: u64) -> (u64, usize) {
        self.regions.remove(&handle).expect("a DMA region")
    }
}

impl Registers for Card {
    fn read(&self, offset: usize, _size: usize) -> u64 {
        self.with(|s| match offset {
            0x08 => DOORBELLS as u64,
            0x10 => DOORBELL_SIZE as u64,
            0x18 => SHM as u64,
            _ if (SHM..SHM + 32).contains(&offset) => u64::from(s.smc[(offset - SHM) / 4]),
            _ => 0,
        })
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        self.with(|s| {
            if (SHM..SHM + 32).contains(&offset) {
                assert_eq!(size, 4);
                s.smc[(offset - SHM) / 4] = value as u32;
                if offset == SHM + 28 {
                    s.smc();
                }
            } else if offset >= DOORBELLS {
                assert_eq!(size, 8, "doorbells are written whole");
                s.doorbell(offset, value);
            } else {
                panic!("write to {offset:#x}");
            }
        });
    }
}

fn started(card: &Arc<Card>, interrupt: Interrupt) -> Mana {
    start(card.mmio(), interrupt, &mut alloc, &mut no_delay).expect("the card starts")
}

fn frame(ethertype: u16, fill: u8) -> Vec<u8> {
    let mut frame = std::vec![fill; 60];
    frame[..6].copy_from_slice(&[0xff; 6]);
    frame[6..12].copy_from_slice(&MAC);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame
}

#[test]
fn the_virtual_function_is_known() {
    assert!(identify(VENDOR, DEVICE));
    assert!(identify(0x1414, 0x00ba));
    assert!(!identify(0x1414, 0x00b9), "the host's physical function");
    assert!(!identify(0x1d0f, 0x00ba));
}

#[test]
fn the_shared_memory_message_packs_four_pages() {
    let pages = [0x1000, 0x2_3456_7000, 0xabc_def0_1234_5000, 0x7000];
    let message = establish_message(pages, 0x0102);
    // Each page's frame number: its low 48 bits, then the high 4 bits of
    // all four.
    assert_eq!(&message[0..6], &[1, 0, 0, 0, 0, 0]);
    assert_eq!(&message[6..12], &[0x67, 0x45, 0x23, 0, 0, 0]);
    assert_eq!(&message[12..18], &[0x45, 0x23, 0x01, 0xef, 0xcd, 0xab]);
    assert_eq!(u16_at(&message, 24), 0x0000);
    let high = establish_message([0, 0, 0x5000_0000_0000_0000, 0], 0);
    assert_eq!(u16_at(&high, 24), 0x5 << 8, "page 3's high bits");
    assert_eq!(u16_at(&message, 26), 0x0102);
    assert_eq!(u32_at(&message, 28), 1, "an ESTABLISH request");
}

#[test]
fn a_card_starts_and_its_port_runs() {
    let card = Card::new();
    let mana = started(&card, Interrupt::Polled);
    assert_eq!(mana.info().mac, MAC);
    assert_eq!(mana.info().model, "MANA");
    assert!(mana.link().up);
    assert!(!mana.failed());
    card.with(|s| {
        assert_eq!(
            s.requests,
            [
                10, // the channel's test event
                1,
                2,
                3,
                4,
                25,
                12,
                10, // the event queue's test event
                QUERY_DEV_CONFIG,
                QUERY_VPORT_CONFIG,
                CONFIG_VPORT_TX,
                25,
                25,
                CREATE_WQ_OBJ,
                25,
                25,
                CREATE_WQ_OBJ,
                CONFIG_VPORT_RX,
                FENCE_RQ,
            ]
        );
        // The channel's events are polled: its message names MSI-X 0, the
        // queues' event queue 1.
        assert_eq!(u16_at(&s.established.unwrap(), 26), 0);
        assert_eq!(s.eq_vector, Some(1));
        assert_eq!(s.steering, [(true, RX_OBJECT)]);
        assert!(s.regions.is_empty(), "every region went to a queue");
        // Every receive slot is posted.
        assert_eq!(s.rq.unwrap().posted, 256);
        assert!(!s.destroyed);
    });
}

#[test]
fn frames_go_both_ways() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Polled);
    let mut buffer = [0u8; 2048];
    assert_eq!(mana.receive(&mut buffer), None);

    let ipv4 = frame(0x0800, 0x44);
    let ipv6 = frame(0x86dd, 0x66);
    let arp = frame(0x0806, 0x88);
    for frame in [&ipv4, &ipv6, &arp] {
        assert!(mana.send(frame));
    }
    card.with(|s| assert!(s.sent.is_empty(), "nothing goes before the doorbell"));
    mana.flush();
    card.with(|s| {
        assert_eq!(s.sent.len(), 3);
        for (sent, (frame, outer)) in
            s.sent.iter().zip([(&ipv4, 1 << 2), (&ipv6, 1 << 3), (&arp, 0)])
        {
            assert_eq!(&sent.frame, frame);
            // The short format, the outer IP version, and the completion
            // queue; vPort offset 0.
            assert_eq!(sent.oob, [outer | TX_CQ << 8, 0]);
        }
    });

    card.deliver(&ipv4);
    card.deliver(&arp);
    assert_eq!(mana.receive(&mut buffer), Some(60));
    assert_eq!(&buffer[..60], &ipv4[..]);
    assert_eq!(mana.receive(&mut buffer), Some(60));
    assert_eq!(&buffer[..60], &arp[..]);
    assert_eq!(mana.receive(&mut buffer), None);
    // The two slots go back to the card at the next flush.
    card.with(|s| assert_eq!(s.rq.unwrap().posted, 256));
    mana.flush();
    card.with(|s| assert_eq!(s.rq.unwrap().posted, 258));
    assert_eq!(mana.dropped(), 0);
}

#[test]
fn every_slot_comes_round_again() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Polled);
    let mut buffer = [0u8; 2048];
    for round in 0..3 * crate::DEPTH {
        let sent = frame(0x0800, round as u8);
        assert!(mana.send(&sent), "send {round}");
        mana.flush();
        card.deliver(&sent);
        assert_eq!(mana.receive(&mut buffer), Some(60), "frame {round}");
        assert_eq!(&buffer[..60], &sent[..]);
        mana.flush();
    }
    assert!(!mana.failed());
}

#[test]
fn a_full_send_queue_takes_no_more() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Polled);
    card.with(|s| s.hold = true);
    let sent = frame(0x0800, 1);
    for i in 0..crate::DEPTH {
        assert!(mana.send(&sent), "frame {i}");
    }
    assert!(!mana.can_send());
    assert!(!mana.send(&sent));
    mana.flush();
    card.with(|s| {
        assert_eq!(s.sent.len(), crate::DEPTH);
        s.complete_send();
    });
    assert!(mana.can_send());
    assert!(mana.send(&sent));
    assert!(!mana.can_send());
    // Frames too short or too long are refused.
    card.with(State::complete_send);
    assert!(!mana.send(&[0u8; 13]));
    assert!(!mana.send(&[0u8; 1519]));
}

#[test]
fn a_far_vport_takes_the_long_format() {
    let card = Card::new();
    card.with(|s| s.vport_offset = 300);
    let mut mana = started(&card, Interrupt::Polled);
    let sent = frame(0x86dd, 7);
    for _ in 0..crate::DEPTH + 10 {
        assert!(mana.send(&sent));
        mana.flush();
    }
    card.with(|s| {
        assert_eq!(s.sent.len(), crate::DEPTH + 10);
        for sent in &s.sent {
            assert_eq!(sent.oob, [1 | 1 << 3 | TX_CQ << 8, 0, 0, 300 << 16, 0, 0]);
        }
    });
}

#[test]
fn damaged_frames_are_dropped_and_their_slots_given_back() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Polled);
    let mut buffer = [0u8; 2048];
    let good = frame(0x0800, 9);
    card.deliver_as(&good, 4);
    card.deliver_as(&good[..10], 1);
    card.deliver(&good);
    assert_eq!(mana.receive(&mut buffer), Some(60));
    assert_eq!(&buffer[..60], &good[..]);
    assert_eq!(mana.dropped(), 2);
    mana.flush();
    card.with(|s| assert_eq!(s.rq.unwrap().posted, 259));
}

#[test]
fn a_short_buffer_takes_what_fits() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Polled);
    let mut buffer = [0u8; 20];
    let sent = frame(0x0800, 3);
    card.deliver(&sent);
    assert_eq!(mana.receive(&mut buffer), Some(20));
    assert_eq!(&buffer[..], &sent[..20]);
}

#[test]
fn the_interrupt_is_asked_for_again_once_nothing_is_left() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Msi);
    card.with(|s| {
        assert!(s.eq.unwrap().armed);
        assert!(s.rx_cq.unwrap().armed && s.tx_cq.unwrap().armed);
    });
    let sent = frame(0x0800, 5);
    card.deliver(&sent);
    card.with(|s| {
        assert_eq!(s.interrupts, 1);
        assert!(!s.eq.unwrap().armed && !s.rx_cq.unwrap().armed);
    });
    // A second frame raises nothing until the driver asks again.
    card.deliver(&sent);
    card.with(|s| assert_eq!(s.interrupts, 1));
    let mut buffer = [0u8; 2048];
    assert_eq!(mana.receive(&mut buffer), Some(60));
    assert_eq!(mana.receive(&mut buffer), Some(60));
    assert_eq!(mana.receive(&mut buffer), None);
    card.with(|s| assert!(s.eq.unwrap().armed && s.rx_cq.unwrap().armed));
    card.deliver(&sent);
    card.with(|s| assert_eq!(s.interrupts, 2));
}

#[test]
fn shutting_down_takes_everything_back() {
    let card = Card::new();
    let mana = started(&card, Interrupt::Polled);
    card.with(|s| s.requests.clear());
    assert!(mana.shut_down(&mut no_delay));
    card.with(|s| {
        assert_eq!(s.requests, [CONFIG_VPORT_RX, FENCE_RQ, DESTROY_WQ_OBJ, DESTROY_WQ_OBJ, 13, 5]);
        assert_eq!(s.steering.last(), Some(&(false, RX_OBJECT)));
        assert_eq!(s.destroyed_objects, [(2, RX_OBJECT), (1, TX_OBJECT)]);
        assert_eq!(s.disabled, [DATA_EQ]);
        assert!(s.deregistered);
        assert!(s.destroyed, "the hardware channel is taken down");
    });
}

#[test]
fn stopping_takes_the_card_down_once() {
    let card = Card::new();
    let mut mana = started(&card, Interrupt::Polled);
    mana.stop();
    card.with(|s| {
        assert!(s.destroyed && s.deregistered);
        s.requests.clear();
    });
    mana.stop();
    assert!(!mana.link().up);
    assert!(!mana.send(&frame(0x0800, 1)));
    let mut buffer = [0u8; 64];
    assert_eq!(mana.receive(&mut buffer), None);
    drop(mana);
    card.with(|s| assert!(s.requests.is_empty()));
}

#[test]
fn a_refused_queue_takes_the_card_down() {
    let card = Card::new();
    card.with(|s| s.refuse = Some(CREATE_WQ_OBJ));
    let error = start(card.mmio(), Interrupt::Polled, &mut alloc, &mut no_delay).err();
    assert_eq!(error, Some(Error::Device("creating a queue")));
    card.with(|s| {
        // The send queue's two regions went back; nothing was steered.
        assert_eq!(s.destroyed_regions.len(), 2);
        assert!(s.regions.is_empty());
        assert!(s.steering.is_empty());
        assert_eq!(s.disabled, [DATA_EQ]);
        assert!(s.deregistered && s.destroyed);
    });
}

#[test]
fn a_function_without_mana_is_left() {
    let card = Card::new();
    card.with(|s| s.no_mana = true);
    let error = start(card.mmio(), Interrupt::Polled, &mut alloc, &mut no_delay).err();
    assert_eq!(error, Some(Error::Device("it has no MANA device")));
    card.with(|s| {
        assert!(!s.deregistered, "nothing was registered");
        assert!(s.destroyed);
    });
}

#[test]
fn a_silent_channel_times_out_and_is_not_used_again() {
    let card = Card::new();
    card.with(|s| s.silent = Some(3));
    let mut waited = 0u64;
    let mut delay = |us: u32| waited += u64::from(us);
    let error = start(card.mmio(), Interrupt::Polled, &mut alloc, &mut delay).err();
    assert_eq!(error, Some(Error::Timeout("listing the card's devices")));
    assert!(waited >= 30_000_000, "the channel's 30 seconds were waited");
    card.with(|s| {
        assert_eq!(s.requests, [10, 1, 2, 3]);
        assert!(s.destroyed);
    });
}

#[test]
fn a_host_that_never_sets_the_channel_up_is_left() {
    struct Silent;
    impl Registers for Silent {
        fn read(&self, offset: usize, _size: usize) -> u64 {
            match offset {
                0x08 => DOORBELLS as u64,
                0x10 => DOORBELL_SIZE as u64,
                0x18 => SHM as u64,
                // The host's driver keeps the shared memory.
                _ if offset == SHM + 28 => 0x8000_0000,
                _ => 0,
            }
        }
        fn write(&self, _offset: usize, _size: usize, _value: u64) {}
    }
    let regs = Mmio::fake(Arc::new(Silent), BAR);
    let error = start(regs, Interrupt::Polled, &mut alloc, &mut no_delay).err();
    assert_eq!(error, Some(Error::Timeout("the shared memory")));
}

#[test]
fn registers_too_small_are_refused() {
    let card = Card::new();
    let regs = Mmio::fake(card.clone(), 0x1000);
    let error = start(regs, Interrupt::Polled, &mut alloc, &mut no_delay).err();
    assert_eq!(error, Some(Error::Registers));
}
