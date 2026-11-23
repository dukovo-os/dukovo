// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A simulated controller, for the tests.
//!
//! One [`Sim`] plays either interface, by the family it is made for. Its
//! registers are the real ones' (the layout is in [`crate::regs`]); the
//! firmware behind them moves through its states as the doorbell is rung,
//! and time passes only when the test's `wait` closure (see
//! [`Sim::waiter`]) says so.
//!
//! - MFI: a frame's address comes through the inbound queue port. The
//!   INIT frame gives the reply queue, and each command's context is put on
//!   it, as QEMU's `megasas` does: the producer moves only while interrupts
//!   are enabled, and masking them all resets the controller.
//! - Fusion: an MFA descriptor carries the INIT frame and the IOC INIT
//!   request it points to; later descriptors name request frames, and the
//!   replies go to a ring that is reused only as far as the host reported
//!   it read.
//!
//! Volumes and disks are RAM disks of 512-byte blocks. Management commands
//! are answered by a callback the test gives. Every command the firmware
//! saw is kept in [`Inner::seen`], with the bytes of its frame, for the
//! tests to check what the driver wrote.
//!
//! When a test goes wrong, look at [`Inner::seen`], [`Inner::posts`],
//! [`Inner::doorbell_log`] and [`Inner::reply_index_writes`] first.

use std::boxed::Box;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers, fake_memory, fake_memory_write};

use crate::Family;
use crate::frame::{self, cmd, flag, status};
use crate::regs::{REGISTERS, reg};

/// The firmware states, as the top four bits of the status word.
pub mod fw {
    /// Not yet saying.
    pub const UNDEFINED: u8 = 0x0;
    /// Waiting for the host to answer a handshake.
    pub const WAIT_HANDSHAKE: u8 = 0x6;
    /// Looking for disks.
    pub const SCANNING: u8 = 0x8;
    /// Waiting for the host to answer a boot message.
    pub const BOOT_MESSAGE: u8 = 0x9;
    /// Ready to be given its queues.
    pub const READY: u8 = 0xb;
    /// Running.
    pub const OPERATIONAL: u8 = 0xc;
    /// Stopped by a fault.
    pub const FAULT: u8 = 0xf;
}

/// The request frame size on fusion.
const REQUEST: usize = 256;
/// The first scatter-gather entry of a request frame.
const SGL_AT: usize = 0x80;
/// The RAID context's place in a request frame.
const CONTEXT_AT: usize = 0x60;
/// The size of the IOC INIT request.
const IOC_INIT_SIZE: usize = 0x48;
/// The size of an MFI frame, at most.
const MFI_FRAME: usize = 512;

/// Reads `len` bytes of host memory at `phys`.
pub fn read_bytes(phys: u64, len: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; len];
    fake_memory(phys, &mut out);
    out
}

/// Writes `data` to host memory at `phys`.
pub fn write_bytes(phys: u64, data: &[u8]) {
    fake_memory_write(phys, data);
}

/// The little-endian 16-bit word of `bytes` at `at`.
pub fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

/// The little-endian 32-bit word of `bytes` at `at`.
pub fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// The little-endian 64-bit word of `bytes` at `at`.
pub fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// The scratch pad 2 a fusion controller of `family` says: a chain frame
/// of `chain_bytes`, `vectors` interrupt vectors, and whether it takes
/// SYNCHRONIZE CACHE for a volume. A chain size of zero says none.
pub fn pad2(family: Family, chain_bytes: u32, vectors: u32, sync_cache: bool) -> u32 {
    let mut word = 0;
    if chain_bytes != 0 {
        if chain_bytes.is_multiple_of(128) && chain_bytes / 128 <= 31 {
            word |= (chain_bytes / 128) << 5;
        } else {
            word |= ((chain_bytes / 512) << 5) | 0x40_0000;
        }
    }
    let vectors = vectors.max(1) - 1;
    word |= if family == Family::Thunderbolt { vectors & 0x1f } else { (vectors & 0xff) << 14 };
    if sync_cache {
        word |= 0x0100_0000;
    }
    word
}

/// A RAM disk.
#[derive(Clone, Debug)]
pub struct Disk {
    /// Its bytes, a whole number of blocks.
    pub data: Vec<u8>,
    /// What its INQUIRY says it is.
    pub product: &'static str,
}

impl Disk {
    /// A disk of `blocks` blocks, each filled with its own number.
    fn new(blocks: usize, product: &'static str) -> Disk {
        let data = (0..blocks * 512).map(|n| (n / 512) as u8).collect();
        Disk { data, product }
    }

    /// How many blocks it has.
    pub fn blocks(&self) -> usize {
        self.data.len() / 512
    }
}

/// The reply queue an MFI INIT frame gave.
#[derive(Clone, Debug)]
pub struct Queue {
    /// The queue information's flags.
    pub flags: u32,
    /// How many contexts the queue holds.
    pub entries: u32,
    /// Where the queue is.
    pub queue: u64,
    /// Where the producer index is.
    pub producer: u64,
    /// Where the consumer index is.
    pub consumer: u64,
    /// The next slot the firmware writes.
    pub head: u32,
}

/// What a fusion IOC INIT gave.
#[derive(Clone, Debug)]
pub struct Ioc {
    /// The request as the firmware read it.
    pub raw: Vec<u8>,
    /// The capability word of the MFI INIT frame that carried it.
    pub capabilities: u32,
    /// The reply ring's depth.
    pub depth: u32,
    /// Where the request frames are.
    pub requests: u64,
    /// Where the reply ring is.
    pub replies: u64,
    /// The high half of every sense buffer's address.
    pub sense_high: u32,
}

/// One scatter-gather entry as the firmware read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its address.
    pub addr: u64,
    /// Its length.
    pub len: u32,
    /// Its flags byte.
    pub flags: u8,
    /// Whether it was in a chain frame.
    pub in_chain: bool,
}

/// One command the firmware saw.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    /// A management command.
    pub dcmd: bool,
    /// The opcode of a management command.
    pub opcode: u32,
    /// A SCSI command to a volume.
    pub volume: bool,
    /// The target.
    pub target: u16,
    /// The logical unit.
    pub lun: u8,
    /// The command descriptor block.
    pub cdb: Vec<u8>,
    /// The runs of memory the data took.
    pub runs: Vec<(u64, u32)>,
    /// The frame or request as posted (a request's 256 bytes, an MFI frame's
    /// first 512).
    pub frame: Vec<u8>,
    /// Fusion: the entries of the list, chain elements included.
    pub entries: Vec<Entry>,
    /// Fusion: the descriptor's type, SMID and device handle.
    pub descriptor: (u8, u16, u16),
    /// MFI: the word written to the inbound queue port.
    pub word: u32,
}

/// What a management command was given.
#[derive(Clone, Debug)]
pub struct DcmdSeen {
    /// The opcode.
    pub opcode: u32,
    /// The mailbox.
    pub mailbox: [u8; 12],
    /// The data the host sent, if the command carried any to the device.
    pub sent: Vec<u8>,
}

/// The test's answer to management commands: the opcode and mailbox in,
/// the status and the bytes out.
pub type Answerer = Box<dyn FnMut(u32, &[u8; 12]) -> (u8, Vec<u8>) + Send>;

enum Held {
    Mfi { at: u64, remaining: usize },
    Fusion { smid: u16 },
}

/// How a SCSI command ended.
struct Outcome {
    status: u8,
    scsi_status: u8,
    sense: Vec<u8>,
}

impl Outcome {
    fn good() -> Outcome {
        Outcome { status: status::OK, scsi_status: 0, sense: Vec::new() }
    }

    fn status(status: u8) -> Outcome {
        Outcome { status, scsi_status: 0, sense: Vec::new() }
    }

    fn check(sense: Vec<u8>) -> Outcome {
        Outcome { status: status::SCSI_DONE_WITH_ERROR, scsi_status: 2, sense }
    }
}

/// Fixed-format sense data of `key`, `asc` and `ascq`.
fn sense_data(key: u8, asc: u8, ascq: u8) -> Vec<u8> {
    let mut sense = std::vec![0u8; 18];
    sense[0] = 0x70;
    sense[2] = key;
    sense[7] = 10;
    sense[12] = asc;
    sense[13] = ascq;
    sense
}

/// Copies `data` into `runs`, as far as they go; how much went.
fn scatter(runs: &[(u64, u32)], data: &[u8]) -> usize {
    let mut done = 0;
    for &(phys, len) in runs {
        let take = (len as usize).min(data.len() - done);
        if take == 0 {
            break;
        }
        write_bytes(phys, &data[done..done + take]);
        done += take;
    }
    done
}

/// Reads up to `max` bytes from `runs`.
fn gather(runs: &[(u64, u32)], max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for &(phys, len) in runs {
        let take = (len as usize).min(max - out.len());
        if take == 0 {
            break;
        }
        out.extend_from_slice(&read_bytes(phys, take));
    }
    out
}

/// What a simulated controller keeps. Tests read and set these through
/// [`Sim::lock`], never while the driver runs.
pub struct Inner {
    family: Family,
    /// The simulated time, in milliseconds.
    pub now: u64,
    /// The firmware's state (see [`fw`]).
    pub state: u8,
    /// The fault code, when the state is [`fw::FAULT`].
    pub fault_code: u16,
    /// The command count the status word says.
    pub commands: u16,
    /// The scatter-gather limit the status word says.
    pub max_sge: u8,
    /// Whether the status word says MSI-X is supported.
    pub msix: bool,
    /// The second scratch pad.
    pub scratch2: u32,
    /// What the state does after the handshake is answered: the delay in
    /// milliseconds from the last step, and the state.
    pub after_handshake: Vec<(u32, u8)>,
    /// The same after the boot message is answered.
    pub after_boot_message: Vec<(u32, u8)>,
    /// The same after an operational firmware is sent back to ready.
    pub after_reset: Vec<(u32, u8)>,
    /// How long a fusion reset's bit in the doorbell stays set, in ms.
    pub reset_ms: u32,
    /// Every word written to the doorbell.
    pub doorbell_log: Vec<u32>,
    timeline: Vec<(u64, u8)>,
    doorbell: u32,
    door_clears_at: Option<u64>,
    /// A fault to raise at this time, with this code.
    pub fault_at: Option<(u64, u16)>,

    /// The interrupt mask.
    pub mask: u32,
    raised: u32,
    ranges: Vec<(u64, usize)>,
    frame_high: u32,
    inbound_low: u32,

    /// MFI: the reply queue, once given.
    pub queue: Option<Queue>,
    /// MFI: the INIT frame as read.
    pub init_frame: Vec<u8>,
    /// MFI: how many times the reply queue overran the consumer.
    pub overruns: usize,
    /// MFI: every word and high half written to the inbound queue.
    pub posts: Vec<(u32, u32)>,
    /// MFI: how many frames had an address that no memory held.
    pub unresolved: usize,

    /// Fusion: what the IOC INIT gave.
    pub ioc: Option<Ioc>,
    /// Fusion: every request descriptor written, low and high half.
    pub descriptors: Vec<(u32, u32)>,
    /// Fusion: the next reply descriptor the firmware writes.
    pub fw_index: u32,
    /// Fusion: how many replies were written, ever.
    pub posted_total: u64,
    /// Fusion: how many the host reported read, ever.
    pub reported_total: u64,
    last_reported: u32,
    backlog: VecDeque<(u32, u32)>,
    /// Fusion: every write to a reply index register.
    pub reply_index_writes: Vec<(usize, u32)>,
    /// Fusion: how many times the firmware wrote over a reply the host had
    /// not cleared.
    pub overwrites: usize,
    /// Fusion: how many requests came when the firmware could not take one.
    pub dropped: usize,

    /// The volumes by target.
    pub volumes: BTreeMap<u16, Disk>,
    /// The pass-through disks by device ID.
    pub disks: BTreeMap<u16, Disk>,
    answerer: Option<Answerer>,
    /// What every command that came looked like.
    pub seen: Vec<Seen>,
    /// The management commands and what they carried.
    pub dcmds: Vec<DcmdSeen>,

    /// Whether commands wait for [`Sim::release`].
    pub holding: bool,
    held: VecDeque<Held>,
    /// The status the next command ends with instead of running.
    pub refuse: Option<u8>,
    /// Sense data the next SCSI command ends with, as a check condition.
    pub sense_next: Option<Vec<u8>>,
    /// The status the INIT is answered with.
    pub init_status: Option<u8>,
    /// Whether the INIT is never answered.
    pub ignore_init: bool,
}

impl Inner {
    fn new(family: Family) -> Inner {
        let fusion = family.fusion();
        Inner {
            family,
            now: 0,
            state: fw::READY,
            fault_code: 0,
            // Fusion firmware counts one more than it takes.
            commands: if fusion { 65 } else { 64 },
            max_sge: 0,
            msix: false,
            scratch2: if fusion { pad2(family, 512, 1, false) } else { 0 },
            after_handshake: std::vec![(0, fw::READY)],
            after_boot_message: std::vec![(0, fw::READY)],
            after_reset: std::vec![(0, fw::READY)],
            reset_ms: 0,
            doorbell_log: Vec::new(),
            timeline: Vec::new(),
            doorbell: 0,
            door_clears_at: None,
            fault_at: None,
            mask: u32::MAX,
            raised: 0,
            ranges: Vec::new(),
            frame_high: 0,
            inbound_low: 0,
            queue: None,
            init_frame: Vec::new(),
            overruns: 0,
            posts: Vec::new(),
            unresolved: 0,
            ioc: None,
            descriptors: Vec::new(),
            fw_index: 0,
            posted_total: 0,
            reported_total: 0,
            last_reported: 0,
            backlog: VecDeque::new(),
            reply_index_writes: Vec::new(),
            overwrites: 0,
            dropped: 0,
            volumes: BTreeMap::new(),
            disks: BTreeMap::new(),
            answerer: None,
            seen: Vec::new(),
            dcmds: Vec::new(),
            holding: false,
            held: VecDeque::new(),
            refuse: None,
            sense_next: None,
            init_status: None,
            ignore_init: false,
        }
    }

    /// The firmware's status word.
    pub fn status_word(&self) -> u32 {
        if self.state == fw::FAULT {
            return u32::from(fw::FAULT) << 28 | u32::from(self.fault_code);
        }
        u32::from(self.state) << 28
            | u32::from(self.msix) << 26
            | u32::from(self.max_sge) << 16
            | u32::from(self.commands)
    }

    /// Whether the controller's interrupts are enabled.
    pub fn interrupts_enabled(&self) -> bool {
        self.mask != u32::MAX
    }

    /// Puts the firmware in `state` at once, dropping what was to come.
    pub fn set_state(&mut self, state: u8) {
        self.timeline.clear();
        self.state = state;
    }

    /// Adds a volume at `target` of `blocks` blocks.
    pub fn add_volume(&mut self, target: u16, blocks: usize) {
        self.volumes.insert(target, Disk::new(blocks, "VOLUME"));
    }

    /// Adds a pass-through disk with device ID `id` of `blocks` blocks.
    pub fn add_disk(&mut self, id: u16, blocks: usize) {
        self.disks.insert(id, Disk::new(blocks, "DISK"));
    }

    /// Sets how management commands are answered.
    pub fn answer_with(&mut self, answerer: Answerer) {
        self.answerer = Some(answerer);
    }

    /// Stops the firmware with a fault of `code`.
    pub fn fault(&mut self, code: u16) {
        self.timeline.clear();
        self.state = fw::FAULT;
        self.fault_code = code;
    }

    /// Whether some memory handed out has `high` as its address's high half.
    pub fn memory_with_high(&self, high: u32) -> bool {
        self.ranges.iter().any(|&(phys, len)| {
            (phys >> 32) as u32 == high || ((phys + len as u64 - 1) >> 32) as u32 == high
        })
    }

    /// Whether `phys` lies in memory handed out.
    pub fn holds(&self, phys: u64) -> bool {
        self.ranges.iter().any(|&(start, len)| phys >= start && phys < start + len as u64)
    }

    /// The interrupt status bits raised and not yet cleared.
    pub fn raised(&self) -> u32 {
        self.raised
    }

    /// Raises interrupt status bits as they are, whoever they are for.
    pub fn raise_bits(&mut self, bits: u32) {
        self.raised |= bits;
    }

    /// How many commands wait to be released.
    pub fn held(&self) -> usize {
        self.held.len() + self.backlog.len()
    }

    /// How many reply slots the firmware may still write.
    pub fn reply_room(&self) -> u64 {
        let depth = self.ioc.as_ref().map_or(0, |ioc| u64::from(ioc.depth));
        depth.saturating_sub(self.posted_total - self.reported_total)
    }

    /// Writes a raw reply descriptor at the firmware's index, as if it
    /// came from a command, and moves the index on.
    pub fn inject_reply(&mut self, low: u32, high: u32) {
        let Some(ioc) = &self.ioc else { return };
        let at = ioc.replies + u64::from(self.fw_index) * 8;
        write_bytes(at, &(u64::from(high) << 32 | u64::from(low)).to_le_bytes());
        self.fw_index = (self.fw_index + 1) % ioc.depth;
        self.posted_total += 1;
        self.raise();
    }

    /// MFI: puts `context` on the reply queue as if a command ended.
    pub fn inject_context(&mut self, context: u32) {
        self.complete_mfi(context);
    }

    fn doorbell_offset(&self) -> usize {
        match self.family {
            Family::Mfi1078 | Family::MfiGen2 => reg::INBOUND_DOORBELL,
            _ => reg::DOORBELL,
        }
    }

    /// Applies what the passing of time brings.
    fn tick(&mut self) {
        if let Some((at, code)) = self.fault_at
            && self.now >= at
        {
            self.fault_at = None;
            self.fault(code);
        }
        while let Some(&(at, state)) = self.timeline.first() {
            if at > self.now {
                break;
            }
            self.timeline.remove(0);
            if self.state != fw::FAULT {
                self.state = state;
            }
        }
        if let Some(at) = self.door_clears_at
            && self.now >= at
        {
            self.door_clears_at = None;
            self.doorbell &= !1;
        }
    }

    /// Queues `steps` of states, each its delay after the one before.
    fn schedule(&mut self, steps: &[(u32, u8)]) {
        let mut at = self.now;
        for &(delay, state) in steps {
            at += u64::from(delay);
            self.timeline.push((at, state));
        }
        self.tick();
    }

    fn raise(&mut self) {
        self.raised |= if self.family == Family::Mfi1078 { 0x8000_0000 } else { 1 };
    }

    /// Forgets the queues and goes to ready, as a reset does.
    fn soft_reset(&mut self, go_ready: bool) {
        self.queue = None;
        self.ioc = None;
        self.held.clear();
        self.backlog.clear();
        self.fw_index = 0;
        self.posted_total = 0;
        self.reported_total = 0;
        self.last_reported = 0;
        self.raised = 0;
        if go_ready && self.state != fw::FAULT {
            self.timeline.clear();
            self.state = fw::READY;
        }
    }

    fn read32(&mut self, offset: usize) -> u32 {
        self.tick();
        match offset {
            reg::OUTBOUND_MESSAGE_0 if self.family == Family::Mfi1078 => self.status_word(),
            reg::SCRATCH_PAD_0 if self.family != Family::Mfi1078 => self.status_word(),
            reg::SCRATCH_PAD_2 if self.family.fusion() => self.scratch2,
            reg::OUTBOUND_STATUS => {
                if self.interrupts_enabled() {
                    self.raised
                } else {
                    0
                }
            }
            reg::OUTBOUND_MASK => self.mask,
            reg::DOORBELL if self.doorbell_offset() == reg::DOORBELL => self.doorbell,
            _ => 0,
        }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        self.tick();
        if offset == self.doorbell_offset() {
            self.ring(value);
            return;
        }
        match offset {
            reg::OUTBOUND_MASK => {
                self.mask = value;
                if value == u32::MAX && !self.family.fusion() {
                    self.soft_reset(true);
                }
            }
            reg::OUTBOUND_STATUS | reg::OUTBOUND_DOORBELL_CLEAR => self.raised &= !value,
            reg::INBOUND_QUEUE if !self.family.fusion() => self.post_mfi(value, 0),
            reg::INBOUND_HIGH => match self.family {
                Family::MfiSkinny => self.frame_high = value,
                f if f.fusion() => self.post_fusion(self.inbound_low, value),
                _ => {}
            },
            reg::INBOUND_LOW => match self.family {
                Family::MfiSkinny => {
                    let high = std::mem::take(&mut self.frame_high);
                    self.post_mfi(value, high);
                }
                f if f.fusion() => self.inbound_low = value,
                _ => {}
            },
            reg::REPLY_INDEX | reg::REPLY_INDEX_COMBINED if self.family.fusion() => {
                self.report(offset, value);
            }
            _ => {}
        }
    }

    /// Takes what was written to the doorbell.
    fn ring(&mut self, value: u32) {
        self.doorbell_log.push(value);
        let handshake = match self.family {
            Family::Mfi1078 | Family::MfiGen2 => 0x08,
            _ => 0x08 | 0x10,
        };
        match self.state {
            fw::WAIT_HANDSHAKE if value & handshake == handshake => {
                let steps = self.after_handshake.clone();
                self.schedule(&steps);
            }
            fw::BOOT_MESSAGE if value & 0x10 != 0 => {
                let steps = self.after_boot_message.clone();
                self.schedule(&steps);
            }
            fw::OPERATIONAL | fw::READY if value & 0x02 != 0 => {
                self.soft_reset(false);
                if self.family.fusion() && value & 1 != 0 {
                    self.doorbell |= 1;
                    self.door_clears_at = Some(self.now + u64::from(self.reset_ms));
                }
                if self.state != fw::FAULT {
                    let steps = self.after_reset.clone();
                    self.schedule(&steps);
                }
            }
            _ => {}
        }
        self.tick();
    }

    /// Resolves a frame address of the inbound queue: the memory whose
    /// low 32 bits hold it. Returns its physical address and the bytes
    /// of the memory from there.
    fn resolve(&self, high: u32, low: u32) -> Option<(u64, usize)> {
        let full = u64::from(high) << 32 | u64::from(low);
        let mut found = None;
        for &(phys, len) in &self.ranges {
            let offset = low.wrapping_sub(phys as u32) as usize;
            if offset < len {
                let candidate = (phys + offset as u64, len - offset);
                if high != 0 && candidate.0 == full {
                    return Some(candidate);
                }
                found.get_or_insert(candidate);
            }
        }
        found
    }

    // ---- MFI

    fn post_mfi(&mut self, word: u32, high: u32) {
        self.posts.push((word, high));
        if self.state == fw::FAULT {
            return;
        }
        let Some((at, remaining)) = self.resolve(high, word & !0x1f) else {
            self.unresolved += 1;
            return;
        };
        let head = read_bytes(at, remaining.min(MFI_FRAME));
        if head[0] == cmd::INIT {
            self.init_mfi(at, &head);
        } else if self.holding {
            let seen = Seen { word, ..Self::note_mfi(&head) };
            self.seen.push(seen);
            self.held.push_back(Held::Mfi { at, remaining });
        } else {
            let seen = Seen { word, ..Self::note_mfi(&head) };
            self.seen.push(seen);
            self.run_mfi(at, remaining);
        }
    }

    /// What the log keeps of an MFI frame.
    fn note_mfi(frame: &[u8]) -> Seen {
        let flags = le16(frame, frame::header::FLAGS);
        let mut seen = Seen { frame: frame.to_vec(), ..Seen::default() };
        match frame[0] {
            cmd::DCMD => {
                seen.dcmd = true;
                seen.opcode = le32(frame, frame::dcmd::OPCODE);
                seen.runs = mfi_runs(frame, frame::dcmd::SGL, flags);
            }
            cmd::LD_SCSI_IO | cmd::PD_SCSI_IO => {
                seen.volume = frame[0] == cmd::LD_SCSI_IO;
                seen.target = u16::from(frame[frame::header::TARGET]);
                seen.lun = frame[frame::header::LUN];
                let len = usize::from(frame[frame::header::CDB_LEN]).min(frame::CDB_SIZE);
                seen.cdb = frame[frame::pass::CDB..frame::pass::CDB + len].to_vec();
                seen.runs = mfi_runs(frame, frame::pass::SGL, flags);
            }
            _ => {}
        }
        seen
    }

    fn init_mfi(&mut self, at: u64, frame: &[u8]) {
        if self.ignore_init {
            return;
        }
        self.init_frame = frame.to_vec();
        let answer = if let Some(forced) = self.init_status {
            forced
        } else {
            let info = le64(frame, frame::init::QUEUE_INFO_LOW);
            let raw = read_bytes(info, frame::init::QI_SIZE);
            let entries = le32(&raw, frame::init::QI_ENTRIES) & 0xffff;
            if entries > u32::from(self.commands) || entries == 0 {
                status::INVALID_PARAMETER
            } else {
                let producer = le64(&raw, frame::init::QI_PRODUCER);
                let head = le32(&read_bytes(producer, 4), 0) % entries;
                self.queue = Some(Queue {
                    flags: le32(&raw, frame::init::QI_FLAGS),
                    entries,
                    queue: le64(&raw, frame::init::QI_QUEUE),
                    producer,
                    consumer: le64(&raw, frame::init::QI_CONSUMER),
                    head,
                });
                self.state = fw::OPERATIONAL;
                status::OK
            }
        };
        write_bytes(at + frame::header::CMD_STATUS as u64, &[answer]);
    }

    /// Carries out the MFI frame at `at` and puts its context on the queue.
    fn run_mfi(&mut self, at: u64, remaining: usize) {
        let frame = read_bytes(at, remaining.min(MFI_FRAME));
        let context = le32(&frame, frame::header::CONTEXT);
        let flags = le16(&frame, frame::header::FLAGS);
        let (code, scsi, sense_len) = match frame[0] {
            cmd::DCMD => {
                let runs = mfi_runs(&frame, frame::dcmd::SGL, flags);
                (self.dcmd(&frame, &runs, flags), 0, 0)
            }
            cmd::LD_SCSI_IO | cmd::PD_SCSI_IO => {
                let seen = Self::note_mfi(&frame);
                let outcome = self.scsi(seen.volume, seen.target, seen.lun, &seen.cdb, &seen.runs);
                let sense = u64::from(le32(&frame, frame::pass::SENSE_LOW))
                    | u64::from(le32(&frame, frame::pass::SENSE_HIGH)) << 32;
                if !outcome.sense.is_empty() {
                    write_bytes(sense, &outcome.sense);
                }
                (outcome.status, outcome.scsi_status, outcome.sense.len() as u8)
            }
            _ => (status::INVALID_CMD, 0, 0),
        };
        if sense_len != 0 {
            write_bytes(at + frame::header::SENSE_LEN as u64, &[sense_len]);
        }
        write_bytes(at + frame::header::CMD_STATUS as u64, &[code, scsi]);
        self.complete_mfi(context);
    }

    fn complete_mfi(&mut self, context: u32) {
        let enabled = self.interrupts_enabled();
        let Some(queue) = self.queue.as_mut() else { return };
        write_bytes(queue.queue + u64::from(queue.head) * 4, &context.to_le_bytes());
        if !enabled {
            return;
        }
        let consumer = le32(&read_bytes(queue.consumer, 4), 0);
        queue.head = (queue.head + 1) % queue.entries;
        if queue.head == consumer {
            self.overruns += 1;
        }
        write_bytes(queue.producer, &queue.head.to_le_bytes());
        self.raise();
    }

    // ---- fusion

    fn post_fusion(&mut self, low: u32, high: u32) {
        self.descriptors.push((low, high));
        if self.state == fw::FAULT {
            return;
        }
        let kind = ((low >> 1) & 0xf) as u8;
        if kind == 1 {
            self.init_fusion(u64::from(high) << 32 | u64::from(low & !0x1f));
            return;
        }
        let (smid, handle) = ((low >> 16) as u16, (high >> 16) as u16);
        if self.ioc.is_none() || self.state != fw::OPERATIONAL {
            self.dropped += 1;
            return;
        }
        if self.holding {
            self.held.push_back(Held::Fusion { smid });
            self.note_fusion(smid, kind, handle);
        } else {
            self.note_fusion(smid, kind, handle);
            self.run_fusion(smid);
        }
    }

    fn init_fusion(&mut self, at: u64) {
        if self.ignore_init {
            return;
        }
        let frame = read_bytes(at, 64);
        self.init_frame = frame.clone();
        let answer = if let Some(forced) = self.init_status {
            forced
        } else if self.state != fw::READY {
            status::WRONG_STATE
        } else if frame[0] != cmd::INIT {
            status::INVALID_CMD
        } else {
            let request = read_bytes(le64(&frame, frame::init::QUEUE_INFO_LOW), IOC_INIT_SIZE);
            let depth = u32::from(le16(&request, 0x1c));
            if request[3] != 0x02 || depth == 0 || le16(&request, 0x1a) != (REQUEST / 4) as u16 {
                status::INVALID_PARAMETER
            } else {
                self.ioc = Some(Ioc {
                    capabilities: le32(&frame, frame::init::CAPABILITIES),
                    depth,
                    requests: le64(&request, 0x28),
                    replies: le64(&request, 0x30),
                    sense_high: le32(&request, 0x20),
                    raw: request,
                });
                self.state = fw::OPERATIONAL;
                status::OK
            }
        };
        write_bytes(at + frame::header::CMD_STATUS as u64, &[answer]);
    }

    /// The frame of request `smid`.
    fn request_at(&self, smid: u16) -> u64 {
        self.ioc.as_ref().unwrap().requests + u64::from(smid) * REQUEST as u64
    }

    /// Reads the scatter-gather list of a request: its entries, chain
    /// elements included, and the runs.
    fn fusion_entries(frame: &[u8]) -> (Vec<Entry>, Vec<(u64, u32)>) {
        let wanted = le32(frame, 0x1c);
        let chain_at = usize::from(frame[2]) * 16;
        let mut entries = Vec::new();
        let mut runs = Vec::new();
        let mut total = 0u32;
        let mut at = SGL_AT;
        while wanted != 0 && at + 16 <= REQUEST && total < wanted {
            let entry = read_entry(&frame[at..at + 16], false);
            entries.push(entry);
            if chain_at != 0 && at == chain_at && entry.flags & 0x80 != 0 {
                let count = entry.len as usize / 16;
                let chain = read_bytes(entry.addr, count * 16);
                for n in 0..count {
                    let inner = read_entry(&chain[n * 16..n * 16 + 16], true);
                    entries.push(inner);
                    runs.push((inner.addr, inner.len));
                    total += inner.len;
                    if inner.flags & 0x40 != 0 || total >= wanted {
                        break;
                    }
                }
                break;
            }
            runs.push((entry.addr, entry.len));
            total += entry.len;
            if entry.flags & 0x40 != 0 {
                break;
            }
            at += 16;
        }
        (entries, runs)
    }

    fn note_fusion(&mut self, smid: u16, kind: u8, handle: u16) {
        let frame = read_bytes(self.request_at(smid), REQUEST);
        let mut seen = Seen { descriptor: (kind, smid, handle), ..Seen::default() };
        if frame[3] == 0xf0 {
            seen.dcmd = true;
        } else {
            let (entries, runs) = Self::fusion_entries(&frame);
            seen.entries = entries;
            seen.runs = runs;
            seen.volume = frame[CONTEXT_AT + 24] & 0x10 == 0;
            seen.target = le16(&frame, CONTEXT_AT + 6);
            seen.lun = frame[0x35];
            let len = usize::from(le16(&frame, 0x24) & 0xff).min(frame::CDB_SIZE);
            seen.cdb = frame[0x40..0x40 + len].to_vec();
        }
        seen.frame = frame;
        self.seen.push(seen);
    }

    fn run_fusion(&mut self, smid: u16) {
        let at = self.request_at(smid);
        let frame = read_bytes(at, REQUEST);
        match frame[3] {
            0xf0 => {
                let element = read_entry(&frame[SGL_AT..SGL_AT + 16], false);
                let mfi = read_bytes(element.addr, (element.len as usize).clamp(64, MFI_FRAME));
                let flags = le16(&mfi, frame::header::FLAGS);
                if matches!(mfi[0], cmd::LD_SCSI_IO | cmd::PD_SCSI_IO) {
                    // A SCSI command in an MFI frame, as a member's goes.
                    let inner = Self::note_mfi(&mfi);
                    if let Some(last) = self.seen.iter_mut().rev().find(|s| s.descriptor.1 == smid)
                    {
                        last.dcmd = false;
                        last.volume = inner.volume;
                        last.target = inner.target;
                        last.lun = inner.lun;
                        last.cdb.clone_from(&inner.cdb);
                        last.runs.clone_from(&inner.runs);
                    }
                    let outcome =
                        self.scsi(inner.volume, inner.target, inner.lun, &inner.cdb, &inner.runs);
                    if !outcome.sense.is_empty() {
                        let sense = u64::from(le32(&mfi, frame::pass::SENSE_LOW))
                            | u64::from(le32(&mfi, frame::pass::SENSE_HIGH)) << 32;
                        write_bytes(sense, &outcome.sense);
                        let len = outcome.sense.len() as u8;
                        write_bytes(element.addr + frame::header::SENSE_LEN as u64, &[len]);
                    }
                    let answer = [outcome.status, outcome.scsi_status];
                    write_bytes(element.addr + frame::header::CMD_STATUS as u64, &answer);
                    self.backlog.push_back((u32::from(smid) << 16, 0));
                    self.flush_replies();
                    return;
                }
                let runs = mfi_runs(&mfi, frame::dcmd::SGL, flags);
                if let Some(last) = self.seen.iter_mut().rev().find(|s| s.descriptor.1 == smid) {
                    last.opcode = le32(&mfi, frame::dcmd::OPCODE);
                    last.runs.clone_from(&runs);
                }
                let code = self.dcmd(&mfi, &runs, flags);
                write_bytes(element.addr + frame::header::CMD_STATUS as u64, &[code]);
            }
            0x00 | 0xf1 => {
                let seen = self
                    .seen
                    .iter()
                    .rev()
                    .find(|s| s.descriptor.1 == smid)
                    .cloned()
                    .unwrap_or_default();
                let outcome = if seen.cdb.is_empty() {
                    Outcome::status(status::INVALID_CMD)
                } else {
                    self.scsi(seen.volume, seen.target, seen.lun, &seen.cdb, &seen.runs)
                };
                if !outcome.sense.is_empty() {
                    let high = self.ioc.as_ref().unwrap().sense_high;
                    let sense = u64::from(le32(&frame, 0x0c)) | u64::from(high) << 32;
                    write_bytes(sense, &outcome.sense);
                }
                let at = at + CONTEXT_AT as u64;
                write_bytes(at + 22, &[outcome.scsi_status, outcome.status]);
            }
            _ => write_bytes(at + (CONTEXT_AT + 23) as u64, &[status::INVALID_CMD]),
        }
        self.backlog.push_back((u32::from(smid) << 16, 0));
        self.flush_replies();
    }

    /// Writes the replies that wait, as far as the ring has room.
    fn flush_replies(&mut self) {
        while self.reply_room() > 0 {
            let Some((low, high)) = self.backlog.pop_front() else { break };
            let ioc = self.ioc.as_ref().unwrap();
            let at = ioc.replies + u64::from(self.fw_index) * 8;
            if read_bytes(at, 8) != [0xff; 8] {
                self.overwrites += 1;
            }
            self.inject_reply(low, high);
        }
    }

    /// Takes what the host wrote to a reply index register.
    fn report(&mut self, offset: usize, value: u32) {
        self.reply_index_writes.push((offset, value));
        let Some(depth) = self.ioc.as_ref().map(|ioc| ioc.depth) else { return };
        if value < depth {
            // The index says only where the host stopped; a whole turn of the
            // ring reads the same, so take as many turns as were written.
            let behind = self.posted_total - self.reported_total;
            let mut read = u64::from((value + depth - self.last_reported) % depth);
            if read <= behind {
                read += (behind - read) / u64::from(depth) * u64::from(depth);
            }
            self.reported_total += read;
            self.last_reported = value;
        }
        self.flush_replies();
    }

    // ---- commands

    /// Carries out a management command in `frame`; its status.
    fn dcmd(&mut self, frame: &[u8], runs: &[(u64, u32)], flags: u16) -> u8 {
        let opcode = le32(frame, frame::dcmd::OPCODE);
        let mut mailbox = [0u8; 12];
        mailbox.copy_from_slice(&frame[frame::dcmd::MAILBOX..frame::dcmd::MAILBOX + 12]);
        let sent = if flags & flag::WRITE != 0 {
            gather(runs, runs.iter().map(|&(_, len)| len as usize).sum())
        } else {
            Vec::new()
        };
        self.dcmds.push(DcmdSeen { opcode, mailbox, sent });
        if let Some(forced) = self.refuse.take() {
            return forced;
        }
        let (code, bytes) = match self.answerer.as_mut() {
            Some(answer) => answer(opcode, &mailbox),
            None => (status::OK, Vec::new()),
        };
        scatter(runs, &bytes);
        code
    }

    /// Carries a SCSI command to its disk.
    fn scsi(
        &mut self,
        volume: bool,
        target: u16,
        lun: u8,
        cdb: &[u8],
        runs: &[(u64, u32)],
    ) -> Outcome {
        if let Some(forced) = self.refuse.take() {
            return Outcome::status(forced);
        }
        let disks = if volume { &mut self.volumes } else { &mut self.disks };
        let Some(disk) = disks.get_mut(&target).filter(|_| lun == 0) else {
            return Outcome::status(status::DEVICE_NOT_FOUND);
        };
        if let Some(sense) = self.sense_next.take() {
            return Outcome::check(sense);
        }
        carry(disk, cdb, runs)
    }
}

/// Carries `cdb` out on `disk`, its data in `runs`.
fn carry(disk: &mut Disk, cdb: &[u8], runs: &[(u64, u32)]) -> Outcome {
    let blocks = disk.blocks() as u64;
    let total: usize = runs.iter().map(|&(_, len)| len as usize).sum();
    let needed = match cdb[0] {
        0x28 | 0x2a => 10,
        0x88 | 0x8a => 14,
        0x12 => 5,
        0x9e => 2,
        _ => 1,
    };
    if cdb.len() < needed {
        return Outcome::check(sense_data(0x05, 0x24, 0));
    }
    match cdb[0] {
        // TEST UNIT READY, SYNCHRONIZE CACHE(10).
        0x00 | 0x35 => Outcome::good(),
        // INQUIRY.
        0x12 => {
            let mut data = std::vec![0u8; 36];
            data[2] = 5;
            data[3] = 2;
            data[4] = 31;
            data[8..16].copy_from_slice(b"RADIXSIM");
            let product = disk.product.as_bytes();
            data[16..32].fill(b' ');
            data[16..16 + product.len()].copy_from_slice(product);
            data[32..36].copy_from_slice(b"0001");
            let asked = usize::from(u16::from_be_bytes([cdb[3], cdb[4]]));
            scatter(runs, &data[..asked.min(36)]);
            Outcome::good()
        }
        // READ CAPACITY(10).
        0x25 => {
            let mut data = [0u8; 8];
            data[..4].copy_from_slice(&((blocks - 1) as u32).to_be_bytes());
            data[4..].copy_from_slice(&512u32.to_be_bytes());
            scatter(runs, &data);
            Outcome::good()
        }
        // READ CAPACITY(16).
        0x9e if cdb[1] & 0x1f == 0x10 => {
            let mut data = [0u8; 32];
            data[..8].copy_from_slice(&(blocks - 1).to_be_bytes());
            data[8..12].copy_from_slice(&512u32.to_be_bytes());
            scatter(runs, &data);
            Outcome::good()
        }
        0x28 | 0x2a | 0x88 | 0x8a => {
            let (lba, count) = if cdb[0] & 0x80 == 0 {
                (
                    u64::from(u32::from_be_bytes(cdb[2..6].try_into().unwrap())),
                    u64::from(u16::from_be_bytes(cdb[7..9].try_into().unwrap())),
                )
            } else {
                (
                    u64::from_be_bytes(cdb[2..10].try_into().unwrap()),
                    u64::from(u32::from_be_bytes(cdb[10..14].try_into().unwrap())),
                )
            };
            if lba + count > blocks {
                return Outcome::check(sense_data(0x05, 0x21, 0));
            }
            let (start, bytes) = (lba as usize * 512, (count as usize * 512).min(total));
            if cdb[0] & 0x02 == 0 {
                scatter(runs, &disk.data[start..start + bytes]);
            } else {
                let data = gather(runs, bytes);
                disk.data[start..start + data.len()].copy_from_slice(&data);
            }
            Outcome::good()
        }
        _ => Outcome::check(sense_data(0x05, 0x20, 0)),
    }
}

/// The runs of an MFI frame's list at `at`, by the frame's `flags`.
fn mfi_runs(frame: &[u8], at: usize, flags: u16) -> Vec<(u64, u32)> {
    let count = usize::from(frame[frame::header::SG_COUNT]);
    let size = if flags & flag::IEEE != 0 {
        16
    } else if flags & flag::SGL64 != 0 {
        12
    } else {
        8
    };
    (0..count)
        .filter(|n| at + (n + 1) * size <= frame.len())
        .map(|n| {
            let entry = at + n * size;
            if size == 8 {
                (u64::from(le32(frame, entry)), le32(frame, entry + 4))
            } else {
                (le64(frame, entry), le32(frame, entry + 8))
            }
        })
        .collect()
}

/// The 16-byte entry in `bytes`.
fn read_entry(bytes: &[u8], in_chain: bool) -> Entry {
    Entry { addr: le64(bytes, 0), len: le32(bytes, 8), flags: bytes[15], in_chain }
}

/// A simulated controller.
pub struct Sim {
    family: Family,
    inner: Mutex<Inner>,
}

impl Sim {
    /// A controller of `family`, in its ready state, with no disks.
    pub fn new(family: Family) -> Arc<Sim> {
        Arc::new(Sim { family, inner: Mutex::new(Inner::new(family)) })
    }

    /// Its family.
    pub fn family(&self) -> Family {
        self.family
    }

    /// Its registers, a window of the size the driver needs.
    pub fn regs(self: &Arc<Sim>) -> Mmio {
        Mmio::fake(self.clone(), REGISTERS)
    }

    /// Its registers, a window of `len` bytes.
    pub fn regs_of(self: &Arc<Sim>, len: usize) -> Mmio {
        Mmio::fake(self.clone(), len)
    }

    /// Its state, to read or set. Never held while the driver runs.
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    /// DMA memory of `pages` pages, which the controller will reach.
    pub fn alloc(&self, pages: usize) -> Option<Dma> {
        let memory = Dma::fake(pages * crate::PAGE);
        self.lock().ranges.push((memory.phys(), memory.len()));
        Some(memory)
    }

    /// Lets `ms` milliseconds pass.
    pub fn advance(&self, ms: u32) {
        let mut inner = self.lock();
        inner.now += u64::from(ms);
        inner.tick();
    }

    /// The simulated time, in milliseconds.
    pub fn now(&self) -> u64 {
        self.lock().now
    }

    /// The `wait` closure of the driver's calls: time passes in it.
    pub fn waiter(self: &Arc<Sim>) -> impl FnMut(u32) + use<> {
        let sim = self.clone();
        move |ms| sim.advance(ms)
    }

    /// Carries out up to `count` commands that were held, in order.
    pub fn release(&self, count: usize) {
        for _ in 0..count {
            let mut inner = self.lock();
            match inner.held.pop_front() {
                Some(Held::Mfi { at, remaining }) => inner.run_mfi(at, remaining),
                Some(Held::Fusion { smid }) => inner.run_fusion(smid),
                None => break,
            }
        }
    }

    /// Carries out every command that was held.
    pub fn release_all(&self) {
        let count = self.lock().held.len();
        self.release(count);
    }
}

impl Registers for Sim {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let mut inner = self.lock();
        match size {
            8 => u64::from(inner.read32(offset)) | u64::from(inner.read32(offset + 4)) << 32,
            4 => u64::from(inner.read32(offset)),
            _ => {
                let word = inner.read32(offset & !3) >> (8 * (offset & 3));
                u64::from(word) & ((1u64 << (8 * size)) - 1)
            }
        }
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        let mut inner = self.lock();
        inner.write32(offset, value as u32);
        if size == 8 {
            inner.write32(offset + 4, (value >> 32) as u32);
        }
    }
}
