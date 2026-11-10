// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB floppy drives' transport (ADR 0436): the *Universal Serial Bus
//! Mass Storage Class Control/Bulk/Interrupt (CBI) Transport, Revision
//! 1.1*, which most USB floppy drives speak, with the commands of the UFI
//! command set (subclass 4, `radix_scsi::ufi`).
//!
//! A command goes in three stages, as over the Bulk-Only Transport, but
//! by other pipes:
//!
//! - The command block goes on the default control pipe, as the class's
//!   Accept Device-Specific Command request (ADSC), padded to the 12
//!   bytes every UFI command has. A device that refuses the command
//!   stalls the request: the command failed.
//! - The data, if any, move on the bulk IN or OUT pipe. A stall ends the
//!   stage early; its halt is cleared, and the status says how it went.
//! - The status comes on the interrupt IN pipe, two bytes: for a UFI
//!   device, the additional sense code and qualifier of the command, all
//!   zeros when it worked; for another, a type of 0 and a status of
//!   passed, failed, phase error or persistent failure. A
//!   UFI device's status for REQUEST SENSE and INQUIRY says nothing of
//!   them, so those are taken as passed.
//!
//! A device of the CB protocol (protocol 1) has no interrupt pipe, and
//! says nothing of how a command ended: it is asked with REQUEST SENSE
//! after each one, as FreeBSD's `umass` does, and a sense key of no sense
//! means the command worked.
//!
//! A command that failed has the device asked why with REQUEST SENSE, as
//! over Bulk-Only. A device that broke the protocol, or did not answer in
//! time, is reset: the Command Block Reset, a SEND DIAGNOSTIC
//! whose parameters say so, then both bulk pipes' halts cleared.

use radix_scsi::{Cdb, Data, Reply, SENSE_LENGTH, Sense, Transport, TransportError, key, op};

use super::{PipeError, Pipes};
use crate::{CLASS, Setup, TO_INTERFACE};

#[cfg(test)]
mod tests;

/// The mass storage class's UFI subclass: a floppy drive's commands.
pub const SUBCLASS_UFI: u8 = 0x04;
/// The CBI protocol, with a command completion interrupt.
pub const PROTOCOL_CBI: u8 = 0x00;
/// The CB protocol: CBI with no interrupt pipe.
pub const PROTOCOL_CB: u8 = 0x01;
/// A UFI command block's length: every command is padded to it.
pub const COMMAND_LENGTH: usize = 12;
/// The interrupt data block's length.
pub const STATUS_LENGTH: usize = 2;

/// Accept Device-Specific Command: the class request that carries a
/// command block.
const ADSC: u8 = 0x00;
/// The command block of a Command Block Reset: SEND DIAGNOSTIC
/// with SelfTest clear and the rest of the block all ones.
const RESET_BLOCK: [u8; COMMAND_LENGTH] =
    [0x1d, 0x04, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

impl Setup {
    /// Accept Device-Specific Command to interface `interface`: the
    /// `length` bytes of a command block follow.
    pub fn adsc(interface: u8, length: u16) -> Setup {
        Setup {
            request_type: CLASS | TO_INTERFACE,
            request: ADSC,
            value: 0,
            index: u16::from(interface),
            length,
        }
    }
}

/// The interrupt pipe of a CBI interface, beside its bulk and control
/// pipes.
pub trait InterruptPipe {
    /// Reads the interrupt data block the device sends at the end of a
    /// command into `data`, waiting at most `timeout_ms`; returns how many
    /// bytes came.
    fn interrupt(&mut self, data: &mut [u8], timeout_ms: u32) -> Result<usize, PipeError>;
}

/// How a command ended, as its status stage says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Passed,
    Failed,
    /// The device does not say: a CB device's, which REQUEST SENSE tells.
    Unknown,
}

/// A CBI or CB interface's transport: UFI commands to its drive, over its
/// pipes.
pub struct Cbi<P> {
    pipes: P,
    /// Its number, which the class's requests go to.
    interface: u8,
    /// Whether it has an interrupt pipe (CBI) or not (CB).
    interrupt: bool,
    /// Whether its commands are UFI's, whose status is a sense code.
    ufi: bool,
}

impl<P: Pipes + InterruptPipe> Cbi<P> {
    /// The transport of interface `interface`, over `pipes`: of the CBI
    /// protocol with `interrupt`, else of the CB protocol; `ufi` for a
    /// device of the UFI subclass.
    pub fn new(pipes: P, interface: u8, interrupt: bool, ufi: bool) -> Cbi<P> {
        Cbi { pipes, interface, interrupt, ufi }
    }

    /// Its pipes.
    pub fn pipes(&mut self) -> &mut P {
        &mut self.pipes
    }

    /// The Command Block Reset, then both bulk pipes' halts
    /// cleared: the device is ready for the next command.
    pub fn reset(&mut self) -> Result<(), TransportError> {
        let gone = |err| {
            if err == PipeError::Gone {
                TransportError::Gone
            } else {
                TransportError::Failed("the reset failed")
            }
        };
        let setup = Setup::adsc(self.interface, COMMAND_LENGTH as u16);
        self.pipes.control(setup, Data::Out(&RESET_BLOCK)).map_err(gone)?;
        self.pipes.clear_halt(true).map_err(gone)?;
        self.pipes.clear_halt(false).map_err(gone)
    }

    /// Resets the device after it broke the protocol; fails the command
    /// with `why`.
    fn recover(&mut self, why: &'static str) -> TransportError {
        match self.reset() {
            Err(TransportError::Gone) => TransportError::Gone,
            _ => TransportError::Failed(why),
        }
    }

    /// Runs one command's three stages: returns how it ended, and how many
    /// bytes of its data moved.
    fn run(
        &mut self,
        cdb: &Cdb,
        data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<(Status, usize), TransportError> {
        let mut block = [0u8; COMMAND_LENGTH];
        let bytes = cdb.bytes();
        let n = bytes.len().min(COMMAND_LENGTH);
        block[..n].copy_from_slice(&bytes[..n]);
        let setup = Setup::adsc(self.interface, COMMAND_LENGTH as u16);
        match self.pipes.control(setup, Data::Out(&block)) {
            Ok(_) => {}
            // The device refused the command: REQUEST SENSE says why.
            Err(PipeError::Stall(_)) => return Ok((Status::Failed, 0)),
            Err(PipeError::Gone) => return Err(TransportError::Gone),
            Err(_) => return Err(self.recover("it did not take a command")),
        }
        let into_host = data.into_host();
        let mut moved = 0;
        if !data.is_empty() {
            match self.pipes.bulk(data, timeout_ms) {
                Ok(came) => moved = came,
                // The device ended the data early; the status says why.
                Err(PipeError::Stall(came)) => {
                    moved = came;
                    self.pipes.clear_halt(into_host).map_err(|err| match err {
                        PipeError::Gone => TransportError::Gone,
                        _ => self.recover("its data pipe stayed halted"),
                    })?;
                }
                Err(PipeError::Gone) => return Err(TransportError::Gone),
                Err(PipeError::TimedOut) => return Err(self.recover("it did not answer in time")),
                Err(PipeError::Failed) => return Err(self.recover("its data did not move")),
            }
        }
        if !self.interrupt {
            return Ok((Status::Unknown, moved));
        }
        let mut status = [0u8; STATUS_LENGTH];
        match self.pipes.interrupt(&mut status, timeout_ms) {
            Ok(STATUS_LENGTH) => {}
            Ok(_) => return Err(self.recover("it sent a status that is not valid")),
            Err(PipeError::Gone) => return Err(TransportError::Gone),
            Err(PipeError::TimedOut) => return Err(self.recover("it did not answer in time")),
            Err(_) => return Err(self.recover("its status could not be read")),
        }
        let status = if self.ufi {
            // The command's additional sense code: none, if it worked.
            // REQUEST SENSE and INQUIRY leave the sense data as it was.
            let says_nothing = matches!(cdb.opcode(), op::REQUEST_SENSE | op::INQUIRY);
            if says_nothing || status[0] == 0 { Status::Passed } else { Status::Failed }
        } else {
            // A type of 0, and the status in the low two bits.
            match (status[0], status[1] & 0x03) {
                (0, 0) => Status::Passed,
                (0, 1 | 3) => Status::Failed,
                (0, _) => return Err(self.recover("it reported a phase error")),
                _ => return Err(self.recover("it sent a status that is not valid")),
            }
        };
        Ok((status, moved))
    }

    /// Asks the device why its last command failed: REQUEST SENSE, whose
    /// own status is not read for a UFI device.
    fn sense(&mut self, timeout_ms: u32) -> Result<Option<Sense>, TransportError> {
        let mut bytes = [0u8; SENSE_LENGTH as usize];
        let cdb = Cdb::request_sense(SENSE_LENGTH);
        match self.run(&cdb, Data::In(&mut bytes), timeout_ms)? {
            (Status::Failed, _) => Ok(None),
            (_, came) => Ok(Sense::parse(&bytes[..came.min(bytes.len())])),
        }
    }
}

impl<P: Pipes + InterruptPipe> Transport for Cbi<P> {
    /// Sends `cdb` to the drive. A UFI drive has one logical unit, so
    /// `lun` is not used.
    fn command(
        &mut self,
        _lun: u8,
        cdb: &Cdb,
        data: Data<'_>,
        timeout_ms: u32,
    ) -> Result<Reply, TransportError> {
        let length = data.len();
        let (status, moved) = self.run(cdb, data, timeout_ms)?;
        let residue = length.saturating_sub(moved) as u32;
        match status {
            Status::Passed => Ok(Reply::Good { residue }),
            Status::Failed => Ok(Reply::Failed(self.sense(timeout_ms)?)),
            // REQUEST SENSE and INQUIRY change no sense data to read.
            Status::Unknown if matches!(cdb.opcode(), op::REQUEST_SENSE | op::INQUIRY) => {
                Ok(Reply::Good { residue })
            }
            Status::Unknown => match self.sense(timeout_ms)? {
                Some(sense) if sense.key != key::NO_SENSE => Ok(Reply::Failed(Some(sense))),
                _ => Ok(Reply::Good { residue }),
            },
        }
    }
}
