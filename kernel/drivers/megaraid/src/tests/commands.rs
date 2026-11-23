// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Commands on every interface: management commands with their data, SCSI
//! commands to volumes and to pass-through disks, what ends them badly,
//! and the commands the driver refuses before sending.

use std::boxed::Box;
use std::vec::Vec;

use crate::frame::status;
use crate::tests::{
    MFI_FAMILIES, READ_CAPACITY_10, SYNC_CACHE, all_families, block_runs, buffer, bytes_of, exec,
    inquiry, read_capacity16, read10, read16, up, write10, write16,
};
use crate::{Direction, Error, Target};

const OPCODE: u32 = 0x0102_0000;

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|n| (n * 7 + 3) as u8).collect()
}

fn mailbox() -> [u8; 12] {
    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
}

// ---- management commands

#[test]
fn a_management_command_brings_its_data_back() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let reply = pattern(200);
        let sent = reply.clone();
        sim.lock().answer_with(Box::new(move |opcode, mb| {
            assert_eq!(opcode, OPCODE);
            assert_eq!(*mb, mailbox());
            (status::OK, sent.clone())
        }));
        let memory = buffer(4096);
        let mut wait = sim.waiter();
        let answer = raid
            .run_dcmd(
                OPCODE,
                &mailbox(),
                &[(memory.phys(), 4096)],
                Direction::FromDevice,
                &mut wait,
                100,
            )
            .unwrap();
        assert!(answer.good(), "{family:?}");
        assert_eq!((answer.asked, answer.moved), (4096, 4096), "{family:?}");
        let got = bytes_of(&memory, 4096);
        assert_eq!(&got[..200], &reply[..], "{family:?}");
        assert!(got[200..].iter().all(|&b| b == 0), "{family:?}");
        let inner = sim.lock();
        assert_eq!(inner.dcmds.len(), 1);
        assert_eq!(inner.dcmds[0].opcode, OPCODE);
        assert_eq!(inner.dcmds[0].mailbox, mailbox());
        assert!(inner.dcmds[0].sent.is_empty());
        assert_eq!(raid.room(), raid.commands());
    }
}

#[test]
fn a_management_command_may_carry_data_to_the_controller() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let data = pattern(100);
        memory.write(0, &data);
        let mut wait = sim.waiter();
        let answer = raid
            .run_dcmd(
                OPCODE,
                &mailbox(),
                &[(memory.phys(), 100)],
                Direction::ToDevice,
                &mut wait,
                100,
            )
            .unwrap();
        assert!(answer.good(), "{family:?}");
        assert_eq!(sim.lock().dcmds[0].sent, data, "{family:?}");
    }
}

#[test]
fn a_management_command_without_data_goes_without_a_list() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let mut wait = sim.waiter();
        let answer =
            raid.run_dcmd(OPCODE, &mailbox(), &[], Direction::None, &mut wait, 100).unwrap();
        assert!(answer.good());
        assert_eq!((answer.asked, answer.moved), (0, 0));
        assert!(sim.lock().seen[0].runs.is_empty());
    }
}

#[test]
fn a_management_command_may_have_four_runs() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let reply = pattern(400);
        let sent = reply.clone();
        sim.lock().answer_with(Box::new(move |_, _| (status::OK, sent.clone())));
        let memories: Vec<_> = (0..4).map(|_| buffer(4096)).collect();
        let runs: Vec<(u64, u32)> = memories.iter().map(|m| (m.phys(), 100)).collect();
        let mut wait = sim.waiter();
        let answer = raid
            .run_dcmd(OPCODE, &mailbox(), &runs, Direction::FromDevice, &mut wait, 100)
            .unwrap();
        assert_eq!((answer.asked, answer.moved), (400, 400), "{family:?}");
        for (n, memory) in memories.iter().enumerate() {
            assert_eq!(bytes_of(memory, 100), reply[n * 100..n * 100 + 100], "{family:?} {n}");
        }
        assert_eq!(sim.lock().seen[0].runs, runs, "{family:?}");
    }
}

#[test]
fn a_management_command_with_five_runs_is_refused() {
    for (family, _) in all_families() {
        let (_, mut raid) = up(family);
        let runs = [(0x1000u64, 16u32); 5];
        let result = raid.submit_dcmd(OPCODE, &mailbox(), &runs, Direction::FromDevice);
        assert_eq!(result, Err(Error::BadRequest), "{family:?}");
        assert_eq!(raid.in_flight(), 0);
    }
}

#[test]
fn a_management_command_the_firmware_refuses_says_so() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().refuse = Some(status::INVALID_DCMD);
        let memory = buffer(4096);
        let mut wait = sim.waiter();
        let answer = raid
            .run_dcmd(
                OPCODE,
                &mailbox(),
                &[(memory.phys(), 64)],
                Direction::FromDevice,
                &mut wait,
                100,
            )
            .unwrap();
        assert!(!answer.good(), "{family:?}");
        assert_eq!(answer.status, status::INVALID_DCMD, "{family:?}");
        assert_eq!((answer.asked, answer.moved), (64, 0), "{family:?}");
        assert_eq!(answer.why(), "the management command is not known");
    }
}

#[test]
fn a_management_command_waits_for_nothing_else_in_flight() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        raid.submit_dcmd(OPCODE, &mailbox(), &[], Direction::None).unwrap();
        let mut wait = sim.waiter();
        let result = raid.run_dcmd(OPCODE, &mailbox(), &[], Direction::None, &mut wait, 100);
        assert_eq!(result.err(), Some(Error::Full), "{family:?}");
    }
}

// ---- SCSI to volumes and disks

#[test]
fn a_volume_is_written_and_read_back() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        // Three blocks from three runs.
        let (_memories, runs) = block_runs(family, &[0xa0, 0xa1, 0xa2]);
        let answer = exec(&mut raid, Target::Volume(0), &write10(4, 3), &runs, Direction::ToDevice);
        assert!(answer.good(), "{family:?}");
        assert_eq!((answer.asked, answer.moved), (1536, 1536), "{family:?}");
        {
            let inner = sim.lock();
            let disk = &inner.volumes[&0];
            assert!(disk.data[4 * 512..5 * 512].iter().all(|&b| b == 0xa0));
            assert!(disk.data[6 * 512..7 * 512].iter().all(|&b| b == 0xa2));
            // The other volume and the disk were not touched.
            assert!(inner.volumes[&2].data[4 * 512..5 * 512].iter().all(|&b| b == 4));
            assert!(inner.disks[&5].data[4 * 512..5 * 512].iter().all(|&b| b == 4));
        }
        // Back, into one run.
        let memory = buffer(4096);
        let answer = exec(
            &mut raid,
            Target::Volume(0),
            &read10(4, 3),
            &[(memory.phys(), 1536)],
            Direction::FromDevice,
        );
        assert!(answer.good(), "{family:?}");
        let got = bytes_of(&memory, 1536);
        assert!(got[..512].iter().all(|&b| b == 0xa0), "{family:?}");
        assert!(got[1024..].iter().all(|&b| b == 0xa2), "{family:?}");
    }
}

#[test]
fn a_volume_is_read_and_written_with_sixteen_byte_commands() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        memory.write(0, &[0x5c; 1024]);
        let target = Target::Volume(2);
        let answer =
            exec(&mut raid, target, &write16(10, 2), &[(memory.phys(), 1024)], Direction::ToDevice);
        assert!(answer.good(), "{family:?}");
        assert!(sim.lock().volumes[&2].data[10 * 512..12 * 512].iter().all(|&b| b == 0x5c));
        let other = buffer(4096);
        let answer =
            exec(&mut raid, target, &read16(11, 1), &[(other.phys(), 512)], Direction::FromDevice);
        assert!(answer.good(), "{family:?}");
        assert!(bytes_of(&other, 512).iter().all(|&b| b == 0x5c), "{family:?}");
    }
}

#[test]
fn a_pass_through_disk_is_written_and_read_back() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let (_memories, runs) = block_runs(family, &[0x11, 0x22]);
        let answer = exec(&mut raid, Target::Disk(5), &write10(20, 2), &runs, Direction::ToDevice);
        assert!(answer.good(), "{family:?}");
        assert!(answer.good() && answer.moved == 1024, "{family:?}");
        {
            let inner = sim.lock();
            assert!(inner.disks[&5].data[20 * 512..21 * 512].iter().all(|&b| b == 0x11));
            assert!(inner.disks[&5].data[21 * 512..22 * 512].iter().all(|&b| b == 0x22));
            // The volumes were not touched.
            assert!(inner.volumes[&0].data[20 * 512..21 * 512].iter().all(|&b| b == 20));
        }
        let memory = buffer(4096);
        let answer = exec(
            &mut raid,
            Target::Disk(5),
            &read10(20, 2),
            &[(memory.phys(), 1024)],
            Direction::FromDevice,
        );
        assert!(answer.good(), "{family:?}");
        let got = bytes_of(&memory, 1024);
        assert!(got[..512].iter().all(|&b| b == 0x11) && got[512..].iter().all(|&b| b == 0x22));
    }
}

#[test]
fn inquiry_and_capacity_come_from_the_target() {
    for (family, _) in all_families() {
        for (target, product) in [(Target::Volume(0), "VOLUME"), (Target::Disk(5), "DISK")] {
            let (_, mut raid) = up(family);
            let memory = buffer(4096);
            let run = [(memory.phys(), 4096)];
            let answer = exec(&mut raid, target, &inquiry(36), &run, Direction::FromDevice);
            assert!(answer.good(), "{family:?} {target:?}");
            let data = bytes_of(&memory, 40);
            assert_eq!(&data[8..16], b"RADIXSIM");
            assert_eq!(&data[16..16 + product.len()], product.as_bytes());
            assert_eq!(&data[36..], [0; 4], "only 36 bytes were asked for");
            // READ CAPACITY(10): the last block and the block size.
            let answer = exec(&mut raid, target, &READ_CAPACITY_10, &run, Direction::FromDevice);
            assert!(answer.good(), "{family:?} {target:?}");
            let data = bytes_of(&memory, 8);
            assert_eq!(u32::from_be_bytes(data[..4].try_into().unwrap()), 63);
            assert_eq!(u32::from_be_bytes(data[4..].try_into().unwrap()), 512);
            // READ CAPACITY(16).
            let answer = exec(&mut raid, target, &read_capacity16(32), &run, Direction::FromDevice);
            assert!(answer.good(), "{family:?} {target:?}");
            let data = bytes_of(&memory, 12);
            assert_eq!(u64::from_be_bytes(data[..8].try_into().unwrap()), 63);
            assert_eq!(u32::from_be_bytes(data[8..].try_into().unwrap()), 512);
        }
    }
}

#[test]
fn synchronize_cache_and_no_data_commands_end_well() {
    for (family, _) in all_families() {
        for target in [Target::Volume(0), Target::Disk(5)] {
            let (_, mut raid) = up(family);
            let answer = exec(&mut raid, target, &SYNC_CACHE, &[], Direction::None);
            assert!(answer.good(), "{family:?} {target:?}");
            assert_eq!((answer.asked, answer.moved), (0, 0));
            let answer = exec(&mut raid, target, &[0; 6], &[], Direction::None);
            assert!(answer.good(), "{family:?} {target:?}");
        }
    }
}

#[test]
fn a_target_that_is_not_there_is_not_found() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        for target in [Target::Volume(9), Target::Disk(9)] {
            let answer = exec(&mut raid, target, &read10(0, 1), &run, Direction::FromDevice);
            assert!(!answer.good(), "{family:?} {target:?}");
            assert_eq!(answer.status, status::DEVICE_NOT_FOUND, "{family:?} {target:?}");
            assert_eq!(answer.scsi_status, 0);
            assert!(!answer.check_condition());
            assert!(answer.sense().is_empty());
            assert_eq!((answer.asked, answer.moved), (512, 0));
            assert_eq!(answer.why(), "there is no such device");
        }
        // A logical unit that is not there, on a target that is.
        let context =
            raid.submit(Target::Volume(0), 1, &read10(0, 1), &run, Direction::FromDevice).unwrap();
        let (done, answer) = raid.complete().unwrap();
        assert_eq!(done, context);
        assert_eq!(answer.status, status::DEVICE_NOT_FOUND, "{family:?}");
        assert_eq!(sim.lock().seen.last().unwrap().lun, 1, "{family:?}");
    }
}

#[test]
fn a_command_that_ends_with_a_check_condition_brings_its_sense_data() {
    let sense = [0x70, 0, 0x03, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0x11, 0x00, 0, 0, 0, 0];
    for (family, _) in all_families() {
        for target in [Target::Volume(0), Target::Disk(5)] {
            let (sim, mut raid) = up(family);
            sim.lock().sense_next = Some(sense.to_vec());
            let memory = buffer(4096);
            let run = [(memory.phys(), 1024)];
            let answer = exec(&mut raid, target, &read10(0, 2), &run, Direction::FromDevice);
            assert!(!answer.good(), "{family:?} {target:?}");
            assert!(answer.check_condition(), "{family:?} {target:?}");
            assert_eq!(answer.status, status::SCSI_DONE_WITH_ERROR);
            assert_eq!(answer.scsi_status, 2);
            assert_eq!(&answer.sense()[..18], &sense, "{family:?} {target:?}");
            assert_eq!((answer.asked, answer.moved), (1024, 0));
            if !family.fusion() {
                // The MFI header says how much sense there is.
                assert_eq!(answer.sense().len(), 18);
            }
            // The next command on the same slot has none.
            let answer = exec(&mut raid, target, &read10(0, 2), &run, Direction::FromDevice);
            assert!(answer.good() && answer.sense().is_empty(), "{family:?} {target:?}");
        }
    }
}

#[test]
fn an_operation_the_target_does_not_know_is_an_illegal_request() {
    for (family, _) in all_families() {
        let (_, mut raid) = up(family);
        let answer =
            exec(&mut raid, Target::Volume(0), &[0xc5, 0, 0, 0, 0, 0], &[], Direction::None);
        assert!(answer.check_condition(), "{family:?}");
        // Sense key 5, additional code 0x20.
        assert_eq!(answer.sense()[2] & 0x0f, 0x05);
        assert_eq!(answer.sense()[12], 0x20);
    }
}

#[test]
fn a_block_beyond_the_end_is_an_illegal_request() {
    for (family, _) in all_families() {
        let (_, mut raid) = up(family);
        let memory = buffer(4096);
        let run = [(memory.phys(), 1024)];
        let answer = exec(&mut raid, Target::Disk(5), &read10(63, 2), &run, Direction::FromDevice);
        assert!(answer.check_condition(), "{family:?}");
        assert_eq!(answer.sense()[12], 0x21);
        assert_eq!((answer.asked, answer.moved), (1024, 0));
    }
}

#[test]
fn a_failure_without_a_check_condition_brings_no_sense_data() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().refuse = Some(status::LD_OFFLINE);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let answer = exec(&mut raid, Target::Volume(0), &read10(0, 1), &run, Direction::FromDevice);
        assert!(!answer.good() && !answer.check_condition(), "{family:?}");
        assert_eq!(answer.status, status::LD_OFFLINE);
        assert!(answer.sense().is_empty(), "{family:?}");
        assert_eq!(answer.why(), "the volume is offline");
        assert_eq!((answer.asked, answer.moved), (512, 0));
    }
}

#[test]
fn how_much_moved_is_what_was_asked_when_it_ends_well() {
    for (family, _) in all_families() {
        let (_, mut raid) = up(family);
        let memory = buffer(8192);
        let run = [(memory.phys(), 2048)];
        let answer = exec(&mut raid, Target::Disk(5), &read10(0, 4), &run, Direction::FromDevice);
        assert_eq!((answer.asked, answer.moved), (2048, 2048), "{family:?}");
        let runs = std::vec![(memory.phys(), 512), (memory.phys() + 512, 1024)];
        let answer = exec(&mut raid, Target::Disk(5), &read10(0, 3), &runs, Direction::FromDevice);
        assert_eq!((answer.asked, answer.moved), (1536, 1536), "{family:?}");
    }
}

// ---- commands refused before they are sent

#[test]
fn commands_the_controller_cannot_carry_are_refused() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        let long = [0u8; 17];
        let target = Target::Volume(0);
        assert_eq!(
            raid.submit(target, 0, &long, &[], Direction::None),
            Err(Error::BadRequest),
            "{family:?}"
        );
        assert_eq!(raid.submit(target, 0, &[], &[], Direction::None), Err(Error::BadRequest));
        let many = std::vec![(0x1000u64, 512u32); raid.max_segments() + 1];
        assert_eq!(
            raid.submit(target, 0, &write10(0, 33), &many, Direction::ToDevice),
            Err(Error::BadRequest),
            "{family:?}"
        );
        assert_eq!(raid.room(), raid.commands(), "{family:?}");
        assert!(sim.lock().seen.is_empty(), "{family:?}");
        // The sixteen-byte block is the longest that goes.
        let memory = buffer(4096);
        let cdb = read16(0, 1);
        let run = [(memory.phys(), 512)];
        assert!(raid.submit(target, 0, &cdb, &run, Direction::FromDevice).is_ok());
    }
}

#[test]
fn an_mfi_frame_names_its_target_in_a_byte() {
    for (family, _) in MFI_FAMILIES {
        let (_, mut raid) = up(family);
        let run = [(0x1000u64, 512u32)];
        let result =
            raid.submit(Target::Volume(256), 0, &read10(0, 1), &run, Direction::FromDevice);
        assert_eq!(result, Err(Error::BadRequest), "{family:?}");
        assert_eq!(raid.room(), raid.commands());
        // QEMU's number for the disk at SCSI ID 5, LUN 0 reaches target 5.
        let memory = buffer(4096);
        let run5 = [(memory.phys(), 512u32)];
        let answer =
            exec(&mut raid, Target::Disk(0x0500), &read10(0, 1), &run5, Direction::FromDevice);
        assert!(answer.good(), "{family:?}");
        // 255 is the last that goes; the firmware does not know it.
        let answer = exec(&mut raid, Target::Disk(255), &read10(0, 1), &run, Direction::FromDevice);
        assert_eq!(answer.status, status::DEVICE_NOT_FOUND, "{family:?}");
    }
}

#[test]
fn a_fusion_request_names_its_target_in_sixteen_bits() {
    for (family, _) in crate::tests::FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().add_volume(300, 64);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let answer =
            exec(&mut raid, Target::Volume(300), &read10(1, 1), &run, Direction::FromDevice);
        assert!(answer.good(), "{family:?}");
        assert!(bytes_of(&memory, 512).iter().all(|&b| b == 1));
        assert_eq!(sim.lock().seen[0].target, 300);
    }
}
