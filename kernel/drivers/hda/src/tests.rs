// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver against a simulated controller and codecs: the controller's
//! registers, with its rings and an output stream's DMA engine played by
//! hand, and codecs that answer commands from a table of widgets.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::vec;
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers, fake_memory, fake_memory_write};

use super::*;
use crate::codec::step;
use crate::regs::*;
use crate::verb::{AmpCaps, param, pincap, wcap};

/// The registers' bytes: GCAP to the end of eight stream descriptors.
const BAR: usize = 0x4000;
/// The first output stream's registers: four input streams come first.
const OUT: usize = SD_BASE + 4 * SD_SIZE;

/// A widget of a simulated codec.
#[derive(Clone, Default)]
struct Node {
    caps: u32,
    pin_caps: u32,
    config: u32,
    connections: Vec<u16>,
    /// Whether the connection list is read in the long form.
    long: bool,
    pcm: u32,
    in_amp: u32,
    out_amp: u32,
}

/// A simulated codec: its widgets, and what it was told.
#[derive(Clone, Default)]
struct FakeCodec {
    vendor: u32,
    subsystem: u32,
    /// The audio function group, and its widgets from `first`.
    first: u8,
    nodes: HashMap<u8, Node>,
    /// The function group's own parameters.
    afg_pcm: u32,
    afg_out_amp: u32,
    afg_in_amp: u32,
    /// The digital codec of a display: its group is not audio.
    not_audio: bool,
    /// Whether it answers at all.
    mute: bool,
    // What it was told.
    log: Vec<(u8, u32)>,
    pin_control: HashMap<u8, u8>,
    select: HashMap<u8, u8>,
    stream: HashMap<u8, u8>,
    format: HashMap<u8, u16>,
    eapd: HashMap<u8, u8>,
    /// Amplifiers: (node, output, input index, right) to gain or mute.
    amps: HashMap<(u8, bool, u8, bool), u16>,
    present: HashMap<u8, bool>,
    coef_index: u16,
    coefs: HashMap<u16, u16>,
}

impl FakeCodec {
    fn answer(&mut self, nid: u8, data: u32) -> u32 {
        self.log.push((nid, data));
        let (verb, payload) = if data & 0x70000 == 0x70000 {
            ((data >> 8) & 0xfff, data & 0xff)
        } else {
            ((data >> 8) & 0xf00, data & 0xffff)
        };
        let afg = 1u8;
        match verb {
            0xf00 => self.parameter(nid, payload as u8),
            0xf02 => {
                let Some(node) = self.nodes.get(&nid) else { return 0 };
                let (per, bits) = if node.long { (2, 16) } else { (4, 8) };
                let mut word = 0;
                for i in 0..per {
                    if let Some(&entry) = node.connections.get(payload as usize + i) {
                        word |= u32::from(entry) << (i * bits);
                    }
                }
                word
            }
            0xf1c => self.nodes.get(&nid).map_or(0, |node| node.config),
            0xf20 if nid == afg => self.subsystem,
            0xf05 => 0,
            0xf09 => {
                if self.present.get(&nid).copied().unwrap_or(false) {
                    1 << 31
                } else {
                    0
                }
            }
            0x701 => {
                self.select.insert(nid, payload as u8);
                0
            }
            0x706 => {
                self.stream.insert(nid, payload as u8);
                0
            }
            0x707 => {
                self.pin_control.insert(nid, payload as u8);
                0
            }
            0x70c => {
                self.eapd.insert(nid, payload as u8);
                0
            }
            0x200 => {
                self.format.insert(nid, payload as u16);
                0
            }
            0x300 => {
                let output = payload & 0x8000 != 0;
                let input = payload & 0x4000 != 0;
                let index = ((payload >> 8) & 0xf) as u8;
                let level = (payload & 0xff) as u16;
                for (side, right) in [(0x2000, false), (0x1000, true)] {
                    if payload & side != 0 {
                        if output {
                            self.amps.insert((nid, true, 0, right), level);
                        }
                        if input {
                            self.amps.insert((nid, false, index, right), level);
                        }
                    }
                }
                0
            }
            0x500 => {
                self.coef_index = payload as u16;
                0
            }
            0xc00 => u32::from(self.coefs.get(&self.coef_index).copied().unwrap_or(0)),
            0x400 => {
                self.coefs.insert(self.coef_index, payload as u16);
                0
            }
            _ => 0,
        }
    }

    fn parameter(&self, nid: u8, which: u8) -> u32 {
        let afg = 1u8;
        if nid == 0 {
            return match which {
                param::VENDOR_ID => self.vendor,
                param::NODE_COUNT => (1 << 16) | 1,
                _ => 0,
            };
        }
        if nid == afg {
            let count = self.nodes.keys().max().map_or(0, |&last| u32::from(last - self.first) + 1);
            return match which {
                param::FUNCTION_TYPE => u32::from(!self.not_audio),
                param::NODE_COUNT => (u32::from(self.first) << 16) | count,
                param::PCM => self.afg_pcm,
                param::OUT_AMP_CAPS => self.afg_out_amp,
                param::IN_AMP_CAPS => self.afg_in_amp,
                _ => 0,
            };
        }
        let Some(node) = self.nodes.get(&nid) else { return 0 };
        match which {
            param::WIDGET_CAPS => node.caps,
            param::PIN_CAPS => node.pin_caps,
            param::PCM => node.pcm,
            param::IN_AMP_CAPS => node.in_amp,
            param::OUT_AMP_CAPS => node.out_amp,
            param::CONNECTION_LIST_LEN => {
                node.connections.len() as u32 | if node.long { 0x80 } else { 0 }
            }
            _ => 0,
        }
    }
}

/// The simulated controller's state.
#[derive(Default)]
struct State {
    regs: Vec<u8>,
    codecs: HashMap<u8, FakeCodec>,
    corb_rp: u16,
    rirb_wp: u16,
    /// Responses since the response flag was last cleared: as QEMU's
    /// controller does, no command is taken once there are RINTCNT.
    rirb_count: u64,
    /// Whether CORBRP's reset bit reads back while set.
    shows_corbrp_reset: bool,
    /// The output stream's DMA engine: its list, read when it starts.
    entries: Vec<(u64, u32)>,
    lpib: u32,
    running: bool,
}

impl State {
    fn get(&self, offset: usize, size: usize) -> u64 {
        let mut bytes = [0u8; 8];
        bytes[..size].copy_from_slice(&self.regs[offset..offset + size]);
        u64::from_le_bytes(bytes)
    }

    fn put(&mut self, offset: usize, size: usize, value: u64) {
        self.regs[offset..offset + size].copy_from_slice(&value.to_le_bytes()[..size]);
    }

    /// Runs the commands written to the CORB, and writes their responses
    /// to the RIRB.
    fn run_corb(&mut self) {
        if self.get(CORBCTL, 1) as u8 & CORBCTL_RUN == 0 {
            return;
        }
        let corb = self.get(CORBLBASE, 4) | self.get(CORBUBASE, 4) << 32;
        let rirb = self.get(RIRBLBASE, 4) | self.get(RIRBUBASE, 4) << 32;
        let wp = self.get(CORBWP, 2) as u16 & 0xff;
        let count = self.get(RINTCNT, 2) & 0xff;
        while self.corb_rp != wp && self.rirb_count != count {
            self.corb_rp = (self.corb_rp + 1) % 256;
            let mut word = [0u8; 4];
            fake_memory(corb + u64::from(self.corb_rp) * 4, &mut word);
            let word = u32::from_le_bytes(word);
            let cad = (word >> 28) as u8;
            let nid = (word >> 20) as u8;
            let Some(codec) = self.codecs.get_mut(&cad) else { continue };
            if codec.mute {
                continue;
            }
            let response = codec.answer(nid, word & 0xf_ffff);
            if self.get(RIRBCTL, 1) as u8 & RIRBCTL_DMAEN == 0 {
                continue;
            }
            self.rirb_wp = (self.rirb_wp + 1) % 256;
            let at = rirb + u64::from(self.rirb_wp) * 8;
            fake_memory_write(at, &response.to_le_bytes());
            fake_memory_write(at + 4, &u32::from(cad).to_le_bytes());
            self.rirb_count += 1;
            if self.rirb_count == count && self.get(RIRBCTL, 1) as u8 & RIRBCTL_RINTCTL != 0 {
                let flags = self.get(RIRBSTS, 1) | 1;
                self.put(RIRBSTS, 1, flags);
            }
        }
        self.put(CORBRP, 2, u64::from(self.corb_rp));
        self.put(RIRBWP, 2, u64::from(self.rirb_wp));
    }

    fn write(&mut self, offset: usize, size: usize, value: u64) {
        match offset {
            GCTL => {
                let was = self.get(GCTL, 4) as u32 & GCTL_CRST;
                self.put(GCTL, 4, value);
                if was == 0 && value as u32 & GCTL_CRST != 0 {
                    let mask = self.codecs.keys().fold(0u16, |mask, &cad| mask | 1 << cad);
                    self.put(STATESTS, 2, u64::from(mask));
                }
            }
            STATESTS => {
                let left = self.get(STATESTS, 2) & !value;
                self.put(STATESTS, 2, left);
            }
            CORBRP => {
                if value as u16 & CORBRP_RST != 0 {
                    self.corb_rp = 0;
                    let shown = if self.shows_corbrp_reset { u64::from(CORBRP_RST) } else { 0 };
                    self.put(CORBRP, 2, shown);
                } else {
                    self.put(CORBRP, 2, u64::from(self.corb_rp));
                }
            }
            CORBWP => {
                self.put(CORBWP, 2, value);
                self.run_corb();
            }
            RIRBWP => {
                if value as u16 & RIRBWP_RST != 0 {
                    self.rirb_wp = 0;
                    self.put(RIRBWP, 2, 0);
                }
            }
            RIRBSTS => {
                let was = self.get(RIRBSTS, 1);
                let left = was & !value;
                self.put(RIRBSTS, 1, left);
                if was & 1 != 0 && left & 1 == 0 {
                    self.rirb_count = 0;
                    self.run_corb();
                }
            }
            CORBSIZE | RIRBSIZE => self.put(offset, 1, u64::from(SIZE_CAP_256) | (value & 3)),
            o if o == OUT + SD_CTL && size == 4 => {
                let value = value as u32 & 0x00ff_ffff;
                if value & SD_CTL_SRST != 0 {
                    self.put(o, 4, u64::from(SD_CTL_SRST));
                    self.lpib = 0;
                    self.running = false;
                    self.entries.clear();
                    return;
                }
                let was = self.running;
                self.put(o, 4, u64::from(value));
                self.running = value & SD_CTL_RUN != 0;
                if self.running && !was {
                    // QEMU's way: the list is read and played from its
                    // start at each start.
                    let list = self.get(OUT + SD_BDPL, 4) | self.get(OUT + SD_BDPU, 4) << 32;
                    let count = self.get(OUT + SD_LVI, 2) as usize + 1;
                    self.entries.clear();
                    for i in 0..count {
                        let mut entry = [0u8; 16];
                        fake_memory(list + i as u64 * 16, &mut entry);
                        let address = u64::from_le_bytes(entry[..8].try_into().unwrap());
                        let length = u32::from_le_bytes(entry[8..12].try_into().unwrap());
                        self.entries.push((address, length));
                    }
                    self.lpib = 0;
                }
            }
            o if o == OUT + SD_STS => {}
            _ => self.put(offset, size, value),
        }
    }

    fn read(&self, offset: usize, size: usize) -> u64 {
        if offset == OUT + SD_LPIB {
            return u64::from(self.lpib);
        }
        self.get(offset, size)
    }

    /// Plays `bytes` of the output stream, as its DMA engine would:
    /// returns them.
    fn play(&mut self, bytes: usize) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.running || self.entries.is_empty() {
            return out;
        }
        let total: u32 = self.entries.iter().map(|entry| entry.1).sum();
        for _ in 0..bytes {
            let mut at = self.lpib;
            let mut byte = [0u8];
            for &(address, length) in &self.entries {
                if at < length {
                    fake_memory(address + u64::from(at), &mut byte);
                    break;
                }
                at -= length;
            }
            out.push(byte[0]);
            self.lpib = (self.lpib + 1) % total;
        }
        out
    }
}

/// A simulated controller.
#[derive(Clone)]
struct Fake(Arc<Mutex<State>>);

impl Registers for Fake {
    fn read(&self, offset: usize, size: usize) -> u64 {
        self.0.lock().unwrap().read(offset, size)
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        self.0.lock().unwrap().write(offset, size, value);
    }
}

impl Fake {
    /// A controller with four input and four output streams that reaches
    /// all of memory, and `codecs` on its link.
    fn new(codecs: Vec<(u8, FakeCodec)>) -> Fake {
        let mut state = State { regs: vec![0; BAR], ..State::default() };
        state.put(GCAP, 2, 0x4401);
        state.put(CORBSIZE, 1, u64::from(SIZE_CAP_256));
        state.put(RIRBSIZE, 1, u64::from(SIZE_CAP_256));
        state.put(OUT + SD_FIFOS, 2, 0xff);
        state.codecs = codecs.into_iter().collect();
        Fake(Arc::new(Mutex::new(state)))
    }

    fn mmio(&self) -> Mmio {
        Mmio::fake(Arc::new(self.clone()), BAR)
    }

    fn codec(&self, cad: u8) -> FakeCodec {
        self.0.lock().unwrap().codecs[&cad].clone()
    }

    fn with_codec(&self, cad: u8, change: impl FnOnce(&mut FakeCodec)) {
        change(self.0.lock().unwrap().codecs.get_mut(&cad).unwrap());
    }

    fn play(&self, bytes: usize) -> Vec<u8> {
        self.0.lock().unwrap().play(bytes)
    }

    fn running(&self) -> bool {
        self.0.lock().unwrap().running
    }
}

/// The kernel, as the driver sees it.
#[derive(Default)]
struct TestHost {
    waited_us: u64,
}

impl Host for TestHost {
    fn delay_us(&mut self, us: u32) {
        self.waited_us += u64::from(us);
    }

    fn alloc(&mut self, pages: usize, _low: bool) -> Option<Dma> {
        Some(Dma::fake(pages * 4096))
    }
}

/// QEMU's amplifier: 74 steps of 1 dB, 0 dB at the top, no mute.
const QEMU_AMP: u32 = (3 << 16) | (0x4a << 8) | 0x4a;
/// 16-bit samples at 16 to 96 kHz, as QEMU's converters take.
const QEMU_PCM: u32 = verb::PCM_16BIT | 0x1fc;

/// QEMU's hda-output: a converter (2) and a line out jack (3).
fn qemu_output() -> FakeCodec {
    let mut nodes = HashMap::new();
    nodes.insert(
        2,
        Node {
            caps: (wcap::OUTPUT << 20)
                | wcap::FORMAT_OVERRIDE
                | wcap::AMP_OVERRIDE
                | wcap::OUT_AMP
                | wcap::STEREO,
            pcm: QEMU_PCM,
            out_amp: QEMU_AMP,
            ..Node::default()
        },
    );
    nodes.insert(
        3,
        Node {
            caps: (wcap::PIN << 20) | wcap::CONN_LIST | wcap::STEREO,
            pin_caps: pincap::OUTPUT,
            // A jack, a line out, green.
            config: 0x0001_4010,
            connections: vec![2],
            ..Node::default()
        },
    );
    FakeCodec {
        vendor: 0x1af4_0012,
        subsystem: 0x1af4_1100,
        first: 2,
        nodes,
        afg_pcm: QEMU_PCM,
        ..FakeCodec::default()
    }
}

/// A Realtek ALC256 as a laptop has it: two converters, two mixers each
/// adding one converter to the loopback mixer, a built-in speaker and a
/// headphone jack that each reach both mixers, a microphone jack, and a
/// line out the firmware says is not connected.
fn laptop() -> FakeCodec {
    let mut nodes = HashMap::new();
    let realtek_amp = (1u32 << 31) | (2 << 16) | (0x57 << 8) | 0x57;
    for dac in [2, 3] {
        nodes.insert(
            dac,
            Node {
                caps: (wcap::OUTPUT << 20)
                    | wcap::AMP_OVERRIDE
                    | wcap::OUT_AMP
                    | wcap::STEREO
                    | wcap::POWER,
                out_amp: realtek_amp,
                ..Node::default()
            },
        );
    }
    // The loopback mixer, from the microphone.
    nodes.insert(
        0x0b,
        Node {
            caps: (wcap::MIXER << 20) | wcap::CONN_LIST | wcap::IN_AMP | wcap::STEREO,
            connections: vec![0x18, 0x19],
            ..Node::default()
        },
    );
    for (mixer, dac) in [(0x0c, 2), (0x0d, 3)] {
        nodes.insert(
            mixer,
            Node {
                caps: (wcap::MIXER << 20) | wcap::CONN_LIST | wcap::IN_AMP | wcap::STEREO,
                connections: vec![dac, 0x0b],
                ..Node::default()
            },
        );
    }
    let output_pin = (wcap::PIN << 20) | wcap::CONN_LIST | wcap::OUT_AMP | wcap::STEREO;
    // The speaker: fixed, inside.
    nodes.insert(
        0x14,
        Node {
            caps: output_pin,
            pin_caps: pincap::OUTPUT | pincap::EAPD,
            config: 0x9017_0110,
            connections: vec![0x0c, 0x0d],
            ..Node::default()
        },
    );
    // The headphone jack, on the left, sensing.
    nodes.insert(
        0x21,
        Node {
            caps: output_pin,
            pin_caps: pincap::OUTPUT | pincap::HP | pincap::PRESENCE | pincap::EAPD,
            config: 0x0221_1020,
            connections: vec![0x0c, 0x0d],
            ..Node::default()
        },
    );
    // A line out the firmware says goes nowhere.
    nodes.insert(
        0x1b,
        Node {
            caps: output_pin,
            pin_caps: pincap::OUTPUT | pincap::PRESENCE,
            config: 0x4110_00f0,
            connections: vec![0x0c, 0x0d],
            ..Node::default()
        },
    );
    // The microphone jack: input only.
    nodes.insert(
        0x19,
        Node {
            caps: (wcap::PIN << 20) | wcap::STEREO,
            pin_caps: pincap::PRESENCE | (1 << 5),
            config: 0x02a1_1030,
            ..Node::default()
        },
    );
    FakeCodec {
        vendor: 0x10ec_0256,
        subsystem: 0x1028_0802,
        first: 2,
        nodes,
        afg_pcm: verb::PCM_16BIT | 0x560,
        afg_in_amp: 1 << 31,
        afg_out_amp: 1 << 31,
        ..FakeCodec::default()
    }
}

/// A display's HDMI codec: a digital converter and an HDMI pin.
fn hdmi() -> FakeCodec {
    let mut nodes = HashMap::new();
    nodes.insert(
        2,
        Node {
            caps: (wcap::OUTPUT << 20) | wcap::DIGITAL | wcap::STEREO,
            pcm: QEMU_PCM,
            ..Node::default()
        },
    );
    nodes.insert(
        3,
        Node {
            caps: (wcap::PIN << 20) | wcap::CONN_LIST | wcap::DIGITAL,
            pin_caps: pincap::OUTPUT | pincap::HDMI | pincap::PRESENCE,
            config: 0x1856_0010,
            connections: vec![2],
            ..Node::default()
        },
    );
    FakeCodec { vendor: 0x8086_2809, first: 2, nodes, ..FakeCodec::default() }
}

fn bind(fake: &Fake) -> (Hda, TestHost) {
    let mut host = TestHost::default();
    let hda = Hda::new(fake.mmio(), &mut host).unwrap();
    (hda, host)
}

#[test]
fn format_words() {
    assert_eq!(verb::format(48000, 2), Some(0x0011));
    assert_eq!(verb::format(44100, 2), Some(0x4011));
    assert_eq!(verb::format(96000, 2), Some(0x0811));
    assert_eq!(verb::format(192000, 2), Some(0x1811));
    assert_eq!(verb::format(8000, 1), Some(0x0510));
    assert_eq!(verb::format(32000, 2), Some(0x0a11));
    assert_eq!(verb::format(22050, 2), Some(0x4111));
    assert_eq!(verb::format(64000, 2), None);
    assert_eq!(verb::format(48000, 0), None);
}

#[test]
fn volume_steps() {
    let qemu = AmpCaps(QEMU_AMP);
    assert_eq!(step(qemu, 100), Some(74));
    // 60 log10(0.5) is 18.06 dB down: 18 steps of 1 dB.
    assert_eq!(step(qemu, 50), Some(56));
    assert_eq!(step(qemu, 70), Some(65));
    // No mute: the lowest step.
    assert_eq!(step(qemu, 0), Some(0));
    assert_eq!(step(qemu, 1), Some(0));
    // Realtek's: 87 steps of 0.75 dB, 0 dB at the top, with a mute.
    let realtek = AmpCaps((1 << 31) | (2 << 16) | (0x57 << 8) | 0x57);
    assert_eq!(step(realtek, 0), None);
    assert_eq!(step(realtek, 100), Some(0x57));
    assert_eq!(step(realtek, 50), Some(0x57 - 24));
    // 0 dB in the middle: never above it.
    let middle = AmpCaps((3 << 16) | (0x7f << 8) | 0x40);
    assert_eq!(step(middle, 100), Some(0x40));
}

#[test]
fn qemu_output_is_found_and_routed() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    let (hda, host) = bind(&fake);
    let codec = hda.codec();
    assert_eq!((codec.cad, codec.afg, codec.vendor), (0, 1, 0x1af4_0012));
    let pins: Vec<_> = codec.pins().map(|pin| (pin.nid, pin.role, pin.dac(), pin.on())).collect();
    assert_eq!(pins, [(3, Role::Line, 2, true)]);
    assert_eq!(codec.dacs(), [2]);
    assert!(codec.hardware_volume);
    for rate in [16000, 22050, 32000, 44100, 48000, 88200, 96000] {
        assert!(hda.plays(rate), "{rate}");
    }
    for rate in [8000, 11025, 176400, 192000, 64000] {
        assert!(!hda.plays(rate), "{rate}");
    }
    assert_eq!(hda.channels(), 2);
    let seen = fake.codec(0);
    assert_eq!(seen.pin_control.get(&3), Some(&verb::PIN_OUT));
    assert_eq!(seen.stream.get(&2), Some(&0x10));
    // One connection: nothing to choose.
    assert!(seen.select.is_empty());
    // The converter's amplifier is left for the volume; nothing else has
    // one.
    assert!(seen.amps.is_empty());
    // Out of reset, the codecs asked for the link, and the rings run.
    let mmio = fake.mmio();
    assert_ne!(mmio.read32(GCTL) & GCTL_CRST, 0);
    assert_eq!(mmio.read8(CORBCTL) & CORBCTL_RUN, CORBCTL_RUN);
    let on = RIRBCTL_DMAEN | RIRBCTL_RINTCTL;
    assert_eq!(mmio.read8(RIRBCTL) & on, on);
    assert_eq!(mmio.read32(INTCTL), 0);
    assert!(host.waited_us >= 1100);
}

#[test]
fn laptop_plays_one_converter_and_headphones_take_the_speaker() {
    let fake = Fake::new(vec![(0, laptop())]);
    let (mut hda, mut host) = bind(&fake);
    let codec = hda.codec();
    let pins: Vec<_> = codec.pins().map(|pin| (pin.nid, pin.role, pin.dac(), pin.on())).collect();
    // The speaker first; the line out that goes nowhere and the
    // microphone are not outputs.
    assert_eq!(pins, [(0x14, Role::Speaker, 2, true), (0x21, Role::Headphones, 2, false)]);
    assert_eq!(codec.dacs(), [2]);
    assert!(codec.hardware_volume);
    assert_eq!(codec.subsystem, 0x1028_0802);
    for rate in [44100, 48000, 96000, 192000] {
        assert!(hda.plays(rate), "{rate}");
    }
    let seen = fake.codec(0);
    // The ALC256's coefficient, as FreeBSD sets it.
    assert_eq!(seen.coefs.get(&0x46), Some(&0x3000));
    // Both pins choose the first mixer, which unmutes the converter and
    // mutes the loopback.
    assert_eq!(seen.select.get(&0x14), Some(&0));
    assert_eq!(seen.select.get(&0x21), Some(&0));
    assert_eq!(seen.amps.get(&(0x0c, false, 0, false)), Some(&0));
    assert_eq!(seen.amps.get(&(0x0c, false, 1, false)), Some(&0x80));
    assert_eq!(seen.amps.get(&(0x0c, false, 1, true)), Some(&0x80));
    // The pins' own amplifiers: unmuted at 0 dB.
    assert_eq!(seen.amps.get(&(0x14, true, 0, false)), Some(&0));
    assert_eq!(seen.amps.get(&(0x21, true, 0, true)), Some(&0));
    assert_eq!(seen.eapd.get(&0x14), Some(&verb::EAPD));
    assert_eq!(seen.eapd.get(&0x21), Some(&verb::EAPD));
    assert_eq!(seen.pin_control.get(&0x14), Some(&verb::PIN_OUT));
    assert_eq!(seen.pin_control.get(&0x21), Some(&0));
    assert!(!seen.pin_control.contains_key(&0x1b));
    // The converter is powered up and takes the stream.
    assert!(seen.log.contains(&(2, verb::v12(verb::SET_POWER_STATE, 0))));
    assert_eq!(seen.stream.get(&2), Some(&0x10));
    assert!(!seen.stream.contains_key(&3));
    // Nothing changed: nothing to say.
    assert_eq!(hda.look(&mut host), None);
    // Headphones in: they play, the speaker stops.
    fake.with_codec(0, |codec| {
        codec.present.insert(0x21, true);
    });
    assert_eq!(hda.look(&mut host), Some(Route { speakers: false, headphones: true, line: false }));
    let seen = fake.codec(0);
    assert_eq!(seen.pin_control.get(&0x14), Some(&0));
    assert_eq!(seen.pin_control.get(&0x21), Some(&(verb::PIN_OUT | verb::PIN_HP)));
    // And out again.
    fake.with_codec(0, |codec| {
        codec.present.insert(0x21, false);
    });
    assert_eq!(hda.look(&mut host), Some(Route { speakers: true, headphones: false, line: false }));
    // The volume is the converter's, each channel its own.
    assert!(hda.set_volume(50, 0, &mut host));
    let seen = fake.codec(0);
    assert_eq!(seen.amps.get(&(2, true, 0, false)), Some(&(0x57 - 24)));
    assert_eq!(seen.amps.get(&(2, true, 0, true)), Some(&0x80));
}

#[test]
fn the_converter_the_most_pins_reach_plays() {
    let mut codec = laptop();
    // The headphones reach only the second mixer: the speaker plays from
    // its converter too, through its second input.
    codec.nodes.get_mut(&0x21).unwrap().connections = vec![0x0d];
    let fake = Fake::new(vec![(0, codec)]);
    let (hda, _) = bind(&fake);
    let pins: Vec<_> = hda.codec().pins().map(|pin| (pin.nid, pin.dac())).collect();
    assert_eq!(pins, [(0x14, 3), (0x21, 3)]);
    assert_eq!(hda.codec().dacs(), [3]);
    let seen = fake.codec(0);
    assert_eq!(seen.select.get(&0x14), Some(&1));
    assert!(!seen.stream.contains_key(&2));
}

#[test]
fn a_pin_that_cannot_reach_the_converter_gets_its_own() {
    let mut codec = laptop();
    // Each pin reaches one mixer: the speaker's converter, the first
    // pin's, plays, and the headphones get the other.
    codec.nodes.get_mut(&0x14).unwrap().connections = vec![0x0c];
    codec.nodes.get_mut(&0x21).unwrap().connections = vec![0x0d];
    let fake = Fake::new(vec![(0, codec)]);
    let (hda, _) = bind(&fake);
    let pins: Vec<_> = hda.codec().pins().map(|pin| (pin.nid, pin.dac())).collect();
    assert_eq!(pins, [(0x14, 2), (0x21, 3)]);
    assert_eq!(hda.codec().dacs(), [2, 3]);
    let seen = fake.codec(0);
    assert_eq!(seen.stream.get(&2), Some(&0x10));
    assert_eq!(seen.stream.get(&3), Some(&0x10));
    // The second mixer passes its converter too.
    assert_eq!(seen.amps.get(&(0x0d, false, 0, false)), Some(&0));
    assert_eq!(seen.amps.get(&(0x0d, false, 1, false)), Some(&0x80));
}

#[test]
fn a_way_without_steps_leaves_the_volume_to_the_kernel() {
    let mut codec = qemu_output();
    codec.nodes.get_mut(&2).unwrap().out_amp = 0;
    let fake = Fake::new(vec![(0, codec)]);
    let (mut hda, mut host) = bind(&fake);
    assert!(!hda.codec().hardware_volume);
    assert!(!hda.set_volume(50, 50, &mut host));
    // The fixed amplifier is still unmuted.
    assert_eq!(fake.codec(0).amps.get(&(2, true, 0, false)), Some(&0));
}

#[test]
fn display_codecs_are_passed_over() {
    let fake = Fake::new(vec![(0, hdmi())]);
    let mut host = TestHost::default();
    assert_eq!(Hda::new(fake.mmio(), &mut host).err(), Some(Error::NoOutput));
    // The analog codec after it is the one.
    let fake = Fake::new(vec![(0, hdmi()), (2, qemu_output())]);
    let (hda, _) = bind(&fake);
    assert_eq!(hda.codec().cad, 2);
    // A codec that does not answer is passed over too.
    let silent = FakeCodec { mute: true, ..qemu_output() };
    let fake = Fake::new(vec![(0, silent), (1, qemu_output())]);
    let (hda, _) = bind(&fake);
    assert_eq!(hda.codec().cad, 1);
    // With no other, the controller says it went unanswered.
    let silent = FakeCodec { mute: true, ..qemu_output() };
    let fake = Fake::new(vec![(0, silent)]);
    assert_eq!(Hda::new(fake.mmio(), &mut TestHost::default()).err(), Some(Error::NoAnswer));
    // A group that is not audio (a modem) has no outputs.
    let modem = FakeCodec { not_audio: true, ..qemu_output() };
    let fake = Fake::new(vec![(0, modem)]);
    assert_eq!(Hda::new(fake.mmio(), &mut TestHost::default()).err(), Some(Error::NoOutput));
    // And a link with no codec.
    let fake = Fake::new(vec![]);
    assert_eq!(Hda::new(fake.mmio(), &mut TestHost::default()).err(), Some(Error::NoOutput));
}

#[test]
fn a_controller_that_is_not_there() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    fake.0.lock().unwrap().put(GCAP, 2, 0xffff);
    assert_eq!(Hda::new(fake.mmio(), &mut TestHost::default()).err(), Some(Error::Gone));
    // More streams than its registers hold.
    let fake = Fake::new(vec![(0, qemu_output())]);
    fake.0.lock().unwrap().put(GCAP, 2, 0xfff9);
    let small = Mmio::fake(Arc::new(fake.clone()), 0x200);
    assert_eq!(Hda::new(small, &mut TestHost::default()).err(), Some(Error::Gone));
}

#[test]
fn the_read_pointers_reset_bit_may_show() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    fake.0.lock().unwrap().shows_corbrp_reset = true;
    let (hda, _) = bind(&fake);
    assert_eq!(hda.codec().vendor, 0x1af4_0012);
}

#[test]
fn connection_lists_in_both_forms_with_ranges() {
    let mut codec = qemu_output();
    // The pin's list: 4, then a range to 7 (5, 6 and 7), then 2, in the
    // long form.
    let pin = codec.nodes.get_mut(&3).unwrap();
    pin.connections = vec![4, 0x8007, 2];
    pin.long = true;
    for nid in 4..=7 {
        codec.nodes.insert(nid, Node { caps: wcap::PIN << 20, ..Node::default() });
    }
    let fake = Fake::new(vec![(0, codec)]);
    let (_, _) = bind(&fake);
    // The converter is the list's fifth entry: 4, 5, 6, 7, 2.
    assert_eq!(fake.codec(0).select.get(&3), Some(&4));
    // And in the short form.
    let mut codec = qemu_output();
    let pin = codec.nodes.get_mut(&3).unwrap();
    pin.connections = vec![4, 0x87, 2];
    for nid in 4..=7 {
        codec.nodes.insert(nid, Node { caps: wcap::PIN << 20, ..Node::default() });
    }
    let fake = Fake::new(vec![(0, codec)]);
    let (_, _) = bind(&fake);
    assert_eq!(fake.codec(0).select.get(&3), Some(&4));
}

/// A period of `byte`s.
fn period(byte: u8) -> Vec<u8> {
    vec![byte; 2048]
}

#[test]
fn the_stream_plays_periods_in_order_and_silence_after() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    let (mut hda, mut host) = bind(&fake);
    assert_eq!(hda.configure(44100, 1, 2048, 8, &mut host), Err(Error::BadFormat));
    assert_eq!(hda.configure(8000, 2, 2048, 8, &mut host), Err(Error::BadFormat));
    assert_eq!(hda.configure(48000, 2, 2000, 8, &mut host), Err(Error::BadFormat));
    hda.configure(48000, 2, 2048, 8, &mut host).unwrap();
    assert_eq!(fake.codec(0).format.get(&2), Some(&0x0011));
    // Half a second at least: 47 slots of 2048 bytes.
    let mmio = fake.mmio();
    assert!(hda.can_submit());
    for byte in 1..=7u8 {
        assert!(hda.submit(&period(byte)), "{byte}");
    }
    // At most count - 1 ahead... count in all.
    assert!(hda.submit(&period(8)));
    assert!(!hda.can_submit());
    assert_eq!(hda.in_flight(), 8);
    assert!(hda.start(&mut host, 0));
    assert!(fake.running());
    assert_eq!(mmio.read32(OUT + SD_CBL), 47 * 2048);
    assert_eq!(mmio.read16(OUT + SD_LVI), 46);
    assert_eq!(mmio.read16(OUT + SD_FMT), 0x0011);
    assert_eq!(mmio.read32(OUT + SD_CTL) >> 20 & 0xf, 1);
    // A period and a half played: one is back.
    let played = fake.play(3072);
    assert_eq!(&played[..2048], &period(1)[..]);
    assert_eq!(&played[2048..], &period(2)[..1024]);
    assert_eq!(hda.collect(1_000_000), Some(true));
    assert_eq!(hda.collect(1_000_000), None);
    assert_eq!(hda.in_flight(), 7);
    // The rest, and then silence.
    let played = fake.play(7 * 2048 - 1024 + 4096);
    for (i, byte) in (2..=8u8).enumerate() {
        let start = i * 2048;
        let from = if i == 0 { 0 } else { start - 1024 };
        let expected = if i == 0 { &period(2)[1024..] } else { &period(byte)[..] };
        assert_eq!(&played[from..from + expected.len()], expected, "period {byte}");
    }
    assert!(played[played.len() - 4096..].iter().all(|&byte| byte == 0));
    let mut back = 0;
    while hda.collect(2_000_000) == Some(true) {
        back += 1;
    }
    assert_eq!(back, 7);
    assert_eq!(hda.in_flight(), 0);
    // A lap later, the slots played are silent: nothing plays twice.
    let lap = fake.play(47 * 2048);
    assert!(lap.iter().all(|&byte| byte == 0));
    assert!(hda.stop(&mut host, 3_000_000));
    assert!(!fake.running());
}

#[test]
fn a_period_goes_ahead_of_what_the_engine_reads() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    let (mut hda, mut host) = bind(&fake);
    hda.configure(48000, 2, 2048, 8, &mut host).unwrap();
    assert!(hda.submit(&period(1)));
    assert!(hda.start(&mut host, 0));
    // The engine is 100 bytes from the end of the first slot when the
    // next period comes: it cannot go in the slot the engine is about to
    // read, so it goes in the one after.
    let _ = fake.play(1948);
    assert_eq!(hda.collect(1_000_000), None);
    assert!(hda.submit(&period(2)));
    let played = fake.play(100 + 2048 + 2048);
    assert_eq!(&played[..100], &period(1)[..100]);
    assert!(played[100..2148].iter().all(|&byte| byte == 0));
    assert_eq!(&played[2148..], &period(2)[..]);
    assert_eq!(hda.collect(2_000_000), Some(true));
    assert_eq!(hda.collect(2_000_000), Some(true));
    assert_eq!(hda.collect(2_000_000), None);
}

#[test]
fn a_stop_keeps_the_periods_and_a_start_plays_them_from_the_list_start() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    let (mut hda, mut host) = bind(&fake);
    hda.configure(48000, 2, 2048, 8, &mut host).unwrap();
    for byte in 1..=3u8 {
        assert!(hda.submit(&period(byte)));
    }
    assert!(hda.start(&mut host, 0));
    let _ = fake.play(1024);
    // Stopped half way through the first period: it counts as played.
    assert!(hda.stop(&mut host, 1_000_000));
    assert_eq!(hda.collect(1_000_000), Some(true));
    assert_eq!(hda.collect(1_000_000), None);
    assert_eq!(hda.in_flight(), 2);
    assert!(hda.submit(&period(4)));
    // QEMU starts the list from its first entry: the list now starts at
    // the second slot, so the rest play in order.
    assert!(hda.start(&mut host, 2_000_000));
    let played = fake.play(3 * 2048);
    assert_eq!(&played[..2048], &period(2)[..]);
    assert_eq!(&played[2048..4096], &period(3)[..]);
    assert_eq!(&played[4096..], &period(4)[..]);
    let mut back = 0;
    while hda.collect(3_000_000) == Some(true) {
        back += 1;
    }
    assert_eq!(back, 3);
}

#[test]
fn a_stream_that_stands_still_fails() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    let (mut hda, mut host) = bind(&fake);
    hda.configure(48000, 2, 2048, 8, &mut host).unwrap();
    assert!(hda.submit(&period(1)));
    assert!(hda.start(&mut host, 0));
    assert_eq!(hda.collect(2_000_000_000), None);
    assert!(!hda.failed());
    assert_eq!(hda.collect(3_500_000_000), None);
    assert!(hda.failed());
    assert!(!hda.can_submit());
}

#[test]
fn release_lets_the_stream_go() {
    let fake = Fake::new(vec![(0, qemu_output())]);
    let (mut hda, mut host) = bind(&fake);
    hda.configure(48000, 2, 2048, 8, &mut host).unwrap();
    assert!(hda.submit(&period(1)));
    assert!(hda.start(&mut host, 0));
    hda.release(&mut host);
    assert!(!fake.running());
    assert_eq!(hda.in_flight(), 0);
    assert!(!hda.can_submit());
    // Set up again, it plays.
    hda.configure(44100, 2, 4096, 4, &mut host).unwrap();
    assert_eq!(fake.codec(0).format.get(&2), Some(&0x4011));
    assert!(hda.submit(&[7; 4096]));
    assert!(hda.start(&mut host, 0));
    assert_eq!(fake.play(4096), [7; 4096]);
    hda.halt(&mut host);
    assert_eq!(fake.mmio().read32(GCTL) & GCTL_CRST, 0);
}

/// A PCI configuration space of bytes.
struct Config(Mutex<Vec<u8>>);

impl Registers for Config {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let bytes = self.0.lock().unwrap();
        let mut value = [0u8; 8];
        value[..size].copy_from_slice(&bytes[offset..offset + size]);
        u64::from_le_bytes(value)
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        self.0.lock().unwrap()[offset..offset + size].copy_from_slice(&value.to_le_bytes()[..size]);
    }
}

/// A controller of `vendor`'s, with a PCI Express capability whose
/// Device Control lets it mark requests No Snoop, and its TCSEL and
/// snoop registers set.
fn controller_config(vendor: u16) -> (Arc<Config>, radix_hw::pci::Function) {
    use radix_hw::pci::{self, Address, Function};
    let mut bytes = vec![0u8; pci::CONFIG_SIZE];
    bytes[0..2].copy_from_slice(&vendor.to_le_bytes());
    bytes[2..4].copy_from_slice(&0x9d71u16.to_le_bytes());
    // Status: a capability list, at 0x60.
    bytes[0x06] = 0x10;
    bytes[0x0a] = 0x03;
    bytes[0x0b] = 0x04;
    bytes[0x34] = 0x60;
    bytes[0x60] = 0x10;
    bytes[0x68..0x6a].copy_from_slice(&0x2810u16.to_le_bytes());
    bytes[0x44] = 0x07;
    bytes[0x42] = 0x05;
    bytes[0x4e] = 0x30;
    let config = Arc::new(Config(Mutex::new(bytes)));
    let address = Address { segment: 0, bus: 0, device: 0x1f, function: 3 };
    let function = Function::probe(address, Mmio::fake(config.clone(), pci::CONFIG_SIZE)).unwrap();
    (config, function)
}

#[test]
fn pci_setup_snoops() {
    // Intel's: traffic class 0, and no No Snoop.
    let (config, function) = controller_config(0x8086);
    assert!(is_hda(function.class()));
    prepare(&function);
    let bytes = config.0.lock().unwrap().clone();
    assert_eq!(bytes[0x44], 0);
    assert_eq!(u16::from_le_bytes([bytes[0x68], bytes[0x69]]), 0x2010);
    assert_eq!(bytes[0x42], 0x05);
    // AMD's: snoop on, its other bits kept.
    let (config, function) = controller_config(0x1022);
    prepare(&function);
    let bytes = config.0.lock().unwrap().clone();
    assert_eq!(bytes[0x42], 0x02);
    assert_eq!(bytes[0x44], 0x07);
    // NVIDIA's.
    let (config, function) = controller_config(0x10de);
    prepare(&function);
    assert_eq!(config.0.lock().unwrap()[0x4e], 0x3f);
}
