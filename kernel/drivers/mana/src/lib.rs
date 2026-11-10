// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Microsoft's Azure Network Adapter, MANA (ADR 0425): the network card an
//! Azure machine with accelerated networking has, a virtual function of
//! the host's card that Hyper-V passes through on a virtual PCI bus.
//!
//! The card is driven through GDMA (the `gdma` module): every request
//! goes on the hardware channel, and the queues are rings in memory the card is told
//! of by DMA regions, lists of their pages. Its one port, vPort 0, gets a
//! queue pair: a send queue and a receive queue, each with its completion
//! queue, whose completions raise an event on an event queue of their
//! own, which raises the card's second MSI-X entry (the first is the
//! hardware channel's, which is polled). Receiving is steered to the one
//! receive queue, and a fence on it says when the steering took.
//!
//! The card carries only some of the machine's traffic: the host sends a
//! flow's frames to the virtual function once it is up, and the rest
//! still comes on the synthetic card (netvsc), which says which virtual
//! function is its partner. The two have the same MAC address, and the
//! kernel bonds them.
//!
//! The layouts and the sequences here follow FreeBSD's mana(4) driver
//! (BSD-2-Clause, credits/freebsd-hyperv.md).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod gdma;

use gdma::{CQE, Card, Channel, EQE, Pages, Queue, REQUEST_HEADER, RESPONSE_HEADER, UNIT};
use gdma::{device, event, kind, request};
use radix_hw::{Dma, Mmio};
use radix_nic::{
    Error, Info, Interrupt, Link, MAX_FRAME, MIN_FRAME, Nic, PAGE, SLOT, Slots, usable_mac,
    wait_for,
};

/// Microsoft's PCI vendor ID.
pub const VENDOR: u16 = 0x1414;
/// The virtual function's PCI device ID.
pub const DEVICE: u16 = 0x00ba;

/// Whether a PCI function is MANA's virtual function.
pub fn identify(vendor: u16, device: u16) -> bool {
    vendor == VENDOR && device == DEVICE
}

/// The MSI-X entry of the hardware channel's events, which are polled, so
/// that the entry stays masked.
pub const CHANNEL_VECTOR: u16 = 0;
/// The MSI-X entry the port's queues raise.
pub const QUEUE_VECTOR: u16 = 1;

/// Frames in each direction: the queues' entries.
pub const DEPTH: usize = 256;

/// MANA's requests.
mod mana {
    pub const QUERY_DEV_CONFIG: u32 = 0x2_0001;
    pub const CONFIG_VPORT_TX: u32 = 0x2_0003;
    pub const CREATE_WQ_OBJ: u32 = 0x2_0004;
    pub const DESTROY_WQ_OBJ: u32 = 0x2_0005;
    pub const FENCE_RQ: u32 = 0x2_0006;
    pub const CONFIG_VPORT_RX: u32 = 0x2_0007;
    pub const QUERY_VPORT_CONFIG: u32 = 0x2_0008;
    /// The protocol's version this driver speaks: 0.1.1.
    pub const VERSION: (u32, u32, u32) = (0, 1, 1);
}

/// What a completion says, in the low six bits of its data.
mod cqe {
    pub const RX_OKAY: u32 = 1;
    pub const RX_FENCE: u32 = 3;
    pub const RX_TRUNCATED: u32 = 4;
}

/// The data queues' memory, in pages: the event queue's 256 entries, the
/// send queue's room for every frame in the long format (two units), the
/// receive queue's one unit a frame, and each completion queue's entry a
/// frame. Each is a power of two, as a DMA region must be.
const EQ_PAGES: usize = 1;
const SQ_PAGES: usize = DEPTH * 2 * UNIT / PAGE;
const RQ_PAGES: usize = DEPTH * UNIT / PAGE;
const CQ_PAGES: usize = DEPTH * CQE / PAGE;
const _: () = assert!(EQ_PAGES * PAGE / EQE == 256 && SQ_PAGES.is_power_of_two());
const _: () = assert!(RQ_PAGES.is_power_of_two() && CQ_PAGES.is_power_of_two());

/// How often the event queue may raise its interrupt: once in 2^3
/// events at most, as FreeBSD asks.
const LOG2_THROTTLE: u32 = 3;
/// How long the fence on the receive queue may take, and how long sent
/// frames are waited for as the card stops, in microseconds.
const FENCE_US: u32 = 10_000_000;
const DRAIN_US: u32 = 1_000_000;
const STEP_US: u32 = 10;
/// The largest vPort offset the short send format holds.
const SHORT_OFFSET_MAX: u16 = 0xff;
/// The receive steering's indirection table: one receive queue in every
/// entry.
const INDIRECTION: usize = 64;
/// A key for the receive hash, which the card asks for though one queue
/// takes everything: the example key of Microsoft's RSS documentation.
const HASH_KEY: [u8; 40] = [
    0x6d, 0x5a, 0x56, 0xda, 0x25, 0x5b, 0x0e, 0xc2, 0x41, 0x67, 0x25, 0x3d, 0x43, 0xa3, 0x8f, 0xb0,
    0xd0, 0xca, 0x2b, 0xcb, 0xae, 0x7b, 0x30, 0xb4, 0x77, 0xcb, 0x2d, 0xa3, 0x80, 0x30, 0xf2, 0x0c,
    0x6a, 0x42, 0xb7, 0x3b, 0xbe, 0xac, 0x01, 0xfa,
];
/// The request that sets the steering: its fixed part, then the table.
const STEERING: usize = 112;

/// The memory a card needs, taken before it is touched.
struct Memory {
    eq: Dma,
    sq: Dma,
    tx_cq: Dma,
    rq: Dma,
    rx_cq: Dma,
    tx_slots: Slots,
    rx_slots: Slots,
}

impl Memory {
    fn new(alloc: &mut dyn FnMut(usize) -> Option<Dma>) -> Option<Memory> {
        Some(Memory {
            eq: alloc(EQ_PAGES)?,
            sq: alloc(SQ_PAGES)?,
            tx_cq: alloc(CQ_PAGES)?,
            rq: alloc(RQ_PAGES)?,
            rx_cq: alloc(CQ_PAGES)?,
            tx_slots: Slots::new(DEPTH, alloc)?,
            rx_slots: Slots::new(DEPTH, alloc)?,
        })
    }
}

/// What the next receive completion was.
enum Received {
    /// A frame, this long, copied out.
    Frame(usize),
    /// A frame dropped: damaged, cut short, or too short.
    Dropped,
    /// The fence on the receive queue.
    Fence,
    /// Something else, which uses no buffer.
    Other,
}

/// A MANA card's port, set up and running.
pub struct Mana {
    channel: Channel,
    eq: Queue,
    sq: Queue,
    tx_cq: Queue,
    rq: Queue,
    rx_cq: Queue,
    tx_slots: Slots,
    rx_slots: Slots,
    /// The MANA device's ID, its doorbell page, its protection domain and
    /// its memory key, which LIST_DEVICES and REGISTER_DEVICE give.
    device: u32,
    doorbell: u32,
    pdid: u32,
    key: u32,
    /// The vPort, its offset on the card, and the units a send takes:
    /// one in the short format, two in the long, which a vPort offset
    /// past 255 needs.
    vport: u64,
    vport_offset: u16,
    tx_units: u32,
    /// What was set up, to be taken down.
    registered: bool,
    eq_created: bool,
    tx_object: Option<u64>,
    rx_object: Option<u64>,
    steering: bool,
    /// Frames handed to the card and frames it is done with, counted.
    tx_head: usize,
    tx_tail: usize,
    /// The receive slot the next frame comes into.
    rx_next: usize,
    /// Doorbells owed: the queues filled, the completions taken.
    ring_tx: bool,
    ring_rx: bool,
    ring_tx_cq: bool,
    ring_rx_cq: bool,
    info: Info,
    interrupt: Interrupt,
    fatal: bool,
    dropped: u64,
    stopped: bool,
    /// Whether it was taken down, and whether that went cleanly.
    closed: bool,
    clean: bool,
    /// Whether a request timed out, so that the channel is not used again.
    broken: bool,
}

/// Sets up the hardware channel of the card whose BAR 0 is `regs`, finds
/// its MANA device and sets its port running, with interrupts as
/// `interrupt` says (on MSI-X entry [`QUEUE_VECTOR`]). `alloc` gives
/// zeroed DMA memory in pages, and `delay` waits the given number of
/// microseconds. A card that fails part way is taken down again.
pub fn start(
    regs: Mmio,
    interrupt: Interrupt,
    alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    delay: &mut dyn FnMut(u32),
) -> Result<Mana, Error> {
    let card = Card::new(regs)?;
    // All the memory first, so that a card that fails to start is taken
    // down before any of it is given back.
    let pages = Pages::new(alloc).ok_or(Error::NoMemory)?;
    let memory = Memory::new(alloc).ok_or(Error::NoMemory)?;
    let channel = Channel::open(card, pages, CHANNEL_VECTOR, delay)?;
    let mut mana = Mana {
        channel,
        eq: Queue::new(memory.eq, EQE),
        sq: Queue::new(memory.sq, 0),
        tx_cq: Queue::new(memory.tx_cq, CQE),
        rq: Queue::new(memory.rq, 0),
        rx_cq: Queue::new(memory.rx_cq, CQE),
        tx_slots: memory.tx_slots,
        rx_slots: memory.rx_slots,
        device: 0,
        doorbell: 0,
        pdid: 0,
        key: 0,
        vport: 0,
        vport_offset: 0,
        tx_units: 1,
        registered: false,
        eq_created: false,
        tx_object: None,
        rx_object: None,
        steering: false,
        tx_head: 0,
        tx_tail: 0,
        rx_next: 0,
        ring_tx: false,
        ring_rx: false,
        ring_tx_cq: false,
        ring_rx_cq: false,
        info: Info {
            model: "MANA",
            mac: [0; 6],
            receive_slots: DEPTH as u16,
            transmit_slots: DEPTH as u16,
        },
        interrupt,
        fatal: false,
        dropped: 0,
        stopped: false,
        closed: false,
        clean: false,
        broken: false,
    };
    match mana.set_up(delay) {
        Ok(()) => Ok(mana),
        Err(error) => {
            mana.close(delay);
            Err(error)
        }
    }
}

impl Mana {
    /// Sends a request on the hardware channel, as [`Channel::call`]
    /// does; a request that times out leaves the channel unused after.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        what: &'static str,
        code: u32,
        device: u32,
        request_len: usize,
        response_len: usize,
        fill: impl FnOnce(&Dma),
        delay: &mut dyn FnMut(u32),
    ) -> Result<&Dma, Error> {
        if self.broken {
            return Err(Error::Device("the hardware channel stopped answering"));
        }
        let result = self.channel.call(what, code, device, request_len, response_len, fill, delay);
        if matches!(result, Err(Error::Timeout(_))) {
            self.broken = true;
        }
        result.map(|()| self.channel.answer())
    }

    /// The card's steps from the hardware channel to a running port, as
    /// FreeBSD's `mana_gd_attach`, `mana_probe` and `mana_alloc_queues`
    /// take them.
    fn set_up(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        self.verify_version(delay)?;
        self.query_resources(delay)?;
        self.find_device(delay)?;
        self.register(delay)?;
        self.create_eq(delay)?;
        self.query_config(delay)?;
        self.query_vport(delay)?;
        self.config_tx(delay)?;
        self.create_queues(delay)?;
        // Every receive slot goes to the card before receiving starts.
        for slot in 0..DEPTH {
            let address = self.rx_slots.phys(slot);
            self.rq.post(&[], address, self.key, SLOT as u32);
        }
        self.channel.card.ring_wq(self.doorbell, kind::RQ, &self.rq);
        self.steer(true, delay)?;
        self.steering = true;
        self.fence(delay)?;
        if self.interrupt == Interrupt::Msi {
            self.rearm()?;
        }
        Ok(())
    }

    /// Says which GDMA protocol this driver speaks (version 1), and what
    /// it is.
    fn verify_version(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        self.call(
            "checking the protocol's version",
            request::VERIFY_VF_DRIVER_VERSION,
            0,
            640,
            72,
            |request| {
                request.write64(40, 1);
                request.write64(48, 1);
                // Another OS than Linux or Windows.
                request.write32(96, 0x30);
                request.write(128, b"Dukovo");
                request.write(256, env!("CARGO_PKG_VERSION").as_bytes());
            },
            delay,
        )?;
        Ok(())
    }

    /// Checks that the function has the queues and interrupts a port
    /// needs.
    fn query_resources(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let answer = self.call(
            "querying the card's resources",
            request::QUERY_MAX_RESOURCES,
            0,
            REQUEST_HEADER,
            72,
            |_| {},
            delay,
        )?;
        // Its largest numbers of send, receive, completion and event
        // queues, and of MSI-X entries.
        let counts = [36, 40, 44, 48].map(|at| answer.read32(at));
        if counts.contains(&0) || answer.read32(68) < 2 {
            return Err(Error::Unsupported("a card with too few queues or interrupts"));
        }
        Ok(())
    }

    /// Finds the function's MANA device among its devices.
    fn find_device(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let answer = self.call(
            "listing the card's devices",
            request::LIST_DEVICES,
            0,
            REQUEST_HEADER,
            RESPONSE_HEADER + 8 + 64 * 4,
            |_| {},
            delay,
        )?;
        // FreeBSD looks at the first four.
        let count = answer.read32(32).min(4) as usize;
        let found =
            (0..count).map(|i| answer.read32(40 + i * 4)).find(|id| id & 0xffff == device::MANA);
        self.device = found.ok_or(Error::Device("it has no MANA device"))?;
        Ok(())
    }

    /// Registers the MANA device: its protection domain, memory key and
    /// doorbell page.
    fn register(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let device = self.device;
        let answer = self.call(
            "registering the MANA device",
            request::REGISTER_DEVICE,
            device,
            REQUEST_HEADER,
            RESPONSE_HEADER + 12,
            |_| {},
            delay,
        )?;
        let (pdid, key, doorbell) = (answer.read32(32), answer.read32(36), answer.read32(40));
        self.registered = true;
        self.pdid = pdid;
        self.key = key;
        self.doorbell = doorbell;
        if !self.channel.card.has_doorbell_page(doorbell) {
            return Err(Error::Registers);
        }
        Ok(())
    }

    /// Tells the card of `memory`, a power of two of pages: the DMA
    /// region's handle, which a queue made on it takes over.
    fn create_region(
        &mut self,
        memory: (u64, usize),
        delay: &mut dyn FnMut(u32),
    ) -> Result<u64, Error> {
        let (address, len) = memory;
        let pages = len / PAGE;
        let answer = self.call(
            "giving the card a queue's memory",
            request::CREATE_DMA_REGION,
            0,
            64 + pages * 8,
            RESPONSE_HEADER + 8,
            |request| {
                request.write64(40, len as u64);
                // Offset 0 in its first page, of 4 KiB pages.
                request.write32(56, pages as u32);
                request.write32(60, pages as u32);
                for page in 0..pages {
                    request.write64(64 + page * 8, address + (page * PAGE) as u64);
                }
            },
            delay,
        )?;
        match answer.read64(32) {
            0 => Err(Error::Device("the card gave no DMA region")),
            handle => Ok(handle),
        }
    }

    /// Takes back a DMA region no queue took.
    fn destroy_region(&mut self, handle: u64, delay: &mut dyn FnMut(u32)) -> bool {
        self.call(
            "taking back a queue's memory",
            request::DESTROY_DMA_REGION,
            0,
            REQUEST_HEADER + 8,
            RESPONSE_HEADER,
            |request| request.write64(40, handle),
            delay,
        )
        .is_ok()
    }

    /// Creates the event queue the port's completions raise their events
    /// on, with its interrupt on [`QUEUE_VECTOR`], and tests it.
    fn create_eq(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let memory = (self.eq.memory.phys(), self.eq.memory.len());
        let region = self.create_region(memory, delay)?;
        let (device, pdid, doorbell) = (self.device, self.pdid, self.doorbell);
        let created = self.call(
            "creating the event queue",
            request::CREATE_QUEUE,
            device,
            96,
            RESPONSE_HEADER + 4,
            |request| {
                request.write32(40, kind::EQ);
                request.write32(48, pdid);
                request.write32(52, doorbell);
                request.write64(56, region);
                request.write32(68, memory.1 as u32);
                request.write32(72, LOG2_THROTTLE);
                request.write32(76, u32::from(QUEUE_VECTOR));
            },
            delay,
        );
        let id = match created {
            Ok(answer) => answer.read32(32),
            Err(error) => {
                self.destroy_region(region, delay);
                return Err(error);
            }
        };
        self.eq.id = id;
        self.eq_created = true;
        self.call(
            "testing the event queue",
            request::GENERATE_TEST_EQE,
            device,
            REQUEST_HEADER + 4,
            RESPONSE_HEADER,
            |request| request.write32(40, id),
            delay,
        )?;
        let mut failed = None;
        let mut tested = false;
        let done = wait_for(delay, FENCE_US, STEP_US, || {
            loop {
                match self.eq.next_event() {
                    Ok(Some((event::TEST, _))) => tested = true,
                    Ok(Some(_)) => {}
                    Ok(None) => return tested,
                    Err(error) => {
                        failed = Some(error);
                        return true;
                    }
                }
            }
        });
        if let Some(error) = failed {
            return Err(error);
        }
        if !done {
            return Err(Error::Timeout("the event queue's test event"));
        }
        self.channel.card.ring_eq(self.doorbell, &self.eq, false);
        Ok(())
    }

    /// Checks that the card has a port and speaks this driver's version.
    fn query_config(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let device = self.device;
        let answer = self.call(
            "querying the card's configuration",
            mana::QUERY_DEV_CONFIG,
            device,
            88,
            80,
            |request| {
                // Version 2 of the answer, which carries the MTU.
                request.write16(24, 2);
                let (major, minor, micro) = mana::VERSION;
                request.write32(72, major);
                request.write32(76, minor);
                request.write32(80, micro);
            },
            delay,
        )?;
        if answer.read16(64) == 0 {
            return Err(Error::Device("it has no ports"));
        }
        Ok(())
    }

    /// Reads port 0's vPort: its handle, queues and MAC address.
    fn query_vport(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let device = self.device;
        let answer = self.call(
            "querying the port",
            mana::QUERY_VPORT_CONFIG,
            device,
            REQUEST_HEADER + 4,
            64,
            |_| {},
            delay,
        )?;
        if answer.read32(32) == 0 || answer.read32(36) == 0 {
            return Err(Error::Device("its port has no queues"));
        }
        let mut mac = [0u8; 6];
        answer.read(48, &mut mac);
        let vport = answer.read64(56);
        if !usable_mac(mac) {
            return Err(Error::Device("its port has no MAC address"));
        }
        self.info.mac = mac;
        self.vport = vport;
        Ok(())
    }

    /// Sets the vPort up for sending, with this device's protection domain
    /// and doorbell page: its offset, which every send names.
    fn config_tx(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let (device, vport, pdid, doorbell) = (self.device, self.vport, self.pdid, self.doorbell);
        let answer = self.call(
            "setting the port up for sending",
            mana::CONFIG_VPORT_TX,
            device,
            56,
            RESPONSE_HEADER + 4,
            |request| {
                request.write64(40, vport);
                request.write32(48, pdid);
                request.write32(52, doorbell);
            },
            delay,
        )?;
        self.vport_offset = answer.read16(32);
        self.tx_units = if self.vport_offset > SHORT_OFFSET_MAX { 2 } else { 1 };
        Ok(())
    }

    /// Creates the send queue and the receive queue, each with its
    /// completion queue, on the event queue.
    fn create_queues(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let (sq, cq, object) = self.create_wq(kind::SQ, delay)?;
        self.sq.id = sq;
        self.tx_cq.id = cq;
        self.tx_object = Some(object);
        let (rq, cq, object) = self.create_wq(kind::RQ, delay)?;
        self.rq.id = rq;
        self.rx_cq.id = cq;
        self.rx_object = Some(object);
        Ok(())
    }

    /// Creates the send or receive queue, as `kind` says, with its
    /// completion queue: their numbers, and the work queue object's
    /// handle.
    fn create_wq(
        &mut self,
        kind: u32,
        delay: &mut dyn FnMut(u32),
    ) -> Result<(u32, u32, u64), Error> {
        let (wq, cq) = if kind == self::kind::SQ {
            (&self.sq.memory, &self.tx_cq.memory)
        } else {
            (&self.rq.memory, &self.rx_cq.memory)
        };
        let (wq, cq) = ((wq.phys(), wq.len()), (cq.phys(), cq.len()));
        let wq_region = self.create_region(wq, delay)?;
        let cq_region = match self.create_region(cq, delay) {
            Ok(region) => region,
            Err(error) => {
                self.destroy_region(wq_region, delay);
                return Err(error);
            }
        };
        let (device, vport, eq) = (self.device, self.vport, self.eq.id);
        let created = self.call(
            "creating a queue",
            mana::CREATE_WQ_OBJ,
            device,
            88,
            48,
            |request| {
                request.write64(40, vport);
                request.write32(48, kind);
                request.write64(56, wq_region);
                request.write64(64, cq_region);
                request.write32(72, wq.1 as u32);
                request.write32(76, cq.1 as u32);
                request.write32(84, eq);
            },
            delay,
        );
        let (wq_id, cq_id, object) = match created {
            Ok(answer) => (answer.read32(32), answer.read32(36), answer.read64(40)),
            Err(error) => {
                self.destroy_region(wq_region, delay);
                self.destroy_region(cq_region, delay);
                return Err(error);
            }
        };
        if object == u64::MAX {
            return Err(Error::Device("the card gave no queue"));
        }
        if cq_id >= self.channel.max_cqs {
            self.destroy_wq(kind, object, delay);
            return Err(Error::Device("the card numbered a completion queue past its own"));
        }
        Ok((wq_id, cq_id, object))
    }

    /// Destroys a send or receive queue's object, and with it its queues.
    fn destroy_wq(&mut self, kind: u32, object: u64, delay: &mut dyn FnMut(u32)) -> bool {
        let device = self.device;
        self.call(
            "destroying a queue",
            mana::DESTROY_WQ_OBJ,
            device,
            56,
            RESPONSE_HEADER,
            |request| {
                request.write32(40, kind);
                request.write64(48, object);
            },
            delay,
        )
        .is_ok()
    }

    /// Steers the vPort's frames to the receive queue, or stops receiving,
    /// as `on` says.
    fn steer(&mut self, on: bool, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let Some(object) = self.rx_object else {
            return Err(Error::Device("no receive queue to steer to"));
        };
        let (device, vport) = (self.device, self.vport);
        self.call(
            "steering the port's frames",
            mana::CONFIG_VPORT_RX,
            device,
            STEERING + INDIRECTION * 8,
            RESPONSE_HEADER,
            |request| {
                request.write64(40, vport);
                request.write16(48, INDIRECTION as u16);
                request.write16(50, STEERING as u16);
                request.write32(52, u32::from(on));
                // No hashing across queues: there is one. The default
                // queue is always set, the key and table when turning on.
                request.write8(60, 1);
                request.write8(61, u8::from(on));
                request.write8(62, u8::from(on));
                request.write64(64, object);
                if on {
                    request.write(72, &HASH_KEY);
                    for entry in 0..INDIRECTION {
                        request.write64(STEERING + entry * 8, object);
                    }
                }
            },
            delay,
        )?;
        Ok(())
    }

    /// Fences the receive queue and waits for the fence to come through
    /// it, so that the steering took; frames that come first are dropped,
    /// as nothing takes them yet (or any longer).
    fn fence(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let Some(object) = self.rx_object else {
            return Ok(());
        };
        let device = self.device;
        self.call(
            "fencing the receive queue",
            mana::FENCE_RQ,
            device,
            REQUEST_HEADER + 8,
            RESPONSE_HEADER,
            |request| request.write64(40, object),
            delay,
        )?;
        let mut failed = None;
        let fenced = wait_for(delay, FENCE_US, STEP_US, || {
            loop {
                match self.next_received(&mut []) {
                    Ok(Some(Received::Fence)) => return true,
                    Ok(Some(_)) => {}
                    Ok(None) => return false,
                    Err(error) => {
                        failed = Some(error);
                        return true;
                    }
                }
            }
        });
        self.flush_rings();
        match failed {
            Some(error) => Err(error),
            None if !fenced => Err(Error::Timeout("the receive queue's fence")),
            None => Ok(()),
        }
    }

    /// Takes the next receive completion, if there is one, copying a
    /// frame into `frame` (cut to its length) and giving its slot back.
    fn next_received(&mut self, frame: &mut [u8]) -> Result<Option<Received>, Error> {
        let Some(completion) = self.rx_cq.next_completion()? else {
            return Ok(None);
        };
        self.ring_rx_cq = true;
        if completion.send || completion.queue != self.rq.id {
            return Err(Error::Device("a receive completion named another queue"));
        }
        let data = completion.at;
        let received = match self.rx_cq.memory.read32(data) & 0x3f {
            cqe::RX_OKAY => {
                // The first frame's length, in the first per-packet entry.
                let len = (self.rx_cq.memory.read32(data + 8) & 0xffff) as usize;
                if (MIN_FRAME..=SLOT).contains(&len) {
                    let len = len.min(frame.len());
                    self.rx_slots.read(self.rx_next, &mut frame[..len]);
                    Received::Frame(len)
                } else {
                    Received::Dropped
                }
            }
            cqe::RX_TRUNCATED => Received::Dropped,
            cqe::RX_FENCE => return Ok(Some(Received::Fence)),
            _ => return Ok(Some(Received::Other)),
        };
        if matches!(received, Received::Dropped) {
            self.dropped += 1;
        }
        // The slot goes back to the card, at the receive queue's head,
        // which is where it was: the queue holds a unit for each slot.
        let slot = self.rx_next;
        self.rq.tail = self.rq.tail.wrapping_add(1);
        self.rq.post(&[], self.rx_slots.phys(slot), self.key, SLOT as u32);
        self.rx_next = (slot + 1) % DEPTH;
        self.ring_rx = true;
        Ok(Some(received))
    }

    /// Takes back the slots of frames the card sent.
    fn reclaim(&mut self) -> Result<(), Error> {
        while let Some(completion) = self.tx_cq.next_completion()? {
            self.ring_tx_cq = true;
            if !completion.send || completion.queue != self.sq.id || self.tx_tail == self.tx_head {
                return Err(Error::Device("a send completion named another queue"));
            }
            // A frame the card refused is lost, as on a wire.
            self.tx_tail += 1;
            self.sq.tail = self.sq.tail.wrapping_add(self.tx_units);
        }
        Ok(())
    }

    /// Takes the event queue's events, which only say completions came,
    /// and asks for an interrupt at the next completion.
    fn rearm(&mut self) -> Result<(), Error> {
        while self.eq.next_event()?.is_some() {}
        let card = &self.channel.card;
        card.ring_cq(self.doorbell, &self.rx_cq, true);
        card.ring_cq(self.doorbell, &self.tx_cq, true);
        card.ring_eq(self.doorbell, &self.eq, true);
        self.ring_rx_cq = false;
        self.ring_tx_cq = false;
        Ok(())
    }

    /// Rings the doorbells owed.
    fn flush_rings(&mut self) {
        let card = &self.channel.card;
        if core::mem::take(&mut self.ring_tx) {
            card.ring_wq(self.doorbell, kind::SQ, &self.sq);
        }
        if core::mem::take(&mut self.ring_rx) {
            card.ring_wq(self.doorbell, kind::RQ, &self.rq);
        }
        if core::mem::take(&mut self.ring_tx_cq) {
            card.ring_cq(self.doorbell, &self.tx_cq, false);
        }
        if core::mem::take(&mut self.ring_rx_cq) {
            card.ring_cq(self.doorbell, &self.rx_cq, false);
        }
    }

    /// Stops a card that failed: nothing more is sent or received until
    /// it is taken down.
    fn fail(&mut self) {
        self.fatal = true;
    }

    /// Takes the port and the card down, as FreeBSD's `mana_detach`,
    /// `mana_remove` and `mana_gd_detach` do: steering off and fenced,
    /// sends waited for, the queues destroyed, the event queue disabled,
    /// the device deregistered and the hardware channel taken down. Each
    /// step is tried even when one before failed. Whether all went well.
    fn close(&mut self, delay: &mut dyn FnMut(u32)) -> bool {
        if self.closed {
            return self.clean;
        }
        self.closed = true;
        self.stopped = true;
        let mut clean = true;
        if core::mem::take(&mut self.steering) {
            clean &= self.steer(false, delay).is_ok() && self.fence(delay).is_ok();
        }
        if self.tx_object.is_some() && !self.fatal {
            let drained = wait_for(delay, DRAIN_US, STEP_US, || {
                self.reclaim().is_err() || self.tx_tail == self.tx_head
            });
            clean &= drained;
        }
        if let Some(object) = self.rx_object.take() {
            clean &= self.destroy_wq(kind::RQ, object, delay);
        }
        if let Some(object) = self.tx_object.take() {
            clean &= self.destroy_wq(kind::SQ, object, delay);
        }
        let (device, id) = (self.device, self.eq.id);
        if core::mem::take(&mut self.eq_created) {
            let disabled = self.call(
                "disabling the event queue",
                request::DISABLE_QUEUE,
                device,
                REQUEST_HEADER + 12,
                RESPONSE_HEADER,
                |request| {
                    request.write32(40, kind::EQ);
                    request.write32(44, id);
                    request.write32(48, 1);
                },
                delay,
            );
            clean &= disabled.is_ok();
        }
        if core::mem::take(&mut self.registered) {
            let deregistered = self.call(
                "deregistering the MANA device",
                request::DEREGISTER_DEVICE,
                device,
                REQUEST_HEADER,
                RESPONSE_HEADER,
                |_| {},
                delay,
            );
            clean &= deregistered.is_ok();
        }
        clean &= self.channel.close(delay);
        self.clean = clean;
        clean
    }

    /// Takes the card down, waiting with `delay`, which may sleep (where
    /// [`Nic::stop`] may not): for a card the host is taking away. A card
    /// that did not stop cleanly may still reach its memory, which is
    /// then never given back. Whether it stopped cleanly.
    pub fn shut_down(mut self, delay: &mut dyn FnMut(u32)) -> bool {
        let clean = self.close(delay);
        if !clean {
            core::mem::forget(self);
        }
        clean
    }
}

impl Nic for Mana {
    fn info(&self) -> Info {
        self.info
    }

    /// Up while it runs: the card says nothing of its link, which is the
    /// host's.
    fn link(&self) -> Link {
        if self.stopped || self.fatal {
            Link::default()
        } else {
            Link { up: true, mbps: 0, full_duplex: true }
        }
    }

    fn failed(&self) -> bool {
        self.fatal
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        if self.stopped || self.fatal {
            return None;
        }
        for pass in 0..2 {
            loop {
                match self.next_received(frame) {
                    Ok(Some(Received::Frame(len))) => return Some(len),
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        self.fail();
                        return None;
                    }
                }
            }
            if pass == 1 || self.interrupt != Interrupt::Msi {
                break;
            }
            // Nothing more: the interrupt again, and a last look for a
            // frame that came in between.
            if self.rearm().is_err() {
                self.fail();
                return None;
            }
            if !self.rx_cq.completion_waits() {
                break;
            }
        }
        None
    }

    fn can_send(&mut self) -> bool {
        if self.stopped || self.fatal {
            return false;
        }
        if self.reclaim().is_err() {
            self.fail();
            return false;
        }
        self.tx_head - self.tx_tail < DEPTH
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if !(MIN_FRAME..=MAX_FRAME).contains(&frame.len()) || !self.can_send() {
            return false;
        }
        let slot = self.tx_head % DEPTH;
        self.tx_slots.write(slot, frame);
        // The outer IP version, which the card wants though it computes
        // no checksum: IPv4 is 0x0800, IPv6 0x86dd.
        let outer = match u16::from_be_bytes([frame[12], frame[13]]) {
            0x0800 => 1 << 2,
            0x86dd => 1 << 3,
            _ => 0,
        };
        let completion_queue = (self.tx_cq.id & 0xff_ffff) << 8;
        let offset = u32::from(self.vport_offset);
        let address = self.tx_slots.phys(slot);
        let len = frame.len() as u32;
        if self.tx_units == 1 {
            let oob = [outer | completion_queue, (offset & 0xff) << 24];
            self.sq.post(&oob, address, self.key, len);
        } else {
            let oob = [1 | outer | completion_queue, 0, 0, (offset & 0xfff) << 16, 0, 0];
            self.sq.post(&oob, address, self.key, len);
        }
        self.tx_head += 1;
        self.ring_tx = true;
        true
    }

    fn flush(&mut self) {
        if !self.stopped && !self.fatal {
            self.flush_rings();
        }
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Takes the card down, waiting without sleeping: the hardware
    /// channel's answers come in microseconds.
    fn stop(&mut self) {
        let mut spin = |us: u32| {
            for _ in 0..us.max(1) {
                core::hint::spin_loop();
            }
        };
        self.close(&mut spin);
    }
}

impl Drop for Mana {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
