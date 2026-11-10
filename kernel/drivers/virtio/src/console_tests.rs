// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console driver against a simulated virtio console of three ports:
//! the same PCI configuration space as the block tests, a BAR with eight
//! queues, a control queue the device answers DEVICE_READY on with its
//! ports (port 1 named as QEMU names its guest agent's), and the ports'
//! queues, on which the test has the device send and take bytes.

use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Mmio, Registers, fake_memory_write};

use crate::console::{BUFFER, Console, Event, Info, NAME_MAX, event};
use crate::snd_tests::{Ring, next_chain, put_used, read_mem};
use crate::tests::{BAR_SIZE, alloc, function};
use crate::{Error, Transport, device_type, feature as common_feature, status};

const AGENT: &[u8] = b"org.qemu.guest_agent.0";

#[derive(Default)]
struct State {
    features: u64,
    device_select: u32,
    driver_select: u32,
    driver_features: u64,
    status: u8,
    select: u16,
    rings: [Ring; 8],
    /// The control messages the driver sent: port, event, value.
    sent: Vec<(u32, u16, u16)>,
    /// Buffers the driver gave for each port's data from the host, and
    /// for control messages: (queue, head, address, length).
    given: Vec<(usize, u16, u64, u32)>,
    /// The bytes each port sent to the host.
    received: [Vec<u8>; 3],
}

struct Device {
    state: Mutex<State>,
}

impl Device {
    fn new(features: u64) -> Arc<Device> {
        Arc::new(Device { state: Mutex::new(State { features, ..State::default() }) })
    }

    /// Takes the buffers the driver made available on `queue` to fill.
    fn take_given(s: &mut State, queue: usize) {
        while let Some((head, buffers)) = next_chain(&mut s.rings[queue]) {
            assert_eq!(buffers.len(), 1);
            let (at, len, writes) = buffers[0];
            assert!(writes, "the device fills these");
            s.given.push((queue, head, at, len));
        }
    }

    /// Fills the oldest buffer given on `queue` with `bytes`.
    fn put(s: &mut State, queue: usize, bytes: &[u8]) {
        Device::take_given(s, queue);
        let i = s.given.iter().position(|g| g.0 == queue).expect("a buffer to fill");
        let (_, head, at, len) = s.given.remove(i);
        assert!(bytes.len() <= len as usize);
        fake_memory_write(at, bytes);
        put_used(&mut s.rings[queue], head, bytes.len() as u32);
    }

    fn control(s: &mut State, port: u32, kind: u16, value: u16, more: &[u8]) {
        let mut message = Vec::new();
        message.extend_from_slice(&port.to_le_bytes());
        message.extend_from_slice(&kind.to_le_bytes());
        message.extend_from_slice(&value.to_le_bytes());
        message.extend_from_slice(more);
        Device::put(s, 2, &message);
    }

    /// Reads the control messages the driver sent, and answers
    /// DEVICE_READY with ports 0 and 1, 0 a console and 1 named.
    fn control_sent(s: &mut State) {
        while let Some((head, buffers)) = next_chain(&mut s.rings[3]) {
            let (at, len, writes) = buffers[0];
            assert!(!writes && len == 8);
            let m = read_mem(at, 8);
            let port = u32::from_le_bytes(m[0..4].try_into().unwrap());
            let kind = u16::from_le_bytes(m[4..6].try_into().unwrap());
            let value = u16::from_le_bytes(m[6..8].try_into().unwrap());
            s.sent.push((port, kind, value));
            put_used(&mut s.rings[3], head, 0);
            if kind == event::DEVICE_READY {
                Device::control(s, 0, event::DEVICE_ADD, 0, &[]);
                Device::control(s, 1, event::DEVICE_ADD, 0, &[]);
                Device::control(s, 0, event::CONSOLE_PORT, 1, &[]);
                let mut name = AGENT.to_vec();
                name.push(0);
                Device::control(s, 1, event::PORT_NAME, 0, &name);
            }
        }
    }

    /// Takes what port `port`'s driver sent on its transmit queue.
    fn transmitted(s: &mut State, queue: usize, port: usize) {
        while let Some((head, buffers)) = next_chain(&mut s.rings[queue]) {
            let (at, len, writes) = buffers[0];
            assert!(!writes);
            let bytes = read_mem(at, len as usize);
            s.received[port].extend_from_slice(&bytes);
            put_used(&mut s.rings[queue], head, 0);
        }
    }

    /// Sends `bytes` from the host on port `port`, a buffer at a time.
    fn send(&self, port: usize, bytes: &[u8]) {
        let mut s = self.state.lock().unwrap();
        let queue = if port == 0 { 0 } else { 2 * port + 2 };
        for chunk in bytes.chunks(BUFFER) {
            Device::put(&mut s, queue, chunk);
        }
    }

    /// Says the program at the host's end of port `port` opened it.
    fn host_open(&self, port: u32) {
        let mut s = self.state.lock().unwrap();
        Device::control(&mut s, port, event::PORT_OPEN, 1, &[]);
    }
}

impl Registers for Device {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let s = self.state.lock().unwrap();
        match (offset, size) {
            (0x04, 4) => (s.features >> (32 * s.device_select)) & 0xffff_ffff,
            (0x12, 2) => 8,
            (0x14, 1) => u64::from(s.status),
            (0x15, 1) => 0,
            (0x18, 2) => 32,
            (0x1e, 2) => u64::from(s.select),
            (0x1a, 2) => 0,
            // 80 columns, 25 rows, 3 ports.
            (0x2000, 4) => 0x50 | 0x19 << 16,
            (0x2004, 4) => 3,
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
                if value & status::FEATURES_OK != 0 && s.driver_features & !s.features != 0 {
                    s.status = value & !status::FEATURES_OK;
                } else {
                    s.status = value;
                }
                if value == 0 {
                    s.rings = [Ring::default(); 8];
                    s.given.clear();
                }
            }
            0x16 => s.select = value as u16,
            0x18 => s.rings[select].size = value as u16,
            0x20 => s.rings[select].desc = value,
            0x28 => s.rings[select].driver = value,
            0x30 => s.rings[select].device = value,
            0x3000.. => {
                assert!(s.status & status::DRIVER_OK != 0, "notified before ready");
                match (offset - 0x3000) / 4 {
                    3 => Device::control_sent(&mut s),
                    1 => Device::transmitted(&mut s, 1, 0),
                    5 => Device::transmitted(&mut s, 5, 1),
                    queue => Device::take_given(&mut s, queue),
                }
            }
            _ => {}
        }
    }
}

const FEATURES: u64 = common_feature::VERSION_1 | common_feature::EVENT_IDX | 1 << 1;

fn transport(device: &Arc<Device>) -> Transport {
    let f = function(3, 0x1043);
    let regs = device.clone();
    Transport::find(&f, &mut |_, _| Some(Mmio::fake(regs.clone(), BAR_SIZE))).unwrap()
}

/// A started console with ports 0 and 1 set up, and its events.
fn setup() -> (Arc<Device>, Console, [crate::console::Port; 2], Vec<Event>) {
    let device = Device::new(FEATURES);
    let mut console = Console::new(transport(&device), 0, &mut alloc).unwrap();
    let ports = [console.port(0, 0, &mut alloc).unwrap(), console.port(1, 0, &mut alloc).unwrap()];
    console.start().unwrap();
    let mut events = Vec::new();
    console.events(&mut |e| events.push(e));
    (device, console, ports, events)
}

fn named(port: u32, name: &[u8]) -> Event {
    let mut bytes = [0u8; NAME_MAX];
    bytes[..name.len()].copy_from_slice(name);
    Event::Named(port, bytes, name.len())
}

#[test]
fn finds_the_console() {
    assert_eq!(device_type(&function(3, 0x1043)), Some(device_type::CONSOLE));
}

#[test]
fn needs_several_ports() {
    let device = Device::new(common_feature::VERSION_1);
    let made = Console::new(transport(&device), 0, &mut alloc);
    assert!(matches!(made, Err(Error::MissingFeature(_))));
    assert_eq!(device.state.lock().unwrap().status & status::FAILED, status::FAILED);
}

#[test]
fn learns_its_ports_and_their_names() {
    let (device, mut console, _, events) = setup();
    assert_eq!(console.info(), Info { ports: 3 });
    assert_eq!(events, [Event::Added(0), Event::Added(1), Event::Console(0), named(1, AGENT)]);
    console.port_ready(1, true).unwrap();
    let s = device.state.lock().unwrap();
    assert_eq!(s.sent, [(0, event::DEVICE_READY, 1), (1, event::PORT_READY, 1)]);
    // Port 2 was never set up: its queues are empty.
    assert_eq!(s.rings[6].size, 0);
}

#[test]
fn sets_up_no_port_past_the_last() {
    let device = Device::new(FEATURES);
    let console = Console::new(transport(&device), 0, &mut alloc).unwrap();
    assert!(matches!(console.port(3, 0, &mut alloc), Err(Error::NoQueue)));
}

#[test]
fn carries_bytes_both_ways() {
    let (device, _console, [_port0, mut port], _) = setup();
    port.fill();
    // Two buffers from the host, read in pieces smaller than either.
    let from_host: Vec<u8> = (0..700u32).map(|i| i as u8).collect();
    device.send(1, &from_host);
    let mut got = Vec::new();
    let mut piece = [0u8; 300];
    loop {
        let n = port.read(&mut piece);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&piece[..n]);
    }
    assert_eq!(got, from_host);
    // Each buffer read went back: the host can send eight more.
    device.send(1, &[7u8; 8 * BUFFER]);
    // To the host, a buffer's worth at a time.
    let to_host = [b'x'; 600];
    assert_eq!(port.write(&to_host), BUFFER);
    assert_eq!(port.write(&to_host[BUFFER..]), 600 - BUFFER);
    assert!(!port.sending(), "the device took both at once");
    assert_eq!(device.state.lock().unwrap().received[1], to_host);
}

#[test]
fn tells_of_ports_opened_at_either_end() {
    let (device, mut console, _, _) = setup();
    console.port_open(1, true).unwrap();
    console.port_open(1, false).unwrap();
    assert_eq!(
        device.state.lock().unwrap().sent[1..],
        [(1, event::PORT_OPEN, 1), (1, event::PORT_OPEN, 0)]
    );
    device.host_open(1);
    let mut events = Vec::new();
    console.events(&mut |e| events.push(e));
    assert_eq!(events, [Event::HostOpen(1, true)]);
}

#[test]
fn resets_the_device_when_dropped() {
    let (device, console, ports, _) = setup();
    drop(ports);
    drop(console);
    assert_eq!(device.state.lock().unwrap().status, 0);
}
