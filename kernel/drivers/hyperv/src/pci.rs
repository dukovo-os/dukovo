// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hyper-V's virtual PCI bus (vPCI): how a device the host passes through
//! reaches the guest, as Azure's network adapter does for accelerated
//! networking (ADR 0425).
//!
//! The host offers each such bus as a channel of its own (of kind
//! [`crate::guid::PCI`]), usually with one function on it. The guest and
//! the host speak the PCI pass-through protocol on it, in INBAND packets,
//! those of the guest's that are answered being answered in completions
//! whose data starts with a status, negative for a failure:
//!
//! 1. QUERY_PROTOCOL_VERSION asks for a version of the protocol, 1.4 and
//!    then 1.1; the host refuses one it does not speak with
//!    STATUS_REVISION_MISMATCH;
//! 2. QUERY_BUS_RELATIONS asks for the bus's functions, which the host
//!    lists in a BUS_RELATIONS packet (BUS_RELATIONS2 from 1.4): each
//!    one's IDs, class, subsystem, slot and serial number;
//! 3. QUERY_RESOURCE_REQUIREMENTS asks, for each function, what its BARs
//!    read once all ones were written to them: their sizes;
//! 4. BUS_D0ENTRY powers the bus on and names the window of 8 KiB of the
//!    guest's physical addresses, outside its memory, through which the
//!    guest reaches the functions' configuration spaces: it writes a
//!    function's slot at the window's start, then reads and writes that
//!    function's configuration space in the window's second page;
//! 5. RESOURCES_ASSIGNED (RESOURCES_ASSIGNED2 from 1.4) says, for each
//!    function, that the guest gave it its resources.
//!
//! The guest then gives each BAR an address in the ranges the firmware
//! sets apart for VMBus, and drives the function as any PCI function but
//! for two things. Its header reads all ones where the host gives what it
//! is, which the descriptor says ([`Descriptor::given`]). And its MSI-X
//! entries hold what the host gives for each interrupt the guest asks for
//! (CREATE_INTERRUPT_MESSAGE, or CREATE_INTERRUPT_MESSAGE3 from 1.4): the
//! guest names the vector and the virtual processor to interrupt, and the
//! host answers with the address and data to write. The host takes a
//! function away with EJECT, which the guest answers with
//! EJECTION_COMPLETE once it has stopped the function.
//!
//! The layouts and the sequence follow FreeBSD's vmbus_pcib
//! (BSD-2-Clause, credits/freebsd-hyperv.md).

use radix_hw::pci::Given;
use radix_nic::wait_for;

use crate::Signal;
use crate::ring::{self, COMPLETION, INBAND, Rings};

/// Version 1.1 of the protocol: Windows Server 2016's.
pub const VERSION_1_1: u32 = 0x1_0001;
/// Version 1.4: Windows Server 2019's and Azure's, which lists more of
/// each function and names any virtual processor.
pub const VERSION_1_4: u32 = 0x1_0004;
/// The versions the kernel speaks, newest first.
pub const VERSIONS: [u32; 2] = [VERSION_1_4, VERSION_1_1];

/// The bytes of the configuration window BUS_D0ENTRY names.
pub const CONFIG_WINDOW: usize = 0x2000;
/// Where in the window the chosen function's configuration space is.
pub const CONFIG_PAGE: usize = 0x1000;
/// The most functions of one bus the kernel keeps: more are logged and
/// left.
pub const FUNCTIONS: usize = 8;

/// The message types.
mod message {
    const BASE: u32 = 0x4249_0000;
    pub const BUS_RELATIONS: u32 = BASE;
    pub const QUERY_BUS_RELATIONS: u32 = BASE + 0x1;
    pub const QUERY_RESOURCE_REQUIREMENTS: u32 = BASE + 0x5;
    pub const BUS_D0ENTRY: u32 = BASE + 0x7;
    pub const EJECT: u32 = BASE + 0xb;
    pub const EJECTION_COMPLETE: u32 = BASE + 0xf;
    pub const RESOURCES_ASSIGNED: u32 = BASE + 0x10;
    pub const QUERY_PROTOCOL_VERSION: u32 = BASE + 0x13;
    pub const CREATE_INTERRUPT: u32 = BASE + 0x14;
    pub const DELETE_INTERRUPT: u32 = BASE + 0x15;
    pub const RESOURCES_ASSIGNED2: u32 = BASE + 0x16;
    pub const BUS_RELATIONS2: u32 = BASE + 0x19;
    pub const CREATE_INTERRUPT3: u32 = BASE + 0x1b;
}

/// The status of a version request the host does not speak.
const REVISION_MISMATCH: i32 = 0xc000_0059_u32 as i32;

/// The bytes of a function's descriptor in BUS_RELATIONS and in
/// BUS_RELATIONS2, which adds its flags and NUMA node.
const DESCRIPTOR_1: usize = 20;
const DESCRIPTOR_2: usize = 28;

/// The bytes of RESOURCES_ASSIGNED and RESOURCES_ASSIGNED2: the type and
/// the slot, the memory ranges (unused: the guest sets the BARs itself)
/// and the count of MSI descriptors (none), and reserved bytes.
const RESOURCES_1: usize = 148;
const RESOURCES_2: usize = 202;

/// The bytes of CREATE_INTERRUPT_MESSAGE and CREATE_INTERRUPT_MESSAGE3.
const INTERRUPT_1: usize = 24;
const INTERRUPT_3: usize = 82;

/// The most bytes of a packet from the host the bus reads: BUS_RELATIONS2
/// with [`FUNCTIONS`] functions. A longer list is cut there.
const PACKET: usize = 8 + DESCRIPTOR_2 * FUNCTIONS;

/// The bytes of a completion kept: QUERY_RESOURCE_REQUIREMENTS's, the
/// longest: its status and six BARs.
const ANSWER: usize = 28;
/// The bytes of CREATE_INTERRUPT's completion.
const INTERRUPT_ANSWER: usize = 24;

/// How long the host has to answer, in microseconds.
const ANSWER_US: u32 = 10_000_000;

fn u16_at(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from(u32_at(data, at)) | u64::from(u32_at(data, at + 4)) << 32
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

/// The slot the protocol names a function by: its device number in the
/// low 5 bits, its function number in the next 3.
pub fn slot(device: u8, function: u8) -> u32 {
    u32::from(device & 0x1f) | u32::from(function & 0x7) << 5
}

/// A function on the bus, as the host lists it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Descriptor {
    /// Its vendor ID.
    pub vendor: u16,
    /// Its device ID.
    pub device: u16,
    /// Its revision.
    pub revision: u8,
    /// Its programming interface.
    pub prog_if: u8,
    /// Its subclass.
    pub subclass: u8,
    /// Its base class.
    pub class: u8,
    /// Its subsystem vendor ID (low half) and subsystem ID (high half).
    pub subsystem: u32,
    /// Its slot ([`slot`]).
    pub slot: u32,
    /// Its serial number, which netvsc's VF association names, and whose
    /// low half Hyper-V's guests take as the bus's PCI segment.
    pub serial: u32,
}

impl Descriptor {
    fn parse(bytes: &[u8]) -> Descriptor {
        Descriptor {
            vendor: u16_at(bytes, 0),
            device: u16_at(bytes, 2),
            revision: bytes.get(4).copied().unwrap_or(0),
            prog_if: bytes.get(5).copied().unwrap_or(0),
            subclass: bytes.get(6).copied().unwrap_or(0),
            class: bytes.get(7).copied().unwrap_or(0),
            subsystem: u32_at(bytes, 8),
            slot: u32_at(bytes, 12),
            serial: u32_at(bytes, 16),
        }
    }

    /// Its device number on the bus.
    pub fn device_number(&self) -> u8 {
        (self.slot & 0x1f) as u8
    }

    /// Its function number.
    pub fn function_number(&self) -> u8 {
        ((self.slot >> 5) & 0x7) as u8
    }

    /// What its header reads, in place of the all ones its own reads.
    pub fn given(&self) -> Given {
        Given {
            vendor: self.vendor,
            device: self.device,
            revision: self.revision,
            prog_if: self.prog_if,
            subclass: self.subclass,
            class: self.class,
            subsystem: self.subsystem,
        }
    }

    /// Whether `other` lists the same function: FreeBSD's test.
    pub fn same(&self, other: &Descriptor) -> bool {
        self.slot == other.slot
            && self.vendor == other.vendor
            && self.device == other.device
            && self.serial == other.serial
    }
}

/// The bus's functions, as a BUS_RELATIONS packet lists them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Relations {
    /// How many functions the host listed.
    pub listed: usize,
    /// How many of them are kept, the first [`FUNCTIONS`] at most.
    pub count: usize,
    /// The functions kept.
    pub functions: [Descriptor; FUNCTIONS],
}

impl Relations {
    /// The functions kept.
    pub fn iter(&self) -> impl Iterator<Item = &Descriptor> {
        self.functions[..self.count].iter()
    }
}

/// A packet from the host that is not a completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FromHost {
    /// The bus's functions, all of them: one missing was taken away, one
    /// new was added.
    Relations(Relations),
    /// The host takes the function in `slot` away: the guest stops it and
    /// answers [`ejection_complete`].
    Eject {
        /// Its slot.
        slot: u32,
    },
    /// A message of a type the kernel does not use.
    Other(u32),
}

/// Reads a packet's data from the host. A BUS_RELATIONS packet cut short
/// keeps the functions that fit.
pub fn parse(data: &[u8]) -> Option<FromHost> {
    if data.len() < 8 {
        return None;
    }
    let kind = u32_at(data, 0);
    match kind {
        message::BUS_RELATIONS | message::BUS_RELATIONS2 => {
            let size = if kind == message::BUS_RELATIONS { DESCRIPTOR_1 } else { DESCRIPTOR_2 };
            let listed = u32_at(data, 4) as usize;
            let count = listed.min(FUNCTIONS).min((data.len() - 8) / size);
            let mut relations = Relations { listed, count, ..Relations::default() };
            for (i, function) in relations.functions[..count].iter_mut().enumerate() {
                *function = Descriptor::parse(&data[8 + i * size..8 + (i + 1) * size]);
            }
            Some(FromHost::Relations(relations))
        }
        message::EJECT => Some(FromHost::Eject { slot: u32_at(data, 4) }),
        other => Some(FromHost::Other(other)),
    }
}

/// QUERY_PROTOCOL_VERSION for `version`.
pub fn version_request(version: u32) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    put32(&mut bytes, 0, message::QUERY_PROTOCOL_VERSION);
    put32(&mut bytes, 4, version);
    bytes
}

/// QUERY_BUS_RELATIONS.
pub fn query_relations() -> [u8; 4] {
    message::QUERY_BUS_RELATIONS.to_le_bytes()
}

/// A message about the function in `slot`, of type `kind`, with nothing
/// more.
fn about(kind: u32, slot: u32) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    put32(&mut bytes, 0, kind);
    put32(&mut bytes, 4, slot);
    bytes
}

/// QUERY_RESOURCE_REQUIREMENTS for the function in `slot`.
pub fn query_resources(slot: u32) -> [u8; 8] {
    about(message::QUERY_RESOURCE_REQUIREMENTS, slot)
}

/// BUS_D0ENTRY, with the configuration window at `window`.
pub fn d0_entry(window: u64) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    put32(&mut bytes, 0, message::BUS_D0ENTRY);
    put64(&mut bytes, 8, window);
    bytes
}

/// RESOURCES_ASSIGNED for the function in `slot`, as `version` lays it
/// out: the message, and how many of its bytes to send.
pub fn resources_assigned(version: u32, slot: u32) -> ([u8; RESOURCES_2], usize) {
    let mut bytes = [0u8; RESOURCES_2];
    if version >= VERSION_1_4 {
        put32(&mut bytes, 0, message::RESOURCES_ASSIGNED2);
        put32(&mut bytes, 4, slot);
        (bytes, RESOURCES_2)
    } else {
        put32(&mut bytes, 0, message::RESOURCES_ASSIGNED);
        put32(&mut bytes, 4, slot);
        (bytes, RESOURCES_1)
    }
}

/// CREATE_INTERRUPT_MESSAGE for one interrupt of the function in `slot`,
/// on `vector` of virtual processor `processor`, with fixed delivery, as
/// `version` lays it out: the message, and how many of its bytes to send.
/// Before 1.4 the processor is a bit of a mask of 64.
pub fn create_interrupt(
    version: u32,
    slot: u32,
    vector: u8,
    processor: u32,
) -> ([u8; INTERRUPT_3], usize) {
    let mut bytes = [0u8; INTERRUPT_3];
    put32(&mut bytes, 4, slot);
    if version >= VERSION_1_4 {
        put32(&mut bytes, 0, message::CREATE_INTERRUPT3);
        put32(&mut bytes, 8, u32::from(vector));
        // Delivery (12) fixed, a reserved byte, then one vector on one
        // processor, which the array names.
        put16(&mut bytes, 14, 1);
        put16(&mut bytes, 16, 1);
        put16(&mut bytes, 18, processor as u16);
        (bytes, INTERRUPT_3)
    } else {
        put32(&mut bytes, 0, message::CREATE_INTERRUPT);
        bytes[8] = vector;
        // Delivery (9) fixed, one vector, a reserved word, the mask.
        put16(&mut bytes, 10, 1);
        put64(&mut bytes, 16, 1u64 << (processor % 64));
        (bytes, INTERRUPT_1)
    }
}

/// What the host gives for an interrupt: the message the function is to
/// send, and the host's own description of it, which DELETE_INTERRUPT
/// names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Translated {
    /// The MSI address to write in the MSI-X entry.
    pub address: u64,
    /// The MSI data.
    pub data: u32,
    /// How many vectors it covers.
    pub count: u16,
}

/// DELETE_INTERRUPT_MESSAGE for `interrupt` of the function in `slot`.
pub fn delete_interrupt(slot: u32, interrupt: &Translated) -> [u8; 24] {
    let mut bytes = [0u8; 24];
    put32(&mut bytes, 0, message::DELETE_INTERRUPT);
    put32(&mut bytes, 4, slot);
    put16(&mut bytes, 10, interrupt.count);
    put32(&mut bytes, 12, interrupt.data);
    put64(&mut bytes, 16, interrupt.address);
    bytes
}

/// EJECTION_COMPLETE for the function in `slot`.
pub fn ejection_complete(slot: u32) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    bytes[..8].copy_from_slice(&about(message::EJECTION_COMPLETE, slot));
    bytes
}

/// Why the bus could not be set up, or did not do what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The channel's ring had no room.
    Full,
    /// The host did not answer in time: what was asked.
    Timeout(&'static str),
    /// The host refused: what was asked, and its status.
    Refused(&'static str, i32),
    /// The host speaks no version the kernel does.
    NoVersion,
    /// What was asked cannot be: why.
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Full => f.write_str("the channel's ring is full"),
            Error::Timeout(what) => write!(f, "the host did not answer: {what}"),
            Error::Refused(what, status) => {
                write!(f, "the host refused {what} (status {:#x})", *status as u32)
            }
            Error::NoVersion => f.write_str("the host speaks no vPCI version the kernel does"),
            Error::Unsupported(what) => write!(f, "not supported: {what}"),
        }
    }
}

/// A function of the bus, set up: what the host listed, and what its BARs
/// read with all ones written, which gives their sizes and kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Child {
    /// What the host listed.
    pub descriptor: Descriptor,
    /// Its BARs, all ones written: 0 for one it does not have.
    pub probed: [u32; 6],
}

/// Something the host said once the bus was set up, for
/// [`Bus::next_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The host takes the function in `slot` away.
    Eject {
        /// Its slot.
        slot: u32,
    },
    /// The host listed the bus's functions again.
    Relations(Relations),
}

/// A completion kept: its transaction, and the first bytes of its data.
#[derive(Clone, Copy)]
struct Answer {
    transaction: u64,
    len: usize,
    data: [u8; ANSWER],
}

impl Answer {
    fn status(&self) -> i32 {
        if self.len < 4 { -1 } else { u32_at(&self.data, 0) as i32 }
    }
}

/// A virtual PCI bus, set up: its functions are reachable through the
/// configuration window, and their interrupts can be asked for.
pub struct Bus {
    rings: Rings,
    connection: u32,
    signal: Signal,
    version: u32,
    children: [Option<Child>; FUNCTIONS],
    answer: Option<Answer>,
    next_transaction: u64,
    /// The last relations the host sent, not yet taken.
    relations: Option<Relations>,
    /// Slots the host asked to eject, not yet taken: [`FUNCTIONS`] at
    /// most, oldest first.
    ejects: [Option<u32>; FUNCTIONS],
}

/// Sets up the bus on the channel whose rings are `rings` (open), which
/// is signalled through `connection` with `signal`, with its
/// configuration window at `window` ([`CONFIG_WINDOW`] bytes of physical
/// addresses no memory or other device uses). `delay` waits the given
/// microseconds, while the host answers.
#[inline(never)]
pub fn start(
    rings: Rings,
    connection: u32,
    signal: Signal,
    window: u64,
    delay: &mut dyn FnMut(u32),
) -> Result<Bus, Error> {
    let mut bus = Bus {
        rings,
        connection,
        signal,
        version: 0,
        children: [None; FUNCTIONS],
        answer: None,
        next_transaction: 1,
        relations: None,
        ejects: [None; FUNCTIONS],
    };
    for version in VERSIONS {
        let status =
            bus.request(&version_request(version), delay, "the protocol version")?.status();
        if status >= 0 {
            bus.version = version;
            break;
        }
        if status != REVISION_MISMATCH {
            return Err(Error::Refused("the protocol version", status));
        }
    }
    if bus.version == 0 {
        return Err(Error::NoVersion);
    }
    bus.send(&query_relations(), 0, false)?;
    wait_for(delay, ANSWER_US, 100, || {
        bus.take();
        bus.relations.is_some()
    });
    let relations = bus.relations.take().ok_or(Error::Timeout("the bus's functions"))?;
    for (index, descriptor) in relations.iter().enumerate() {
        let answer = bus.request(
            &query_resources(descriptor.slot),
            delay,
            "a function's resource requirements",
        )?;
        if answer.status() < 0 || answer.len < ANSWER {
            return Err(Error::Refused("a function's resource requirements", answer.status()));
        }
        let mut probed = [0u32; 6];
        for (i, bar) in probed.iter_mut().enumerate() {
            *bar = u32_at(&answer.data, 4 + 4 * i);
        }
        bus.children[index] = Some(Child { descriptor: *descriptor, probed });
    }
    let status = bus.request(&d0_entry(window), delay, "powering the bus on")?.status();
    if status < 0 {
        return Err(Error::Refused("powering the bus on", status));
    }
    for index in 0..FUNCTIONS {
        let Some(child) = bus.children[index] else { continue };
        let (message, len) = resources_assigned(bus.version, child.descriptor.slot);
        let status = bus.request(&message[..len], delay, "a function's resources")?.status();
        if status < 0 {
            return Err(Error::Refused("a function's resources", status));
        }
    }
    Ok(bus)
}

impl Bus {
    /// The version of the protocol agreed.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The functions set up.
    pub fn children(&self) -> impl Iterator<Item = &Child> {
        self.children.iter().flatten()
    }

    /// Signals the host's end of the channel.
    fn ring_doorbell(&self) {
        (self.signal)(self.connection);
    }

    /// Sends `message`, as transaction `transaction`, asking for a
    /// completion if `answer`.
    fn send(&self, message: &[u8], transaction: u64, answer: bool) -> Result<(), Error> {
        match ring::send(&self.rings, message, transaction, answer) {
            Ok(true) => {
                self.ring_doorbell();
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(_) => Err(Error::Full),
        }
    }

    /// Sends `message` and waits for its completion: `what` names it if
    /// none comes.
    fn request(
        &mut self,
        message: &[u8],
        delay: &mut dyn FnMut(u32),
        what: &'static str,
    ) -> Result<Answer, Error> {
        let transaction = self.next_transaction;
        self.next_transaction += 1;
        self.answer = None;
        self.send(message, transaction, true)?;
        let mut answer = None;
        wait_for(delay, ANSWER_US, 100, || {
            self.take();
            answer = self.answer.take().filter(|answer| answer.transaction == transaction);
            answer.is_some()
        });
        answer.ok_or(Error::Timeout(what))
    }

    /// Takes every packet waiting: completions are kept for the request
    /// that waits, relations and ejections for [`Bus::next_event`].
    fn take(&mut self) {
        let mut data = [0u8; PACKET];
        while let Some((header, len, wake)) = self.rings.read(&mut data) {
            if wake {
                self.ring_doorbell();
            }
            match header.kind {
                COMPLETION => {
                    let mut kept = [0u8; ANSWER];
                    let len = len.min(ANSWER);
                    kept[..len].copy_from_slice(&data[..len]);
                    self.answer = Some(Answer { transaction: header.transaction, len, data: kept });
                }
                INBAND => match parse(&data[..len]) {
                    Some(FromHost::Relations(relations)) => self.relations = Some(relations),
                    Some(FromHost::Eject { slot }) => {
                        if !self.ejects.contains(&Some(slot))
                            && let Some(free) = self.ejects.iter_mut().find(|s| s.is_none())
                        {
                            *free = Some(slot);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// The next thing the host said: an ejection before relations.
    pub fn next_event(&mut self) -> Option<Event> {
        self.take();
        if let Some(slot) = self.ejects[0] {
            self.ejects.copy_within(1.., 0);
            self.ejects[FUNCTIONS - 1] = None;
            return Some(Event::Eject { slot });
        }
        self.relations.take().map(Event::Relations)
    }

    /// Asks the host for an interrupt of the function in `slot`, raised
    /// on `vector` of virtual processor `processor`: returns the message
    /// to write in its MSI-X entry.
    pub fn create_interrupt(
        &mut self,
        slot: u32,
        vector: u8,
        processor: u32,
        delay: &mut dyn FnMut(u32),
    ) -> Result<Translated, Error> {
        if self.version < VERSION_1_4 && processor > 63 {
            return Err(Error::Unsupported("a virtual processor past 63 before vPCI 1.4"));
        }
        let (message, len) = create_interrupt(self.version, slot, vector, processor);
        let answer = self.request(&message[..len], delay, "an interrupt")?;
        if answer.status() < 0 || answer.len < INTERRUPT_ANSWER {
            return Err(Error::Refused("an interrupt", answer.status()));
        }
        // After the status and a reserved word: a reserved half, the count
        // of vectors, the data and the address.
        Ok(Translated {
            count: u16_at(&answer.data, 10),
            data: u32_at(&answer.data, 12),
            address: u64_at(&answer.data, 16),
        })
    }

    /// Tells the host the guest no longer uses `interrupt` of the
    /// function in `slot`.
    pub fn delete_interrupt(&mut self, slot: u32, interrupt: &Translated) -> Result<(), Error> {
        self.send(&delete_interrupt(slot, interrupt), 0, false)
    }

    /// Tells the host the function in `slot` is stopped, after it asked
    /// to eject it, and forgets it.
    pub fn ejection_complete(&mut self, slot: u32) -> Result<(), Error> {
        for child in &mut self.children {
            if child.is_some_and(|child| child.descriptor.slot == slot) {
                *child = None;
            }
        }
        self.send(&ejection_complete(slot), 0, false)
    }

    /// Whether a packet waits in the inbound ring, with the host's
    /// signals turned back on.
    pub fn arm(&self) -> bool {
        self.rings.mask(false)
    }
}
