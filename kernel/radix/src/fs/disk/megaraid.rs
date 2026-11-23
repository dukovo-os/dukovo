// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Broadcom MegaRAID RAID controllers (`radix_megaraid`, ADR 0664): Dell's
//! PERC H330 to H755 and PERC 6 to H800, Lenovo's and Broadcom's own, and
//! QEMU's `megasas` and `megasas-gen2`. Each volume the controller's
//! firmware builds, and each disk it passes through to the host (JBOD),
//! takes the first `sd` letter free, as a SATA disk does.
//!
//! [`bind`] maps the controller's registers, gives it one interrupt
//! (MSI-X entry 0, or MSI) and brings its firmware up. Then, its
//! interrupts on (QEMU's models move their reply queue only so) but the
//! handler not yet taking its replies, it asks the firmware what it is
//! and for its volumes and disks, one management command at a time,
//! polled, and asks each volume and each pass-through disk what it is and
//! how big (`radix_scsi::probe`). A controller with no volume and no
//! pass-through disk is kept: its disks are shown in `/kernel/raid` all
//! the same.
//!
//! The disks share the controller's commands, as the PVSCSI adapter's do
//! ([`super::pvscsi`]): each disk's part of the block queue ([`RaidDisk`])
//! is a handle on the controller ([`Adapter`]), which notes whose each
//! command is. Two commands are kept from the disks: one for a management
//! command and one for the wait on the controller's next event.
//! Management commands go one at a time ([`Adapter::run`]), their answers
//! in the controller's own 64 KiB of DMA memory, waited for in 10 ms steps
//! that each take the replies the interrupt has not, so that they work
//! without it, as at shutdown.
//!
//! The controller's thread (`megaraid<n>`) reads the controller's log of
//! events, the last 256, then waits on each next one: a volume or disk
//! that changes state, a rebuild that moves, a battery that charges. Once
//! the events stop for half a second, and every 30 seconds while a task
//! (a rebuild, a check) runs, it asks the controller again what it has
//! ([`radix_megaraid::report`]), for `/kernel/raid/<n>` (radix-sysfs(5)).
//! A volume or pass-through disk added while the machine runs is logged,
//! and not seen until the next boot. `health/<id>` asks a physical disk,
//! in a volume or not, for its SMART data or log pages, through the
//! firmware's pass-through ([`Adapter::health`]).
//!
//! A volume's write cache is the controller's: a firmware that does not
//! take SYNCHRONIZE CACHE for a volume keeps its cache with a battery or
//! flash, and the volume is said to have none, as Linux answers the
//! command itself. Before the machine is powered off, [`shutdown`] has
//! the controller write its cache and the disks' out, and shut down.
//!
//! A command that ends busy, short or for a reason the firmware may get
//! over goes again (the queue's tries).

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use radix_abi::block as request;
use radix_block::{DeviceRef, Error, Health, Info, Page, SECTOR};
use radix_hal::Devices;
use radix_hw::Dma;
use radix_hw::pci::{Bar, Function, Msi, MsiX};
use radix_klog::{TextBuf, info, warn};
use radix_megaraid::dcmd::{
    Class, CtrlInfo, EventDetail, EventLogState, LdList, PdInfo, PdList, event, locale, mbox,
    opcode,
};
use radix_megaraid::frame::status;
use radix_megaraid::regs::{REGISTERS, State};
use radix_megaraid::report::{self, Files, Place, Summary};
use radix_megaraid::{Answer, CONTEXTS, Direction, Family, MegaRaid, PAGE, Target};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_scsi::health::{self as queries, Failed, Query, RECORD};
use radix_scsi::{
    Absent, Cdb, Data, Found, Meaning, Reply, Sense, Transport as Commands, TransportError,
    device_type, probe,
};
use radix_sync::{Mutex, Once, SpinLock};
use radix_vfs::{Error as FileError, Text};

use super::queue::{self, Controller, Disk, Op, Outcome, Request, SLOTS, Volume};
use super::scsi_health::{self, ATA_VENDOR, query_cdb};
use super::virtio_scsi::{ident, serial_number};
use super::{Entry, Name, SERIAL, Size, partitions, register};
use crate::Arch;
use crate::sched::{self, Options, Policy};

/// The most controllers.
const MAX_CONTROLLERS: usize = 4;
/// The most volumes and pass-through disks of one controller.
const MAX_DISKS: usize = 32;
/// How long a probe's command is polled for before the thread sleeps
/// between looks.
const SPIN_NS: u64 = 1_000_000;
/// How long a management command of the scan may take.
const SCAN_TIMEOUT_MS: u32 = 10_000;
/// How long a management command may take: a large cache takes a while
/// to write out.
const MANAGE_TIMEOUT_MS: u64 = 60_000;
/// How long a health query of a disk may take.
const QUERY_TIMEOUT_MS: u64 = 10_000;
/// How often a waiter for a management command takes the replies itself.
const STEP_NS: u64 = 10_000_000;
/// The bytes of the management commands' memory, as much as the report's
/// answers take at once.
const MANAGE_BYTES: usize = report::SCRATCH;
/// The events the `events` file keeps.
const EVENTS: usize = 256;
/// How long after an event the report is gathered, so that a burst of
/// them is gathered once.
const SETTLE_NS: u64 = 500_000_000;
/// How often the report is gathered while a task runs: its progress moves
/// without an event.
const TASK_REFRESH_NS: u64 = 30_000_000_000;
/// How often it is gathered otherwise, in case an event was missed.
const IDLE_REFRESH_NS: u64 = 300_000_000_000;
/// How long the thread waits before it asks for events again after the
/// controller refused the wait.
const EVENT_RETRY_NS: u64 = 10_000_000_000;
/// The event waits refused in a row before the thread stops asking.
const EVENT_TRIES: u32 = 5;
/// The bytes of a health record as GET_HEALTH writes it: its kind and
/// flags, then the record.
pub const HEALTH_SIZE: usize = 8 + RECORD;
const _: () = assert!(request::HEALTH_SIZE as usize == HEALTH_SIZE);
/// SCSI statuses the disks end commands with.
const BUSY: u8 = 0x08;
const TASK_SET_FULL: u8 = 0x28;
const CHECK_CONDITION: u8 = 0x02;

/// Whose a command in flight is.
#[derive(Clone, Copy)]
enum Owner {
    /// Nobody's.
    None,
    /// A disk's request.
    Disk {
        disk: u8,
        slot: u8,
        /// It is a flush, which a disk with no cache to flush may refuse.
        flush: bool,
        /// It asks a health question, which a disk may refuse or answer
        /// in fewer bytes than the buffer holds.
        query: bool,
    },
    /// The management command.
    Manage,
    /// The wait on the next event.
    Event,
}

/// A controller's commands and what its disks have to collect.
struct Shared {
    raid: MegaRaid,
    /// Whose each context in flight is.
    owner: Vec<Owner, Kmalloc>,
    /// The slots of each disk whose commands ended, not yet collected.
    ended: [u64; MAX_DISKS],
    /// How the command in each disk's slot ended, and why, for the log: at
    /// `disk * SLOTS + slot`.
    outcome: Vec<(Outcome, Option<&'static str>), Kmalloc>,
    /// The management command in flight, if one is, and how the last one
    /// ended, until its waiter takes it.
    manage: Option<u16>,
    managed: Option<Answer>,
    /// The event wait in flight, if one is, and how the last one ended.
    event: Option<u16>,
    evented: Option<Answer>,
}

impl Shared {
    /// How many commands the disks may start: those free, less the ones
    /// kept for a management command and an event wait not in flight.
    fn disk_room(&self) -> usize {
        let kept = usize::from(self.manage.is_none()) + usize::from(self.event.is_none());
        self.raid.room().saturating_sub(kept)
    }

    /// Takes every reply: a disk's into its mailbox, the management
    /// command's and the event wait's into theirs. Returns the disks that
    /// got one, a bit each, and whether those two ended.
    fn drain(&mut self) -> (u32, bool, bool) {
        let (mut mail, mut managed, mut evented) = (0, false, false);
        while let Some((context, answer)) = self.raid.complete() {
            let Some(owner) = self.owner.get_mut(usize::from(context)) else { continue };
            match core::mem::replace(owner, Owner::None) {
                Owner::None => {}
                Owner::Disk { disk, slot, flush, query } => {
                    let at = usize::from(disk) * SLOTS + usize::from(slot);
                    if let Some(outcome) = self.outcome.get_mut(at) {
                        *outcome = judge(&answer, flush, query);
                        self.ended[usize::from(disk)] |= 1 << slot;
                        mail |= 1 << disk;
                    }
                }
                Owner::Manage if self.manage == Some(context) => {
                    self.manage = None;
                    self.managed = Some(answer);
                    managed = true;
                }
                Owner::Event if self.event == Some(context) => {
                    self.event = None;
                    self.evented = Some(answer);
                    evented = true;
                }
                Owner::Manage | Owner::Event => {}
            }
        }
        (mail, managed, evented)
    }

    /// The next of disk `disk`'s commands that ended: its slot, and how.
    fn take(&mut self, disk: u8) -> Option<(u8, Outcome)> {
        let ended = self.ended.get_mut(usize::from(disk))?;
        if *ended == 0 {
            return None;
        }
        let slot = ended.trailing_zeros() as u8;
        *ended &= *ended - 1;
        let (outcome, _) = self.outcome[usize::from(disk) * SLOTS + usize::from(slot)];
        Some((slot, outcome))
    }
}

/// The management channel: its commands' memory, and the room the report
/// holds the answers in.
struct Manage {
    /// [`MANAGE_BYTES`] of DMA memory below 4 GiB.
    memory: Dma,
    /// [`report::SCRATCH`] bytes on the heap.
    scratch: Vec<u8, Kmalloc>,
}

/// What the controller said it has, the last time it was asked: the text
/// of `/kernel/raid/<n>`'s files.
struct Report {
    controller: Text,
    volumes: Text,
    disks: Text,
    battery: Text,
    summary: Summary,
    /// It has been gathered once.
    gathered: bool,
}

/// The events the controller sent, the last [`EVENTS`], as it wrote them.
struct Events {
    details: Vec<[u8; EventDetail::LEN], Kmalloc>,
    /// Where the next goes, once the ring is full.
    next: usize,
}

impl Events {
    /// Keeps `detail`, dropping the oldest if the ring is full.
    fn keep(&mut self, detail: &[u8; EventDetail::LEN]) {
        if self.details.len() < EVENTS {
            let _ = self.details.push_within_capacity(*detail);
        } else if let Some(slot) = self.details.get_mut(self.next) {
            *slot = *detail;
            self.next = (self.next + 1) % EVENTS;
        }
    }

    /// The events, oldest first.
    fn oldest_first(&self) -> impl Iterator<Item = &[u8; EventDetail::LEN]> {
        let (newer, older) = self.details.split_at(self.next.min(self.details.len()));
        older.iter().chain(newer.iter())
    }
}

/// A MegaRAID controller that its disks share.
pub struct Adapter {
    shared: SpinLock<Shared>,
    /// Which controller it is, from 0.
    index: usize,
    /// Where it is, for the log and the report.
    place: radix_hw::pci::Address,
    family: Family,
    /// How many commands it keeps in flight.
    commands: usize,
    /// Set when the management command ended, and when the event wait did.
    managed: AtomicBool,
    evented: AtomicBool,
    manage: Mutex<Manage>,
    /// The event wait's answer: a page of DMA memory below 4 GiB.
    event_page: Dma,
    report: Mutex<Report>,
    events: Mutex<Events>,
    /// When the report was last gathered, by the scheduler's clock.
    gathered_at: AtomicU64,
    /// The names its volumes and pass-through disks took.
    names: Once<Vec<(Target, Name), Kmalloc>>,
}

impl Adapter {
    /// The key a waiter for the management command sleeps on.
    fn manage_key(&self) -> usize {
        core::ptr::from_ref(&self.managed) as usize
    }

    /// The key the thread sleeps on for the next event.
    fn event_key(&self) -> usize {
        core::ptr::from_ref(&self.evented) as usize
    }

    /// Takes the replies the controller has and gives them out.
    fn service(&self) {
        let (mail, managed, evented) = self.shared.lock_irq().drain();
        self.deliver(mail, managed, evented);
    }

    /// Wakes whoever the replies just taken were for: the disks with mail
    /// (`mail`, a bit each), and the management command's and the event
    /// wait's waiters.
    fn deliver(&self, mail: u32, managed: bool, evented: bool) {
        if managed {
            self.managed.store(true, Ordering::Release);
            sched::wait::wake_all(self.manage_key());
        }
        if evented {
            self.evented.store(true, Ordering::Release);
            sched::wait::wake_all(self.event_key());
        }
        if mail == 0 {
            return;
        }
        let Some(disks) = DISKS.get(self.index) else { return };
        for (number, disk) in disks.iter().enumerate() {
            let Some(disk) = disk.get() else { continue };
            if mail & (1 << number) != 0 {
                disk.collect(0);
            } else {
                // Room on the commands the others shared.
                disk.wake_room();
            }
        }
    }

    /// Sends a command through the management channel and waits for it,
    /// at most `timeout_ms`: `send` puts it on the controller, its data
    /// `len` bytes of `memory`, the channel's. Fails if the controller
    /// does not take it or does not answer; a command that did not end
    /// keeps the channel until it does, as the controller may still write
    /// to the memory.
    fn run(
        &self,
        memory: &Dma,
        len: usize,
        timeout_ms: u64,
        send: impl FnOnce(&mut MegaRaid, &[(u64, u32)]) -> Result<u16, radix_megaraid::Error>,
    ) -> Result<Answer, &'static str> {
        let len = len.min(memory.len());
        // An answer shorter than the room leaves nothing of the last one.
        zero(memory, len);
        let whole = [(memory.phys(), len as u32)];
        let runs: &[(u64, u32)] = if len == 0 { &[] } else { &whole };
        {
            let mut shared = self.shared.lock_irq();
            if shared.manage.is_some() {
                return Err("an earlier management command has not ended");
            }
            shared.managed = None;
            self.managed.store(false, Ordering::Release);
            let context =
                send(&mut shared.raid, runs).map_err(|_| "the controller refused the command")?;
            if let Some(owner) = shared.owner.get_mut(usize::from(context)) {
                *owner = Owner::Manage;
            }
            shared.manage = Some(context);
        }
        let deadline = sched::now_ns().saturating_add(timeout_ms.saturating_mul(1_000_000));
        loop {
            let step = sched::now_ns().saturating_add(STEP_NS).min(deadline);
            sched::wait::wait_until_or_timeout(self.manage_key(), "megaraid", step, || {
                self.managed.load(Ordering::Acquire)
            });
            self.service();
            {
                let mut shared = self.shared.lock_irq();
                if let Some(answer) = shared.managed.take() {
                    return Ok(answer);
                }
                if shared.raid.status().state() == State::Fault {
                    return Err("its firmware faulted");
                }
            }
            if sched::now_ns() >= deadline {
                return Err("it did not answer a management command");
            }
        }
    }

    /// Runs the management command `opcode` with `mailbox`, its answer
    /// `len` bytes at most, into the channel's memory. Returns how many
    /// bytes it answered with; `None` if it failed.
    fn dcmd(&self, manage: &Manage, opcode: u32, mailbox: &[u8; 12], len: usize) -> Option<usize> {
        let direction = if len == 0 { Direction::None } else { Direction::FromDevice };
        let answer = self
            .run(&manage.memory, len, MANAGE_TIMEOUT_MS, |raid, runs| {
                raid.submit_dcmd(opcode, mailbox, runs, direction)
            })
            .ok()?;
        answer.good().then(|| (answer.moved as usize).min(len))
    }

    /// Asks the controller what it has, and keeps its answers as the
    /// text of the files. Keeps the last text if it fails.
    fn gather(&self) {
        let mut manage = self.manage.lock();
        let Manage { memory, scratch } = &mut *manage;
        let mut pci = TextBuf::<16>::new();
        let _ = write!(pci, "{}", self.place);
        let place = Place {
            interface: if self.family.fusion() { "fusion" } else { "mfi" },
            chip: self.family.name(),
            pci: pci.as_str(),
            numa: numa_of(self.place).map(u32::from),
            commands: self.commands,
        };
        let names = self.names.get();
        let os = |target: Target, out: &mut dyn Write| -> Result<bool, fmt::Error> {
            let found = names.and_then(|names| names.iter().find(|(t, _)| *t == target));
            match found {
                Some((_, name)) => out.write_str(name.as_str()).map(|()| true),
                None => Ok(false),
            }
        };
        let memory: &Dma = memory;
        let mut fetch = |opcode: u32, mailbox: &[u8; 12], out: &mut [u8]| {
            let len = out.len().min(memory.len());
            let answer = self
                .run(memory, len, MANAGE_TIMEOUT_MS, |raid, runs| {
                    raid.submit_dcmd(opcode, mailbox, runs, Direction::FromDevice)
                })
                .map_err(|_| report::Failed)?;
            if !answer.good() {
                return Err(report::Failed);
            }
            memory.read(0, &mut out[..len]);
            Ok((answer.moved as usize).min(len))
        };
        let (mut controller, mut volumes, mut disks, mut battery) =
            (Text::new(), Text::new(), Text::new(), Text::new());
        let mut files = Files {
            controller: &mut controller,
            volumes: &mut volumes,
            disks: &mut disks,
            battery: &mut battery,
        };
        let gathered = report::gather(&mut fetch, scratch, &place, &os, &mut files);
        drop(manage);
        self.gathered_at.store(sched::now_ns(), Ordering::Relaxed);
        let short = [&controller, &volumes, &disks, &battery].iter().any(|text| text.failed());
        match gathered {
            Ok(summary) if !short => {
                let mut report = self.report.lock();
                *report = Report { controller, volumes, disks, battery, summary, gathered: true };
            }
            Ok(_) => {
                warn!("Disk: MegaRAID controller at {}: out of memory for its report", self.place);
            }
            Err(report::Failed) => {
                warn!("Disk: MegaRAID controller at {}: it did not say what it has", self.place);
            }
        }
    }

    /// Whether a volume or disk had a task running when it was last asked.
    fn task(&self) -> bool {
        self.report.lock().summary.task
    }

    /// Reads the controller's log, its last [`EVENTS`] events of
    /// information or worse, into the `events` file. Returns the sequence
    /// number of the next event, or `None` if it keeps no log.
    fn read_history(&self) -> Option<u32> {
        let manage = self.manage.lock();
        let len = self.dcmd(&manage, opcode::CTRL_EVENT_GET_INFO, &[0; 12], EventLogState::LEN)?;
        let mut bytes = [0u8; EventLogState::LEN];
        manage.memory.read(0, &mut bytes[..len]);
        let state = EventLogState::parse(&bytes).ok()?;
        let mut seq = state.newest.saturating_sub(EVENTS as u32 - 1).max(state.oldest);
        let class = Class::Info.as_i8();
        for _ in 0..EVENTS {
            if state.newest == 0 || seq > state.newest {
                break;
            }
            let mailbox = mbox::event(seq, locale::ALL, class);
            let Some(len) = self.dcmd(&manage, opcode::CTRL_EVENT_GET, &mailbox, EventDetail::LEN)
            else {
                break;
            };
            let mut detail = [0u8; EventDetail::LEN];
            manage.memory.read(0, &mut detail[..len]);
            let Ok(parsed) = EventDetail::parse(&detail) else { break };
            // QEMU's model answers with no event; a firmware that went
            // back is not followed.
            if parsed.sequence() < seq || parsed.sequence() == 0 {
                break;
            }
            seq = parsed.sequence().wrapping_add(1);
            self.events.lock().keep(&detail);
        }
        Some(state.newest.wrapping_add(1))
    }

    /// Asks the controller to answer when event `seq` or a later one of
    /// information or worse comes. Whether it took the wait.
    fn wait_event(&self, seq: u32) -> bool {
        let mailbox = mbox::event(seq, locale::ALL, Class::Info.as_i8());
        let runs = [(self.event_page.phys(), EventDetail::LEN as u32)];
        zero(&self.event_page, EventDetail::LEN);
        let mut shared = self.shared.lock_irq();
        if shared.event.is_some() {
            return true;
        }
        shared.evented = None;
        self.evented.store(false, Ordering::Release);
        let sent = shared.raid.submit_dcmd(
            opcode::CTRL_EVENT_WAIT,
            &mailbox,
            &runs,
            Direction::FromDevice,
        );
        let Ok(context) = sent else { return false };
        if let Some(owner) = shared.owner.get_mut(usize::from(context)) {
            *owner = Owner::Event;
        }
        shared.event = Some(context);
        true
    }

    /// How the event wait ended, if it has.
    fn take_event(&self) -> Option<Answer> {
        self.evented.store(false, Ordering::Release);
        self.shared.lock_irq().evented.take()
    }

    /// Keeps the event in the wait's memory, and logs it if it is a
    /// warning or worse, or changes what the host sees. Returns its
    /// sequence number.
    fn note_event(&self) -> Option<u32> {
        let mut detail = [0u8; EventDetail::LEN];
        self.event_page.read(0, &mut detail);
        radix_hw::device_wrote(&mut detail);
        let parsed = EventDetail::parse(&detail).ok()?;
        let place = self.place;
        let text = parsed.description();
        match parsed.code() {
            event::LD_CREATED
            | event::LD_DELETED
            | event::PD_INSERTED
            | event::FOREIGN_CFG_IMPORTED
            | event::CFG_CLEARED => info!(
                "Disk: MegaRAID controller at {place}: {text}; a volume or pass-through disk it \
                 adds or takes away is not seen until the next boot"
            ),
            _ => match parsed.class() {
                Class::Warning | Class::Critical | Class::Fatal | Class::Dead => {
                    warn!("Disk: MegaRAID controller at {place}: {text}");
                }
                _ => {}
            },
        }
        self.events.lock().keep(&detail);
        Some(parsed.sequence())
    }

    /// Writes the `controller` file: what the controller said, then
    /// whether its firmware runs now.
    pub fn write_controller(&self, text: &mut Text) -> fmt::Result {
        {
            let report = self.report.lock();
            if report.gathered {
                text.write_bytes(report.controller.bytes())?;
            } else {
                let interface = if self.family.fusion() { "fusion" } else { "mfi" };
                writeln!(text, "interface {interface}")?;
                writeln!(text, "chip {}", self.family.name())?;
                writeln!(text, "pci {}", self.place)?;
            }
        }
        let status = self.shared.lock_irq().raid.status();
        if status.state() == State::Fault {
            writeln!(text, "state fault")?;
            writeln!(text, "fault {:#x}", status.fault_code())
        } else {
            writeln!(text, "state operational")?;
            writeln!(text, "fault -")
        }
    }

    /// Writes the `volumes` file.
    pub fn write_volumes(&self, text: &mut Text) -> fmt::Result {
        text.write_bytes(self.report.lock().volumes.bytes())
    }

    /// Writes the `disks` file.
    pub fn write_disks(&self, text: &mut Text) -> fmt::Result {
        text.write_bytes(self.report.lock().disks.bytes())
    }

    /// Whether the controller has a battery or flash to keep its cache,
    /// which the `battery` file tells of.
    pub fn has_battery(&self) -> bool {
        self.report.lock().summary.battery
    }

    /// Writes the `battery` file.
    pub fn write_battery(&self, text: &mut Text) -> fmt::Result {
        text.write_bytes(self.report.lock().battery.bytes())
    }

    /// Writes the `events` file, oldest first.
    pub fn write_events(&self, text: &mut Text) -> fmt::Result {
        let events = self.events.lock();
        for detail in events.oldest_first() {
            report::write_event(detail, text)?;
        }
        Ok(())
    }

    /// Calls `f` with the device ID of each physical disk the controller
    /// has, as its `disks` file lists them.
    pub fn disk_ids(&self, mut f: impl FnMut(u16) -> bool) {
        let report = self.report.lock();
        for line in report.disks.bytes().split(|&b| b == b'\n') {
            let id = field(line, b"id").and_then(|id| core::str::from_utf8(id).ok());
            if let Some(id) = id.and_then(|id| id.parse().ok())
                && !f(id)
            {
                return;
            }
        }
    }

    /// Asks physical disk `device` what it says of its health, through
    /// the firmware's pass-through, into `out` as GET_HEALTH writes it
    /// (radix-block(2)): its kind and flags, then the record. A SATA disk
    /// is read as AHCI reads one, SMART data and thresholds; any other is
    /// asked its log pages and grown defects, as [`scsi_health`] does.
    pub fn health(&self, device: u16, out: &mut [u8; HEALTH_SIZE]) -> Result<(), FileError> {
        let ata = {
            let report = self.report.lock();
            let line = report
                .disks
                .bytes()
                .split(|&b| b == b'\n')
                .find(|line| field(line, b"id").is_some_and(|id| u16_of(id) == Some(device)));
            field(line.ok_or(FileError::NotFound)?, b"interface") == Some(b"sata".as_slice())
        };
        out.fill(0);
        let manage = self.manage.lock();
        let (head, record) = out.split_at_mut(8);
        let record: &mut [u8; RECORD] = record.try_into().map_err(|_| FileError::Io)?;
        // Why a query failed, if the controller did: a disk that refused
        // is not an error of the controller's.
        let mut lost = false;
        let mut ask = |cdb: &Cdb, out: &mut [u8]| -> Result<usize, Failed> {
            let len = out.len().min(manage.memory.len());
            let sent = self.run(&manage.memory, len, QUERY_TIMEOUT_MS, |raid, runs| {
                raid.submit_member(device, cdb.bytes(), runs, Direction::FromDevice)
            });
            let Ok(answer) = sent else {
                lost = true;
                return Err(Failed);
            };
            if !answer.good() {
                return Err(Failed);
            }
            manage.memory.read(0, &mut out[..len]);
            radix_hw::device_wrote(&mut out[..len]);
            Ok((answer.moved as usize).min(len))
        };
        let kind = if ata {
            for (feature, at) in
                [(queries::SMART_READ_DATA, 0), (queries::SMART_READ_THRESHOLDS, 512)]
            {
                if ask(&Cdb::smart_read(feature), &mut record[at..at + 512]).is_err() {
                    break;
                }
            }
            if record[..512].iter().all(|&byte| byte == 0) {
                return Err(if lost { FileError::Io } else { FileError::NotSupported });
            }
            request::HEALTH_ATA
        } else {
            let mut fetch = |query: Query, out: &mut [u8]| {
                let length = u16::try_from(out.len()).unwrap_or(u16::MAX);
                let cdb = match query {
                    Query::LogSense(page) => Cdb::log_sense(page, length),
                    Query::GrownDefects => Cdb::read_defect_data_10(length),
                };
                ask(&cdb, out)
            };
            if queries::gather(&mut fetch, record).is_err() {
                return Err(if lost { FileError::Io } else { FileError::NotSupported });
            }
            request::HEALTH_SCSI
        };
        head[..4].copy_from_slice(&kind.to_le_bytes());
        Ok(())
    }
}

/// The value of field `key` of a record line (`key=value`, TAB between).
fn field<'a>(line: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    line.split(|&b| b == b'\t').find_map(|field| {
        let rest = field.strip_prefix(key)?;
        rest.strip_prefix(b"=")
    })
}

/// A decimal number of 16 bits.
fn u16_of(text: &[u8]) -> Option<u16> {
    core::str::from_utf8(text).ok()?.parse().ok()
}

/// Fills `list`, which has room reserved, with `len` of `value`.
fn filled<T: Copy>(list: &mut Vec<T, Kmalloc>, len: usize, value: T) {
    while list.len() < len && list.push_within_capacity(value).is_ok() {}
}

/// Zeroes the first `len` bytes of `memory`.
fn zero(memory: &Dma, len: usize) {
    const ZEROS: [u8; 512] = [0; 512];
    let mut at = 0;
    while at < len.min(memory.len()) {
        let n = (len - at).min(ZEROS.len());
        memory.write(at, &ZEROS[..n]);
        at += n;
    }
}

/// The NUMA node of the function at `place`, as its root bus's host
/// bridge gave it.
fn numa_of(place: radix_hw::pci::Address) -> Option<u8> {
    let mut node = None;
    super::pci_functions(|function| {
        if function.address == place {
            node = super::numa_node(function);
        }
    });
    node
}

/// A volume or a pass-through disk of a controller, as the block queue
/// drives it.
pub struct RaidDisk {
    adapter: Arc<Adapter, Kmalloc>,
    /// Which of the controller's disks it is.
    number: u8,
    target: Target,
    /// The 512-byte sectors in one of its blocks.
    unit: u64,
    /// Whether its reads and writes take 16-byte commands.
    sixteen: bool,
    /// Whether it is a SATA disk passed through (its INQUIRY vendor is
    /// "ATA"), whose health is SMART.
    ata: bool,
}

impl Controller for RaidDisk {
    fn room(&self, _op: Op, _segments: usize) -> bool {
        self.adapter.shared.lock_irq().disk_room() > 0
    }

    fn submit(&mut self, slot: u8, request: &Request<'_>, _more: bool) -> Result<(), Error> {
        let Request { op, sector, data, .. } = *request;
        let (cdb, direction) = match op {
            Op::Read | Op::Write | Op::WriteThrough => {
                let sectors = data.iter().map(|&(_, len)| u64::from(len)).sum::<u64>() / SECTOR;
                if !sector.is_multiple_of(self.unit) || !sectors.is_multiple_of(self.unit) {
                    return Err(Error::OutOfRange);
                }
                let (lba, blocks) = (sector / self.unit, sectors / self.unit);
                let blocks = u32::try_from(blocks).map_err(|_| Error::OutOfRange)?;
                // Never asked to write through: a volume's cache is kept.
                let (cdb, direction) = if op == Op::Read {
                    (Cdb::read(lba, blocks, self.sixteen), Direction::FromDevice)
                } else {
                    (Cdb::write(lba, blocks, self.sixteen), Direction::ToDevice)
                };
                (cdb.ok_or(Error::OutOfRange)?, direction)
            }
            Op::Flush => (Cdb::synchronize_cache(), Direction::None),
            // A volume's health is its disks', in /kernel/raid.
            Op::Query(query) if matches!(self.target, Target::Disk(_)) => {
                (query_cdb(query, data), Direction::FromDevice)
            }
            Op::Smart(feature @ (queries::SMART_READ_DATA | queries::SMART_READ_THRESHOLDS), _)
                if matches!(self.target, Target::Disk(_)) =>
            {
                (Cdb::smart_read(feature), Direction::FromDevice)
            }
            Op::Query(_) | Op::Discard(_) | Op::Smart(..) | Op::PowerMode => {
                return Err(Error::Unsupported);
            }
        };
        let mut shared = self.adapter.shared.lock_irq();
        let context =
            shared.raid.submit(self.target, 0, cdb.bytes(), data, direction).map_err(|err| {
                match err {
                    radix_megaraid::Error::BadRequest => Error::OutOfRange,
                    _ => Error::Io,
                }
            })?;
        let flush = op == Op::Flush;
        let query = matches!(op, Op::Query(_) | Op::Smart(..));
        if let Some(owner) = shared.owner.get_mut(usize::from(context)) {
            *owner = Owner::Disk { disk: self.number, slot, flush, query };
        }
        Ok(())
    }

    fn complete(&mut self) -> Option<(u8, Outcome)> {
        self.adapter.shared.lock_irq().take(self.number)
    }

    fn why(&self, slot: u8) -> Option<&'static str> {
        let at = usize::from(self.number) * SLOTS + usize::from(slot);
        self.adapter.shared.lock_irq().outcome.get(at).and_then(|&(_, why)| why)
    }

    fn health(disk: &Disk<Self>, _target: u32, page: &mut Page) -> Result<Health, Error> {
        let (volume, ata) = disk
            .controller(|lane| (matches!(lane.target, Target::Volume(_)), lane.ata))
            .ok_or(Error::Io)?;
        if volume {
            return Err(Error::Unsupported);
        }
        scsi_health::health(disk, page, ata)
    }
}

/// How a command that ended with `answer` ends its request, and why for
/// the log: a `flush` the disk does not take did not need doing, and a
/// `query` it does not take or answers short is answered as it is.
fn judge(answer: &Answer, flush: bool, query: bool) -> (Outcome, Option<&'static str>) {
    if answer.good() {
        if answer.moved >= answer.asked || query {
            return (Outcome::Done, None);
        }
        return (Outcome::Retry, Some("it moved less than asked"));
    }
    match answer.status {
        status::SCSI_DONE_WITH_ERROR => {}
        status::DEVICE_NOT_FOUND => return (Outcome::Failed, Some("the disk is gone")),
        status::LD_OFFLINE => return (Outcome::Failed, Some("the volume is offline")),
        status::SCSI_IO_FAILED => {
            return (Outcome::Retry, Some("the controller failed the command"));
        }
        _ => return (Outcome::Failed, Some(answer.why())),
    }
    let sense = match answer.scsi_status {
        BUSY | TASK_SET_FULL => return (Outcome::Retry, Some("the disk is busy")),
        CHECK_CONDITION => Sense::parse(answer.sense()),
        _ => return (Outcome::Failed, Some("the disk ended the command with another status")),
    };
    let Some(sense) = sense else {
        return (Outcome::Retry, Some("it failed the command, and did not say why"));
    };
    match sense.meaning() {
        Meaning::Fine => (Outcome::Done, None),
        Meaning::Unsupported if flush || query => (Outcome::Done, None),
        Meaning::Attention | Meaning::Aborted => (Outcome::Retry, Some(sense.key_name())),
        Meaning::BecomingReady => (Outcome::Retry, Some("it is becoming ready")),
        Meaning::WriteProtected => (Outcome::Failed, Some("it is write-protected")),
        _ => (Outcome::Failed, Some(sense.key_name())),
    }
}

/// The controllers, for their interrupt handlers and `/kernel/raid`: set
/// once each, after their disks.
static ADAPTERS: [Once<Arc<Adapter, Kmalloc>>; MAX_CONTROLLERS] =
    [const { Once::new() }; MAX_CONTROLLERS];
/// How many controllers [`bind`] was given, set up or not.
static BOUND: AtomicUsize = AtomicUsize::new(0);
/// A disk of a controller, once it is set up.
type DiskSlot = Once<Arc<Disk<RaidDisk>, Kmalloc>>;
/// Each controller's disks, in order.
static DISKS: [[DiskSlot; MAX_DISKS]; MAX_CONTROLLERS] =
    [const { [const { Once::new() }; MAX_DISKS] }; MAX_CONTROLLERS];

/// The controller numbered `index`, if it is set up.
pub fn adapter(index: usize) -> Option<&'static Arc<Adapter, Kmalloc>> {
    ADAPTERS.get(index).and_then(Once::get)
}

/// How many controller numbers there are: one past the last set up.
pub fn count() -> usize {
    BOUND.load(Ordering::Acquire).min(MAX_CONTROLLERS)
}

fn on_interrupt(index: usize) {
    let Some(adapter) = adapter(index) else { return };
    let (mail, managed, evented) = {
        let mut shared = adapter.shared.lock_irq();
        shared.raid.acknowledge();
        shared.drain()
    };
    adapter.deliver(mail, managed, evented);
}

/// Carries a scan's commands to one volume or disk of a controller, one
/// at a time, polled, through a page of DMA memory: before the disks
/// exist.
struct Scan<'a> {
    raid: &'a mut MegaRaid,
    page: &'a Dma,
    target: Target,
    /// A command went unanswered: the controller is given up.
    broken: bool,
}

impl Commands for Scan<'_> {
    fn command(
        &mut self,
        lun: u8,
        cdb: &Cdb,
        data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<Reply, TransportError> {
        if self.broken {
            return Err(TransportError::Failed("the controller stopped answering"));
        }
        let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let page = [(self.page.phys(), length)];
        let runs: &[(u64, u32)] = match &data {
            Data::None => &[],
            Data::Frames { frames, .. } => frames,
            Data::In(_) | Data::Out(_) if data.len() > self.page.len() => {
                return Err(TransportError::Failed("its answer does not fit a page"));
            }
            Data::Out(bytes) => {
                self.page.write(0, bytes);
                &page
            }
            Data::In(_) => &page,
        };
        let direction = match &data {
            Data::None => Direction::None,
            _ if data.into_host() => Direction::FromDevice,
            _ => Direction::ToDevice,
        };
        let context = self
            .raid
            .submit(self.target, lun, cdb.bytes(), runs, direction)
            .map_err(|_| TransportError::Failed("the controller refused the command"))?;
        let started = sched::now_ns();
        let deadline = started.saturating_add(u64::from(timeout_ms) * 1_000_000);
        let answer = loop {
            if let Some((done, answer)) = self.raid.complete() {
                if done == context {
                    break answer;
                }
                continue;
            }
            let now = sched::now_ns();
            if now >= deadline {
                self.broken = true;
                return Err(TransportError::Failed("the controller did not answer"));
            }
            if now < started + SPIN_NS {
                core::hint::spin_loop();
            } else {
                queue::delay(100);
            }
        };
        if let Data::In(bytes) = data {
            self.page.read(0, bytes);
        }
        let residue = u32::try_from(answer.asked.saturating_sub(answer.moved)).unwrap_or(u32::MAX);
        match (answer.status, answer.scsi_status) {
            (status::OK, _) => Ok(Reply::Good { residue }),
            (status::SCSI_DONE_WITH_ERROR, CHECK_CONDITION) => {
                Ok(Reply::Failed(Sense::parse(answer.sense())))
            }
            (status::SCSI_DONE_WITH_ERROR, BUSY | TASK_SET_FULL) => {
                Err(TransportError::Failed("the disk is busy"))
            }
            (status::SCSI_DONE_WITH_ERROR, _) => Ok(Reply::Failed(None)),
            // Nothing there: it is not there.
            (status::DEVICE_NOT_FOUND, _) => Err(TransportError::Gone),
            _ => Err(TransportError::Failed(answer.why())),
        }
    }
}

/// A volume or disk the scan found: where, and what it is.
#[derive(Clone, Copy)]
struct Found1 {
    target: Target,
    found: Found,
    /// Its serial number, padded with spaces; blank if it gives none.
    serial: [u8; SERIAL],
}

/// What the controller said of itself, for the log.
struct Facts {
    model: TextBuf<40>,
    package: TextBuf<32>,
    volumes: usize,
    disks: usize,
}

/// Runs a management command of the scan into `memory`, polled: `len`
/// bytes of answer at most. Returns how many it answered with.
fn scan_dcmd(
    raid: &mut MegaRaid,
    memory: &Dma,
    opcode: u32,
    mailbox: &[u8; 12],
    len: usize,
) -> Result<usize, &'static str> {
    let len = len.min(memory.len());
    zero(memory, len);
    let runs = [(memory.phys(), len as u32)];
    let mut wait = |ms: u32| queue::delay(ms.saturating_mul(1000));
    let answer = raid
        .run_dcmd(opcode, mailbox, &runs, Direction::FromDevice, &mut wait, SCAN_TIMEOUT_MS)
        .map_err(|_| "it did not answer a management command")?;
    if !answer.good() {
        return Err(answer.why());
    }
    Ok((answer.moved as usize).min(len))
}

/// Looks for the volumes and the pass-through disks of `raid`, the
/// controller at `place`, up to [`MAX_DISKS`] into `found`, holding the
/// firmware's lists in `scratch`. Fails if the controller stopped
/// answering. Kept out of [`bind`]'s frame: its answers are large.
#[inline(never)]
fn scan(
    raid: &mut MegaRaid,
    memory: &Dma,
    scratch: &mut [u8],
    place: radix_hw::pci::Address,
    found: &mut Vec<Found1, Kmalloc>,
) -> Result<Facts, &'static str> {
    let mut facts = Facts { model: TextBuf::new(), package: TextBuf::new(), volumes: 0, disks: 0 };
    let len = scan_dcmd(raid, memory, opcode::CTRL_GET_INFO, &[0; 12], CtrlInfo::LEN)?;
    memory.read(0, &mut scratch[..len]);
    if let Ok(info) = CtrlInfo::parse(&scratch[..len]) {
        let _ = write!(facts.model, "{}", info.product_name());
        let _ = write!(facts.package, "{}", info.package_version());
    }
    // The volumes, then the disks passed through, each asked what it is
    // through a page of the memory past the lists.
    let (lists, rest) = scratch.split_at_mut(PdList::LEN.max(LdList::LEN));
    let page = memory;
    let mut targets: Vec<Target, Kmalloc> = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    targets.try_reserve_exact(MAX_DISKS * 2).map_err(|_| "out of memory")?;
    let len = scan_dcmd(raid, memory, opcode::LD_GET_LIST, &[0; 12], LdList::LEN)?;
    memory.read(0, &mut lists[..len]);
    let volumes = LdList::parse(&lists[..len]).map_err(|_| "its list of volumes is not valid")?;
    for volume in volumes.iter() {
        facts.volumes += 1;
        let _ = targets.push_within_capacity(Target::Volume(u16::from(volume.target_id)));
    }
    let len = scan_dcmd(raid, memory, opcode::PD_GET_LIST, &[0; 12], PdList::LEN)?;
    memory.read(0, &mut lists[..len]);
    let disks = PdList::parse(&lists[..len]).map_err(|_| "its list of disks is not valid")?;
    let info = &mut rest[..PdInfo::LEN];
    for disk in disks.iter().filter(|disk| disk.scsi_device_type == device_type::DISK) {
        let mailbox = mbox::pd_info(disk.device_id);
        let Ok(len) = scan_dcmd(raid, memory, opcode::PD_GET_INFO, &mailbox, PdInfo::LEN) else {
            continue;
        };
        memory.read(0, &mut info[..len]);
        let passed = PdInfo::parse(&info[..len]).is_ok_and(|pd| pd.state().is_jbod());
        if passed {
            facts.disks += 1;
            let _ = targets.push_within_capacity(Target::Disk(disk.device_id));
        }
    }
    for &target in &targets {
        let mut commands = Scan { raid: &mut *raid, page, target, broken: false };
        let mut wait = |ms: u32| queue::delay(ms.saturating_mul(1000));
        match probe(&mut commands, 0, &mut wait) {
            Ok(disk) if found.len() < MAX_DISKS => {
                let serial = serial_number(&mut commands, 0);
                let _ = found.push_within_capacity(Found1 { target, found: disk, serial });
            }
            Ok(_) => warn!(
                "Disk: MegaRAID controller at {place}: more than {MAX_DISKS} volumes and disks; \
                 {} is not used",
                What(target)
            ),
            Err(Absent::NotDisk(kind)) => info!(
                "Disk: MegaRAID controller at {place}: {} is a device of type {kind:#x}, which \
                 is not used",
                What(target)
            ),
            Err(why) => {
                warn!("Disk: MegaRAID controller at {place}: {}: {why}", What(target));
            }
        }
        if commands.broken {
            return Err("it stopped answering");
        }
    }
    Ok(facts)
}

/// A volume or disk, for the log: `volume 0`, `disk 4`.
struct What(Target);

impl fmt::Display for What {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Target::Volume(id) => write!(f, "volume {id}"),
            Target::Disk(id) => write!(f, "pass-through disk {id}"),
        }
    }
}

/// The interrupt `function` raises for controller `index`: MSI-X entry
/// 0, or else MSI. Returns its vector, which kind, and whether it is
/// MSI-X.
fn interrupt(function: &Function, index: usize) -> Result<(u32, &'static str, bool), &'static str> {
    if let Some(msix) = MsiX::find(function) {
        let (bar, offset) = msix.table_location();
        let Some(Bar::Memory { address, .. }) = function.bar(bar) else {
            return Err("its MSI-X table is not in memory");
        };
        let table = <Arch as Devices>::map_device(address + offset, msix.table_len())
            .ok_or("its MSI-X table could not be mapped")?;
        let (vector, message) =
            <Arch as Devices>::allocate_interrupt(function.address(), on_interrupt, index)
                .ok_or("no interrupt vector is free")?;
        msix.enable_masked(function);
        msix.set_entry(&table, 0, message);
        msix.unmask_all(function);
        return Ok((vector, "MSI-X", true));
    }
    let msi = Msi::find(function).ok_or("it has neither MSI-X nor MSI")?;
    let (vector, message) =
        <Arch as Devices>::allocate_interrupt(function.address(), on_interrupt, index)
            .ok_or("no interrupt vector is free")?;
    msi.enable(function, message);
    Ok((vector, "MSI", false))
}

/// Sets up the MegaRAID controller `function` as controller number
/// `index`, and its volumes and pass-through disks, counted in `disks`.
/// Kept out of `init`'s frame.
#[inline(never)]
pub fn bind(function: &Function, index: usize, disks: &mut usize) -> Result<(), &'static str> {
    BOUND.fetch_max(index + 1, Ordering::AcqRel);
    if index >= MAX_CONTROLLERS {
        return Err("more RAID controllers than the kernel drives");
    }
    let device = function.device_id();
    let family = Family::of(device).ok_or("not a MegaRAID controller")?;
    // Its registers are in its first memory BAR.
    let bar = (0..6).find_map(|n| match function.bar(n) {
        Some(Bar::Memory { address, size, .. }) if size >= REGISTERS as u64 => Some(address),
        _ => None,
    });
    let address = bar.ok_or("its registers are not in memory")?;
    let regs = <Arch as Devices>::map_device(address, REGISTERS)
        .ok_or("its registers could not be mapped")?;
    function.enable_memory_and_dma();
    // Its interrupt first: the firmware is told whether it has MSI-X.
    let (vector, kind, msix) = interrupt(function, index)?;
    let place = function.address();
    let mut alloc = |pages: usize| <Arch as Devices>::alloc_dma(place, pages, true);
    let mut wait = |ms: u32| queue::delay(ms.saturating_mul(1000));
    let mut raid = MegaRaid::new(regs, device, msix, &mut alloc, &mut wait).map_err(|err| {
        warn!("Disk: MegaRAID controller at {place}: {err}");
        "its firmware did not come up"
    })?;
    raid.enable_interrupts();
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let held = (|| {
        let memory = <Arch as Devices>::alloc_dma(place, MANAGE_BYTES / PAGE, true)?;
        let event_page = <Arch as Devices>::alloc_dma(place, 1, true)?;
        let mut scratch = Vec::new_in(heap);
        scratch.try_reserve_exact(MANAGE_BYTES).ok()?;
        filled(&mut scratch, MANAGE_BYTES, 0);
        let mut found = Vec::new_in(heap);
        found.try_reserve_exact(MAX_DISKS).ok()?;
        Some((memory, event_page, scratch, found))
    })();
    let Some((memory, event_page, mut scratch, mut found)) = held else {
        raid.reset();
        return Err("out of memory");
    };
    let facts = match scan(&mut raid, &memory, &mut scratch, place, &mut found) {
        Ok(facts) => facts,
        Err(why) => {
            raid.reset();
            return Err(why);
        }
    };
    let manage = Manage { memory, scratch };
    let adapter = new_adapter(raid, manage, event_page, place, index)?;
    let mut made = Vec::new_in(heap);
    made.try_reserve_exact(found.len()).map_err(|_| "out of memory")?;
    let mut names = Vec::new_in(heap);
    names.try_reserve_exact(found.len()).map_err(|_| "out of memory")?;
    for (number, disk) in found.iter().enumerate() {
        let (letter, name) = super::take_sd_name().ok_or("too many SATA and SCSI disks")?;
        match add(function, (index, number), &adapter, disk, (vector, kind), &name) {
            Ok(info) => {
                let _ = names.push_within_capacity((disk.target, name.clone()));
                let _ = made.push_within_capacity((number, name, info));
                *disks += 1;
            }
            Err(why) => {
                super::give_back_sd_letter(letter);
                warn!("Disk: {name}: {why}");
            }
        }
    }
    let _ = adapter.names.set(names);
    // Its interrupts reach the disks from here; what came before is taken
    // now. Then what it has, for /kernel/raid, and the disks' partition
    // tables, read through their queues.
    let _ = ADAPTERS[index].set(adapter.clone());
    adapter.service();
    adapter.gather();
    info!(
        "Disk: MegaRAID {} controller at {place}: {}, firmware package {}, {} {}, {} \
         pass-through {}, {kind} vector {vector:#x}",
        family.name(),
        facts.model,
        facts.package,
        facts.volumes,
        if facts.volumes == 1 { "volume" } else { "volumes" },
        facts.disks,
        if facts.disks == 1 { "disk" } else { "disks" },
    );
    for (number, name, info) in &made {
        if let Some(disk) = DISKS[index][*number].get() {
            let one = &found[*number];
            publish(disk, ident(&one.found.inquiry, one.serial), name, *info);
        }
    }
    let options = Options::new().held().policy(Policy::Fair { nice: 0 });
    let mut name = TextBuf::<16>::new();
    let _ = write!(name, "megaraid{index}");
    match sched::spawn(name.as_str(), events_main, index, options) {
        Ok(thread) => sched::release(&thread),
        Err(_) => warn!(
            "Disk: MegaRAID controller at {place}: its thread could not be started; its events \
             are not followed"
        ),
    }
    Ok(())
}

/// The controller that drives `raid`, on the heap, with room for every
/// context's owner and the events. Kept out of [`bind`]'s frame.
#[inline(never)]
fn new_adapter(
    raid: MegaRaid,
    manage: Manage,
    event_page: Dma,
    place: radix_hw::pci::Address,
    index: usize,
) -> Result<Arc<Adapter, Kmalloc>, &'static str> {
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut owner = Vec::new_in(heap);
    owner.try_reserve_exact(CONTEXTS).map_err(|_| "out of memory")?;
    filled(&mut owner, CONTEXTS, Owner::None);
    let mut outcome = Vec::new_in(heap);
    outcome.try_reserve_exact(MAX_DISKS * SLOTS).map_err(|_| "out of memory")?;
    filled(&mut outcome, MAX_DISKS * SLOTS, (Outcome::Failed, None));
    let mut details = Vec::new_in(heap);
    details.try_reserve_exact(EVENTS).map_err(|_| "out of memory")?;
    let family = raid.family();
    let commands = raid.commands();
    let shared = Shared {
        raid,
        owner,
        ended: [0; MAX_DISKS],
        outcome,
        manage: None,
        managed: None,
        event: None,
        evented: None,
    };
    let report = Report {
        controller: Text::new(),
        volumes: Text::new(),
        disks: Text::new(),
        battery: Text::new(),
        summary: Summary::default(),
        gathered: false,
    };
    let adapter = Adapter {
        shared: SpinLock::new(shared),
        index,
        place,
        family,
        commands,
        managed: AtomicBool::new(false),
        evented: AtomicBool::new(false),
        manage: Mutex::new(manage),
        event_page,
        report: Mutex::new(report),
        events: Mutex::new(Events { details, next: 0 }),
        gathered_at: AtomicU64::new(0),
        names: Once::new(),
    };
    Arc::try_new_in(adapter, heap).map_err(|_| "out of memory")
}

/// Makes the disk number `number` of controller `index`, which `disk`
/// says is there, named `name`, and sets it in [`DISKS`]; returns what it
/// is. Kept out of [`bind`]'s frame: the disk is built on the stack
/// before it moves.
#[inline(never)]
fn add(
    function: &Function,
    (index, number): (usize, usize),
    adapter: &Arc<Adapter, Kmalloc>,
    disk: &Found1,
    (vector, kind): (u32, &'static str),
    name: &Name,
) -> Result<Info, &'static str> {
    let Found1 { target, found, .. } = *disk;
    let capacity = found.capacity;
    let unit = u64::from(capacity.block_length) / SECTOR;
    let (max_segments, flushes) = {
        let shared = adapter.shared.lock_irq();
        (shared.raid.max_segments(), shared.raid.volume_sync_cache())
    };
    let info = Info {
        sectors: capacity.blocks.saturating_mul(unit),
        logical_block: capacity.block_length,
        block_size: capacity.physical_block(),
        read_only: found.read_only,
        max_segments,
        discard_sectors: 0,
        // A volume or disk that does not say spins, as Linux takes it.
        rotational: true,
    };
    // A volume's cache is the controller's: where the firmware does not
    // take SYNCHRONIZE CACHE for one, its battery or flash keeps it.
    let write_cache = match target {
        Target::Volume(_) => found.write_cache && flushes,
        Target::Disk(_) => found.write_cache,
    };
    let controller = RaidDisk {
        adapter: adapter.clone(),
        number: number as u8,
        target,
        unit,
        sixteen: found.sixteen,
        ata: found.inquiry.vendor() == ATA_VENDOR,
    };
    let part = queue::part(controller).ok_or("out of memory")?;
    let once = core::iter::once(part);
    let source = function.address();
    let made = Disk::new(once, info, write_cache, false, true, name.clone(), source)
        .ok_or("out of memory")?;
    let made = Arc::try_new_in(made, HEAP.with(Flags::MAY_SLEEP)).map_err(|_| "out of memory")?;
    let _ = DISKS[index][number].set(made);
    info!(
        "Disk: {name} is {} of {} on the MegaRAID controller at {source}: {} {}, {} bytes a \
         block{}{}{}, {kind} vector {vector:#x}",
        What(target),
        Size(info.sectors * SECTOR),
        found.inquiry.vendor(),
        found.inquiry.product(),
        info.block_size,
        super::sector_note(&info),
        if info.read_only { ", read-only" } else { "" },
        if write_cache { ", write cache" } else { "" },
    );
    Ok(info)
}

/// Registers `disk`, named `name`, which says it is `ident` and is
/// `info`, with its partitions, once its controller's interrupt reaches
/// it.
fn publish(disk: &Arc<Disk<RaidDisk>, Kmalloc>, ident: super::Ident, name: &Name, info: Info) {
    let Some(volume) = Volume::new(disk.clone(), 0, info) else {
        warn!("Disk: {name}: out of memory");
        return;
    };
    let Ok(device) = Arc::try_new_in(volume, HEAP.with(Flags::MAY_SLEEP)) else {
        warn!("Disk: {name}: out of memory");
        return;
    };
    let device: DeviceRef = device;
    register(Entry { name: name.clone(), device: device.clone(), place: None, ident: Some(ident) });
    partitions(name, &device);
}

/// The controller's thread: reads its log of events, then follows them,
/// gathering what it has again after each burst, and every while.
fn events_main(index: usize) {
    let Some(adapter) = adapter(index) else { return };
    let mut next = adapter.read_history();
    // Again, now that the firmware's tables have told the NUMA nodes.
    adapter.gather();
    let (mut refused, mut retry_at) = (0, 0);
    let mut waiting = false;
    let mut settle: Option<u64> = None;
    loop {
        let now = sched::now_ns();
        if !waiting
            && refused < EVENT_TRIES
            && now >= retry_at
            && let Some(seq) = next
        {
            waiting = adapter.wait_event(seq);
        }
        let every = if adapter.task() { TASK_REFRESH_NS } else { IDLE_REFRESH_NS };
        let due = adapter.gathered_at.load(Ordering::Relaxed).saturating_add(every);
        let mut deadline = settle.map_or(due, |settle| settle.min(due));
        if !waiting && refused < EVENT_TRIES {
            deadline = deadline.min(retry_at.max(now));
        }
        sched::wait::wait_until_or_timeout(adapter.event_key(), "megaraid", deadline, || {
            adapter.evented.load(Ordering::Acquire)
        });
        adapter.service();
        if let Some(answer) = adapter.take_event() {
            waiting = false;
            if answer.good() {
                refused = 0;
                if let Some(seq) = adapter.note_event() {
                    next = Some(seq.wrapping_add(1));
                }
                settle.get_or_insert(sched::now_ns().saturating_add(SETTLE_NS));
            } else {
                refused += 1;
                retry_at = sched::now_ns().saturating_add(EVENT_RETRY_NS);
                if refused == EVENT_TRIES {
                    warn!(
                        "Disk: MegaRAID controller at {}: it refused to tell of its events ({}); \
                         what it has is asked every {} minutes",
                        adapter.place,
                        answer.why(),
                        IDLE_REFRESH_NS / 60_000_000_000
                    );
                }
            }
        }
        let now = sched::now_ns();
        if settle.is_some_and(|settle| now >= settle) || now >= due {
            settle = None;
            adapter.gather();
        }
    }
}

/// Has each controller write its cache and its disks' out, and shut its
/// firmware down. Runs once the disks have flushed, before the machine is
/// powered off, restarted or halted.
pub fn shutdown() {
    for adapter in ADAPTERS.iter().filter_map(Once::get) {
        let manage = adapter.manage.lock();
        let steps = [
            (opcode::CTRL_CACHE_FLUSH, mbox::cache_flush(), "its cache could not be written out"),
            (opcode::CTRL_SHUTDOWN, mbox::shutdown(false), "it did not shut down"),
        ];
        for (opcode, mailbox, what) in steps {
            let answer = adapter.run(&manage.memory, 0, MANAGE_TIMEOUT_MS, |raid, runs| {
                raid.submit_dcmd(opcode, &mailbox, runs, Direction::None)
            });
            match answer {
                Ok(answer) if answer.good() => {}
                Ok(answer) => {
                    warn!(
                        "Disk: MegaRAID controller at {}: {what}: {}",
                        adapter.place,
                        answer.why()
                    );
                }
                Err(why) => warn!("Disk: MegaRAID controller at {}: {what}: {why}", adapter.place),
            }
        }
    }
}
