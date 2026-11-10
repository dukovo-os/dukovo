// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! GDMA, the layer under MANA: the function's few registers, its queues
//! in memory, and the hardware channel (HWC) that every request to the
//! card goes through.
//!
//! The registers say where three things are in BAR 0: the doorbell pages,
//! one for each device registered on the function, and the shared memory
//! the driver and the host's own driver (the PF's) take turns at. The
//! shared memory carries one message each way, 256 bits: the driver asks
//! the host to set up the hardware channel, naming four pages of its
//! memory, and the host answers there and then by events in the first of
//! them. Everything else is a request sent on the channel's send queue,
//! whose answer comes into a buffer posted on its receive queue.
//!
//! Queues are rings in DMA memory. Event and completion queues are
//! written by the card, each entry with three owner bits that count how
//! many times the card has gone round, so that the driver can tell a new
//! entry from an old one; work queues (send and receive) are written by
//! the driver, in 32-byte basic units. A doorbell tells the card how far
//! the driver has gone: the work queues' heads, the event and completion
//! queues' heads with a bit that asks for an event, or an interrupt, at
//! the next entry.

use radix_hw::{Dma, Mmio, device_barrier};
use radix_nic::{Error, PAGE, wait_for};

/// The registers, in BAR 0: the doorbell pages' offset and size, and the
/// shared memory's offset.
mod reg {
    pub const DB_PAGE_OFFSET: usize = 0x08;
    pub const DB_PAGE_SIZE: usize = 0x10;
    pub const SHM_OFFSET: usize = 0x18;
    pub const END: usize = SHM_OFFSET + 8;
}

/// Each queue type's doorbell, within a doorbell page.
mod doorbell {
    pub const SQ: usize = 0x000;
    pub const RQ: usize = 0x400;
    pub const CQ: usize = 0x800;
    pub const EQ: usize = 0xff8;
}

/// The shared memory's message: 256 bits, the protocol's header in the
/// last 32.
mod smc {
    pub const DWORDS: usize = 8;
    pub const LAST: usize = (DWORDS - 1) * 4;
    pub const ESTABLISH: u32 = 1;
    pub const DESTROY: u32 = 2;
    /// The header's direction bit: set in an answer.
    pub const RESPONSE: u32 = 1 << 7;
    /// Set while the host's driver has the shared memory.
    pub const PF_OWNS: u32 = 1 << 31;
    /// How long the host may take to answer, in microseconds.
    pub const TIMEOUT_US: u32 = 20_000_000;
}

/// Queue types, as requests name them.
pub mod kind {
    pub const SQ: u32 = 1;
    pub const RQ: u32 = 2;
    pub const EQ: u32 = 4;
}

/// Event types.
pub mod event {
    pub const TEST: u8 = 64;
    pub const HWC_EQ_ID_DB: u8 = 129;
    pub const HWC_DATA: u8 = 130;
    pub const HWC_DONE: u8 = 131;
}

/// What the HWC_DATA events carry, by their type.
mod data {
    pub const CQ_ID: u32 = 1;
    pub const RQ_ID: u32 = 2;
    pub const SQ_ID: u32 = 3;
    pub const MAX_REQUEST: u32 = 5;
    pub const MAX_CQS: u32 = 7;
    pub const GPA_MKEY: u32 = 9;
}

/// Requests to the GDMA layer itself.
pub mod request {
    pub const VERIFY_VF_DRIVER_VERSION: u32 = 1;
    pub const QUERY_MAX_RESOURCES: u32 = 2;
    pub const LIST_DEVICES: u32 = 3;
    pub const REGISTER_DEVICE: u32 = 4;
    pub const DEREGISTER_DEVICE: u32 = 5;
    pub const GENERATE_TEST_EQE: u32 = 10;
    pub const CREATE_QUEUE: u32 = 12;
    pub const DISABLE_QUEUE: u32 = 13;
    pub const CREATE_DMA_REGION: u32 = 25;
    pub const DESTROY_DMA_REGION: u32 = 27;
}

/// The device types LIST_DEVICES gives, in the low half of a device ID.
pub mod device {
    /// The hardware channel, instance 0: the ID requests to it carry.
    pub const HWC: u32 = 1;
    pub const MANA: u32 = 2;
}

/// Each event queue entry's bytes, and each completion queue entry's.
pub const EQE: usize = 16;
pub const CQE: usize = 64;
/// A work queue's basic unit, in bytes.
pub const UNIT: usize = 32;
/// The bytes of a request's header, and of an answer's.
pub const REQUEST_HEADER: usize = 40;
pub const RESPONSE_HEADER: usize = 32;
/// A request's and an answer's largest size on the channel: a page each.
const MESSAGE: usize = PAGE;
/// How long the channel may take to start, and to answer, in
/// microseconds.
const START_US: u32 = 60_000_000;
const ANSWER_US: u32 = 30_000_000;
/// How often the channel is looked at while waiting, in microseconds.
const STEP_US: u32 = 10;

/// The function's registers and doorbells.
pub struct Card {
    pub regs: Mmio,
    doorbells: usize,
    doorbell_size: usize,
    shm: usize,
}

impl Card {
    /// The card whose BAR 0 is `regs`, once its registers say where its
    /// doorbells and shared memory are, inside the BAR.
    pub fn new(regs: Mmio) -> Result<Card, Error> {
        if regs.len() < reg::END {
            return Err(Error::Registers);
        }
        let doorbells = usize::try_from(regs.read64(reg::DB_PAGE_OFFSET)).unwrap_or(usize::MAX);
        let doorbell_size = (regs.read32(reg::DB_PAGE_SIZE) & 0xffff) as usize;
        let shm = usize::try_from(regs.read64(reg::SHM_OFFSET)).unwrap_or(usize::MAX);
        if doorbell_size < doorbell::EQ + 8 || !shm.is_multiple_of(4) {
            return Err(Error::Device("its registers name no doorbells or shared memory"));
        }
        if shm.checked_add(smc::DWORDS * 4).is_none_or(|end| end > regs.len()) {
            return Err(Error::Registers);
        }
        let card = Card { regs, doorbells, doorbell_size, shm };
        if !card.has_doorbell_page(0) {
            return Err(Error::Registers);
        }
        Ok(card)
    }

    /// Whether doorbell page `page` is inside the BAR.
    pub fn has_doorbell_page(&self, page: u32) -> bool {
        (page as usize)
            .checked_add(1)
            .and_then(|pages| pages.checked_mul(self.doorbell_size))
            .and_then(|end| end.checked_add(self.doorbells))
            .is_some_and(|end| end <= self.regs.len())
    }

    /// Rings the doorbell at `offset` in page `page` with `value`, after
    /// the writes to memory the card is told of.
    fn ring(&self, page: u32, offset: usize, value: u64) {
        device_barrier();
        self.regs.write64(self.doorbells + page as usize * self.doorbell_size + offset, value);
    }

    /// Tells the card that work queue `queue`, of type `kind`, was filled
    /// up to its head.
    pub fn ring_wq(&self, page: u32, kind: u32, queue: &Queue) {
        let tail = u64::from(queue.head.wrapping_mul(UNIT as u32));
        let id = u64::from(queue.id & 0xff_ffff);
        let offset = if kind == self::kind::RQ { doorbell::RQ } else { doorbell::SQ };
        self.ring(page, offset, id | tail << 32);
    }

    /// Tells the card how far completion queue `queue` was taken, and
    /// whether to raise an event at its next entry.
    pub fn ring_cq(&self, page: u32, queue: &Queue, arm: bool) {
        let tail = u64::from(queue.head % (queue.entries(CQE) << 3)) & 0x7fff_ffff;
        let id = u64::from(queue.id & 0xff_ffff);
        self.ring(page, doorbell::CQ, id | tail << 32 | u64::from(arm) << 63);
    }

    /// Tells the card how far event queue `queue` was taken, and whether
    /// to raise its interrupt at its next entry.
    pub fn ring_eq(&self, page: u32, queue: &Queue, arm: bool) {
        let tail = u64::from(queue.head % (queue.entries(EQE) << 3)) & 0x7fff_ffff;
        let id = u64::from(queue.id & 0xffff);
        self.ring(page, doorbell::EQ, id | tail << 32 | u64::from(arm) << 63);
    }

    /// Waits until the host's driver leaves the shared memory to this
    /// one: whether it did.
    fn wait_smc(&self, delay: &mut dyn FnMut(u32)) -> bool {
        let last = self.shm + smc::LAST;
        wait_for(delay, smc::TIMEOUT_US, 1000, || self.regs.read32(last) & smc::PF_OWNS == 0)
    }

    /// Sends the shared memory message `message`, of type `kind` (in its
    /// header), and waits for the host's answer.
    fn smc(&self, kind: u32, message: [u8; 32], delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        if !self.wait_smc(delay) {
            return Err(Error::Timeout("the shared memory"));
        }
        // The header, written last, hands the shared memory to the host.
        for (i, word) in message.as_chunks::<4>().0.iter().enumerate() {
            self.regs.write32(self.shm + i * 4, u32::from_le_bytes(*word));
        }
        if !self.wait_smc(delay) {
            return Err(Error::Timeout("the host's answer in the shared memory"));
        }
        let header = self.regs.read32(self.shm + smc::LAST);
        let status = (header >> 8) & 0xff;
        if header & 0x7 != kind || header & smc::RESPONSE == 0 || (header >> 3) & 0x7 != 0 {
            return Err(Error::Device("the host answered something else in the shared memory"));
        }
        if status != 0 {
            return Err(Error::Device("the host refused the hardware channel"));
        }
        Ok(())
    }
}

/// The shared memory message that asks for the hardware channel on the
/// pages of its event, completion, receive and send queues (in that
/// order), with its events on MSI-X entry `vector`: the low 48 bits of
/// each page's frame number, then the high 4 bits of all four, the
/// vector, and the header.
pub fn establish_message(pages: [u64; 4], vector: u16) -> [u8; 32] {
    let mut message = [0u8; 32];
    let mut high = 0u16;
    for (i, &address) in pages.iter().enumerate() {
        let frame = address >> 12;
        message[i * 6..i * 6 + 6].copy_from_slice(&frame.to_le_bytes()[..6]);
        high |= ((frame >> 48) as u16 & 0xf) << (i * 4);
    }
    message[24..26].copy_from_slice(&high.to_le_bytes());
    message[26..28].copy_from_slice(&vector.to_le_bytes());
    message[28..32].copy_from_slice(&smc::ESTABLISH.to_le_bytes());
    message
}

/// A queue in DMA memory, as the driver goes round it.
pub struct Queue {
    pub memory: Dma,
    /// The card's number for it.
    pub id: u32,
    /// An event or completion queue's entries taken, counted from one
    /// pass round, so that the owner bits of the first pass are 1; a work
    /// queue's basic units posted.
    pub head: u32,
    /// A work queue's basic units the card is done with.
    pub tail: u32,
}

/// What a completion queue entry says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    /// The work queue it is for, and whether that is a send queue.
    pub queue: u32,
    pub send: bool,
    /// Where its 60 bytes of data are, in the queue's memory.
    pub at: usize,
}

impl Queue {
    /// A queue on `memory`, with entries of `entry` bytes (0 for a work
    /// queue), not yet numbered.
    pub fn new(memory: Dma, entry: usize) -> Queue {
        let head = memory.len().checked_div(entry).unwrap_or(0) as u32;
        Queue { memory, id: u32::MAX, head, tail: 0 }
    }

    /// How many entries of `entry` bytes it holds.
    pub fn entries(&self, entry: usize) -> u32 {
        (self.memory.len() / entry) as u32
    }

    /// The owner bits the entry at the head has in this pass and had in
    /// the last, for entries of `entry` bytes.
    fn owners(&self, entry: usize) -> (u32, u32) {
        let pass = self.head / self.entries(entry);
        (pass & 0x7, pass.wrapping_sub(1) & 0x7)
    }

    /// Where the entry at the head is, for entries of `entry` bytes, and
    /// its owner bits, from its last word.
    fn at_head(&self, entry: usize) -> (usize, u32) {
        let at = (self.head % self.entries(entry)) as usize * entry;
        (at, self.memory.read32(at + entry - 4) >> 29)
    }

    /// The next event, if the card wrote one: its type and first word.
    pub fn next_event(&mut self) -> Result<Option<(u8, u32)>, Error> {
        let (at, owner) = self.at_head(EQE);
        let (new, old) = self.owners(EQE);
        if owner == old {
            return Ok(None);
        }
        if owner != new {
            return Err(Error::Device("an event queue overflowed"));
        }
        device_barrier();
        let kind = self.memory.read32(at + 12) as u8;
        let detail = self.memory.read32(at);
        self.head = self.head.wrapping_add(1);
        Ok(Some((kind, detail)))
    }

    /// The next completion, if the card wrote one.
    pub fn next_completion(&mut self) -> Result<Option<Completion>, Error> {
        let (at, owner) = self.at_head(CQE);
        let (new, old) = self.owners(CQE);
        if owner == old {
            return Ok(None);
        }
        if owner != new {
            return Err(Error::Device("a completion queue overflowed"));
        }
        device_barrier();
        let info = self.memory.read32(at + 60);
        self.head = self.head.wrapping_add(1);
        Ok(Some(Completion { queue: info & 0xff_ffff, send: info & 1 << 24 != 0, at }))
    }

    /// Whether a completion waits, without taking it.
    pub fn completion_waits(&self) -> bool {
        let (_, owner) = self.at_head(CQE);
        owner != self.owners(CQE).1
    }

    /// Writes a work request at the head: its header, its out-of-band
    /// data `oob` (8 or 24 bytes; 8 zero bytes on a receive queue, when
    /// empty) and one scatter-gather entry, `length` bytes at `address`
    /// under memory key `key`. Returns the basic units it took.
    pub fn post(&mut self, oob: &[u32], address: u64, key: u32, length: u32) -> u32 {
        let oob_words = if oob.is_empty() { 2 } else { oob.len() };
        let bytes = 8 + oob_words * 4 + 16;
        let units = bytes.div_ceil(UNIT) as u32;
        // The header (one entry, and the out-of-band data's length), the
        // out-of-band data, the entry, and zeros to the end of the last
        // unit, which the card ignores: two units at most.
        let mut words = [0u32; 2 * UNIT / 4];
        words[1] = 1 | (oob_words as u32) << 8;
        for (i, word) in words[2..2 + oob_words].iter_mut().enumerate() {
            *word = oob.get(i).copied().unwrap_or(0);
        }
        let sge = 2 + oob_words;
        words[sge] = address as u32;
        words[sge + 1] = (address >> 32) as u32;
        words[sge + 2] = key;
        words[sge + 3] = length;
        let start = self.head as usize * UNIT;
        let len = self.memory.len();
        for (i, &word) in words[..units as usize * UNIT / 4].iter().enumerate() {
            self.memory.write32((start + i * 4) % len, word);
        }
        self.head = self.head.wrapping_add(units);
        units
    }
}

/// The hardware channel: the queues every request goes through, polled,
/// one request at a time.
pub struct Channel {
    pub card: Card,
    eq: Queue,
    cq: Queue,
    rq: Queue,
    sq: Queue,
    /// The page each request is written into, and the page each answer
    /// comes into.
    request: Dma,
    response: Dma,
    /// The channel's doorbell page and memory key, which the host gives.
    doorbell: Option<u32>,
    key: u32,
    /// The largest request the host takes, in bytes.
    max_request: u32,
    /// How many completion queues the function can have: their numbers
    /// are below it.
    pub max_cqs: u32,
    /// Whether the answer buffer is posted on the receive queue.
    posted: bool,
    /// Whether the host said the channel is set up, and whether a test
    /// event came.
    ready: bool,
    tested: bool,
    /// Whether the host set the channel up, so that it is to be taken
    /// down.
    open: bool,
    /// The last MANA request's number, which its answer echoes.
    activity: u32,
}

/// What [`Channel::open`] needs: a page for each of the channel's
/// queues and buffers.
pub struct Pages {
    pub eq: Dma,
    pub cq: Dma,
    pub rq: Dma,
    pub sq: Dma,
    pub request: Dma,
    pub response: Dma,
}

impl Pages {
    /// The six pages from `alloc`.
    pub fn new(alloc: &mut dyn FnMut(usize) -> Option<Dma>) -> Option<Pages> {
        Some(Pages {
            eq: alloc(1)?,
            cq: alloc(1)?,
            rq: alloc(1)?,
            sq: alloc(1)?,
            request: alloc(1)?,
            response: alloc(1)?,
        })
    }
}

impl Channel {
    /// Asks the host for the hardware channel on `pages`, with its
    /// events on MSI-X entry `vector`, waits for it to be set up and
    /// tests it with an event.
    pub fn open(
        card: Card,
        pages: Pages,
        vector: u16,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Channel, Error> {
        let mut channel = Channel {
            card,
            eq: Queue::new(pages.eq, EQE),
            cq: Queue::new(pages.cq, CQE),
            rq: Queue::new(pages.rq, 0),
            sq: Queue::new(pages.sq, 0),
            request: pages.request,
            response: pages.response,
            doorbell: None,
            key: 0,
            max_request: 0,
            max_cqs: 0,
            posted: false,
            ready: false,
            tested: false,
            open: false,
            activity: 0,
        };
        let message = establish_message(
            [
                channel.eq.memory.phys(),
                channel.cq.memory.phys(),
                channel.rq.memory.phys(),
                channel.sq.memory.phys(),
            ],
            vector,
        );
        channel.card.smc(smc::ESTABLISH, message, delay)?;
        channel.open = true;
        let result = channel.start(delay);
        if result.is_err() {
            channel.close(delay);
        }
        result.map(|()| channel)
    }

    /// Waits for the host's events that set the channel up, then tests
    /// it.
    fn start(&mut self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let mut failed = None;
        let ready = wait_for(delay, START_US, STEP_US, || match self.events() {
            Ok(()) => self.ready,
            Err(error) => {
                failed = Some(error);
                true
            }
        });
        if let Some(error) = failed {
            return Err(error);
        }
        if !ready {
            return Err(Error::Timeout("setting up the hardware channel"));
        }
        let Some(doorbell) = self.doorbell else {
            return Err(Error::Device("the host named no doorbell for the hardware channel"));
        };
        if !self.card.has_doorbell_page(doorbell) {
            return Err(Error::Registers);
        }
        if [self.cq.id, self.rq.id, self.sq.id].contains(&u32::MAX) || self.cq.id >= self.max_cqs {
            return Err(Error::Device("the host named no queues for the hardware channel"));
        }
        let eq = self.eq.id;
        self.tested = false;
        self.call(
            "testing the hardware channel",
            request::GENERATE_TEST_EQE,
            device::HWC,
            REQUEST_HEADER + 4,
            RESPONSE_HEADER,
            |request| request.write32(REQUEST_HEADER, eq),
            delay,
        )?;
        let mut failed = None;
        let tested = wait_for(delay, ANSWER_US, STEP_US, || match self.events() {
            Ok(()) => self.tested,
            Err(error) => {
                failed = Some(error);
                true
            }
        });
        match failed {
            Some(error) => Err(error),
            None if !tested => Err(Error::Timeout("the hardware channel's test event")),
            None => Ok(()),
        }
    }

    /// The largest request the host takes, in bytes.
    pub fn max_request(&self) -> usize {
        (self.max_request as usize).min(MESSAGE)
    }

    /// Takes the channel's events: the host's that set it up, and test
    /// events.
    fn events(&mut self) -> Result<(), Error> {
        let mut taken = false;
        while let Some((kind, detail)) = self.eq.next_event()? {
            taken = true;
            match kind {
                event::HWC_EQ_ID_DB => {
                    self.eq.id = detail & 0xffff;
                    self.doorbell = Some(detail >> 16);
                }
                event::HWC_DATA => {
                    let value = detail & 0xff_ffff;
                    match detail >> 24 {
                        data::CQ_ID => self.cq.id = value,
                        data::RQ_ID => self.rq.id = value,
                        data::SQ_ID => self.sq.id = value,
                        data::MAX_REQUEST => self.max_request = value,
                        data::MAX_CQS => self.max_cqs = value,
                        data::GPA_MKEY => self.key = value,
                        _ => {}
                    }
                }
                event::HWC_DONE => self.ready = true,
                event::TEST => self.tested = true,
                // Completions are polled.
                _ => {}
            }
        }
        if taken && let Some(page) = self.doorbell {
            self.card.ring_eq(page, &self.eq, false);
        }
        Ok(())
    }

    /// Sends request `code` (as `what`, for errors) to device `device`,
    /// `request_len` bytes with its header, which `fill` writes the rest
    /// of into the request page, and waits for its answer, at least
    /// `response_len` bytes counted with its header, in [`Channel::answer`].
    /// An answer whose status is not 0 is an error; so is a MANA request's
    /// whose type, device or number is not the request's.
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &mut self,
        what: &'static str,
        code: u32,
        device: u32,
        request_len: usize,
        response_len: usize,
        fill: impl FnOnce(&Dma),
        delay: &mut dyn FnMut(u32),
    ) -> Result<(), Error> {
        let Some(doorbell) = self.doorbell else {
            return Err(Error::Device("the hardware channel is not set up"));
        };
        if request_len > self.max_request() || response_len > MESSAGE {
            return Err(Error::Unsupported("a request larger than the hardware channel takes"));
        }
        // The answer's page, zeroed, so that a shorter answer than this
        // driver reads leaves zeros.
        if !self.posted {
            self.response.zero();
            self.rq.post(&[], self.response.phys(), self.key, MESSAGE as u32);
            self.card.ring_wq(doorbell, kind::RQ, &self.rq);
            self.posted = true;
        }
        let mana = code >= 0x2_0000;
        if mana {
            self.activity = self.activity.wrapping_add(1);
        }
        let request = &self.request;
        request.zero();
        // The request's header, then the answer it expects, each
        // version 1 (MANA's device query asks for version 2).
        request.write32(4, code);
        request.write16(8, 1);
        request.write32(12, request_len as u32);
        request.write32(20, code);
        request.write16(24, 1);
        request.write32(28, response_len as u32);
        request.write32(32, device);
        request.write32(36, if mana { self.activity } else { 0 });
        fill(request);
        // The request goes to the host's side of the channel: its queues
        // 0, and this end's completion and send queues.
        let oob = [0, 0, 0, 0, self.cq.id & 0xff_ffff, self.sq.id & 0xff_ffff];
        self.sq.post(&oob, self.request.phys(), self.key, request_len as u32);
        self.card.ring_wq(doorbell, kind::SQ, &self.sq);
        let mut answer = None;
        let mut failed = None;
        let done = wait_for(delay, ANSWER_US, STEP_US, || {
            if let Err(error) = self.events() {
                failed = Some(error);
                return true;
            }
            match self.answered() {
                Ok(Some(len)) => {
                    answer = Some(len);
                    true
                }
                Ok(None) => false,
                Err(error) => {
                    failed = Some(error);
                    true
                }
            }
        });
        if let Some(error) = failed {
            return Err(error);
        }
        let Some(len) = answer.filter(|_| done) else {
            return Err(Error::Timeout(what));
        };
        let response = &self.response;
        if len < RESPONSE_HEADER || response.read16(10) != 0 {
            return Err(Error::Device(what));
        }
        if mana
            && (response.read32(4) != code
                || response.read16(8) == 0
                || response.read32(16) != device
                || response.read32(20) != self.activity)
        {
            return Err(Error::Device(what));
        }
        if response.read32(24) != 0 {
            return Err(Error::Device(what));
        }
        Ok(())
    }

    /// Takes the channel's completions: its answer's length once it came.
    fn answered(&mut self) -> Result<Option<usize>, Error> {
        let mut answer = None;
        let mut taken = false;
        while let Some(completion) = self.cq.next_completion()? {
            taken = true;
            if !completion.send && completion.queue == self.rq.id {
                self.posted = false;
                self.rq.tail = self.rq.head;
                // The answer's length, in the completion's data.
                let len = self.cq.memory.read32(completion.at + 24) as usize;
                answer = Some(len.min(MESSAGE));
            } else if completion.send {
                self.sq.tail = self.sq.head;
            }
        }
        if taken && let Some(page) = self.doorbell {
            self.card.ring_cq(page, &self.cq, false);
        }
        Ok(answer)
    }

    /// The last answer.
    pub fn answer(&self) -> &Dma {
        &self.response
    }

    /// Takes the channel down: the host's driver frees its side, and the
    /// card stops using its pages. Whether it did.
    pub fn close(&mut self, delay: &mut dyn FnMut(u32)) -> bool {
        if !self.open {
            return true;
        }
        self.open = false;
        // Only the header is written: the rest of the message is unused.
        if !self.card.wait_smc(delay) {
            return false;
        }
        self.card.regs.write32(self.card.shm + smc::LAST, smc::DESTROY);
        if !self.card.wait_smc(delay) {
            return false;
        }
        let header = self.card.regs.read32(self.card.shm + smc::LAST);
        header & 0x7 == smc::DESTROY && header & smc::RESPONSE != 0 && (header >> 8) & 0xff == 0
    }
}
