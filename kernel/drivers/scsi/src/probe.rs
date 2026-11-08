// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Finding out what the disk behind a transport is, as Linux's SCSI scan
//! and `sd` do for a USB stick: what it is (INQUIRY, [`identify`]), that
//! it is ready (TEST UNIT READY, starting it if it asks to be), how big
//! it is (READ CAPACITY), and whether it is write-protected and has a
//! write cache (MODE SENSE): [`medium`], which [`probe`] does after
//! [`identify`].
//!
//! A drive whose medium comes and goes (ADR 0430), a card reader's slot or
//! a stick that says its medium is removable, is looked at now and then
//! as Linux's `sd_check_events` does ([`look`]): TEST UNIT READY says
//! whether a medium is in it, and a UNIT ATTENTION of a medium put in or
//! taken out that one was changed meanwhile. [`prevent_removal`] locks the
//! medium in while it is used, and [`eject`] lets it go and ejects it.

use core::fmt;

use crate::{
    ALL_PAGES, CAPACITY_10_LENGTH, CAPACITY_16_LENGTH, Capacity, Cdb, Data, INQUIRY_LENGTH,
    Inquiry, MODE_SENSE_ALL_LENGTH, Meaning, Reply, Sense, Transport, TransportError, device_type,
    key, write_cache, write_protected,
};

/// How long a command of the probe may take: what Linux gives TEST UNIT
/// READY and READ CAPACITY.
const TIMEOUT_MS: u32 = 30_000;
/// How long a device that is not ready has to become ready: Linux's.
const READY_MS: u32 = 100_000;
/// How long between looks at a device that is not ready yet.
const READY_POLL_MS: u32 = 1_000;
/// How many times a command goes when a UNIT ATTENTION, an abort or a
/// reset of the transport ends it.
const TRIES: u32 = 3;

/// A disk behind a transport, as [`probe`] found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    /// What it says it is.
    pub inquiry: Inquiry,
    /// How big its medium is.
    pub capacity: Capacity,
    /// Whether its medium is write-protected.
    pub read_only: bool,
    /// Whether it says it keeps writes in a cache, which SYNCHRONIZE
    /// CACHE empties.
    pub write_cache: bool,
    /// Whether its reads and writes take 16-byte commands: it has more
    /// blocks than READ(10)'s 32-bit addresses reach.
    pub sixteen: bool,
}

/// Why there is no disk to use behind a transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Absent {
    /// Nothing answers as the logical unit.
    NoUnit,
    /// It is not a disk: its peripheral device type.
    NotDisk(u8),
    /// It has no medium in it.
    NoMedium,
    /// It was unplugged.
    Gone,
    /// It failed a command: which, and why if it said.
    Failed(&'static str, Option<Sense>),
    /// The transport failed.
    Transport(&'static str),
    /// It says a capacity the kernel cannot use.
    BadCapacity(Capacity),
}

impl From<TransportError> for Absent {
    fn from(err: TransportError) -> Absent {
        match err {
            TransportError::Gone => Absent::Gone,
            TransportError::Failed(why) => Absent::Transport(why),
        }
    }
}

impl fmt::Display for Absent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Absent::NoUnit => f.write_str("no logical unit answers"),
            Absent::NotDisk(device_type::CD) => f.write_str("it is a CD or DVD drive"),
            Absent::NotDisk(kind) => write!(f, "it is a SCSI device of type {kind:#04x}"),
            Absent::NoMedium => f.write_str("it has no medium in it"),
            Absent::Gone => f.write_str("it was unplugged"),
            Absent::Failed(command, Some(sense)) => write!(f, "it failed {command}: {sense}"),
            Absent::Failed(command, None) => write!(f, "it failed {command}"),
            Absent::Transport(why) => f.write_str(why),
            Absent::BadCapacity(capacity) => write!(
                f,
                "it says it has {} blocks of {} bytes",
                capacity.blocks, capacity.block_length
            ),
        }
    }
}

/// How a command the probe sent ended.
enum Asked {
    /// It worked, and this many bytes came.
    Came(usize),
    /// It failed, with why if the device said.
    Failed(Option<Sense>),
}

/// Sends `cdb` to `lun`, its data coming into `buffer` (none if it is
/// empty), again after a UNIT ATTENTION, an abort or a reset of the
/// transport, up to [`TRIES`] times.
fn ask<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    cdb: &Cdb,
    buffer: &mut [u8],
) -> Result<Asked, TransportError> {
    let mut last = Err(TransportError::Failed("the command was not sent"));
    for _ in 0..TRIES {
        buffer.fill(0);
        let data = if buffer.is_empty() { Data::None } else { Data::In(&mut *buffer) };
        match transport.command(lun, cdb, data, TIMEOUT_MS) {
            Ok(Reply::Good { residue }) => {
                return Ok(Asked::Came(buffer.len().saturating_sub(residue as usize)));
            }
            Ok(Reply::Failed(sense)) => {
                let again = sense.map(|s| s.meaning());
                if !matches!(again, Some(Meaning::Attention | Meaning::Aborted)) {
                    return Ok(Asked::Failed(sense));
                }
                last = Ok(Asked::Failed(sense));
            }
            Err(TransportError::Gone) => return Err(TransportError::Gone),
            Err(err) => last = Err(err),
        }
    }
    last
}

/// Finds out what the disk at logical unit `lun` behind `transport` is:
/// [`identify`], then [`medium`] for a disk. `wait` waits that many
/// milliseconds, while a device that is not ready gets ready.
pub fn probe<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    wait: &mut dyn FnMut(u32),
) -> Result<Found, Absent> {
    let inquiry = identify(transport, lun)?;
    if !inquiry.is_disk() {
        return Err(Absent::NotDisk(inquiry.device_type));
    }
    medium(transport, lun, inquiry, wait)
}

/// What the device at logical unit `lun` behind `transport` says it is
/// (INQUIRY): a disk or another kind, and whether its medium can be taken
/// out. Fails with [`Absent::NoUnit`] if nothing answers as the unit.
pub fn identify<T: Transport + ?Sized>(transport: &mut T, lun: u8) -> Result<Inquiry, Absent> {
    let mut bytes = [0u8; INQUIRY_LENGTH as usize];
    let inquiry = match ask(transport, lun, &Cdb::inquiry(INQUIRY_LENGTH), &mut bytes)? {
        Asked::Came(came) => Inquiry::parse(&bytes[..came]),
        Asked::Failed(sense) => return Err(Absent::Failed("INQUIRY", sense)),
    };
    let inquiry = inquiry.ok_or(Absent::Failed("INQUIRY", None))?;
    if !inquiry.present() {
        return Err(Absent::NoUnit);
    }
    Ok(inquiry)
}

/// The medium in the disk at logical unit `lun`, which `inquiry` says it
/// is: that it is ready (waiting for it, as `wait` waits), how big it is,
/// and whether it is write-protected and has a write cache. Fails with
/// [`Absent::NoMedium`] if none is in it: an empty card reader's slot.
pub fn medium<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    inquiry: Inquiry,
    wait: &mut dyn FnMut(u32),
) -> Result<Found, Absent> {
    ready(transport, lun, wait)?;
    let capacity = capacity(transport, lun)?;
    if !capacity.usable() {
        return Err(Absent::BadCapacity(capacity));
    }
    let (read_only, write_cache) = modes(transport, lun)?;
    Ok(Found { inquiry, capacity, read_only, write_cache, sixteen: capacity.needs_16() })
}

/// What a look at a drive whose medium comes and goes found ([`look`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// Whether a medium is in it: `None` when it did not say, as when the
    /// transport failed.
    pub medium: Option<bool>,
    /// Whether it said that its medium may have changed since it was last
    /// asked ([`Sense::medium_changed`]): what is in it now is not what
    /// was, though it may look the same.
    pub changed: bool,
}

/// Looks whether a medium is in the drive at logical unit `lun`, as
/// Linux's `sd_check_events` does: TEST UNIT READY, again after a UNIT
/// ATTENTION, up to [`LOOK_TRIES`] times. A medium is in unless the drive
/// says none is (sense code 3Ah), as Linux has it: one getting ready, or
/// stopped, is in. A UNIT ATTENTION of a medium put in or taken out says
/// it changed; one of a reset alone does not, so that a reset of the
/// transport (after a command that took too long) does not take a mounted
/// medium away. Fails only if the device is gone.
pub fn look<T: Transport + ?Sized>(transport: &mut T, lun: u8) -> Result<Look, TransportError> {
    let mut found = Look { medium: None, changed: false };
    for _ in 0..LOOK_TRIES {
        match transport.command(lun, &Cdb::test_unit_ready(), Data::None, TIMEOUT_MS) {
            Ok(Reply::Good { .. }) => {
                found.medium = Some(true);
                return Ok(found);
            }
            Ok(Reply::Failed(Some(sense))) if sense.key == key::UNIT_ATTENTION => {
                found.changed |= sense.medium_changed();
            }
            Ok(Reply::Failed(Some(sense))) if sense.asc == 0x3a => {
                found.medium = Some(false);
                return Ok(found);
            }
            Ok(Reply::Failed(_)) => {
                found.medium = Some(true);
                return Ok(found);
            }
            Err(TransportError::Gone) => return Err(TransportError::Gone),
            // Whether one is in is not known: the next look says.
            Err(TransportError::Failed(_)) => return Ok(found),
        }
    }
    Ok(found)
}

/// How many TEST UNIT READYs a [`look`] sends at most: a drive with a new
/// medium answers two UNIT ATTENTIONs first, as QEMU's do (medium not
/// present, then medium changed).
const LOOK_TRIES: u32 = 4;

/// Locks the medium of the drive at logical unit `lun` in (`prevent`), or
/// lets it go, as Linux's `sd` does while its disk is open: PREVENT ALLOW
/// MEDIUM REMOVAL. Returns whether the drive took it: one that refuses it
/// does not lock, and is asked no more (Linux's `lockable`).
pub fn prevent_removal<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    prevent: bool,
) -> Result<bool, TransportError> {
    let cdb = Cdb::prevent_allow_medium_removal(prevent);
    match ask(transport, lun, &cdb, &mut [])? {
        Asked::Came(_) => Ok(true),
        Asked::Failed(_) => Ok(false),
    }
}

/// Lets the medium of the drive at logical unit `lun` go and ejects it, as
/// util-linux's eject does through Linux's `CDROMEJECT`: PREVENT ALLOW
/// MEDIUM REMOVAL to allow it, then START STOP UNIT with LoEj, which
/// stops the medium and opens a tray, or has a card reader or a stick
/// report no medium until one is put in again. Fails with
/// [`Absent::Failed`] if the drive refuses, as a reader that cannot eject
/// does.
pub fn eject<T: Transport + ?Sized>(transport: &mut T, lun: u8) -> Result<(), Absent> {
    // A drive that does not lock refuses this too: the eject says.
    let _ = prevent_removal(transport, lun, false)?;
    match ask(transport, lun, &Cdb::start_stop_unit(false, true), &mut [])? {
        Asked::Came(_) => Ok(()),
        Asked::Failed(sense) => Err(Absent::Failed("START STOP UNIT", sense)),
    }
}

/// Waits for the device to be ready, starting it once if it asks to be,
/// for up to [`READY_MS`].
fn ready<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    wait: &mut dyn FnMut(u32),
) -> Result<(), Absent> {
    let mut waited = 0;
    let mut started = false;
    loop {
        let sense = match ask(transport, lun, &Cdb::test_unit_ready(), &mut [])? {
            Asked::Came(_) => return Ok(()),
            Asked::Failed(sense) => sense,
        };
        match sense.map(|s| s.meaning()) {
            Some(Meaning::NoMedium) => return Err(Absent::NoMedium),
            Some(Meaning::Error) => return Err(Absent::Failed("TEST UNIT READY", sense)),
            Some(Meaning::BecomingReady) => {}
            Some(Meaning::NeedsStart) if !started => {
                // As Linux: whatever it answers, the next look says.
                started = true;
                ask(transport, lun, &Cdb::start_stop_unit(true, false), &mut [])?;
            }
            Some(Meaning::NeedsStart) => {}
            // Anything else, as a device that does not take the command:
            // reading its capacity says whether it works.
            _ => return Ok(()),
        }
        if waited >= READY_MS {
            return Err(Absent::Failed("TEST UNIT READY", sense));
        }
        wait(READY_POLL_MS);
        waited += READY_POLL_MS;
    }
}

/// Reads how big the medium is: READ CAPACITY(10), and READ CAPACITY(16)
/// for one past what that can say.
fn capacity<T: Transport + ?Sized>(transport: &mut T, lun: u8) -> Result<Capacity, Absent> {
    let failed = |command, sense: Option<Sense>| match sense.map(|s| s.meaning()) {
        Some(Meaning::NoMedium) => Absent::NoMedium,
        _ => Absent::Failed(command, sense),
    };
    let mut bytes = [0u8; CAPACITY_16_LENGTH as usize];
    let ten = match ask(transport, lun, &Cdb::read_capacity_10(), &mut bytes[..CAPACITY_10_LENGTH])?
    {
        Asked::Came(came) => Capacity::parse_10(&bytes[..came]),
        Asked::Failed(sense) => return Err(failed("READ CAPACITY(10)", sense)),
    };
    let ten = ten.ok_or(Absent::Failed("READ CAPACITY(10)", None))?;
    if !ten.needs_16() {
        return Ok(ten);
    }
    let cdb = Cdb::read_capacity_16(CAPACITY_16_LENGTH);
    match ask(transport, lun, &cdb, &mut bytes)? {
        Asked::Came(came) => {
            Capacity::parse_16(&bytes[..came]).ok_or(Absent::Failed("READ CAPACITY(16)", None))
        }
        Asked::Failed(sense) => Err(failed("READ CAPACITY(16)", sense)),
    }
}

/// Whether the medium is write-protected, and whether the device has a
/// write cache: MODE SENSE(6) of every page as usb-storage asks for it,
/// then of page 0 as Linux tries next. A device that answers neither is
/// taken to be writable, and one whose answer has no caching page to
/// write through, as on Linux (whose usb-storage asks no device for the
/// caching page alone, which some do not survive).
fn modes<T: Transport + ?Sized>(transport: &mut T, lun: u8) -> Result<(bool, bool), Absent> {
    let mut bytes = [0u8; MODE_SENSE_ALL_LENGTH as usize];
    for (page, length) in [(ALL_PAGES, MODE_SENSE_ALL_LENGTH), (0, 4)] {
        let cdb = Cdb::mode_sense_6(page, length);
        if let Asked::Came(came) = ask(transport, lun, &cdb, &mut bytes[..usize::from(length)])?
            && let Some(protected) = write_protected(&bytes[..came])
        {
            return Ok((protected, write_cache(&bytes[..came]).unwrap_or(false)));
        }
    }
    Ok((false, false))
}
