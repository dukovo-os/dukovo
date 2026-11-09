// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The sound driver against a simulated virtio sound card: the same PCI
//! configuration space as the block tests, a BAR with four queues, a
//! control queue the card answers at once (or not at all), and a tx queue
//! whose periods the test has the card play.

use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Mmio, Registers, fake_memory, fake_memory_write};

use crate::snd::{Output, Snd, code};
use crate::tests::{BAR_SIZE, alloc, function};
use crate::{Error, Transport, device_type, feature as common_feature, status};

#[derive(Clone, Copy, Default)]
struct Ring {
    size: u16,
    desc: u64,
    driver: u64,
    device: u64,
    last_avail: u16,
    used_idx: u16,
}

/// A stream the card says it has.
#[derive(Clone, Copy)]
struct Stream {
    output: bool,
    formats: u64,
    rates: u64,
    channels: (u8, u8),
}

/// What SET_PARAMS asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Params {
    stream: u32,
    buffer: u32,
    period: u32,
    features: u32,
    channels: u8,
    format: u8,
    rate: u8,
}

/// A period the card holds: its chain's head, the samples, and where its
/// status goes.
struct Period {
    head: u16,
    stream: u32,
    samples: Vec<u8>,
    status: u64,
}

#[derive(Default)]
struct State {
    device_select: u32,
    driver_select: u32,
    driver_features: u64,
    status: u8,
    select: u16,
    rings: [Ring; 4],
    streams: Vec<Stream>,
    /// Whether the card answers control requests.
    silent: bool,
    /// A request code the card refuses with NOT_SUPP.
    refuse: Option<u32>,
    /// The codes of the requests, in order.
    requests: Vec<u32>,
    params: Params,
    started: bool,
    /// Periods taken from the tx queue and not yet played.
    holding: Vec<Period>,
    /// The samples played, in order.
    played: Vec<Vec<u8>>,
}

struct Card {
    state: Mutex<State>,
}

fn read_mem(phys: u64, len: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; len];
    fake_memory(phys, &mut out);
    out
}

fn u16_at(phys: u64) -> u16 {
    u16::from_le_bytes(read_mem(phys, 2).try_into().unwrap())
}

fn u32_of(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// One descriptor: address, length, flags, next.
fn descriptor(ring: &Ring, index: u16) -> (u64, u32, u16, u16) {
    let d = read_mem(ring.desc + 16 * u64::from(index), 16);
    let addr = u64::from_le_bytes(d[0..8].try_into().unwrap());
    let len = u32::from_le_bytes(d[8..12].try_into().unwrap());
    let flags = u16::from_le_bytes(d[12..14].try_into().unwrap());
    let next = u16::from_le_bytes(d[14..16].try_into().unwrap());
    (addr, len, flags, next)
}

/// A buffer of a chain: its address, its length, and whether the card
/// writes it.
type Buffer = (u64, u32, bool);

/// The next chain the driver made available on `ring`: its head and its
/// buffers.
fn next_chain(ring: &mut Ring) -> Option<(u16, Vec<Buffer>)> {
    if u16_at(ring.driver + 2) == ring.last_avail {
        return None;
    }
    let slot = ring.driver + 4 + 2 * (u64::from(ring.last_avail) % u64::from(ring.size));
    let head = u16_at(slot);
    ring.last_avail = ring.last_avail.wrapping_add(1);
    let mut buffers = Vec::new();
    let mut index = head;
    loop {
        let (addr, len, flags, next) = descriptor(ring, index);
        assert_eq!(flags & 4, 0, "no indirect tables were agreed");
        buffers.push((addr, len, flags & 2 != 0));
        if flags & 1 == 0 {
            break;
        }
        index = next;
    }
    // Told of what comes next, with event indexes.
    let event = ring.device + 4 + 8 * u64::from(ring.size);
    fake_memory_write(event, &ring.last_avail.to_le_bytes());
    Some((head, buffers))
}

fn put_used(ring: &mut Ring, head: u16, len: u32) {
    let slot = ring.device + 4 + 8 * (u64::from(ring.used_idx) % u64::from(ring.size));
    fake_memory_write(slot, &u32::from(head).to_le_bytes());
    fake_memory_write(slot + 4, &len.to_le_bytes());
    ring.used_idx = ring.used_idx.wrapping_add(1);
    fake_memory_write(ring.device + 2, &ring.used_idx.to_le_bytes());
}

impl Card {
    fn new(streams: &[Stream]) -> Arc<Card> {
        let state = State { streams: streams.to_vec(), ..State::default() };
        Arc::new(Card { state: Mutex::new(state) })
    }

    /// Answers the control requests the driver made available.
    fn control(s: &mut State) {
        if s.silent {
            return;
        }
        while let Some((head, buffers)) = next_chain(&mut s.rings[0]) {
            assert_eq!(buffers.len(), 2, "a request and its answer");
            let (request_at, request_len, writes) = buffers[0];
            assert!(!writes);
            let (answer_at, answer_len, writes) = buffers[1];
            assert!(writes);
            let request = read_mem(request_at, request_len as usize);
            let code = u32_of(&request, 0);
            s.requests.push(code);
            let mut answer = std::vec![0u8; answer_len as usize];
            let status = if s.refuse == Some(code) {
                code::NOT_SUPP
            } else {
                Card::carry_out(s, code, &request, &mut answer[4..])
            };
            answer[..4].copy_from_slice(&status.to_le_bytes());
            fake_memory_write(answer_at, &answer);
            put_used(&mut s.rings[0], head, answer_len);
        }
    }

    fn carry_out(s: &mut State, code: u32, request: &[u8], answer: &mut [u8]) -> u32 {
        match code {
            code::PCM_INFO => {
                let (start, count, size) =
                    (u32_of(request, 4), u32_of(request, 8), u32_of(request, 12));
                assert_eq!(size, 32);
                if (start + count) as usize > s.streams.len() {
                    return code::BAD_MSG;
                }
                for i in 0..count as usize {
                    let stream = s.streams[start as usize + i];
                    let info = &mut answer[i * 32..(i + 1) * 32];
                    info[8..16].copy_from_slice(&stream.formats.to_le_bytes());
                    info[16..24].copy_from_slice(&stream.rates.to_le_bytes());
                    info[24] = if stream.output { 0 } else { 1 };
                    info[25] = stream.channels.0;
                    info[26] = stream.channels.1;
                }
                code::OK
            }
            code::PCM_SET_PARAMS => {
                assert_eq!(request.len(), 24);
                s.params = Params {
                    stream: u32_of(request, 4),
                    buffer: u32_of(request, 8),
                    period: u32_of(request, 12),
                    features: u32_of(request, 16),
                    channels: request[20],
                    format: request[21],
                    rate: request[22],
                };
                code::OK
            }
            code::PCM_START => {
                s.started = true;
                code::OK
            }
            code::PCM_STOP => {
                s.started = false;
                code::OK
            }
            code::PCM_RELEASE => {
                // Every period held goes back, unplayed (5.14.6.6.5.1).
                for period in std::mem::take(&mut s.holding) {
                    fake_memory_write(period.status, &code::OK.to_le_bytes());
                    put_used(&mut s.rings[2], period.head, 8);
                }
                code::OK
            }
            code::PCM_PREPARE => code::OK,
            _ => code::NOT_SUPP,
        }
    }

    /// Takes the periods the driver made available to play.
    fn transmit(s: &mut State) {
        while let Some((head, buffers)) = next_chain(&mut s.rings[2]) {
            assert_eq!(buffers.len(), 2, "header and samples, then the status");
            let (at, len, writes) = buffers[0];
            assert!(!writes);
            let bytes = read_mem(at, len as usize);
            let (status_at, status_len, writes) = buffers[1];
            assert!(writes && status_len == 8);
            s.holding.push(Period {
                head,
                stream: u32_of(&bytes, 0),
                samples: bytes[4..].to_vec(),
                status: status_at,
            });
        }
    }

    /// Plays the `count` oldest periods held, ending each with `status`.
    fn play(&self, count: usize, status: u32) {
        let mut s = self.state.lock().unwrap();
        assert!(s.started, "played only once started");
        for _ in 0..count {
            let period = s.holding.remove(0);
            assert_eq!(period.stream, s.params.stream);
            fake_memory_write(period.status, &status.to_le_bytes());
            put_used(&mut s.rings[2], period.head, 8);
            s.played.push(period.samples);
        }
    }
}

impl Registers for Card {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let s = self.state.lock().unwrap();
        let features = common_feature::VERSION_1 | common_feature::EVENT_IDX;
        match (offset, size) {
            (0x04, 4) => (features >> (32 * s.device_select)) & 0xffff_ffff,
            (0x12, 2) => 4,
            (0x14, 1) => u64::from(s.status),
            (0x15, 1) => 0,
            (0x18, 2) => 64,
            (0x1e, 2) => u64::from(s.select),
            (0x1a, 2) => 0,
            (0x2000, 4) => 0,
            (0x2004, 4) => s.streams.len() as u64,
            (0x2008, 4) => 0,
            _ => 0,
        }
    }

    fn write(&self, offset: usize, _size: usize, value: u64) {
        let mut s = self.state.lock().unwrap();
        let select = usize::from(s.select);
        let features = common_feature::VERSION_1 | common_feature::EVENT_IDX;
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
                if value & status::FEATURES_OK != 0 && s.driver_features & !features != 0 {
                    s.status = value & !status::FEATURES_OK;
                } else {
                    s.status = value;
                }
                if value == 0 {
                    s.rings = [Ring::default(); 4];
                    s.holding.clear();
                    s.started = false;
                }
            }
            0x16 => s.select = value as u16,
            0x18 => s.rings[select].size = value as u16,
            0x20 => s.rings[select].desc = value,
            0x28 => s.rings[select].driver = value,
            0x30 => s.rings[select].device = value,
            0x3000 => {
                assert!(s.status & status::DRIVER_OK != 0, "asked before ready");
                Card::control(&mut s);
            }
            0x3008 => {
                assert!(s.status & status::DRIVER_OK != 0, "notified before ready");
                Card::transmit(&mut s);
            }
            _ => {}
        }
    }
}

/// Signed 16-bit samples, in the format bits of a stream.
const S16: u64 = 1 << 5;
/// 44,100 and 48,000 frames a second, bits 6 and 7.
const RATES: u64 = 0b1100_0000;

/// A card with a microphone, then speakers.
fn streams() -> [Stream; 2] {
    [
        Stream { output: false, formats: S16, rates: RATES, channels: (1, 2) },
        Stream { output: true, formats: S16 | 1 << 3, rates: RATES, channels: (1, 2) },
    ]
}

fn never() -> bool {
    false
}

fn setup(streams: &[Stream]) -> (Arc<Card>, Result<Snd, Error>) {
    let card = Card::new(streams);
    let f = function(25, 0x1059);
    let regs = card.clone();
    let transport =
        Transport::find(&f, &mut |_, _| Some(Mmio::fake(regs.clone(), BAR_SIZE))).unwrap();
    // The card answers at once, so the driver never needs to wait.
    let snd = Snd::new(transport, 0, &mut alloc, &mut never);
    (card, snd)
}

fn period(n: u8) -> Vec<u8> {
    (0..1024).map(|i| n.wrapping_add(i as u8)).collect()
}

#[test]
fn finds_the_sound_card() {
    assert_eq!(device_type(&function(25, 0x1059)), Some(25));
}

#[test]
fn finds_the_output_stream() {
    let (card, snd) = setup(&streams());
    let snd = snd.unwrap();
    assert_eq!(
        snd.output(),
        Output { stream: 1, rates: 0b1100_0000, channels_min: 1, channels_max: 2 }
    );
    assert_eq!(card.state.lock().unwrap().requests, [code::PCM_INFO]);
    assert_eq!(snd.max_periods(), 32);
}

#[test]
fn needs_a_stream_of_16_bit_samples() {
    // Only 8-bit samples, and only a microphone of 16-bit ones.
    let only = [
        Stream { output: true, formats: 1 << 3, rates: RATES, channels: (1, 2) },
        Stream { output: false, formats: S16, rates: RATES, channels: (1, 2) },
    ];
    let (_, snd) = setup(&only);
    assert!(matches!(snd, Err(Error::MissingFeature(_))));
    let (_, snd) = setup(&[]);
    assert!(matches!(snd, Err(Error::MissingFeature(_))));
}

#[test]
fn plays_periods_in_order() {
    let (card, snd) = setup(&streams());
    let mut snd = snd.unwrap();
    snd.configure(7, 2, 1024, 4, &mut alloc, &mut never).unwrap();
    {
        let s = card.state.lock().unwrap();
        let params = Params {
            stream: 1,
            buffer: 4096,
            period: 1024,
            features: 0,
            channels: 2,
            format: 5,
            rate: 7,
        };
        assert_eq!(s.params, params);
        assert_eq!(s.requests[1..], [code::PCM_SET_PARAMS, code::PCM_PREPARE]);
    }
    for n in 0..3 {
        snd.submit(&period(n)).unwrap();
    }
    snd.notify();
    assert_eq!(snd.in_flight(), 3);
    snd.start(&mut never).unwrap();
    card.play(2, code::OK);
    assert_eq!(snd.collect(), Some(true));
    assert_eq!(snd.collect(), Some(true));
    assert_eq!(snd.collect(), None);
    assert_eq!(snd.in_flight(), 1);
    // The two slots played are free again: three more fit, then none.
    for n in 3..6 {
        assert!(snd.can_submit());
        snd.submit(&period(n)).unwrap();
    }
    assert!(!snd.can_submit());
    assert_eq!(snd.submit(&period(9)), Err(Error::Full));
    snd.notify();
    card.play(4, code::OK);
    let played = card.state.lock().unwrap().played.clone();
    let expected: Vec<Vec<u8>> = (0..6).map(period).collect();
    assert_eq!(played, expected);
}

#[test]
fn a_period_that_failed_is_counted() {
    let (card, snd) = setup(&streams());
    let mut snd = snd.unwrap();
    snd.configure(6, 1, 1024, 2, &mut alloc, &mut never).unwrap();
    snd.submit(&period(1)).unwrap();
    snd.notify();
    snd.start(&mut never).unwrap();
    card.play(1, code::IO_ERR);
    assert_eq!(snd.collect(), Some(false));
}

#[test]
fn release_hands_every_period_back() {
    let (card, snd) = setup(&streams());
    let mut snd = snd.unwrap();
    snd.configure(7, 2, 1024, 4, &mut alloc, &mut never).unwrap();
    snd.submit(&period(1)).unwrap();
    snd.submit(&period(2)).unwrap();
    snd.notify();
    snd.release(&mut never).unwrap();
    assert_eq!(snd.in_flight(), 0);
    assert_eq!(snd.collect(), None);
    assert!(!snd.can_submit(), "released: configure again first");
    assert_eq!(snd.submit(&period(3)), Err(Error::BadRequest));
    // Configuring again releases nothing more, and plays anew.
    snd.configure(7, 2, 2048, 2, &mut alloc, &mut never).unwrap();
    assert_eq!(card.state.lock().unwrap().params.buffer, 4096);
    snd.submit(&std::vec![5; 2048]).unwrap();
    snd.notify();
    snd.start(&mut never).unwrap();
    card.play(1, code::OK);
    assert_eq!(snd.collect(), Some(true));
}

#[test]
fn configure_checks_its_arguments() {
    let (_, snd) = setup(&streams());
    let mut snd = snd.unwrap();
    // 22,050 frames a second (bit 4) is not the stream's.
    assert_eq!(snd.configure(4, 2, 1024, 4, &mut alloc, &mut never), Err(Error::BadRequest));
    assert_eq!(snd.configure(7, 2, 1024, 0, &mut alloc, &mut never), Err(Error::BadRequest));
    assert_eq!(snd.configure(7, 2, 1024, 33, &mut alloc, &mut never), Err(Error::BadRequest));
    assert_eq!(snd.configure(7, 2, 1023, 4, &mut alloc, &mut never), Err(Error::BadRequest));
    assert_eq!(snd.submit(&period(0)), Err(Error::BadRequest), "not configured");
}

#[test]
fn a_refused_request_is_an_error() {
    let (card, snd) = setup(&streams());
    let mut snd = snd.unwrap();
    card.state.lock().unwrap().refuse = Some(code::PCM_SET_PARAMS);
    assert_eq!(
        snd.configure(7, 2, 1024, 4, &mut alloc, &mut never),
        Err(Error::Refused(code::NOT_SUPP))
    );
    assert!(!snd.failed());
}

#[test]
fn a_card_that_does_not_answer_is_reset() {
    let (card, snd) = setup(&streams());
    let mut snd = snd.unwrap();
    card.state.lock().unwrap().silent = true;
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        waits < 3
    };
    assert_eq!(snd.configure(7, 2, 1024, 4, &mut alloc, &mut wait), Err(Error::NoAnswer));
    assert_eq!(waits, 3);
    assert!(snd.failed());
    assert_eq!(card.state.lock().unwrap().status, 0, "reset");
    assert_eq!(snd.start(&mut never), Err(Error::DeviceFailed));
}
