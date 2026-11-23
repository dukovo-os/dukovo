// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MFI frames and the way they are posted, checked in the bytes the
//! simulated firmware read.

use crate::Family;
use crate::frame::{self, cmd, dcmd, flag, header, pass};
use crate::sim::{le16, le32, le64};
use crate::tests::{
    MFI_FAMILIES, block_runs, buffer, bytes_of, exec, read10, try_new, up, write10,
};
use crate::{Direction, Target};

#[test]
fn a_volume_read_is_an_ld_frame_with_its_fields() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let run = [(memory.phys(), 1024)];
        let context =
            raid.submit(Target::Volume(2), 0, &read10(7, 2), &run, Direction::FromDevice).unwrap();
        raid.complete().unwrap();
        let inner = sim.lock();
        let seen = &inner.seen[0];
        let f = &seen.frame;
        assert_eq!(f[header::CMD], cmd::LD_SCSI_IO, "{family:?}");
        assert_eq!(f[header::TARGET], 2);
        assert_eq!(f[header::LUN], 0);
        assert_eq!(f[header::CDB_LEN], 10);
        assert_eq!(f[header::SG_COUNT], 1);
        assert_eq!(f[header::SENSE_LEN], 96);
        assert_eq!(le32(f, header::CONTEXT), u32::from(context) + 1, "a context is never 0");
        assert_eq!(le32(f, header::CONTEXT_HIGH), 0);
        assert_eq!(le32(f, header::DATA_LEN), 1024);
        assert_eq!(&f[pass::CDB..pass::CDB + 10], &read10(7, 2), "{family:?}");
        assert!(f[pass::CDB + 10..pass::CDB + 16].iter().all(|&b| b == 0));
        let ieee = if family == Family::MfiSkinny { flag::IEEE } else { 0 };
        let expected = flag::READ | flag::SGL64 | flag::SENSE64 | ieee;
        assert_eq!(le16(f, header::FLAGS), expected, "{family:?}");
        // The entry: the run's address and length (the list starts at 48).
        assert_eq!(le64(f, pass::SGL), memory.phys());
        assert_eq!(le32(f, pass::SGL + 8), 1024);
        assert!(seen.volume);
    }
}

#[test]
fn a_disk_write_is_a_pd_frame_with_the_write_flag() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        exec(&mut raid, Target::Disk(5), &write10(0, 1), &run, Direction::ToDevice);
        let f = sim.lock().seen[0].frame.clone();
        assert_eq!(f[header::CMD], cmd::PD_SCSI_IO, "{family:?}");
        assert_eq!(f[header::TARGET], 5);
        let ieee = if family == Family::MfiSkinny { flag::IEEE } else { 0 };
        let expected = flag::WRITE | flag::SGL64 | flag::SENSE64 | ieee;
        assert_eq!(le16(&f, header::FLAGS), expected, "{family:?}");
    }
}

#[test]
fn a_command_without_data_has_no_direction() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        exec(&mut raid, Target::Volume(0), &[0; 6], &[], Direction::None);
        let f = sim.lock().seen[0].frame.clone();
        assert_eq!(f[header::SG_COUNT], 0);
        assert_eq!(le32(&f, header::DATA_LEN), 0);
        let ieee = if family == Family::MfiSkinny { flag::IEEE } else { 0 };
        assert_eq!(le16(&f, header::FLAGS), flag::SGL64 | flag::SENSE64 | ieee, "{family:?}");
    }
}

#[test]
fn a_management_frame_has_its_opcode_mailbox_and_entry() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let mailbox = [9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 11, 12];
        let mut wait = sim.waiter();
        raid.run_dcmd(
            0x0301_0000,
            &mailbox,
            &[(memory.phys(), 2048)],
            Direction::FromDevice,
            &mut wait,
            100,
        )
        .unwrap();
        let f = sim.lock().seen[0].frame.clone();
        assert_eq!(f[header::CMD], cmd::DCMD, "{family:?}");
        assert_eq!(f[header::SG_COUNT], 1);
        assert_eq!(le32(&f, dcmd::OPCODE), 0x0301_0000);
        assert_eq!(&f[dcmd::MAILBOX..dcmd::MAILBOX + 12], &mailbox);
        // A management command's entries are always 64-bit, not IEEE.
        assert_eq!(le16(&f, header::FLAGS), flag::READ | flag::SGL64, "{family:?}");
        assert_eq!(le32(&f, header::DATA_LEN), 2048);
        assert_eq!(le64(&f, dcmd::SGL), memory.phys());
        assert_eq!(le32(&f, dcmd::SGL + 8), 2048);
    }
}

#[test]
fn each_command_has_a_frame_and_a_sense_buffer_of_its_own() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let contexts: std::vec::Vec<u16> = (0..4)
            .map(|_| {
                raid.submit(Target::Disk(5), 0, &read10(0, 1), &run, Direction::FromDevice).unwrap()
            })
            .collect();
        assert_eq!(contexts, [0, 1, 2, 3]);
        let inner = sim.lock();
        let mut senses = std::vec::Vec::new();
        for (n, seen) in inner.seen.iter().enumerate() {
            assert_eq!(le32(&seen.frame, header::CONTEXT), n as u32 + 1, "{family:?}");
            let sense = u64::from(le32(&seen.frame, pass::SENSE_LOW))
                | u64::from(le32(&seen.frame, pass::SENSE_HIGH)) << 32;
            assert!(inner.holds(sense) && inner.holds(sense + 95), "{family:?}");
            senses.push(sense);
        }
        // The frames were posted 512 bytes apart, and the sense buffers
        // do not overlap.
        let frames: std::vec::Vec<u64> =
            inner.posts.iter().skip(1).map(|&(word, _)| u64::from(word & !0x1f)).collect();
        for pair in frames.windows(2) {
            assert_eq!(pair[1] - pair[0], 512, "{family:?}");
        }
        for pair in senses.windows(2) {
            assert!(pair[1] - pair[0] >= 96, "{family:?}");
        }
    }
}

#[test]
fn the_inbound_word_counts_the_frames_after_the_first() {
    // (runs, extra frames): a pass frame is 48 bytes and its entries, in
    // 64-byte frames, seven extra at most.
    let skinny = [(0usize, 0u32), (1, 0), (2, 1), (8, 2), (9, 2), (29, 7)];
    let older = [(0usize, 0u32), (1, 0)];
    for (family, _) in MFI_FAMILIES {
        let table: &[(usize, u32)] = if family == Family::MfiSkinny { &skinny } else { &older };
        for &(runs, extra) in table {
            let (sim, mut raid) = up(family);
            let memory = buffer(32 * 512);
            let list: std::vec::Vec<(u64, u32)> =
                (0..runs).map(|n| (memory.phys() + n as u64 * 512, 512)).collect();
            let dir = if runs == 0 { Direction::None } else { Direction::FromDevice };
            let cdb = read10(0, runs as u16);
            exec(&mut raid, Target::Disk(5), &cdb, &list, dir);
            let word = sim.lock().seen[0].word;
            assert_eq!(word & 1, 1, "{family:?} {runs} runs");
            assert_eq!((word >> 1) & 0xf, extra, "{family:?} {runs} runs");
            assert_eq!(word & 0x1e0, 0, "the frame's address is 512-byte aligned");
        }
    }
}

#[test]
fn a_management_frame_with_one_run_is_one_frame() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let mut wait = sim.waiter();
        raid.run_dcmd(1, &[0; 12], &[(memory.phys(), 16)], Direction::FromDevice, &mut wait, 100)
            .unwrap();
        let word = sim.lock().seen[0].word;
        assert_eq!(word & 0x1f, 1, "{family:?}");
    }
}

#[test]
fn skinny_entries_are_the_sixteen_byte_ones_in_a_list_of_the_most_runs() {
    let (sim, mut raid) = up(Family::MfiSkinny);
    let max = raid.max_segments();
    assert_eq!(max, 29);
    let fills: std::vec::Vec<u8> = (0..max as u8).map(|n| 0x40 + n).collect();
    let (_memories, runs) = block_runs(Family::MfiSkinny, &fills);
    let answer =
        exec(&mut raid, Target::Volume(0), &write10(10, max as u16), &runs, Direction::ToDevice);
    assert!(answer.good());
    let inner = sim.lock();
    let f = &inner.seen[0].frame;
    assert_eq!(usize::from(f[header::SG_COUNT]), max);
    assert_eq!(inner.seen[0].runs, runs);
    // The list fills the frame to its last byte.
    assert_eq!(frame::pass_size(max, true), 512);
    for (n, run) in runs.iter().enumerate() {
        assert_eq!(le64(f, pass::SGL + n * 16), run.0);
        assert_eq!(le32(f, pass::SGL + n * 16 + 8), 512);
        assert_eq!(le32(f, pass::SGL + n * 16 + 12), 0);
    }
    let disk = &inner.volumes[&0].data;
    for n in 0..max {
        assert_eq!(disk[(10 + n) * 512], 0x40 + n as u8, "block {n}");
    }
}

#[test]
fn the_older_parts_take_a_list_of_the_most_runs() {
    // One run of all 32 blocks; the test below splits them.
    for family in [Family::Mfi1078, Family::MfiGen2] {
        let (_, mut raid) = up(family);
        assert_eq!(raid.max_segments(), 32);
        let memory = buffer(32 * 512);
        let answer = exec(
            &mut raid,
            Target::Volume(0),
            &read10(0, 32),
            &[(memory.phys(), 32 * 512)],
            Direction::FromDevice,
        );
        assert!(answer.good());
        assert_eq!(bytes_of(&memory, 512 * 32)[31 * 512], 31);
    }
}

#[test]
fn the_older_parts_take_data_in_several_runs() {
    for family in [Family::Mfi1078, Family::MfiGen2] {
        let (sim, mut raid) = up(family);
        let fills: std::vec::Vec<u8> = (0..32u8).map(|n| 0x40 + n).collect();
        let runs: std::vec::Vec<(u64, u32)> = fills
            .iter()
            .map(|&fill| {
                let memory = buffer(4096).kept();
                memory.write(0, &[fill; 512]);
                (memory.phys(), 512)
            })
            .collect();
        let answer =
            exec(&mut raid, Target::Volume(0), &write10(0, 32), &runs, Direction::ToDevice);
        assert!(answer.good(), "{family:?}");
        assert_eq!(sim.lock().volumes[&0].data[31 * 512], 0x40 + 31);
    }
}

#[test]
fn a_command_goes_out_when_interrupts_are_enabled_and_comes_back_through_the_queue() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        raid.submit(Target::Disk(5), 0, &read10(1, 1), &run, Direction::FromDevice).unwrap();
        let inner = sim.lock();
        let queue = inner.queue.as_ref().unwrap();
        // One context was put on the queue and the producer moved on.
        assert_eq!(queue.head, 1, "{family:?}");
        drop(inner);
        assert!(raid.complete().is_some());
        assert!(raid.complete().is_none());
    }
}

#[test]
fn a_management_command_ends_once_interrupts_are_enabled() {
    for (family, _) in MFI_FAMILIES {
        let sim = crate::sim::Sim::new(family);
        let mut raid = try_new(&sim, false).unwrap();
        let mut wait = sim.waiter();
        // As on QEMU, the reply queue does not move while they are masked.
        let masked = raid.run_dcmd(1, &[0; 12], &[], Direction::None, &mut wait, 100);
        assert!(masked.is_err(), "{family:?}");
        let mut raid = try_new(&sim, false).unwrap();
        raid.enable_interrupts();
        let result = raid.run_dcmd(1, &[0; 12], &[], Direction::None, &mut wait, 100);
        assert!(result.is_ok(), "{family:?}");
    }
}
