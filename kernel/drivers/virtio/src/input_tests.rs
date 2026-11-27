// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The input driver against a simulated virtio input device: the same
//! PCI configuration space as the block tests, a configuration that
//! answers what the driver selects, an event queue the test has the
//! device write events into, and a status queue whose events it keeps.

use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Mmio, Registers, fake_memory_write};

use crate::input::{ANSWER_MAX, EVENT_SIZE, Event, Ids, Input, QUEUE_SIZE, Range, Vectors};
use crate::snd_tests::{Ring, next_chain, put_used, read_mem};
use crate::tests::{BAR_SIZE, alloc, function};
use crate::{Error, NO_VECTOR, Transport, device_type, feature as common_feature, status};

const NAME: &[u8] = b"A keyboard with a pointer";
const SERIAL: &[u8] = b"vk-0001";

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const EV_LED: u16 = 0x11;
const KEY_A: u16 = 30;
const BTN_LEFT: u16 = 0x110;
const REL_X: u16 = 0;

#[derive(Default)]
struct State {
    features: u64,
    device_select: u32,
    driver_select: u32,
    driver_features: u64,
    status: u8,
    select: u16,
    rings: [Ring; 2],
    /// What the driver last asked the configuration: `select`, `subsel`.
    asked: (u8, u8),
    /// The size the device claims for its name, whatever its length.
    name_size: Option<u8>,
    /// Event buffers the driver gave: head, address, length.
    given: Vec<(u16, u64, u32)>,
    /// The events the driver sent on the status queue.
    lit: Vec<Event>,
    /// The device leaves the status queue unread.
    deaf: bool,
    /// The MSI-X entry the driver gave for a change of the
    /// configuration, and whether the device takes none.
    config_vector: u16,
    no_config_vector: bool,
    /// How many times the driver read a register.
    reads: u32,
}

struct Device {
    state: Mutex<State>,
}

/// A bitmap with the bits `codes` name set.
fn bitmap(codes: &[u16]) -> Vec<u8> {
    let mut bits = std::vec![0u8; usize::from(codes.iter().max().map_or(0, |max| max / 8 + 1))];
    for &code in codes {
        bits[usize::from(code / 8)] |= 1 << (code % 8);
    }
    bits
}

fn words(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

impl Device {
    fn new(features: u64) -> Arc<Device> {
        Arc::new(Device { state: Mutex::new(State { features, ..State::default() }) })
    }

    /// The answer to what the driver asked, and the size the device
    /// says it has.
    fn answer(s: &State) -> (Vec<u8>, u8) {
        let answer = match s.asked {
            (0x01, 0) => NAME.to_vec(),
            (0x02, 0) => SERIAL.to_vec(),
            (0x03, 0) => {
                [6u16, 0x0627, 0x0001, 0x0003].iter().flat_map(|n| n.to_le_bytes()).collect()
            }
            (0x11, 1) => bitmap(&[KEY_A, BTN_LEFT]),
            (0x11, 2) => bitmap(&[REL_X, 1, 8]),
            (0x11, 3) => bitmap(&[0, 1]),
            (0x11, 0x11) => bitmap(&[0, 1, 2]),
            (0x12, 0 | 1) => words(&[0, 32767, 0, 0, 0]),
            _ => Vec::new(),
        };
        let size = match (s.asked, s.name_size) {
            ((0x01, 0), Some(size)) => size,
            _ => answer.len() as u8,
        };
        (answer, size)
    }

    /// Takes the buffers the driver made available on the event queue.
    fn take_given(s: &mut State) {
        while let Some((head, buffers)) = next_chain(&mut s.rings[0]) {
            assert_eq!(buffers.len(), 1);
            let (at, len, writes) = buffers[0];
            assert!(writes && len as usize == EVENT_SIZE, "a buffer for one event");
            s.given.push((head, at, len));
        }
    }

    /// Writes `bytes` into the oldest buffer given, as an event.
    fn put(s: &mut State, bytes: &[u8]) {
        Device::take_given(s);
        assert!(!s.given.is_empty(), "the driver keeps the queue full");
        let (head, at, len) = s.given.remove(0);
        assert!(bytes.len() <= len as usize);
        fake_memory_write(at, bytes);
        put_used(&mut s.rings[0], head, bytes.len() as u32);
    }

    /// The device says `kind`, `code`, `value`.
    fn say(&self, kind: u16, code: u16, value: i32) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(&code.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
        Device::put(&mut self.state.lock().unwrap(), &bytes);
    }

    /// Reads the events the driver sent on the status queue.
    fn lights(s: &mut State) {
        if s.deaf {
            return;
        }
        while let Some((head, buffers)) = next_chain(&mut s.rings[1]) {
            let (at, len, writes) = buffers[0];
            assert!(!writes && len as usize == EVENT_SIZE);
            let m = read_mem(at, EVENT_SIZE);
            s.lit.push(Event {
                kind: u16::from_le_bytes([m[0], m[1]]),
                code: u16::from_le_bytes([m[2], m[3]]),
                value: i32::from_le_bytes([m[4], m[5], m[6], m[7]]),
            });
            put_used(&mut s.rings[1], head, 0);
        }
    }
}

impl Registers for Device {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let mut s = self.state.lock().unwrap();
        s.reads += 1;
        match (offset, size) {
            (0x04, 4) => (s.features >> (32 * s.device_select)) & 0xffff_ffff,
            (0x10, 2) => u64::from(s.config_vector),
            (0x12, 2) => 2,
            (0x14, 1) => u64::from(s.status),
            (0x15, 1) => 0,
            (0x18, 2) => 64,
            (0x1e, 2) => u64::from(s.select),
            (0x1a, 2) => 0,
            (0x2002, 1) => u64::from(Device::answer(&s).1),
            (0x2008..0x2088, 4) => {
                let answer = Device::answer(&s).0;
                let at = offset - 0x2008;
                let byte = |i: usize| u64::from(answer.get(at + i).copied().unwrap_or(0xee));
                (0..4).fold(0, |word, i| word | byte(i) << (8 * i))
            }
            _ => 0,
        }
    }

    fn write(&self, offset: usize, _size: usize, value: u64) {
        let mut s = self.state.lock().unwrap();
        let select = usize::from(s.select).min(1);
        match offset {
            0x00 => s.device_select = value as u32,
            0x08 => s.driver_select = value as u32,
            0x0c => {
                let shift = 32 * s.driver_select;
                s.driver_features =
                    (s.driver_features & !(0xffff_ffff << shift)) | (value << shift);
            }
            0x10 => s.config_vector = if s.no_config_vector { NO_VECTOR } else { value as u16 },
            0x14 => {
                let value = value as u8;
                if value & status::FEATURES_OK != 0 && s.driver_features & !s.features != 0 {
                    s.status = value & !status::FEATURES_OK;
                } else {
                    s.status = value;
                }
                if value == 0 {
                    s.rings = [Ring::default(); 2];
                    s.given.clear();
                }
            }
            0x16 => s.select = value as u16,
            0x18 => s.rings[select].size = value as u16,
            0x20 => s.rings[select].desc = value,
            0x28 => s.rings[select].driver = value,
            0x30 => s.rings[select].device = value,
            0x2000 => s.asked.0 = value as u8,
            0x2001 => s.asked.1 = value as u8,
            0x3000.. => {
                assert!(s.status & status::DRIVER_OK != 0, "notified before ready");
                match (offset - 0x3000) / 4 {
                    0 => Device::take_given(&mut s),
                    _ => Device::lights(&mut s),
                }
            }
            _ => {}
        }
    }
}

const FEATURES: u64 = common_feature::VERSION_1 | common_feature::EVENT_IDX;

fn transport(device: &Arc<Device>) -> Transport {
    let f = function(18, 0x1052);
    let regs = device.clone();
    Transport::find(&f, &mut |_, _| Some(Mmio::fake(regs.clone(), BAR_SIZE))).unwrap()
}

/// Entry 0 for the queues and entry 1 for a change of the configuration.
const VECTORS: Vectors = Vectors { queues: 0, config: 1 };

/// A started device.
fn setup() -> (Arc<Device>, Input) {
    let device = Device::new(FEATURES);
    let mut input = Input::new(transport(&device), VECTORS, &mut alloc).unwrap();
    input.start().unwrap();
    (device, input)
}

/// Every event the device has written.
fn all(input: &mut Input) -> Vec<Event> {
    let events = core::iter::from_fn(|| input.event()).collect();
    input.listen();
    events
}

fn event(kind: u16, code: u16, value: i32) -> Event {
    Event { kind, code, value }
}

#[test]
fn finds_the_input_device() {
    assert_eq!(device_type(&function(18, 0x1052)), Some(device_type::INPUT));
}

#[test]
fn refuses_a_device_older_than_virtio_1() {
    let device = Device::new(common_feature::EVENT_IDX);
    let made = Input::new(transport(&device), VECTORS, &mut alloc);
    assert!(matches!(made, Err(Error::MissingFeature(_))));
    assert_eq!(device.state.lock().unwrap().status & status::FAILED, status::FAILED);
}

#[test]
fn says_what_the_device_is() {
    let (_device, input) = setup();
    let mut text = [0u8; ANSWER_MAX];
    let len = input.name(&mut text);
    assert_eq!(&text[..len], NAME);
    let len = input.serial(&mut text);
    assert_eq!(&text[..len], SERIAL);
    assert_eq!(input.ids(), Ids { bus: 6, vendor: 0x0627, product: 1, version: 3 });
    let keys = input.codes(EV_KEY);
    assert_eq!(keys[usize::from(KEY_A / 8)], 1 << (KEY_A % 8));
    assert_eq!(keys[usize::from(BTN_LEFT / 8)], 1 << (BTN_LEFT % 8));
    assert_eq!(keys.iter().map(|byte| byte.count_ones()).sum::<u32>(), 2);
    assert_eq!(input.codes(EV_REL)[..2], [0b11, 0b1]);
    assert_eq!(input.codes(EV_LED)[0], 0b111);
    // Kinds it has none of, and a kind no byte can name.
    assert_eq!(input.codes(5), [0; ANSWER_MAX]);
    assert_eq!(input.codes(0x111), [0; ANSWER_MAX]);
    let range = Range { min: 0, max: 32767, fuzz: 0, flat: 0, resolution: 0 };
    assert_eq!(input.range(1), Some(range));
    assert_eq!(input.range(2), None);
    assert_eq!(input.range(0x100), None);
}

#[test]
fn an_answer_is_held_to_the_size_the_device_says_and_to_its_room() {
    let (device, input) = setup();
    let mut text = [0u8; ANSWER_MAX];
    // Shorter than the name: what is past the size is not the name's.
    device.state.lock().unwrap().name_size = Some(5);
    let len = input.name(&mut text);
    assert_eq!((&text[..len], &text[len..8]), (&NAME[..5], &[0u8; 3][..]));
    // More than there is room for.
    device.state.lock().unwrap().name_size = Some(200);
    let len = input.name(&mut text);
    assert_eq!(len, ANSWER_MAX);
    assert_eq!(&text[..NAME.len()], NAME);
}

#[test]
fn events_come_in_the_order_the_device_wrote_them() {
    let (device, mut input) = setup();
    assert_eq!(input.event(), None);
    device.say(EV_KEY, KEY_A, 1);
    device.say(EV_SYN, 0, 0);
    device.say(EV_REL, REL_X, -3);
    device.say(EV_ABS, 1, 32767);
    device.say(EV_SYN, 0, 0);
    let expected = [
        event(EV_KEY, KEY_A, 1),
        event(EV_SYN, 0, 0),
        event(EV_REL, REL_X, -3),
        event(EV_ABS, 1, 32767),
        event(EV_SYN, 0, 0),
    ];
    assert_eq!(all(&mut input), expected);
    assert_eq!(input.event(), None);
}

#[test]
fn every_buffer_goes_back_to_the_device() {
    let (device, mut input) = setup();
    // Several times round the queue, a full queue at a time.
    for round in 0..5 {
        for nth in 0..i32::from(QUEUE_SIZE) {
            device.say(EV_REL, REL_X, round * 100 + nth);
        }
        let events = all(&mut input);
        assert_eq!(events.len(), usize::from(QUEUE_SIZE));
        assert_eq!(events[0], event(EV_REL, REL_X, round * 100));
        assert_eq!(events[63], event(EV_REL, REL_X, round * 100 + 63));
    }
    assert_eq!((input.short_events(), input.bad_entries()), (0, 0));
}

#[test]
fn an_event_the_device_wrote_half_of_is_dropped() {
    let (device, mut input) = setup();
    device.say(EV_KEY, KEY_A, 1);
    Device::put(&mut device.state.lock().unwrap(), &[1, 0, 30, 0]);
    Device::put(&mut device.state.lock().unwrap(), &[]);
    device.say(EV_SYN, 0, 0);
    assert_eq!(all(&mut input), [event(EV_KEY, KEY_A, 1), event(EV_SYN, 0, 0)]);
    assert_eq!(input.short_events(), 2);
    // Their buffers went back all the same.
    for nth in 0..i32::from(QUEUE_SIZE) {
        device.say(EV_REL, REL_X, nth);
    }
    assert_eq!(all(&mut input).len(), usize::from(QUEUE_SIZE));
}

#[test]
fn a_completion_that_names_no_buffer_is_skipped() {
    let (device, mut input) = setup();
    {
        let mut s = device.state.lock().unwrap();
        Device::take_given(&mut s);
        put_used(&mut s.rings[0], 9999, 8);
    }
    device.say(EV_KEY, KEY_A, 0);
    assert_eq!(all(&mut input), [event(EV_KEY, KEY_A, 0)]);
    assert_eq!(input.bad_entries(), 1);
}

#[test]
fn lights_go_to_the_device_as_one_update() {
    let (device, mut input) = setup();
    assert!(input.light(&[(0, true), (1, false), (2, true)]));
    let expected =
        [event(EV_LED, 0, 1), event(EV_LED, 1, 0), event(EV_LED, 2, 1), event(EV_SYN, 0, 0)];
    assert_eq!(device.state.lock().unwrap().lit, expected);
    // Many times over: what the device has read frees its buffers.
    for _ in 0..20 {
        assert!(input.light(&[(1, true)]));
    }
    assert_eq!(device.state.lock().unwrap().lit.len(), 4 + 40);
}

#[test]
fn lights_wait_while_the_device_reads_none() {
    let (device, mut input) = setup();
    device.state.lock().unwrap().deaf = true;
    assert!(input.light(&[(0, true), (1, true), (2, true)]));
    assert!(input.light(&[(0, false), (1, false), (2, false)]));
    // Eight buffers are out: a third update has no room, and none of it
    // is sent.
    assert!(!input.light(&[(0, true)]));
    let mut s = device.state.lock().unwrap();
    s.deaf = false;
    Device::lights(&mut s);
    assert_eq!(s.lit.len(), 8);
    drop(s);
    assert!(input.light(&[(0, true)]));
    assert_eq!(device.state.lock().unwrap().lit.len(), 10);
}

#[test]
fn the_device_is_told_where_a_change_of_its_configuration_goes() {
    let (device, _input) = setup();
    assert_eq!(device.state.lock().unwrap().config_vector, VECTORS.config);

    let device = Device::new(FEATURES);
    device.state.lock().unwrap().no_config_vector = true;
    let made = Input::new(transport(&device), VECTORS, &mut alloc);
    assert!(matches!(made, Err(Error::NoVector)));
    assert_eq!(device.state.lock().unwrap().status & status::FAILED, status::FAILED);
}

#[test]
fn taking_events_reads_no_register_of_the_device() {
    let (device, mut input) = setup();
    let before = device.state.lock().unwrap().reads;
    for _ in 0..3 * usize::from(QUEUE_SIZE) {
        device.say(EV_REL, REL_X, 1);
        device.say(EV_SYN, 0, 0);
        assert_eq!(all(&mut input).len(), 2);
    }
    assert_eq!(device.state.lock().unwrap().reads, before);
}

#[test]
fn a_device_that_needs_a_reset_says_so_when_asked() {
    let (device, input) = setup();
    assert!(!input.failed());
    device.state.lock().unwrap().status |= status::NEEDS_RESET;
    assert!(input.failed());
}

#[test]
fn resets_the_device_when_dropped() {
    let (device, input) = setup();
    drop(input);
    assert_eq!(device.state.lock().unwrap().status, 0);
}
