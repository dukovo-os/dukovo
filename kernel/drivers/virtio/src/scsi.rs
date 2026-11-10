// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The virtio SCSI host bus adapter (virtio 1.2, section 5.6), as cloud
//! machines (Google's, and QEMU's `virtio-scsi-pci`) attach their disks.
//!
//! The adapter carries SCSI commands to the logical units of its targets.
//! Its queues are a control queue (0), an event queue (1) and one or more
//! request queues (2 on); the driver uses the first request queue alone,
//! with its completions on MSI-X entry `vector`, and sets up neither of
//! the others: it sends no task management functions and takes no
//! hot-plug events, so a disk attached after boot is not seen until the
//! next one (ADR 0327).
//!
//! A command is a chain of buffers: a request the device reads (where it
//! goes, a tag and the command descriptor block), the data the device
//! reads for a write, a response it writes (how the command ended, and
//! the sense data of one that failed), then the data it writes for a read.
//! Requests and responses live in one block of DMA memory, in a slot per
//! descriptor number, as [`crate::blk`] keeps its headers: a chain's head
//! is unique among the chains in flight, so its slot is free. The data are
//! the caller's buffers, by physical address. With indirect descriptors a
//! command takes one entry of the ring and up to
//! [`TABLE_SEGMENTS`](crate::blk::TABLE_SEGMENTS) data buffers.
//!
//! The driver asks for no feature of the adapter's own (bidirectional
//! commands, hot-plug, capacity change events, protection information),
//! only indirect descriptors, event indexes and the platform's IOMMU.

use radix_hw::{Dma, device_barrier};

use crate::blk::{RING_SEGMENTS, TABLE_SEGMENTS};
use crate::queue::{Buffer, Queue};
use crate::{Error, Transport, feature as common_feature, status};

/// The queue commands go on: the first request queue.
pub const REQUEST_QUEUE: u16 = 2;
/// The size of a command descriptor block in a request, which the driver
/// sets: the device's default (5.6.4).
pub const CDB_SIZE: usize = 32;
/// The most sense data a response holds, which the driver sets: the
/// device's default.
pub const SENSE_SIZE: usize = 96;
/// The highest target the driver looks for: the adapter may say more,
/// up to 65,535, but a request names a target in one byte.
pub const MAX_TARGET: u16 = 255;
/// The highest logical unit number a request names: 14 bits, in flat
/// space addressing (5.6.6.1).
pub const MAX_LUN: u16 = 0x3fff;

/// How long a request is: the logical unit's address (8), the tag (8),
/// the task attribute, priority and command reference number (3), and the
/// command descriptor block.
const REQUEST: usize = 19 + CDB_SIZE;
/// How long a response is: the sense data's length (4), the residual
/// (4), the status qualifier (2), the status and the response (2), and the
/// sense data.
const RESPONSE: usize = 12 + SENSE_SIZE;
/// The room for a request in the slots, and for a response.
const REQUEST_SLOT: usize = 64;
const RESPONSE_SLOT: usize = 128;

/// Where the device configuration's fields are (5.6.4).
mod config {
    pub const SEG_MAX: usize = 4;
    pub const MAX_SECTORS: usize = 8;
    pub const SENSE_SIZE: usize = 20;
    pub const CDB_SIZE: usize = 24;
    pub const MAX_TARGET: usize = 30;
    pub const MAX_LUN: usize = 32;
}

/// How the adapter says a command went, before the logical unit's
/// status (5.6.6.1): it delivered the command, or why not.
pub mod response {
    /// The command reached the logical unit, whose status says how it
    /// ended.
    pub const OK: u8 = 0;
    /// The data were longer than the buffers.
    pub const OVERRUN: u8 = 1;
    /// A task management function aborted the command.
    pub const ABORTED: u8 = 2;
    /// No such target, or no such logical unit.
    pub const BAD_TARGET: u8 = 3;
    /// The command was dropped by a reset.
    pub const RESET: u8 = 4;
    /// The adapter is busy: the command may go again.
    pub const BUSY: u8 = 5;
    /// The path to the target failed: another path may work.
    pub const TRANSPORT_FAILURE: u8 = 6;
    /// The target failed: the command should not go again.
    pub const TARGET_FAILURE: u8 = 7;
    /// The initiator and target are not connected.
    pub const NEXUS_FAILURE: u8 = 8;
    /// Anything else.
    pub const FAILURE: u8 = 9;
}

/// The SCSI statuses the logical unit ends a command with (SAM-6, 5.3).
pub mod scsi_status {
    /// It worked.
    pub const GOOD: u8 = 0x00;
    /// It failed, and the sense data say why.
    pub const CHECK_CONDITION: u8 = 0x02;
    /// The logical unit is busy: the command may go again.
    pub const BUSY: u8 = 0x08;
    /// Its queue is full: the command may go again.
    pub const TASK_SET_FULL: u8 = 0x28;
}

/// A logical unit of a target of the adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    /// The target, from 0 to [`Info::max_target`].
    pub target: u8,
    /// The logical unit, from 0 to [`Info::max_lun`].
    pub lun: u16,
}

impl Address {
    /// The 8-byte address a request gives (5.6.6.1): 1, the target, and
    /// the logical unit in SAM's flat space addressing, as Linux writes it.
    fn bytes(self) -> [u8; 8] {
        let lun = self.lun & MAX_LUN;
        [1, self.target, 0x40 | (lun >> 8) as u8, lun as u8, 0, 0, 0, 0]
    }
}

/// Which way a command's data go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From memory to the device: a write.
    ToDevice,
    /// From the device into memory: a read, or what a command answers.
    FromDevice,
}

/// How a command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// What the adapter says of it ([`response`]).
    pub response: u8,
    /// The logical unit's status ([`scsi_status`]), if it reached one.
    pub status: u8,
    /// How many bytes of its data did not move.
    pub residual: u32,
    /// The first bytes of the sense data of a command that failed: as
    /// many as [`Answer::sense_len`] says, at most 32, which hold every
    /// field the kernel reads.
    pub sense: [u8; 32],
    /// How many bytes of [`Answer::sense`] the device wrote.
    pub sense_len: u8,
}

impl Answer {
    /// The sense data the device wrote.
    pub fn sense(&self) -> &[u8] {
        &self.sense[..usize::from(self.sense_len)]
    }

    /// Whether the command worked, and moved all its data.
    pub fn good(&self) -> bool {
        self.response == response::OK && self.status == scsi_status::GOOD && self.residual == 0
    }
}

/// What the adapter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// The highest target number, at most [`MAX_TARGET`].
    pub max_target: u8,
    /// The highest logical unit number, at most [`MAX_LUN`].
    pub max_lun: u16,
    /// The most data buffers in one command: the adapter's limit, but at
    /// most [`TABLE_SEGMENTS`] or [`RING_SEGMENTS`].
    pub max_segments: u32,
    /// The most 512-byte sectors one command moves (0 for no limit).
    pub max_sectors: u32,
}

/// A virtio SCSI adapter, set up and running.
pub struct Scsi {
    transport: Transport,
    queue: Queue,
    slots: Dma,
    info: Info,
}

impl Scsi {
    /// Sets the adapter up: resets it, agrees on features, sets the sizes
    /// of command descriptor blocks and sense data, reads its
    /// configuration, sets up its first request queue with completions on
    /// MSI-X entry `vector`, and tells it the driver is ready. `requests`
    /// is the most commands the caller has in flight at once, at most
    /// [`MAX_TABLES`](crate::queue::MAX_TABLES): with indirect
    /// descriptors the queue keeps a table for each. `alloc` gives zeroed
    /// DMA memory, in pages.
    pub fn new(
        transport: Transport,
        vector: u16,
        requests: usize,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    ) -> Result<Scsi, Error> {
        if !transport.reset() {
            return Err(Error::DeviceFailed);
        }
        transport.add_status(status::ACKNOWLEDGE);
        transport.add_status(status::DRIVER);
        let offered = transport.device_features();
        if offered & common_feature::VERSION_1 == 0 {
            transport.add_status(status::FAILED);
            return Err(Error::MissingFeature("virtio 1.0"));
        }
        let common = common_feature::INDIRECT_DESC
            | common_feature::EVENT_IDX
            | common_feature::ACCESS_PLATFORM;
        let features = common_feature::VERSION_1 | (offered & common);
        if let Err(err) = transport.set_driver_features(features) {
            transport.add_status(status::FAILED);
            return Err(err);
        }
        // The queues are numbered from the control queue: a request queue
        // is the third.
        if transport.queues() <= REQUEST_QUEUE {
            transport.add_status(status::FAILED);
            return Err(Error::NoQueue);
        }
        transport.no_config_interrupt();
        // The sizes are the device's defaults, set again in case a driver
        // before this one changed them: a reset restores them anyway.
        let device = transport.device_config();
        device.write32(config::SENSE_SIZE, SENSE_SIZE as u32);
        device.write32(config::CDB_SIZE, CDB_SIZE as u32);
        let info = transport.read_config(|space| {
            let seg_max = space.read32(config::SEG_MAX);
            let max_sectors = space.read32(config::MAX_SECTORS);
            let max_target = space.read16(config::MAX_TARGET);
            let max_lun = space.read32(config::MAX_LUN);
            Info {
                max_target: max_target.min(MAX_TARGET) as u8,
                max_lun: max_lun.min(u32::from(MAX_LUN)) as u16,
                max_segments: if seg_max == 0 { u32::MAX } else { seg_max },
                max_sectors,
            }
        });
        // A table holds a command's request, its buffers and its response.
        let tables = (requests, TABLE_SEGMENTS as u16 + 2);
        let size = crate::queue::MAX_SIZE;
        let queue =
            match Queue::new(&transport, REQUEST_QUEUE, size, vector, features, tables, alloc) {
                Ok(queue) => queue,
                Err(err) => {
                    transport.add_status(status::FAILED);
                    return Err(err);
                }
            };
        // A command takes three descriptors at least: its request, a
        // buffer and its response.
        if queue.size() < 3 {
            transport.add_status(status::FAILED);
            return Err(Error::NoQueue);
        }
        let size = usize::from(queue.size());
        let Some(slots) = alloc((size * (REQUEST_SLOT + RESPONSE_SLOT)).div_ceil(4096)) else {
            transport.reset();
            return Err(Error::NoMemory);
        };
        let most = match queue.table_len() {
            0 => RING_SEGMENTS,
            len => u32::from(len) - 2,
        };
        let max_segments = info.max_segments.min(u32::from(queue.size()) - 2).min(most);
        let info = Info { max_segments, ..info };
        transport.add_status(status::DRIVER_OK);
        Ok(Scsi { transport, queue, slots, info })
    }

    /// What the adapter is.
    pub fn info(&self) -> Info {
        self.info
    }

    /// Whether the device asks to be reset after an error.
    pub fn failed(&self) -> bool {
        self.transport.status() & status::NEEDS_RESET != 0
    }

    /// How many more commands of `segments` data buffers fit in the queue.
    pub fn room(&self, segments: usize) -> usize {
        self.queue.room(segments + 2)
    }

    /// Adds the command `cdb` for the logical unit `at` to the queue, with
    /// `data` buffers (physical address, length) that go `direction`,
    /// none for a command that moves nothing, without ringing the device's
    /// doorbell: [`Scsi::notify`] does, for every command added meanwhile.
    /// Returns its token, which [`Scsi::complete`] gives back. The buffers
    /// must stay the command's alone until then.
    pub fn submit_held(
        &mut self,
        at: Address,
        cdb: &[u8],
        data: &[(u64, u32)],
        direction: Direction,
    ) -> Result<u16, Error> {
        let bytes: u64 = data.iter().map(|&(_, len)| u64::from(len)).sum();
        let valid = !cdb.is_empty()
            && cdb.len() <= CDB_SIZE
            && at.target <= self.info.max_target
            && at.lun <= self.info.max_lun
            && data.len() <= self.info.max_segments as usize
            && data.iter().all(|&(_, len)| len > 0)
            && (self.info.max_sectors == 0
                || bytes <= u64::from(self.info.max_sectors) * crate::blk::SECTOR);
        if !valid {
            return Err(Error::BadRequest);
        }
        let buffers = data.len() + 2;
        if self.queue.room(buffers) == 0 {
            return Err(Error::Full);
        }
        // The head is the next free descriptor, whose slot is free: fill
        // in the request and clear the response before handing them over.
        let head = self.queue.next_free();
        let request = usize::from(head) * REQUEST_SLOT;
        let answer = self.response_at(head);
        let mut bytes = [0u8; REQUEST];
        bytes[..8].copy_from_slice(&at.bytes());
        // The tag is the head, which no other command in flight has. The
        // task attribute (simple), priority and reference number are 0.
        bytes[8..16].copy_from_slice(&u64::from(head).to_le_bytes());
        bytes[19..19 + cdb.len()].copy_from_slice(cdb);
        self.slots.write(request, &bytes);
        self.slots.write(answer, &[0; 12]);
        // The adapter's own failure, until it writes how the command went.
        self.slots.write8(answer + 11, response::FAILURE);
        let slots = self.slots.phys();
        let request =
            Buffer { phys: slots + request as u64, len: REQUEST as u32, device_writes: false };
        let response =
            Buffer { phys: slots + answer as u64, len: RESPONSE as u32, device_writes: true };
        // What the device reads, then what it writes: the request, data
        // out, the response, data in.
        let into_host = direction == Direction::FromDevice;
        let chain = |i: usize| {
            let data_at = |n: usize| {
                let (phys, len) = data[n];
                Buffer { phys, len, device_writes: into_host }
            };
            match (i, into_host) {
                (0, _) => request,
                (1, true) => response,
                (i, true) => data_at(i - 2),
                (i, false) if i == buffers - 1 => response,
                (i, false) => data_at(i - 1),
            }
        };
        let added = self.queue.add(buffers, chain)?;
        debug_assert_eq!(added, head);
        Ok(added)
    }

    /// Rings the device's doorbell for the commands added since it last
    /// rang, unless the device said it will look at them anyway.
    pub fn notify(&mut self) {
        self.queue.notify();
    }

    /// Where the response of the chain at `head` is in the slots: after
    /// the requests.
    fn response_at(&self, head: u16) -> usize {
        usize::from(self.queue.size()) * REQUEST_SLOT + usize::from(head) * RESPONSE_SLOT
    }

    /// Stops the device's interrupts while `quiet`, for a caller that
    /// polls [`Scsi::complete`]; starts them again when not, and then the
    /// next call of [`Scsi::complete`] that finds nothing asks for one
    /// ([`Queue::set_quiet`]).
    pub fn set_quiet(&mut self, quiet: bool) {
        self.queue.set_quiet(quiet);
    }

    /// The next finished command: its token and how it ended.
    pub fn complete(&mut self) -> Option<(u16, Answer)> {
        let (head, _) = self.queue.used()?;
        device_barrier();
        let at = self.response_at(head);
        let mut fields = [0u8; 12];
        self.slots.read(at, &mut fields);
        let sense_len = u32::from_le_bytes([fields[0], fields[1], fields[2], fields[3]]);
        let residual = u32::from_le_bytes([fields[4], fields[5], fields[6], fields[7]]);
        let mut answer = Answer {
            response: fields[11],
            status: fields[10],
            residual,
            sense: [0; 32],
            sense_len: sense_len.min(32) as u8,
        };
        let len = usize::from(answer.sense_len);
        self.slots.read(at + 12, &mut answer.sense[..len]);
        Some((head, answer))
    }

    /// Resets the device: it stops using every buffer it was given.
    pub fn reset(&self) {
        self.transport.reset();
    }

    /// How many completions the device gave that named no command in
    /// flight, and were skipped: a device that misbehaves.
    pub fn bad_entries(&self) -> u32 {
        self.queue.bad_entries()
    }
}

impl Drop for Scsi {
    /// Resets the device before its queue's memory and the requests are
    /// given back, so that it writes to none of them after.
    fn drop(&mut self) {
        self.transport.reset();
    }
}
