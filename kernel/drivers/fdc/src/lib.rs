// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The PC's floppy disk controller for the Radix kernel (roadmap 11h,
//! ADR 0435): the Intel 82077AA, and the controllers of Super I/O chips
//! that copy it, as its datasheet (Intel, order number 290166) describes
//! them, with channel 2 of the ISA DMA controller ([`dma`]) moving the
//! data.
//!
//! The controller has eight ports from its base, 0x3f0 on a PC: the
//! digital output register selects a drive and turns its motor on, the
//! main status register says whether the controller wants a byte or has
//! one, the FIFO carries commands and their results, and the digital
//! input register's top bit is the selected drive's disk change line. A
//! drive is selected on the cable only while its motor is on, so the line
//! is read with the motor on ([`Fdc::changed`]). The line goes up when the
//! disk comes out, and only a step of the drive's heads with a disk in it
//! brings it down: so the driver finds a disk put in by stepping
//! ([`Fdc::disk_in`]), which on a real drive is heard, and the kernel
//! looks only when asked.
//!
//! A command is bytes written to the FIFO one at a time, each when the
//! main status register asks for it; its results are read back the same
//! way. SEEK and RECALIBRATE have none: they end with an interrupt, which
//! SENSE INTERRUPT STATUS answers with where the heads went. The kernel
//! does not route the controller's interrupt (ISA interrupt 6): the driver
//! polls the main status register, and after a seek SENSE INTERRUPT
//! STATUS, as IDE channels are polled (ADR 0244). Before each transfer it
//! seeks to the cylinder itself, so that a controller without implied
//! seeks, or one that moved its heads on its own at the end of a
//! cylinder, reads the right one.
//!
//! Disks are 3.5-inch, of 512-byte sectors written in MFM: 1.44 MB ones
//! at 500 kbit/s and 720 KB ones at 250 kbit/s ([`format`]). A track
//! read at the wrong rate shows no sector IDs, so [`Fdc::detect`] tells
//! them apart with READ ID at each rate the drive has, then checks the
//! last sector of the first track and what the FAT boot sector says.
//! Each transfer moves at most a cylinder, both heads, in one command,
//! which the DMA controller's count ends; the kernel's buffer for it is
//! below 16 MiB and within a 64 KiB line, as ISA DMA needs.
//!
//! A track is formatted with FORMAT TRACK ([`Fdc::format_track`], ADR
//! 0436): the controller writes each sector's ID from a list the DMA
//! controller hands it ([`Format::track_ids`]), then the sector, filled
//! with [`format::FILLER`].
//!
//! When it breaks, the kernel log says what the controller answered: the
//! error's name ([`Error::as_str`]) with its status bytes. The crate is a
//! compartment with no `unsafe` code; its tests run it against a
//! simulated controller, DMA controller and drives.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod dma;
pub mod format;
#[cfg(test)]
mod tests;

use radix_hw::IoPorts;

use self::dma::IsaDma;
pub use self::format::{DriveKind, Format, Place, Rate, SECTOR};

/// The controller's ports.
pub const PORTS: u16 = 8;
/// The first controller's base.
pub const BASE: u16 = 0x3f0;
/// The most drives a controller has.
pub const DRIVES: u8 = 4;
/// The most bytes one transfer moves: a cylinder of a 1.44 MB disk.
pub const MAX_TRANSFER: usize = 2 * 18 * SECTOR;

/// The registers, from the base.
mod reg {
    /// Digital output register.
    pub const DOR: u16 = 2;
    /// Main status register when read.
    pub const MSR: u16 = 4;
    pub const FIFO: u16 = 5;
    /// Digital input register when read, configuration control register
    /// when written.
    pub const DIR: u16 = 7;
    pub const CCR: u16 = 7;
}

/// The digital output register's bits, above the drive select in bits 0
/// and 1.
mod dor {
    /// The controller is out of reset.
    pub const RUN: u8 = 1 << 2;
    /// The interrupt and DMA request lines are driven: DMA needs it.
    pub const DMA: u8 = 1 << 3;
    /// Drive 0's motor; drive n's is this shifted left by n.
    pub const MOTOR: u8 = 1 << 4;
}

/// The main status register's bits.
mod msr {
    /// The FIFO takes or has a byte.
    pub const RQM: u8 = 1 << 7;
    /// The byte goes to the host: a result.
    pub const DIO: u8 = 1 << 6;
    /// A command is under way.
    pub const BUSY: u8 = 1 << 4;
}

/// Command bytes.
mod cmd {
    pub const SPECIFY: u8 = 0x03;
    pub const SENSE_DRIVE: u8 = 0x04;
    pub const WRITE: u8 = 0x05;
    pub const READ: u8 = 0x06;
    pub const RECALIBRATE: u8 = 0x07;
    pub const SENSE_INTERRUPT: u8 = 0x08;
    pub const READ_ID: u8 = 0x0a;
    pub const FORMAT: u8 = 0x0d;
    pub const SEEK: u8 = 0x0f;
    pub const VERSION: u8 = 0x10;
    pub const CONFIGURE: u8 = 0x13;
    /// Multi-track: a read or write goes on to the second head's track.
    pub const MT: u8 = 0x80;
    /// MFM, the recording every PC disk since the 360 KB one uses.
    pub const MFM: u8 = 0x40;
}

/// Status register bits.
mod st {
    /// ST0: how the command ended, in bits 7 and 6.
    pub const ENDED: u8 = 0xc0;
    pub const ABNORMAL: u8 = 0x40;
    pub const INVALID: u8 = 0x80;
    /// ST0: a seek ended.
    pub const SEEK_END: u8 = 1 << 5;
    /// ST0: the drive failed: no track 0 after a recalibration's steps.
    pub const EQUIPMENT: u8 = 1 << 4;
    /// ST0: the drive, in bits 1 and 0.
    pub const DRIVE: u8 = 0x03;
    /// ST1: the end of the cylinder was reached.
    pub const END_OF_CYLINDER: u8 = 1 << 7;
    /// ST1: a CRC error in a sector's ID or data.
    pub const DATA_ERROR: u8 = 1 << 5;
    /// ST1: the DMA controller did not keep up.
    pub const OVERRUN: u8 = 1 << 4;
    /// ST1: the sector was not found.
    pub const NO_DATA: u8 = 1 << 2;
    /// ST1: the disk is write-protected.
    pub const NOT_WRITABLE: u8 = 1 << 1;
    /// ST1: no address mark: nothing written on the track.
    pub const MISSING_MARK: u8 = 1 << 0;
    /// ST2: a CRC error in a sector's data.
    pub const DATA_CRC: u8 = 1 << 5;
    /// ST2: the sector IDs are of another cylinder.
    pub const WRONG_CYLINDER: u8 = 1 << 4;
    pub const BAD_CYLINDER: u8 = 1 << 1;
    /// ST2: no data address mark.
    pub const MISSING_DATA_MARK: u8 = 1 << 0;
    /// ST3: the disk is write-protected.
    pub const WRITE_PROTECT: u8 = 1 << 6;
}

/// What VERSION answers on an 82077AA and later controllers; an older
/// one (an 8272A or a µPD765) calls the command invalid.
const ENHANCED: u8 = 0x90;
/// CONFIGURE's second parameter: no implied seek, the FIFO on with a
/// threshold of 8 bytes, and drive polling off, as nothing listens for
/// its interrupts.
const CONFIGURATION: u8 = 0x10 | 0x07;
/// SPECIFY's second byte: a head load time of 2 ms (at 500 kbit/s; 4 at
/// 250), and DMA rather than programmed I/O.
const HEAD_LOAD: u8 = 0x02;
/// The gap length to transfer whole sectors: all of them.
const WHOLE_SECTORS: u8 = 0xff;

/// How long the controller may take to take or give a command's byte.
const BYTE_US: u32 = 50_000;
/// How long a seek or a recalibration may take: 80 steps of up to 16 ms,
/// and the heads' settling.
const SEEK_US: u32 = 3_000_000;
/// How long a transfer may take: a cylinder at 250 kbit/s is two and a
/// half turns of the disk, 500 ms, and the turn that finds the first
/// sector. A format of a track takes the turn that finds the index hole
/// and one more.
const TRANSFER_US: u32 = 3_000_000;
/// How long a reset is held, and how long the controller may take to be
/// ready after it.
const RESET_HOLD_US: u32 = 20;
const RESET_US: u32 = 100_000;

/// What a command could not do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The controller did not take or give a byte in time, or did not
    /// finish: it needs a reset.
    Timeout,
    /// It called the command invalid.
    Invalid,
    /// The drive did not get to track 0, or to the cylinder: no drive, or
    /// one that does not step.
    Seek,
    /// The sector was not found on its track: nothing written there, or a
    /// disk of another format.
    NotFound,
    /// A CRC error in the sector's ID or data: a bad spot on the disk.
    Crc,
    /// The DMA controller did not keep up.
    Overrun,
    /// The disk is write-protected.
    WriteProtected,
    /// The command failed, as its status bytes say.
    Failed {
        /// Status register 0: how it ended, and the drive.
        st0: u8,
        /// Status register 1: what went wrong reading the track.
        st1: u8,
        /// Status register 2: what went wrong with the sector.
        st2: u8,
    },
    /// The request does not fit: past the disk, more than a cylinder, or a
    /// buffer ISA DMA cannot reach.
    BadRequest,
}

impl Error {
    /// What it means, for the log.
    pub fn as_str(&self) -> &'static str {
        match self {
            Error::Timeout => "the floppy disk controller did not answer",
            Error::Invalid => "the floppy disk controller refused the command",
            Error::Seek => "the drive's heads did not get to the track",
            Error::NotFound => "the sector was not found on its track",
            Error::Crc => "the sector could not be read (a CRC error)",
            Error::Overrun => "the DMA controller did not keep up",
            Error::WriteProtected => "the disk is write-protected",
            Error::Failed { .. } => "the floppy disk controller failed the command",
            Error::BadRequest => "the request does not fit the disk",
        }
    }

    /// Whether the command may work if it goes again: a marginal sector,
    /// or a controller that fell behind.
    pub fn may_pass(&self) -> bool {
        matches!(self, Error::NotFound | Error::Crc | Error::Overrun | Error::Failed { .. })
    }
}

/// Which way data moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the disk into memory.
    Read,
    /// From memory onto the disk.
    Write,
}

/// A sector's ID, as READ ID finds the next one on a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Id {
    /// The cylinder the sector says it is on.
    pub cylinder: u8,
    /// The head it says it is under.
    pub head: u8,
    /// Its number on the track, from 1.
    pub sector: u8,
    /// Its size, as 128 << this.
    pub size: u8,
}

/// What [`Fdc::detect`] found on a disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// A disk of this format.
    Disk(&'static Format),
    /// No sector IDs at any rate: an unformatted disk.
    Unformatted,
    /// A disk whose FAT boot sector says it has this many sectors a track
    /// and heads, a format the driver does not read.
    Other {
        /// Sectors a track.
        sectors: u16,
        /// Heads: sides of the disk.
        heads: u16,
    },
}

/// Waits that double from 10 µs to 4 ms, for at most a limit: how the
/// driver waits for the controller between looks.
struct Backoff {
    waited: u32,
    step: u32,
    limit: u32,
}

impl Backoff {
    fn new(limit_us: u32) -> Backoff {
        Backoff { waited: 0, step: 10, limit: limit_us }
    }

    /// Waits the next step in `wait`; false once the limit is spent.
    fn wait(&mut self, wait: &mut dyn FnMut(u32)) -> bool {
        if self.waited >= self.limit {
            return false;
        }
        wait(self.step);
        self.waited = self.waited.saturating_add(self.step);
        self.step = (self.step * 2).min(4_000);
        true
    }
}

/// Waits until `done`, for at most `limit_us`. Whether `done` came.
fn poll(wait: &mut dyn FnMut(u32), limit_us: u32, mut done: impl FnMut() -> bool) -> bool {
    let mut backoff = Backoff::new(limit_us);
    loop {
        if done() {
            return true;
        }
        if !backoff.wait(wait) {
            return false;
        }
    }
}

/// A floppy disk controller and channel 2 of the ISA DMA controller,
/// driven by one thread: every command waits, asleep in `wait`, until
/// it is done.
pub struct Fdc<P> {
    ports: P,
    dma: IsaDma<P>,
    /// What the digital output register holds.
    dor: u8,
    /// The data rate set, and SPECIFY's step rate for it; `None` after a
    /// reset.
    rate: Option<(Rate, u8)>,
    /// It answered VERSION as an 82077AA does.
    enhanced: bool,
    /// The cylinder each drive's heads are over, as the controller counts
    /// them; `None` until a recalibration.
    cylinders: [Option<u8>; DRIVES as usize],
}

impl<P: IoPorts> Fdc<P> {
    /// The controller at `ports`, with the DMA controller's `dma` and
    /// `pages` ([`dma::PORTS`], [`dma::PAGE_PORTS`]). Nothing is touched
    /// until [`Fdc::reset`].
    pub fn new(ports: P, dma: P, pages: P) -> Fdc<P> {
        Fdc {
            ports,
            dma: IsaDma::new(dma, pages),
            dor: dor::RUN | dor::DMA,
            rate: None,
            enhanced: false,
            cylinders: [None; DRIVES as usize],
        }
    }

    /// Whether anything answers at the ports: an absent controller's
    /// status register reads all ones.
    pub fn present(&self) -> bool {
        self.ports.read8(reg::MSR) != 0xff
    }

    /// Whether it answered VERSION as an 82077AA does.
    pub fn enhanced(&self) -> bool {
        self.enhanced
    }

    /// Resets the controller, which forgets where the drives' heads are
    /// and the data rate, and turns the motors off: answers the reset's
    /// interrupt, asks its version and, on an 82077AA, configures it.
    pub fn reset(&mut self, wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        self.dma.stop();
        self.ports.write8(reg::DOR, 0);
        wait(RESET_HOLD_US);
        self.dor = dor::RUN | dor::DMA;
        self.ports.write8(reg::DOR, self.dor);
        self.rate = None;
        self.cylinders = [None; DRIVES as usize];
        let ready = |fdc: &Self| fdc.ports.read8(reg::MSR) & (msr::RQM | msr::DIO) == msr::RQM;
        if !poll(wait, RESET_US, || ready(self)) {
            return Err(Error::Timeout);
        }
        // Out of reset, the controller interrupts once for each drive it
        // polls; an older one, or one whose polling is off, once or not
        // at all, and calls the next SENSE INTERRUPT STATUS invalid.
        for _ in 0..DRIVES {
            match self.sense_interrupt(wait) {
                Ok(_) => {}
                Err(Error::Invalid) => break,
                Err(err) => return Err(err),
            }
        }
        self.enhanced = self.version(wait)? == ENHANCED;
        if self.enhanced {
            self.command(&[cmd::CONFIGURE, 0, CONFIGURATION, 0], wait)?;
        }
        // The step rate for the seeks before a disk's format is known:
        // a 1.44 MB drive's.
        self.set_rate(&format::HD_1440, wait)
    }

    /// Turns drive `unit`'s motor on or off; on, it is the drive
    /// selected. Returns whether it was on. A motor takes about half a
    /// second to come up to speed, which the caller waits before a
    /// transfer.
    pub fn motor(&mut self, unit: u8, on: bool) -> bool {
        let bit = dor::MOTOR << unit;
        let was = self.dor & bit != 0;
        if on {
            self.dor = (self.dor & !0x03) | bit | unit;
        } else {
            self.dor &= !bit;
        }
        self.ports.write8(reg::DOR, self.dor);
        was
    }

    /// Whether drive `unit`'s disk change line is up: its disk came out
    /// since its heads last stepped with a disk in. Selects the drive,
    /// whose motor must be on.
    pub fn changed(&mut self, unit: u8) -> bool {
        self.select(unit);
        self.ports.read8(reg::DIR) & 0x80 != 0
    }

    /// Selects drive `unit`, keeping the motors as they are.
    fn select(&mut self, unit: u8) {
        if self.dor & 0x03 != unit {
            self.dor = (self.dor & !0x03) | unit;
            self.ports.write8(reg::DOR, self.dor);
        }
    }

    /// Whether a disk is in drive `unit`, whose motor is on: steps its
    /// heads to cylinder 1 and back, which brings a drive's change line
    /// down if a disk is in it, and reads the line. A real drive is heard
    /// doing it.
    pub fn disk_in(&mut self, unit: u8, wait: &mut dyn FnMut(u32)) -> Result<bool, Error> {
        if !self.changed(unit) {
            return Ok(true);
        }
        self.recalibrate(unit, wait)?;
        self.seek(unit, 1, wait)?;
        self.seek(unit, 0, wait)?;
        Ok(!self.changed(unit))
    }

    /// Brings drive `unit`'s heads to track 0. A drive that does not get
    /// there in 80 steps is tried once more, as one of more than 80
    /// cylinders needs; one that still does not fails with
    /// [`Error::Seek`]: there is no drive, or it is broken.
    pub fn recalibrate(&mut self, unit: u8, wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        self.cylinders[usize::from(unit)] = None;
        let mut result = Err(Error::Seek);
        for _ in 0..2 {
            self.command(&[cmd::RECALIBRATE, unit], wait)?;
            result = self.seek_end(unit, wait);
            if result.is_ok() {
                self.cylinders[usize::from(unit)] = Some(0);
                break;
            }
        }
        result.map(|_| ())
    }

    /// Moves drive `unit`'s heads over `cylinder`, recalibrating first if
    /// where they are is not known.
    pub fn seek(&mut self, unit: u8, cylinder: u8, wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        if self.cylinders[usize::from(unit)].is_none() {
            self.recalibrate(unit, wait)?;
        }
        self.command(&[cmd::SEEK, unit, cylinder], wait)?;
        let reached = self.seek_end(unit, wait);
        self.cylinders[usize::from(unit)] = reached.ok();
        match reached {
            Ok(at) if at == cylinder => Ok(()),
            Ok(_) => Err(Error::Seek),
            Err(err) => Err(err),
        }
    }

    /// Waits for drive `unit`'s seek to end: asks SENSE INTERRUPT STATUS
    /// until it says so. Returns the cylinder the heads are over.
    fn seek_end(&mut self, unit: u8, wait: &mut dyn FnMut(u32)) -> Result<u8, Error> {
        let mut backoff = Backoff::new(SEEK_US);
        loop {
            // The drive's bit in the status register is up while it steps.
            let stepping = self.ports.read8(reg::MSR) & (1 << unit) != 0;
            if !stepping {
                match self.sense_interrupt(wait) {
                    Ok((st0, cylinder)) if st0 & st::DRIVE == unit && st0 & st::SEEK_END != 0 => {
                        let failed = st0 & (st::ENDED | st::EQUIPMENT) != 0;
                        return if failed { Err(Error::Seek) } else { Ok(cylinder) };
                    }
                    // Another drive's interrupt, or none yet.
                    Ok(_) | Err(Error::Invalid) => {}
                    Err(err) => return Err(err),
                }
            }
            if !backoff.wait(wait) {
                return Err(Error::Timeout);
            }
        }
    }

    /// SENSE INTERRUPT STATUS: the ST0 of the interrupt waiting, and the
    /// cylinder its drive's heads are over. [`Error::Invalid`] when none
    /// waits.
    fn sense_interrupt(&mut self, wait: &mut dyn FnMut(u32)) -> Result<(u8, u8), Error> {
        self.write_bytes(&[cmd::SENSE_INTERRUPT], wait)?;
        let st0 = self.read_byte(wait)?;
        if st0 == st::INVALID {
            return Err(Error::Invalid);
        }
        let cylinder = self.read_byte(wait)?;
        Ok((st0, cylinder))
    }

    /// VERSION's answer: [`ENHANCED`] on an 82077AA, the invalid
    /// command's 0x80 on an older controller.
    fn version(&mut self, wait: &mut dyn FnMut(u32)) -> Result<u8, Error> {
        self.write_bytes(&[cmd::VERSION], wait)?;
        self.read_byte(wait)
    }

    /// Whether the disk in drive `unit` is write-protected: SENSE DRIVE
    /// STATUS.
    pub fn write_protected(&mut self, unit: u8, wait: &mut dyn FnMut(u32)) -> Result<bool, Error> {
        self.write_bytes(&[cmd::SENSE_DRIVE, unit], wait)?;
        let st3 = self.read_byte(wait)?;
        Ok(st3 & st::WRITE_PROTECT != 0)
    }

    /// Sets the data rate of `format`, and the step rate and head times
    /// that go with it, if they are not set already.
    fn set_rate(&mut self, format: &Format, wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let wanted = (format.rate, format.step);
        if self.rate == Some(wanted) {
            return Ok(());
        }
        self.rate = None;
        self.ports.write8(reg::CCR, format.rate.bits());
        self.command(&[cmd::SPECIFY, format.step, HEAD_LOAD], wait)?;
        self.rate = Some(wanted);
        Ok(())
    }

    /// READ ID: the ID of the next sector under drive `unit`'s `head`, at
    /// the data rate of `format`.
    pub fn read_id(
        &mut self,
        unit: u8,
        head: u8,
        format: &Format,
        wait: &mut dyn FnMut(u32),
    ) -> Result<Id, Error> {
        self.set_rate(format, wait)?;
        self.write_bytes(&[cmd::READ_ID | cmd::MFM, head << 2 | unit], wait)?;
        // It ends when the next ID passes under the head: within a turn.
        self.execution(TRANSFER_US, wait)?;
        let [st0, st1, st2, cylinder, head, sector, size] = self.results(wait)?;
        judge(st0, st1, st2)?;
        Ok(Id { cylinder, head, sector, size })
    }

    /// Moves `count` sectors from sector `first` of the disk of `format`
    /// in drive `unit`, whose motor is up to speed, between it and the
    /// buffer at `buffer`, which ISA DMA reaches: at most a cylinder,
    /// from `first` to its end.
    #[allow(clippy::too_many_arguments, reason = "a transfer is all of these")]
    pub fn transfer(
        &mut self,
        unit: u8,
        direction: Direction,
        format: &Format,
        first: u32,
        count: u32,
        buffer: u64,
        wait: &mut dyn FnMut(u32),
    ) -> Result<(), Error> {
        let place = format.place(first).ok_or(Error::BadRequest)?;
        if count == 0 || count > format.left_in_cylinder(first) {
            return Err(Error::BadRequest);
        }
        let bytes = count as usize * SECTOR;
        self.set_rate(format, wait)?;
        self.seek(unit, place.cylinder, wait)?;
        self.dma.start(buffer, bytes, direction == Direction::Read)?;
        let command = match direction {
            Direction::Read => cmd::READ,
            Direction::Write => cmd::WRITE,
        };
        let sent = self.write_bytes(
            &[
                command | cmd::MT | cmd::MFM,
                place.head << 2 | unit,
                place.cylinder,
                place.head,
                place.sector,
                format::SIZE_CODE,
                format.sectors,
                format.gap,
                WHOLE_SECTORS,
            ],
            wait,
        );
        let ended = sent.and_then(|()| self.execution(TRANSFER_US, wait));
        let results = ended.and_then(|()| self.results(wait));
        self.dma.stop();
        let [st0, st1, st2, ..] = results?;
        judge(st0, st1, st2)
    }

    /// Formats track `cylinder`, `head` of the disk in drive `unit`, whose
    /// motor is up to speed, as a track of `format`: FORMAT TRACK at the
    /// format's data rate, the IDs of its sectors read by DMA from
    /// `buffer`, where the caller put what [`Format::track_ids`] writes.
    /// What the track held is gone, its sectors filled with
    /// [`format::FILLER`].
    pub fn format_track(
        &mut self,
        unit: u8,
        format: &Format,
        (cylinder, head): (u8, u8),
        buffer: u64,
        wait: &mut dyn FnMut(u32),
    ) -> Result<(), Error> {
        if cylinder >= format.cylinders || head >= format.heads {
            return Err(Error::BadRequest);
        }
        let bytes = usize::from(format.sectors) * format::ID_BYTES;
        self.set_rate(format, wait)?;
        self.seek(unit, cylinder, wait)?;
        // The IDs go from memory to the controller, as a write's data do.
        self.dma.start(buffer, bytes, false)?;
        let sent = self.write_bytes(
            &[
                cmd::FORMAT | cmd::MFM,
                head << 2 | unit,
                format::SIZE_CODE,
                format.sectors,
                format.format_gap,
                format::FILLER,
            ],
            wait,
        );
        let ended = sent.and_then(|()| self.execution(TRANSFER_US, wait));
        let results = ended.and_then(|()| self.results(wait));
        self.dma.stop();
        let [st0, st1, st2, ..] = results?;
        judge(st0, st1, st2)
    }

    /// Finds what disk is in drive `unit`, a drive of `kind` whose motor
    /// is up to speed: tries each format the drive reads with READ ID at
    /// its data rate, then reads the last sector of the first track and
    /// the boot sector into `buffer`, which `memory` reads back.
    pub fn detect(
        &mut self,
        unit: u8,
        kind: DriveKind,
        buffer: u64,
        memory: &mut dyn FnMut(u64, &mut [u8; SECTOR]),
        wait: &mut dyn FnMut(u32),
    ) -> Result<Found, Error> {
        self.seek(unit, 0, wait)?;
        for format in kind.formats() {
            match self.read_id(unit, 0, format, wait) {
                Ok(id) if id.size == format::SIZE_CODE => {}
                Ok(_) => continue,
                Err(err) if err.may_pass() || err == Error::Seek => continue,
                Err(err) => return Err(err),
            }
            let last = u32::from(format.sectors) - 1;
            match self.transfer(unit, Direction::Read, format, last, 1, buffer, wait) {
                Ok(()) => {}
                // A track of fewer sectors: a format of the same rate
                // with fewer, which none of those known is.
                Err(Error::NotFound) => continue,
                Err(err) => return Err(err),
            }
            self.transfer(unit, Direction::Read, format, 0, 1, buffer, wait)?;
            let mut boot = [0u8; SECTOR];
            memory(buffer, &mut boot);
            return Ok(match format::boot_geometry(&boot) {
                Some((sectors, heads))
                    if sectors != u16::from(format.sectors) || heads != u16::from(format.heads) =>
                {
                    Found::Other { sectors, heads }
                }
                _ => Found::Disk(format),
            });
        }
        Ok(Found::Unformatted)
    }

    /// Writes a command that has no results.
    fn command(&mut self, bytes: &[u8], wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        self.write_bytes(bytes, wait)?;
        // A command the controller calls invalid answers 0x80 at once.
        if self.ports.read8(reg::MSR) & (msr::RQM | msr::DIO) == msr::RQM | msr::DIO {
            let _ = self.read_byte(wait)?;
            return Err(Error::Invalid);
        }
        Ok(())
    }

    /// Writes a command's bytes, each when the controller asks for it.
    fn write_bytes(&mut self, bytes: &[u8], wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        for &byte in bytes {
            let asks = |fdc: &Self| fdc.ports.read8(reg::MSR) & (msr::RQM | msr::DIO) == msr::RQM;
            if !poll(wait, BYTE_US, || asks(self)) {
                return Err(Error::Timeout);
            }
            self.ports.write8(reg::FIFO, byte);
        }
        Ok(())
    }

    /// Reads a result byte when the controller has one.
    fn read_byte(&mut self, wait: &mut dyn FnMut(u32)) -> Result<u8, Error> {
        let mut ready = false;
        let mut asks = false;
        poll(wait, BYTE_US, || {
            let status = self.ports.read8(reg::MSR);
            ready = status & (msr::RQM | msr::DIO) == msr::RQM | msr::DIO;
            asks = status & (msr::RQM | msr::DIO | msr::BUSY) == msr::RQM;
            ready || asks
        });
        if !ready {
            // It wants a command instead: the one before had fewer
            // results than read.
            return Err(if asks { Error::Invalid } else { Error::Timeout });
        }
        Ok(self.ports.read8(reg::FIFO))
    }

    /// Waits for a read or write to end: for its results, for at most
    /// `limit_us`.
    fn execution(&mut self, limit_us: u32, wait: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let has_results =
            |fdc: &Self| fdc.ports.read8(reg::MSR) & (msr::RQM | msr::DIO) == msr::RQM | msr::DIO;
        if poll(wait, limit_us, || has_results(self)) { Ok(()) } else { Err(Error::Timeout) }
    }

    /// The seven result bytes of a read, a write or a READ ID.
    fn results(&mut self, wait: &mut dyn FnMut(u32)) -> Result<[u8; 7], Error> {
        let mut bytes = [0u8; 7];
        for byte in &mut bytes {
            *byte = self.read_byte(wait)?;
        }
        Ok(bytes)
    }
}

/// How a read, write or READ ID that ended with `st0`, `st1` and `st2`
/// went. A transfer that the DMA controller's count ended at the last
/// sector of a cylinder may say it reached the end of the cylinder, and
/// nothing else: that is done.
fn judge(st0: u8, st1: u8, st2: u8) -> Result<(), Error> {
    match st0 & st::ENDED {
        0 => return Ok(()),
        st::INVALID => return Err(Error::Invalid),
        st::ABNORMAL => {}
        _ => return Err(Error::Failed { st0, st1, st2 }),
    }
    if st1 & st::NOT_WRITABLE != 0 {
        Err(Error::WriteProtected)
    } else if st1 & st::OVERRUN != 0 {
        Err(Error::Overrun)
    } else if st1 & st::DATA_ERROR != 0 || st2 & st::DATA_CRC != 0 {
        Err(Error::Crc)
    } else if st2 & (st::WRONG_CYLINDER | st::BAD_CYLINDER) != 0 || st0 & st::EQUIPMENT != 0 {
        Err(Error::Seek)
    } else if st1 & (st::NO_DATA | st::MISSING_MARK) != 0 || st2 & st::MISSING_DATA_MARK != 0 {
        Err(Error::NotFound)
    } else if st1 == st::END_OF_CYLINDER && st2 == 0 {
        Ok(())
    } else {
        Err(Error::Failed { st0, st1, st2 })
    }
}
