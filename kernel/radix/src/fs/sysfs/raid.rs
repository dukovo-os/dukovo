// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/kernel/raid` (ADR 0664): the RAID controllers the kernel drives
//! ([`disk::megaraid`]), a directory each, `0` first in PCI order, which
//! `storage raid`, diskhealth(8), `hardware check`, metricsd(8) and the
//! web console read (radix-sysfs(5) holds the formats).
//!
//! Each has `controller` (key lines: what it is, its counts, and whether
//! its firmware runs now), `volumes` and `disks` (a record line each),
//! `battery` while it has one to keep its cache, and `events`, the last
//! 256 it told of, oldest first. These are the controller's answers the
//! last time it was asked, which its thread does after each burst of
//! events and every 30 seconds while a task runs, so reading them never
//! waits on it. `health/<device id>`, for each physical disk, does: each
//! open asks the disk, through the controller, for its SMART data or log
//! pages, the 1,032 bytes GET_HEALTH writes (radix-block(2)); only root
//! may read it, and it fails to open (ENOTSUP) for a disk that keeps no
//! record.

use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

use radix_klog::TextBuf;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_vfs::synthetic::{FixedDir, ListedDir, Listing, read_bytes};
use radix_vfs::{Access, Error, Kind, Node, NodeRef, Out, Result, Stat, mem, next_id};

use super::super::disk::megaraid::{self, Adapter, HEALTH_SIZE};
use super::value;

/// Adds `raid` to `root`, when a controller is there.
pub(super) fn add(root: &FixedDir) -> Result<()> {
    if megaraid::count() == 0 {
        return Ok(());
    }
    let controllers: radix_vfs::mem::KBox<dyn Listing> = mem::boxed(Controllers)?;
    root.add("raid", ListedDir::make(next_id(), controllers)?)
}

/// A number as a name.
fn number(n: usize) -> TextBuf<8> {
    let mut text = TextBuf::new();
    let _ = write!(text, "{n}");
    text
}

/// `raid`: a directory for each controller set up.
struct Controllers;

impl Listing for Controllers {
    fn list(&self, f: &mut dyn FnMut(&str, Kind) -> bool) {
        for index in 0..megaraid::count() {
            if megaraid::adapter(index).is_some() && !f(number(index).as_str(), Kind::Directory) {
                return;
            }
        }
    }

    fn lookup(&self, name: &str) -> Result<NodeRef> {
        let index = (0..megaraid::count())
            .find(|&index| number(index).as_str() == name)
            .ok_or(Error::NotFound)?;
        let adapter: &'static Adapter =
            megaraid::adapter(index).map(|a| &**a).ok_or(Error::NotFound)?;
        let dir = FixedDir::new(next_id())?;
        dir.add("controller", value(move |text| adapter.write_controller(text))?)?;
        dir.add("volumes", value(move |text| adapter.write_volumes(text))?)?;
        dir.add("disks", value(move |text| adapter.write_disks(text))?)?;
        if adapter.has_battery() {
            dir.add("battery", value(move |text| adapter.write_battery(text))?)?;
        }
        dir.add("events", value(move |text| adapter.write_events(text))?)?;
        let health: radix_vfs::mem::KBox<dyn Listing> = mem::boxed(Healths(adapter))?;
        dir.add("health", ListedDir::make(next_id(), health)?)?;
        Ok(dir)
    }
}

/// `health`: a file for each physical disk of a controller, by its
/// device ID.
struct Healths(&'static Adapter);

impl Listing for Healths {
    fn list(&self, f: &mut dyn FnMut(&str, Kind) -> bool) {
        self.0.disk_ids(|id| f(number(usize::from(id)).as_str(), Kind::File));
    }

    fn lookup(&self, name: &str) -> Result<NodeRef> {
        let mut device = None;
        self.0.disk_ids(|id| {
            if number(usize::from(id)).as_str() == name {
                device = Some(id);
            }
            device.is_none()
        });
        let device = device.ok_or(Error::NotFound)?;
        Ok(mem::arc(HealthFile { id: next_id(), adapter: self.0, device })?)
    }
}

/// `health/<device id>`: asks the disk when opened.
struct HealthFile {
    id: u64,
    adapter: &'static Adapter,
    device: u16,
}

impl HealthFile {
    /// What the disk says now, on the heap: a record in the frame, and a
    /// copy of it in the frame of whoever keeps it, would be over a stack
    /// frame's budget (ADR 0058).
    fn read_now(&self) -> Result<Vec<u8, Kmalloc>> {
        let mut bytes = radix_mm::bytes::zeroed(HEALTH_SIZE, HEAP.with(Flags::MAY_SLEEP))
            .map_err(|_| Error::NoMemory)?;
        let record: &mut [u8; HEALTH_SIZE] =
            bytes.as_mut_slice().try_into().map_err(|_| Error::NoMemory)?;
        self.adapter.health(self.device, record)?;
        Ok(bytes)
    }
}

impl Node for HealthFile {
    fn access(&self) -> Access {
        // Each open sends the disk commands: root's alone.
        Access { owner: 0, group: 0, mode: 0o400 }
    }

    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::File,
            size: HEALTH_SIZE as u64,
            allocated: 0,
            id: self.id,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn open(&self) -> Result<Option<NodeRef>> {
        let bytes = self.read_now()?;
        Ok(Some(mem::arc(Record { id: self.id, bytes, taken: radix_vfs::now() })?))
    }

    fn read(&self, offset: u64, out: &mut dyn Out) -> Result<usize> {
        // Read without a handle (no `open` first): asked now.
        read_bytes(&self.read_now()?, offset, out)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A disk's health record as one handle read it.
struct Record {
    id: u64,
    bytes: Vec<u8, Kmalloc>,
    taken: u64,
}

impl Node for Record {
    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o400 }
    }

    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::File,
            size: HEALTH_SIZE as u64,
            allocated: 0,
            id: self.id,
            created: self.taken,
            modified: self.taken,
            accessed: self.taken,
            changed: self.taken,
            links: 1,
            device: 0,
        }
    }

    fn read(&self, offset: u64, out: &mut dyn Out) -> Result<usize> {
        read_bytes(&self.bytes, offset, out)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
