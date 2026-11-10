// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The CBI and CB transports against a simulated USB floppy drive: its
//! command blocks on the control pipe, data on the bulk pipes, and the
//! status of UFI on the interrupt pipe (or none, over CB); failures with
//! their sense, stalls, and a drive that stops answering.

use std::vec::Vec;

use radix_scsi::{
    Cdb, Data, Meaning, Reply, Sense, Transport, TransportError, key, op, probe, ufi,
};

use super::*;
use crate::storage::{PipeError, Pipes};

/// What the drive does to the next command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    /// Stalls the command block.
    RefuseCommand,
    /// Ends the data stage with a stall.
    StallData,
    /// Sends nothing on the interrupt pipe.
    Silent,
    /// Sends a status of a phase error (a device that is not UFI's).
    PhaseError,
}

/// Where the drive is in a command.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    Command,
    /// The data it sends, or the block it writes to, then its status.
    Data {
        bytes: Vec<u8>,
        write: Option<usize>,
        sense: Sense,
    },
    Status(Sense),
}

const FINE: Sense = Sense { key: key::NO_SENSE, asc: 0, ascq: 0 };

/// A simulated USB floppy drive with a 1.44 MB disk in it.
struct Drive {
    blocks: Vec<[u8; 512]>,
    disk: bool,
    protected: bool,
    /// A disk went in: the next command but INQUIRY and REQUEST SENSE
    /// fails with a UNIT ATTENTION.
    attention: bool,
    /// The sense of the last command, which REQUEST SENSE reads.
    sense: Sense,
    stage: Stage,
    next: Fault,
    fault: Fault,
    /// Whether it has an interrupt pipe (CBI), and its commands are UFI's.
    interrupt: bool,
    ufi: bool,
    /// Halted bulk pipes: IN, OUT.
    halted: [bool; 2],
    resets: u32,
    gone: bool,
    /// Every command block, and the tracks FORMAT UNIT formatted.
    blocks_sent: Vec<[u8; COMMAND_LENGTH]>,
    formatted: Vec<(u8, u8)>,
}

fn pipe(into_host: bool) -> usize {
    usize::from(!into_host)
}

fn sense(key: u8, asc: u8) -> Sense {
    Sense { key, asc, ascq: 0 }
}

impl Drive {
    fn new() -> Drive {
        let mut blocks = std::vec![[0u8; 512]; 2880];
        for (i, block) in blocks.iter_mut().enumerate() {
            block.fill(i as u8);
        }
        Drive {
            blocks,
            disk: true,
            protected: false,
            attention: false,
            sense: FINE,
            stage: Stage::Command,
            next: Fault::None,
            fault: Fault::None,
            interrupt: true,
            ufi: true,
            halted: [false; 2],
            resets: 0,
            gone: false,
            blocks_sent: Vec::new(),
            formatted: Vec::new(),
        }
    }

    /// What `cdb` asks for: the data it sends, the block it writes to, or
    /// why it fails.
    fn execute(&mut self, cdb: &[u8]) -> Result<(Vec<u8>, Option<usize>), Sense> {
        let lba = u32::from_be_bytes(cdb[2..6].try_into().unwrap()) as usize;
        let count = usize::from(u16::from_be_bytes([cdb[7], cdb[8]]));
        let needs_disk = !matches!(cdb[0], op::INQUIRY | op::REQUEST_SENSE);
        if needs_disk && self.attention {
            self.attention = false;
            return Err(sense(key::UNIT_ATTENTION, 0x28));
        }
        if needs_disk && !self.disk {
            return Err(sense(key::NOT_READY, 0x3a));
        }
        match cdb[0] {
            op::INQUIRY => {
                let mut bytes = std::vec![0u8; 36];
                bytes[1] = 0x80;
                bytes[8..16].copy_from_slice(b"TEAC    ");
                bytes[16..32].copy_from_slice(b"FD-05PUB        ");
                bytes[32..36].copy_from_slice(b"3000");
                Ok((bytes, None))
            }
            op::REQUEST_SENSE => {
                let mut bytes = std::vec![0u8; 18];
                bytes[0] = 0x70;
                bytes[2] = self.sense.key;
                bytes[7] = 10;
                bytes[12] = self.sense.asc;
                Ok((bytes, None))
            }
            op::TEST_UNIT_READY => Ok((Vec::new(), None)),
            op::READ_CAPACITY_10 => {
                let mut bytes = 2879u32.to_be_bytes().to_vec();
                bytes.extend(512u32.to_be_bytes());
                Ok((bytes, None))
            }
            op::MODE_SENSE_10 => {
                let wp = if self.protected { 0x80 } else { 0 };
                Ok((std::vec![0, 6, 0x94, wp, 0, 0, 0, 0], None))
            }
            op::READ_10 if lba + count <= self.blocks.len() => {
                Ok((self.blocks[lba..lba + count].iter().flatten().copied().collect(), None))
            }
            op::WRITE_10 | op::FORMAT_UNIT if self.protected => Err(sense(key::DATA_PROTECT, 0x27)),
            op::WRITE_10 if lba + count <= self.blocks.len() => Ok((Vec::new(), Some(lba))),
            op::FORMAT_UNIT => Ok((Vec::new(), Some(usize::MAX))),
            op::READ_10 | op::WRITE_10 => Err(sense(key::ILLEGAL_REQUEST, 0x21)),
            _ => Err(sense(key::ILLEGAL_REQUEST, 0x20)),
        }
    }

    /// The two bytes of a status: UFI's sense code, or another device's
    /// type and status.
    fn status(&self, sense: Sense) -> [u8; 2] {
        if self.ufi {
            return [sense.asc, sense.ascq];
        }
        match self.fault {
            Fault::PhaseError => [0, 2],
            _ => [0, u8::from(sense.key != key::NO_SENSE)],
        }
    }

    /// Ends a command with `sense`, which REQUEST SENSE reads after; INQUIRY
    /// and REQUEST SENSE leave what it read as it was.
    fn end(&mut self, cdb: u8, sense: Sense) -> Sense {
        if !matches!(cdb, op::INQUIRY | op::REQUEST_SENSE) {
            self.sense = sense;
        }
        sense
    }
}

impl Pipes for Drive {
    fn bulk(&mut self, data: Data<'_>, _timeout_ms: u32) -> Result<usize, PipeError> {
        if self.gone {
            return Err(PipeError::Gone);
        }
        let into_host = data.into_host();
        if self.halted[pipe(into_host)] {
            return Err(PipeError::Stall(0));
        }
        let Stage::Data { bytes, write, sense } = self.stage.clone() else {
            self.halted[pipe(into_host)] = true;
            return Err(PipeError::Stall(0));
        };
        self.stage = Stage::Status(sense);
        let stall = self.fault == Fault::StallData;
        match data {
            Data::In(buffer) => {
                let n = if stall { 0 } else { bytes.len().min(buffer.len()) };
                buffer[..n].copy_from_slice(&bytes[..n]);
                if stall {
                    self.halted[pipe(true)] = true;
                    return Err(PipeError::Stall(n));
                }
                Ok(n)
            }
            Data::Out(out) => {
                if stall {
                    self.halted[pipe(false)] = true;
                    return Err(PipeError::Stall(0));
                }
                match write {
                    Some(usize::MAX) => {
                        let list: [u8; ufi::FORMAT_PARAMETERS] = out.try_into().unwrap();
                        let track = self.blocks_sent.last().unwrap()[2];
                        self.formatted.push((track, list[1] & 1));
                    }
                    Some(lba) => {
                        for (i, block) in out.chunks(512).enumerate() {
                            self.blocks[lba + i][..block.len()].copy_from_slice(block);
                        }
                    }
                    None => {}
                }
                Ok(out.len())
            }
            _ => unreachable!("the tests move bytes, not frames"),
        }
    }

    fn clear_halt(&mut self, into_host: bool) -> Result<(), PipeError> {
        if self.gone {
            return Err(PipeError::Gone);
        }
        self.halted[pipe(into_host)] = false;
        Ok(())
    }

    fn control(&mut self, setup: Setup, data: Data<'_>) -> Result<usize, PipeError> {
        if self.gone {
            return Err(PipeError::Gone);
        }
        assert_eq!((setup.request_type, setup.request, setup.index), (0x21, 0, 3), "ADSC");
        assert_eq!(setup.length, 12, "a UFI command block's 12 bytes");
        let Data::Out(block) = data else { panic!("a command block goes to the device") };
        let block: [u8; COMMAND_LENGTH] = block.try_into().unwrap();
        self.blocks_sent.push(block);
        if block == RESET_BLOCK {
            self.resets += 1;
            self.stage = Stage::Command;
            return Ok(COMMAND_LENGTH);
        }
        self.fault = core::mem::replace(&mut self.next, Fault::None);
        if self.fault == Fault::RefuseCommand {
            self.sense = sense(key::ILLEGAL_REQUEST, 0x20);
            return Err(PipeError::Stall(0));
        }
        let (stage, ended) = match self.execute(&block) {
            Ok((bytes, write)) => {
                let expects_data = !bytes.is_empty() || write.is_some();
                if expects_data {
                    (Stage::Data { bytes, write, sense: FINE }, FINE)
                } else {
                    (Stage::Status(FINE), FINE)
                }
            }
            Err(why) => (Stage::Status(why), why),
        };
        self.end(block[0], ended);
        self.stage = stage;
        Ok(COMMAND_LENGTH)
    }
}

impl InterruptPipe for Drive {
    fn interrupt(&mut self, data: &mut [u8], _timeout_ms: u32) -> Result<usize, PipeError> {
        assert!(self.interrupt, "a CB device has no interrupt pipe");
        if self.gone {
            return Err(PipeError::Gone);
        }
        if self.fault == Fault::Silent {
            return Err(PipeError::TimedOut);
        }
        let sense = match self.stage {
            Stage::Status(sense) => sense,
            // Its data never came: a stall ended them, or the host sent
            // none. The status says how the command ended.
            Stage::Data { sense, .. } => sense,
            Stage::Command => FINE,
        };
        self.stage = Stage::Command;
        let status = self.status(sense);
        data[..2].copy_from_slice(&status);
        Ok(2)
    }
}

fn cbi(drive: Drive) -> Cbi<Drive> {
    let (interrupt, ufi) = (drive.interrupt, drive.ufi);
    Cbi::new(drive, 3, interrupt, ufi)
}

fn failed_with(reply: Result<Reply, TransportError>) -> Option<Meaning> {
    match reply {
        Ok(Reply::Failed(Some(sense))) => Some(sense.meaning()),
        _ => None,
    }
}

#[test]
fn a_usb_floppy_drive_is_probed_with_twelve_byte_commands() {
    for protected in [false, true] {
        let mut drive = cbi(Drive { protected, ..Drive::new() });
        let found = probe(&mut drive, 0, &mut |_| {}).expect("a disk in a drive");
        assert_eq!(found.capacity.blocks, 2880);
        assert_eq!(found.capacity.block_length, 512);
        assert!(found.inquiry.removable);
        assert_eq!(found.read_only, protected, "from MODE SENSE(10)");
        for block in &drive.pipes().blocks_sent {
            let short = match block[0] {
                op::INQUIRY | op::TEST_UNIT_READY | op::MODE_SENSE_6 | op::REQUEST_SENSE => 6,
                _ => 10,
            };
            assert!(block[short..].iter().all(|&b| b == 0), "padded with zeros: {block:x?}");
        }
    }
}

#[test]
fn blocks_are_read_and_written_on_the_bulk_pipes() {
    let mut drive = cbi(Drive::new());
    let mut buffer = std::vec![0u8; 2 * 512];
    let cdb = Cdb::read(100, 2, false).unwrap();
    let reply = drive.command(0, &cdb, Data::In(&mut buffer), 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    assert!(buffer[..512].iter().all(|&b| b == 100) && buffer[512..].iter().all(|&b| b == 101));

    let cdb = Cdb::write(7, 1, false).unwrap();
    let reply = drive.command(0, &cdb, Data::Out(&[0xa5; 512]), 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    assert!(drive.pipes().blocks[7].iter().all(|&b| b == 0xa5));
}

#[test]
fn a_failed_command_is_asked_why() {
    let mut drive = cbi(Drive { disk: false, ..Drive::new() });
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(failed_with(reply), Some(Meaning::NoMedium));
    assert_eq!(drive.pipes().blocks_sent.last().unwrap()[0], op::REQUEST_SENSE);

    let mut drive = cbi(Drive { attention: true, ..Drive::new() });
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    let changed = matches!(reply, Ok(Reply::Failed(Some(s))) if s.medium_changed());
    assert!(changed, "a disk put in is a UNIT ATTENTION of a medium changed");
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }), "said once");

    let mut drive = cbi(Drive { protected: true, ..Drive::new() });
    let cdb = Cdb::write(0, 1, false).unwrap();
    let reply = drive.command(0, &cdb, Data::Out(&[0; 512]), 1000);
    assert_eq!(failed_with(reply), Some(Meaning::WriteProtected));
}

#[test]
fn a_command_the_drive_refuses_is_asked_why() {
    let mut drive = cbi(Drive::new());
    drive.pipes().next = Fault::RefuseCommand;
    let reply = drive.command(0, &Cdb::mode_sense_6(0x3f, 192), Data::In(&mut [0; 192]), 1000);
    assert_eq!(failed_with(reply), Some(Meaning::Unsupported));
    assert_eq!(drive.pipes().resets, 0, "a refusal is no break of the protocol");
}

#[test]
fn a_stalled_data_stage_is_cleared_and_its_status_read() {
    let mut drive = cbi(Drive::new());
    drive.pipes().next = Fault::StallData;
    let mut buffer = [0u8; 512];
    let cdb = Cdb::read(0, 1, false).unwrap();
    let reply = drive.command(0, &cdb, Data::In(&mut buffer), 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 512 }), "nothing moved");
    assert_eq!(drive.pipes().halted, [false, false]);
    let reply = drive.command(0, &cdb, Data::In(&mut buffer), 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }), "the next command works");
}

#[test]
fn a_drive_that_does_not_answer_is_reset() {
    let mut drive = cbi(Drive::new());
    drive.pipes().next = Fault::Silent;
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(reply, Err(TransportError::Failed("it did not answer in time")));
    assert_eq!(drive.pipes().resets, 1, "the Command Block Reset");
    assert_eq!(drive.pipes().blocks_sent.last(), Some(&RESET_BLOCK));
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
}

#[test]
fn a_device_that_is_not_ufi_says_its_status_in_its_second_byte() {
    let mut drive = cbi(Drive { ufi: false, ..Drive::new() });
    assert_eq!(
        drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000),
        Ok(Reply::Good { residue: 0 })
    );
    drive.pipes().disk = false;
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(failed_with(reply), Some(Meaning::NoMedium));
    drive.pipes().next = Fault::PhaseError;
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(reply, Err(TransportError::Failed("it reported a phase error")));
    assert_eq!(drive.pipes().resets, 1);
}

#[test]
fn a_cb_device_is_asked_after_each_command_how_it_went() {
    let mut drive = cbi(Drive { interrupt: false, ..Drive::new() });
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    let sent: Vec<u8> = drive.pipes().blocks_sent.iter().map(|b| b[0]).collect();
    assert_eq!(sent, [op::TEST_UNIT_READY, op::REQUEST_SENSE]);
    drive.pipes().disk = false;
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(failed_with(reply), Some(Meaning::NoMedium));
    // INQUIRY changes no sense to ask for.
    let reply = drive.command(0, &Cdb::inquiry(36), Data::In(&mut [0; 36]), 1000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    assert_eq!(drive.pipes().blocks_sent.last().unwrap()[0], op::INQUIRY);
}

#[test]
fn a_disk_is_formatted_a_track_at_a_time_through_the_transport() {
    let mut drive = cbi(Drive::new());
    assert_eq!(ufi::format_track(&mut drive, 0, (0, 1), 2880), Ok(()));
    assert_eq!(ufi::format_track(&mut drive, 0, (79, 0), 2880), Ok(()));
    assert_eq!(drive.pipes().formatted, [(0, 1), (79, 0)]);
    drive.pipes().protected = true;
    let refused = ufi::format_track(&mut drive, 0, (1, 0), 2880);
    assert!(
        matches!(refused, Err(radix_scsi::Absent::Failed("FORMAT UNIT", Some(s))) if s.key == key::DATA_PROTECT)
    );
}

#[test]
fn an_unplugged_drive_is_gone() {
    let mut drive = cbi(Drive::new());
    drive.pipes().gone = true;
    let reply = drive.command(0, &Cdb::test_unit_ready(), Data::None, 1000);
    assert_eq!(reply, Err(TransportError::Gone));
    assert_eq!(drive.reset(), Err(TransportError::Gone));
}
