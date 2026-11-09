// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Optical drives on a port: IDENTIFY PACKET DEVICE, and the SCSI
//! commands [`Packets`] carries in ATA packets against a simulated drive
//! that answers as QEMU's do, by DMA and by PIO, with the sense of a
//! failed command fetched after it.

use std::vec::Vec;

use radix_hw::fake_memory_write;
use radix_scsi::mmc::{self, Content, Profile};
use radix_scsi::{Cdb, Data, Reply, Sense, Transport, TransportError, identify, key, op};

use super::{Disk, PortRegs, Sim, buffers, checksum, no_delay, one_port, probe, read_mem};
use crate::ata::{self, command};
use crate::{PACKET_BUFFER_SIZE, PacketEnd, Packets};

/// A CD holds at most this many blocks: a disc of more is a DVD, as QEMU
/// has it.
const CD_MAX_BLOCKS: u32 = 360_000;

/// The disc in a simulated optical drive, and what the drive answers.
pub(super) struct Drive {
    /// The disc's blocks of 2,048 bytes: none without a disc.
    blocks: u32,
    /// Whether it moves data by DMA, and needs DMADIR said.
    dma: bool,
    dmadir: bool,
    /// The sense of the last command that failed, for REQUEST SENSE.
    sense: (u8, u8, u8),
    /// Blocks that fail to read.
    bad: Vec<u32>,
    locked: bool,
    /// The packets it was sent: operation code, and whether by DMA.
    packets: Vec<(u8, bool)>,
}

impl Drive {
    fn new(blocks: u32) -> Drive {
        Drive {
            blocks,
            dma: true,
            dmadir: false,
            sense: (0, 0, 0),
            bad: Vec::new(),
            locked: false,
            packets: Vec::new(),
        }
    }

    /// IDENTIFY PACKET DEVICE data of a DVD drive.
    fn identify(&self) -> [u16; 256] {
        let mut id = [0u16; 256];
        let text = |id: &mut [u16; 256], from: usize, s: &str, words: usize| {
            let mut bytes = std::vec![b' '; words * 2];
            bytes[..s.len()].copy_from_slice(s.as_bytes());
            for i in 0..words {
                id[from + i] = u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]]);
            }
        };
        // ATAPI, a CD or DVD drive (type 5), removable, 12-byte packets.
        id[0] = 0x8000 | 5 << 8 | 0x80;
        text(&mut id, 10, "QM00003", 10);
        text(&mut id, 23, "2.5+", 4);
        text(&mut id, 27, "QEMU DVD-ROM", 20);
        id[49] = u16::from(self.dma) << 8;
        id[62] = u16::from(self.dmadir) << 15;
        checksum(&mut id);
        id
    }

    /// Answers `cdb`, its data into `out`; or the sense of its failure.
    fn answer(&mut self, cdb: &[u8], out: &mut Vec<u8>) -> Result<(), (u8, u8, u8)> {
        let allocation = |at: usize| usize::from(u16::from_be_bytes([cdb[at], cdb[at + 1]]));
        let no_disc = (key::NOT_READY, 0x3a, 1);
        match cdb[0] {
            op::TEST_UNIT_READY if self.blocks == 0 => return Err(no_disc),
            op::TEST_UNIT_READY => {}
            op::REQUEST_SENSE => {
                let (key, asc, ascq) = core::mem::take(&mut self.sense);
                out.extend([0x70, 0, key, 0, 0, 0, 0, 10, 0, 0, 0, 0, asc, ascq, 0, 0, 0, 0]);
                out.truncate(usize::from(cdb[4]));
            }
            op::INQUIRY => {
                out.extend([5, 0x80, 5, 0x32, 31, 0, 0, 0]);
                out.extend(b"QEMU    QEMU DVD-ROM    2.5+");
                out.truncate(usize::from(cdb[4]));
            }
            op::PREVENT_ALLOW_MEDIUM_REMOVAL => self.locked = cdb[4] & 1 != 0,
            op::START_STOP_UNIT if cdb[4] & 3 == 2 => {
                if self.locked {
                    return Err((key::ILLEGAL_REQUEST, 0x53, 2));
                }
                self.blocks = 0;
            }
            _ if self.blocks == 0 => return Err(no_disc),
            op::READ_CAPACITY_10 => {
                out.extend((self.blocks - 1).to_be_bytes());
                out.extend(2048u32.to_be_bytes());
            }
            op::READ_10 => {
                let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]);
                let count = u32::from(u16::from_be_bytes([cdb[7], cdb[8]]));
                if lba + count > self.blocks {
                    return Err((key::ILLEGAL_REQUEST, 0x21, 0));
                }
                if (lba..lba + count).any(|block| self.bad.contains(&block)) {
                    return Err((key::MEDIUM_ERROR, 0x11, 0));
                }
                for block in lba..lba + count {
                    out.extend(block_pattern(block));
                }
            }
            op::GET_CONFIGURATION => {
                let current: u16 = if self.blocks > CD_MAX_BLOCKS { 0x10 } else { 0x08 };
                out.extend([0, 0, 0, 20, 0, 0]);
                out.extend(current.to_be_bytes());
                out.extend([0, 0, 3, 8, 0, 0x10, u8::from(current == 0x10), 0]);
                out.extend([0, 0x08, u8::from(current == 0x08), 0]);
                out.truncate(allocation(7));
            }
            op::READ_DISC_INFORMATION => {
                out.extend([0, 32, 0x0e, 1, 1, 1, 1, 0x20]);
                out.resize(34, 0);
                out.truncate(allocation(7));
            }
            // QEMU's ATAPI drives take the format from the control byte.
            op::READ_TOC => match cdb[9] >> 6 {
                0 => {
                    out.extend([0, 18, 1, 1, 0, 0x14, 1, 0, 0, 0, 0, 0, 0, 0x14, 0xaa, 0]);
                    out.extend(self.blocks.to_be_bytes());
                    out.truncate(allocation(7));
                }
                1 => {
                    out.extend([0, 10, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
                    out.truncate(allocation(7));
                }
                _ => return Err((key::ILLEGAL_REQUEST, 0x24, 0)),
            },
            _ => return Err((key::ILLEGAL_REQUEST, 0x20, 0)),
        }
        Ok(())
    }
}

/// What block `block` of the disc holds.
fn block_pattern(block: u32) -> Vec<u8> {
    let start = u64::from(block) * 2048;
    (start..start + 2048).map(|i| (i % 251) as u8).collect()
}

impl Disk {
    /// An optical drive holding a disc of `blocks` blocks.
    fn drive(blocks: u32) -> Disk {
        Disk { signature: 0xeb14_0101, drive: Some(Drive::new(blocks)), ..Disk::new() }
    }
}

impl PortRegs {
    /// Carries out the packet command, or IDENTIFY PACKET DEVICE, in slot
    /// `slot`, whose header's flags are `flags`, whose table is at `table`
    /// and holds `fis`, with `buffers`.
    pub(super) fn run_packet(
        &mut self,
        slot: usize,
        flags: u32,
        table: u64,
        fis: &[u8],
        buffers: &[(u64, usize)],
    ) {
        let disk = self.disk.as_mut().unwrap();
        if disk.hold && fis[2] == command::PACKET {
            // Taken, and never answered.
            disk.held.push((slot, false));
            return;
        }
        let drive = disk.drive.as_mut().unwrap();
        let total: usize = buffers.iter().map(|b| b.1).sum();
        let mut out = Vec::new();
        let failed = match fis[2] {
            command::IDENTIFY_PACKET_DEVICE => {
                for word in drive.identify() {
                    out.extend(word.to_le_bytes());
                }
                None
            }
            command::PACKET => {
                assert_ne!(flags & (1 << 5), 0, "the header's ATAPI bit");
                let cdb = read_mem(table + 0x40, 16);
                let features = fis[3];
                let dma = features & 1 != 0;
                let byte_count = usize::from(u16::from_le_bytes([fis[5], fis[6]]));
                if total > 0 {
                    assert_eq!(byte_count, total.min(63 * 1024), "the byte count limit");
                }
                assert!(!dma || drive.dma, "DMA to a drive without it");
                let into_host = flags & (1 << 6) == 0;
                assert_eq!(features & 4 != 0, dma && drive.dmadir && into_host && total > 0);
                drive.packets.push((cdb[0], dma));
                drive.answer(&cdb, &mut out).err()
            }
            other => panic!("unexpected command {other:#x} to an optical drive"),
        };
        // As much as the buffers hold goes to them; the controller counts
        // it in the header.
        let mut moved = 0;
        for &(phys, len) in buffers {
            let take = len.min(out.len() - moved);
            fake_memory_write(phys, &out[moved..moved + take]);
            moved += take;
        }
        fake_memory_write(self.clb + 32 * slot as u64 + 4, &(moved as u32).to_le_bytes());
        match failed {
            Some((key, asc, ascq)) => {
                drive.sense = (key, asc, ascq);
                // The sense key in the error register's top bits, and ABRT.
                disk.error = key << 4 | 0x04;
                self.fail(slot);
            }
            None => self.finish(slot, false),
        }
    }
}

/// The drive on a simulated port holding a disc of `blocks` blocks: the
/// simulation, and the port.
fn drive(
    blocks: u32,
    set: impl FnOnce(&mut Drive),
) -> (std::sync::Arc<Sim>, crate::Hba, crate::Port) {
    let mut disk = Disk::drive(blocks);
    set(disk.drive.as_mut().unwrap());
    let sim = Sim::new(std::vec![(1, disk)]);
    let (hba, port) = one_port(&sim);
    (sim, hba, port)
}

fn packets_sent(sim: &Sim) -> Vec<(u8, bool)> {
    sim.disk(1, |disk| disk.drive.as_ref().unwrap().packets.clone())
}

#[test]
fn identify_packet_data_say_what_the_drive_is() {
    let words = Drive::new(1).identify();
    let identity = ata::identify_packet(&words).unwrap();
    assert_eq!((identity.device_type, identity.removable), (5, true));
    assert_eq!(identity.packet_length, 12);
    assert!(identity.dma && !identity.dmadir);
    assert_eq!(ata::trimmed(&identity.model), "QEMU DVD-ROM");
    assert_eq!(ata::trimmed(&identity.serial), "QM00003");
    assert_eq!(ata::trimmed(&identity.firmware), "2.5+");
    let mut sixteen = words;
    sixteen[0] |= 1;
    checksum(&mut sixteen);
    assert_eq!(ata::identify_packet(&sixteen).unwrap().packet_length, 16);
    // A disk's data, a reserved packet length, a wrong checksum.
    let mut disk = words;
    disk[0] = 0x0040;
    checksum(&mut disk);
    assert_eq!(ata::identify_packet(&disk), Err(ata::Unsupported::NotAtapi));
    let mut reserved = words;
    reserved[0] |= 2;
    checksum(&mut reserved);
    assert_eq!(ata::identify_packet(&reserved), Err(ata::Unsupported::NotAtapi));
    let mut wrong = words;
    wrong[49] ^= 1;
    assert_eq!(ata::identify_packet(&wrong), Err(ata::Unsupported::Checksum));
}

#[test]
fn an_optical_drive_is_found_and_says_what_it_is() {
    let sim = Sim::new(std::vec![(0, Disk::new()), (1, Disk::drive(1000))]);
    let (_, found) = probe(&sim);
    assert_eq!(found.len(), 2);
    assert!(found[0].1.as_ref().unwrap().packet_identity().is_none());
    let (number, port) = &found[1];
    let port = port.as_ref().unwrap();
    assert_eq!(*number, 1);
    let identity = port.packet_identity().unwrap();
    assert_eq!(ata::trimmed(&identity.model), "QEMU DVD-ROM");
    sim.with(|s| {
        assert_eq!(s.ports[1].cmd & 0x11, 0x11, "running");
        // Its interrupt comes on with each command, which its driver's
        // thread waits for; until then it is off.
        assert_eq!(s.ports[1].ie, 0, "its interrupt off until a command");
    });
    let commands = sim.disk(1, |disk| disk.commands.clone());
    assert_eq!(commands.iter().map(|c| c.0).collect::<Vec<_>>(), [command::IDENTIFY_PACKET_DEVICE]);
}

#[test]
fn packets_carry_mmc_commands_and_find_the_disc() {
    let (sim, _hba, mut port) = drive(2_000_000, |_| {});
    let mut wait = no_delay;
    let mut packets = Packets::new(&mut port, &mut wait);
    let inquiry = identify(&mut packets, 0).unwrap();
    assert!(inquiry.is_optical() && inquiry.removable);
    assert_eq!(inquiry.product(), "QEMU DVD-ROM");
    let disc = mmc::disc(&mut packets, 0, &mut |_| {}).unwrap();
    assert_eq!((disc.profile, disc.content), (Profile::DVD_ROM, Content::Data));
    assert_eq!(disc.capacity.unwrap().blocks, 2_000_000);
    assert_eq!(disc.last_session, 0);
    // Answers that come by PIO, through the port's buffer.
    assert!(packets_sent(&sim).iter().all(|&(_, dma)| !dma));
    // Each command turned the port's interrupt on, for the thread waiting.
    sim.with(|s| assert_ne!(s.ports[1].ie, 0, "its interrupt on with a command"));
}

#[test]
fn reads_go_by_dma_into_the_buffers_and_short_answers_say_their_residue() {
    let (sim, _hba, mut port) = drive(1000, |_| {});
    let mut wait = no_delay;
    let mut packets = Packets::new(&mut port, &mut wait);
    let (memory, frames) = buffers(2);
    let cdb = Cdb::read(10, 4, false).unwrap();
    let data = Data::Frames { into_host: true, frames: &frames };
    assert_eq!(packets.command(0, &cdb, data, 30_000), Ok(Reply::Good { residue: 0 }));
    let mut bytes = std::vec![0u8; 8192];
    memory.read(0, &mut bytes);
    for (i, block) in bytes.chunks(2048).enumerate() {
        assert_eq!(block, block_pattern(10 + i as u32), "block {i}");
    }
    assert_eq!(packets_sent(&sim), [(op::READ_10, true)]);
    // A TOC shorter than asked for.
    let mut toc = [0u8; 812];
    let cdb = Cdb::read_toc(mmc::toc_format::TOC, 1, 812);
    let reply = packets.command(0, &cdb, Data::In(&mut toc), 30_000);
    assert_eq!(reply, Ok(Reply::Good { residue: 812 - 20 }));
    assert_eq!(mmc::Toc::parse(&toc[..20]).unwrap().lead_out, Some(1000));
}

#[test]
fn a_drive_that_needs_dmadir_is_told_and_one_without_dma_moves_by_pio() {
    let (sim, _hba, mut port) = drive(1000, |d| d.dmadir = true);
    let mut wait = no_delay;
    let (_memory, frames) = buffers(1);
    let data = Data::Frames { into_host: true, frames: &frames };
    let mut packets = Packets::new(&mut port, &mut wait);
    let reply = packets.command(0, &Cdb::read(0, 2, false).unwrap(), data, 30_000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    assert_eq!(packets_sent(&sim), [(op::READ_10, true)]);

    let (sim, _hba, mut port) = drive(1000, |d| d.dma = false);
    let mut packets = Packets::new(&mut port, &mut wait);
    let data = Data::Frames { into_host: true, frames: &frames };
    let reply = packets.command(0, &Cdb::read(0, 2, false).unwrap(), data, 30_000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    assert_eq!(packets_sent(&sim), [(op::READ_10, false)]);
    // A length not a multiple of 16 goes by PIO on a drive with DMA too.
    let (sim, _hba, mut port) = drive(1000, |_| {});
    let mut packets = Packets::new(&mut port, &mut wait);
    let odd = [(frames[0].0, 2050)];
    let data = Data::Frames { into_host: true, frames: &odd };
    let _ = packets.command(0, &Cdb::read(0, 1, false).unwrap(), data, 30_000);
    assert_eq!(packets_sent(&sim), [(op::READ_10, false)]);
}

#[test]
fn a_failed_command_says_why_and_the_port_goes_on() {
    let (sim, _hba, mut port) = drive(1000, |d| d.bad = std::vec![5]);
    let mut wait = no_delay;
    let (_memory, frames) = buffers(2);
    let mut packets = Packets::new(&mut port, &mut wait);
    let data = Data::Frames { into_host: true, frames: &frames };
    let reply = packets.command(0, &Cdb::read(4, 4, false).unwrap(), data, 30_000);
    let unrecovered = Sense { key: key::MEDIUM_ERROR, asc: 0x11, ascq: 0 };
    assert_eq!(reply, Ok(Reply::Failed(Some(unrecovered))));
    assert_eq!(packets_sent(&sim), [(op::READ_10, true), (op::REQUEST_SENSE, false)]);
    // Running again, and the next command works.
    sim.with(|s| assert_eq!(s.ports[1].cmd & 1, 1));
    let data = Data::Frames { into_host: true, frames: &frames };
    let reply = packets.command(0, &Cdb::read(6, 4, false).unwrap(), data, 30_000);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    // An empty drive.
    sim.disk(1, |disk| disk.drive.as_mut().unwrap().blocks = 0);
    let reply = packets.command(0, &Cdb::test_unit_ready(), Data::None, 30_000);
    let empty = Sense { key: key::NOT_READY, asc: 0x3a, ascq: 1 };
    assert_eq!(reply, Ok(Reply::Failed(Some(empty))));
    let found = mmc::disc(&mut packets, 0, &mut |_| {});
    assert_eq!(found, Err(radix_scsi::Absent::NoMedium));
}

#[test]
fn the_medium_is_locked_in_and_ejected() {
    let (sim, _hba, mut port) = drive(1000, |_| {});
    let mut wait = no_delay;
    let mut packets = Packets::new(&mut port, &mut wait);
    assert_eq!(radix_scsi::prevent_removal(&mut packets, 0, true), Ok(true));
    assert!(sim.disk(1, |disk| disk.drive.as_ref().unwrap().locked));
    assert_eq!(radix_scsi::eject(&mut packets, 0), Ok(()));
    let look = radix_scsi::look(&mut packets, 0).unwrap();
    assert_eq!(look.medium, Some(false));
}

#[test]
fn commands_the_drive_cannot_take_are_refused_before_they_go() {
    let (sim, _hba, mut port) = drive(1000, |_| {});
    let mut wait = no_delay;
    let mut packets = Packets::new(&mut port, &mut wait);
    // A 16-byte command to a drive of 12-byte packets.
    let cdb = Cdb::read(1 << 33, 1, true).unwrap();
    assert!(matches!(packets.command(0, &cdb, Data::None, 1000), Err(TransportError::Failed(_))));
    let mut big = std::vec![0u8; PACKET_BUFFER_SIZE + 2];
    let cdb = Cdb::inquiry(36);
    let reply = packets.command(0, &cdb, Data::In(&mut big), 1000);
    assert!(matches!(reply, Err(TransportError::Failed(_))));
    // Another logical unit is not there.
    let reply = packets.command(1, &Cdb::test_unit_ready(), Data::None, 1000);
    assert!(matches!(reply, Ok(Reply::Failed(Some(Sense { asc: 0x25, .. })))));
    assert!(packets_sent(&sim).is_empty());
}

#[test]
fn a_drive_that_does_not_answer_has_its_link_reset_and_one_gone_is_gone() {
    let (sim, _hba, mut port) = drive(1000, |_| {});
    // The drive holds the command and never finishes it.
    sim.disk(1, |disk| disk.hold = true);
    let mut waited = 0u64;
    let mut wait = |us: u32| waited += u64::from(us);
    let mut packets = Packets::new(&mut port, &mut wait);
    let reply = packets.command(0, &Cdb::test_unit_ready(), Data::None, 100);
    assert!(matches!(reply, Err(TransportError::Failed(_))), "{reply:?}");
    // The link came back: the drive answers again.
    sim.disk(1, |disk| disk.hold = false);
    let reply = packets.command(0, &Cdb::test_unit_ready(), Data::None, 100);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    // Unplugged: the link does not come back.
    sim.disk(1, |disk| disk.hold = true);
    sim.with(|s| {
        s.ports[1].disk = None;
        s.ports[1].link_down();
    });
    let reply = packets.command(0, &Cdb::test_unit_ready(), Data::None, 100);
    assert_eq!(reply, Err(TransportError::Gone));
    let reply = packets.command(0, &Cdb::test_unit_ready(), Data::None, 100);
    assert_eq!(reply, Err(TransportError::Gone));
}

#[test]
fn a_read_sent_without_waiting_is_looked_at_and_answered_after() {
    let (sim, _hba, mut port) = drive(1000, |d| d.bad = std::vec![5]);
    let mut wait = no_delay;
    let (memory, frames) = buffers(2);
    let read = Data::Frames { into_host: true, frames: &frames };
    port.send_packet(&Cdb::read(10, 4, false).unwrap(), &read).unwrap();
    // The simulated drive answers at once; the port's interrupt is on.
    sim.with(|s| assert_ne!(s.ports[1].ie, 0, "its interrupt on with the read"));
    let end = port.packet_ended();
    assert_eq!(end, Some(PacketEnd::Done(8192)));
    assert_eq!(port.packet_ended(), Some(PacketEnd::Done(8192)), "over, and stays so");
    let reply = port.packet_reply(end.unwrap(), 8192, &mut wait);
    assert_eq!(reply, Ok(Reply::Good { residue: 0 }));
    let mut bytes = std::vec![0u8; 8192];
    memory.read(0, &mut bytes);
    assert_eq!(&bytes[..2048], block_pattern(10));
    // A read the drive fails stops the port, its interrupt off; the reply
    // restarts it and asks the drive why.
    port.send_packet(&Cdb::read(4, 4, false).unwrap(), &read).unwrap();
    let end = port.packet_ended().unwrap();
    assert!(matches!(end, PacketEnd::Stopped(_)), "{end:?}");
    sim.with(|s| assert_eq!(s.ports[1].ie, 0, "its interrupt off while stopped"));
    let reply = port.packet_reply(end, 8192, &mut wait);
    let unrecovered = Sense { key: key::MEDIUM_ERROR, asc: 0x11, ascq: 0 };
    assert_eq!(reply, Ok(Reply::Failed(Some(unrecovered))));
    sim.with(|s| assert_eq!(s.ports[1].cmd & 1, 1, "running again"));
    // A read not answered runs until its link is reset; then reads work.
    sim.disk(1, |disk| disk.hold = true);
    port.send_packet(&Cdb::read(10, 4, false).unwrap(), &read).unwrap();
    assert_eq!(port.packet_ended(), None);
    sim.disk(1, |disk| disk.hold = false);
    assert!(matches!(port.packet_timed_out(&mut wait), TransportError::Failed(_)));
    port.send_packet(&Cdb::read(10, 4, false).unwrap(), &read).unwrap();
    assert_eq!(port.packet_ended(), Some(PacketEnd::Done(8192)));
    // The one held was never taken in.
    assert_eq!(packets_sent(&sim).iter().filter(|p| p.0 == op::READ_10).count(), 3);
}

#[test]
fn a_read_sent_with_the_interrupt_on_writes_only_its_command() {
    let (sim, hba, mut port) = drive(1000, |_| {});
    let (_memory, frames) = buffers(2);
    let read = Data::Frames { into_host: true, frames: &frames };
    let cdb = Cdb::read(10, 4, false).unwrap();
    port.send_packet(&cdb, &read).unwrap();
    assert_eq!(port.packet_ended(), Some(PacketEnd::Done(8192)));
    // Marks: the interrupt mask cut to one bit, and a status bit the port
    // raises no interrupt for ("descriptor processed"). Writing either
    // register would lose its mark.
    const MASK: u32 = 1;
    const PROCESSED: u32 = 1 << 5;
    sim.with(|s| {
        s.ports[1].ie = MASK;
        s.ports[1].is = PROCESSED;
    });
    port.send_packet(&cdb, &read).unwrap();
    sim.with(|s| {
        assert_eq!(s.ports[1].ie, MASK, "the interrupt left as it was");
        assert_ne!(s.ports[1].is & PROCESSED, 0, "the status left as it was");
    });
    assert_eq!(port.packet_ended(), Some(PacketEnd::Done(8192)));
    // The controller's handler quiets the port behind the port's back
    // while a thread has it; once it is told, the next read turns the
    // interrupt on again.
    hba.quiet(1);
    port.forget_interrupts();
    port.send_packet(&cdb, &read).unwrap();
    sim.with(|s| assert_ne!(s.ports[1].ie & 1, 0, "its interrupt on again"));
    assert_eq!(port.packet_ended(), Some(PacketEnd::Done(8192)));
    // The port's own quiet is remembered.
    port.quiet();
    sim.with(|s| assert_eq!(s.ports[1].ie, 0));
    port.send_packet(&cdb, &read).unwrap();
    sim.with(|s| assert_ne!(s.ports[1].ie & 1, 0, "its interrupt on again"));
}
