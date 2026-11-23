// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The fusion request frames, descriptors and lists, checked in the bytes
//! the simulated firmware read.

use std::vec::Vec;

use crate::sim::{Seen, le16, le32, le64, read_bytes};
use crate::tests::{
    FUSION_FAMILIES, READ_CAPACITY_10, SYNC_CACHE, block_runs, buffer, exec, inquiry, read10,
    read16, up, write10, write16,
};
use crate::{Direction, Family, Target};

/// The request frame's fields.
const DEV_HANDLE: usize = 0x00;
const CHAIN_OFFSET: usize = 0x02;
const FUNCTION: usize = 0x03;
const SENSE_LOW: usize = 0x0c;
const SGL_FLAGS: usize = 0x10;
const SENSE_LEN: usize = 0x12;
const SGL_OFFSET: usize = 0x14;
const DATA_LENGTH: usize = 0x1c;
const IO_FLAGS: usize = 0x24;
const LUN: usize = 0x34;
const CONTROL: usize = 0x3c;
const CDB: usize = 0x40;
const CONTEXT: usize = 0x60;
const SGL: usize = 0x80;

/// The RAID context's fields, from the context's start.
const TYPE_NSEG: usize = 0;
const TIMEOUT: usize = 2;
const LOCK_OR_ROUTING: usize = 4;
const TARGET: usize = 6;
const RAID_FLAGS: usize = 24;
const NUM_SGE: usize = 25;
const NUM_SGE_G35: usize = 28;
const NUM_SGE_EXT: usize = 30;

/// The type of descriptor an LD read or write of `family` goes as.
fn io_type(family: Family) -> u8 {
    if family == Family::Invader { 2 } else { 7 }
}

/// Sends one command and returns the firmware's note of it.
fn one(family: Family, target: Target, cdb: &[u8], runs: usize, dir: Direction) -> Seen {
    let (sim, mut raid) = up(family);
    let memory = buffer(32 * 512);
    let list: Vec<(u64, u32)> = (0..runs).map(|n| (memory.phys() + n as u64 * 512, 512)).collect();
    exec(&mut raid, target, cdb, &list, dir);
    sim.lock().seen[0].clone()
}

#[test]
fn volume_reads_and_writes_go_as_the_descriptor_type_of_the_family() {
    for (family, _) in FUSION_FAMILIES {
        for cdb in [
            read10(0, 1).to_vec(),
            write10(0, 1).to_vec(),
            read16(0, 1).to_vec(),
            write16(0, 1).to_vec(),
        ] {
            let dir = if cdb[0] & 2 == 0 { Direction::FromDevice } else { Direction::ToDevice };
            let seen = one(family, Target::Volume(0), &cdb, 1, dir);
            assert_eq!(seen.descriptor.0, io_type(family), "{family:?} {:#x}", cdb[0]);
            assert_eq!(seen.descriptor.2, 0, "an LD request has no device handle");
        }
    }
}

#[test]
fn other_volume_commands_go_as_plain_scsi_io() {
    let commands: [Vec<u8>; 4] =
        [inquiry(36).to_vec(), READ_CAPACITY_10.to_vec(), SYNC_CACHE.to_vec(), [0u8; 6].to_vec()];
    for (family, _) in FUSION_FAMILIES {
        for cdb in &commands {
            let seen = one(family, Target::Volume(0), cdb, 0, Direction::None);
            assert_eq!(seen.descriptor.0, 0, "{family:?} {:#x}", cdb[0]);
            assert_eq!(seen.descriptor.2, 0);
            // Still an LD I/O request in the frame, with no CUDA path.
            let f = &seen.frame;
            assert_eq!(f[FUNCTION], 0xf1);
            assert_eq!(f[CONTEXT + TYPE_NSEG], 0, "{family:?}");
            assert_eq!(le16(f, CONTEXT + LOCK_OR_ROUTING), 0, "{family:?}");
        }
    }
}

#[test]
fn a_disk_goes_as_a_system_disk_through_the_firmware() {
    for (family, _) in FUSION_FAMILIES {
        for cdb in [read10(0, 1), write10(0, 1)] {
            let dir = if cdb[0] == 0x28 { Direction::FromDevice } else { Direction::ToDevice };
            let seen = one(family, Target::Disk(5), &cdb, 1, dir);
            // Descriptor type 0 and handle 0xffff, for reads and writes too.
            assert_eq!(seen.descriptor.0, 0, "{family:?}");
            assert_eq!(seen.descriptor.2, 0xffff, "{family:?}");
            let f = &seen.frame;
            assert_eq!(f[FUNCTION], 0xf1);
            assert_eq!(le16(f, DEV_HANDLE), 0xffff);
            assert_eq!(le16(f, CONTEXT + TARGET), 5);
            assert_eq!(f[CONTEXT + RAID_FLAGS], 0x10, "the system disk subtype");
            assert_eq!(le16(f, CONTEXT + TIMEOUT), 30);
            assert_eq!(f[CONTEXT + TYPE_NSEG], 0);
            assert!(!seen.volume);
        }
    }
}

#[test]
fn a_volume_is_named_by_its_target_in_the_frame_and_context() {
    for (family, _) in FUSION_FAMILIES {
        let seen = one(family, Target::Volume(2), &read10(0, 1), 1, Direction::FromDevice);
        let f = &seen.frame;
        assert_eq!(le16(f, DEV_HANDLE), 2, "{family:?}");
        assert_eq!(le16(f, CONTEXT + TARGET), 2, "{family:?}");
        assert_eq!(f[CONTEXT + RAID_FLAGS], 0, "{family:?}");
        assert_eq!(le16(f, CONTEXT + TIMEOUT), 0);
        assert!(seen.volume);
    }
}

#[test]
fn the_context_says_how_the_firmware_may_take_a_read_or_write() {
    for (family, _) in FUSION_FAMILIES {
        let seen = one(family, Target::Volume(0), &read10(0, 1), 1, Direction::FromDevice);
        let c = &seen.frame[CONTEXT..];
        match family {
            Family::Thunderbolt => {
                assert_eq!(c[TYPE_NSEG], 0, "{family:?}");
                assert_eq!(c[LOCK_OR_ROUTING], 0);
            }
            Family::Invader => {
                // A CUDA request of one segment, with no lock to take.
                assert_eq!(c[TYPE_NSEG], 0x12, "{family:?}");
                assert_eq!(c[LOCK_OR_ROUTING], 0x08);
            }
            _ => {
                assert_eq!(c[TYPE_NSEG], 0x12, "{family:?}");
                assert_eq!(le16(c, LOCK_OR_ROUTING), 1 << 4, "the routing flag");
            }
        }
    }
}

#[test]
fn the_segment_count_is_where_the_family_keeps_it() {
    for (family, _) in FUSION_FAMILIES {
        for runs in [1usize, 9, 32] {
            let seen =
                one(family, Target::Volume(0), &write10(0, runs as u16), runs, Direction::ToDevice);
            let c = &seen.frame[CONTEXT..];
            if family.ventura() {
                // Twelve bits, and the legacy bytes are left alone.
                assert_eq!(le16(c, NUM_SGE_G35), runs as u16, "{family:?} {runs}");
                assert_eq!(c[NUM_SGE], 0);
                assert_eq!(c[NUM_SGE_EXT], 0);
            } else {
                assert_eq!(c[NUM_SGE], runs as u8, "{family:?} {runs}");
                assert_eq!(c[NUM_SGE_EXT], 0);
                assert_eq!(le16(c, NUM_SGE_G35), 0);
            }
        }
    }
}

#[test]
fn the_request_has_its_fields_in_place() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let run = [(memory.phys(), 1024)];
        let cdb = read10(9, 2);
        let slot = raid.submit(Target::Volume(0), 3, &cdb, &run, Direction::FromDevice).unwrap();
        raid.complete().unwrap();
        let inner = sim.lock();
        let f = &inner.seen[0].frame;
        assert_eq!(f[FUNCTION], 0xf1, "{family:?}");
        assert_eq!(le16(f, SGL_FLAGS), 0x02);
        assert_eq!(f[SENSE_LEN], 96);
        assert_eq!(f[SGL_OFFSET], 0x20, "the list starts at 0x80, in 4-byte units");
        assert_eq!(le32(f, DATA_LENGTH), 1024);
        assert_eq!(le16(f, IO_FLAGS), 10, "the descriptor block's length");
        assert_eq!((f[LUN], f[LUN + 1]), (0, 3), "a single-level unit number");
        assert_eq!(le32(f, CONTROL), 0x0200_0000, "read");
        assert_eq!(&f[CDB..CDB + 10], &cdb);
        assert!(f[CDB + 10..CDB + 16].iter().all(|&b| b == 0));
        assert_eq!(f[CHAIN_OFFSET], 0);
        // The descriptor: the SMID is the slot plus one.
        let (kind, smid, handle) = inner.seen[0].descriptor;
        assert_eq!(smid, slot + 1);
        let (low, high) = *inner.descriptors.last().unwrap();
        assert_eq!(low, u32::from(kind) << 1 | u32::from(smid) << 16, "{family:?}");
        assert_eq!(high, u32::from(handle) << 16);
        // The request frames are 256 bytes apart from the base.
        let ioc = inner.ioc.as_ref().unwrap();
        assert!(inner.holds(ioc.requests + u64::from(smid) * 256 + 255));
    }
}

#[test]
fn the_control_word_says_which_way_the_data_go() {
    for (family, _) in FUSION_FAMILIES {
        for (dir, control, runs) in [
            (Direction::FromDevice, 0x0200_0000u32, 1),
            (Direction::ToDevice, 0x0100_0000, 1),
            (Direction::None, 0, 0),
        ] {
            let cdb = if runs == 0 { [0x35u8, 0, 0, 0, 0, 0, 0, 0, 0, 0] } else { write10(0, 1) };
            let seen = one(family, Target::Volume(0), &cdb, runs, dir);
            assert_eq!(le32(&seen.frame, CONTROL), control, "{family:?} {dir:?}");
        }
    }
}

#[test]
fn each_command_has_a_sense_buffer_of_its_own() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        for _ in 0..3 {
            raid.submit(Target::Volume(0), 0, &read10(0, 1), &run, Direction::FromDevice).unwrap();
        }
        let inner = sim.lock();
        let high = u64::from(inner.ioc.as_ref().unwrap().sense_high) << 32;
        let senses: Vec<u64> =
            inner.seen.iter().map(|s| high | u64::from(le32(&s.frame, SENSE_LOW))).collect();
        for &sense in &senses {
            assert!(inner.holds(sense) && inner.holds(sense + 95), "{family:?}");
        }
        for pair in senses.windows(2) {
            assert!(pair[1] - pair[0] >= 96, "{family:?}");
        }
    }
}

/// Checks the list a write of `runs` runs made.
fn check_list(family: Family, seen: &Seen, runs: usize, chain_addr_in: impl Fn(u64) -> bool) {
    let f = &seen.frame;
    let end = if family == Family::Thunderbolt { 0 } else { 0x40 };
    if runs <= 8 {
        assert_eq!(seen.entries.len(), runs, "{family:?} {runs}");
        assert_eq!(f[CHAIN_OFFSET], 0, "no chain");
        for (n, entry) in seen.entries.iter().enumerate() {
            let last = n + 1 == runs;
            assert_eq!(entry.flags, if last { end } else { 0 }, "{family:?} {runs} entry {n}");
            assert!(!entry.in_chain);
        }
        return;
    }
    // Seven in the frame, the chain element, the rest in the chain.
    assert_eq!(seen.entries.len(), runs + 1, "{family:?} {runs}");
    assert_eq!(f[CHAIN_OFFSET], 15, "the element is the eighth, at 0xf0");
    let (frame_entries, chain_entries) = seen.entries.split_at(8);
    for entry in &frame_entries[..7] {
        assert_eq!(entry.flags, 0, "{family:?} {runs}");
    }
    let element = frame_entries[7];
    let chain_flag = if family == Family::Thunderbolt { 0x83 } else { 0x80 };
    assert_eq!(element.flags, chain_flag, "{family:?} {runs}");
    assert_eq!(element.len as usize, chain_entries.len() * 16, "{family:?} {runs}");
    assert!(chain_addr_in(element.addr), "{family:?}");
    for (n, entry) in chain_entries.iter().enumerate() {
        assert!(entry.in_chain);
        let last = n + 1 == chain_entries.len();
        assert_eq!(entry.flags, if last { end } else { 0 }, "{family:?} {runs} chain {n}");
    }
}

#[test]
fn data_in_one_eight_nine_and_thirty_two_runs_is_listed_and_moved() {
    for (family, _) in FUSION_FAMILIES {
        for runs in [1usize, 8, 9, 32] {
            let (sim, mut raid) = up(family);
            let fills: Vec<u8> = (0..runs as u8).map(|n| 0x60 + n).collect();
            let (_memories, list) = block_runs(family, &fills);
            assert_eq!(list.len(), runs);
            let answer = exec(
                &mut raid,
                Target::Volume(0),
                &write10(8, runs as u16),
                &list,
                Direction::ToDevice,
            );
            assert!(answer.good(), "{family:?} {runs}");
            assert_eq!((answer.asked, answer.moved), (runs as u64 * 512, runs as u64 * 512));
            let inner = sim.lock();
            let seen = &inner.seen[0];
            assert_eq!(seen.runs, list, "{family:?} {runs}: the firmware read the same runs");
            assert_eq!(le32(&seen.frame, DATA_LENGTH), runs as u32 * 512);
            check_list(family, seen, runs, |addr| inner.holds(addr));
            let disk = &inner.volumes[&0].data;
            for n in 0..runs {
                assert_eq!(
                    disk[(8 + n) * 512 + 511],
                    0x60 + n as u8,
                    "{family:?} {runs} block {n}"
                );
            }
        }
    }
}

#[test]
fn a_chain_list_is_read_back_the_same_for_a_disk() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        let fills: Vec<u8> = (0..12u8).collect();
        let (_memories, list) = block_runs(family, &fills);
        let memory = buffer(4096);
        exec(&mut raid, Target::Disk(5), &write10(0, 12), &list, Direction::ToDevice);
        let runs: Vec<(u64, u32)> = (0..12).map(|n| (memory.phys() + n * 256, 256)).collect();
        let answer = exec(&mut raid, Target::Disk(5), &read10(0, 6), &runs, Direction::FromDevice);
        assert!(answer.good(), "{family:?}");
        let seen = sim.lock().seen[1].clone();
        assert_eq!(seen.runs, runs);
        assert_eq!(seen.entries.len(), 13);
    }
}

#[test]
fn commands_in_flight_have_chain_frames_of_their_own() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let fills: Vec<u8> = (0..9u8).collect();
        let (_memories, list) = block_runs(family, &fills);
        for _ in 0..3 {
            raid.submit(Target::Volume(0), 0, &write10(0, 9), &list, Direction::ToDevice).unwrap();
        }
        let inner = sim.lock();
        let chains: Vec<u64> = inner.seen.iter().map(|s| s.entries[7].addr).collect();
        for pair in chains.windows(2) {
            assert_eq!(pair[1] - pair[0], 512, "{family:?}");
        }
    }
}

#[test]
fn a_management_command_goes_as_a_pass_through_request_to_a_dcmd_frame() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let mailbox = [1u8; 12];
        let mut wait = sim.waiter();
        raid.run_dcmd(
            0x0501_0000,
            &mailbox,
            &[(memory.phys(), 512)],
            Direction::FromDevice,
            &mut wait,
            100,
        )
        .unwrap();
        let inner = sim.lock();
        let seen = &inner.seen[0];
        let f = &seen.frame;
        assert_eq!(f[FUNCTION], 0xf0, "{family:?}");
        assert_eq!(f[SGL_OFFSET], 0x20);
        assert_eq!(f[CHAIN_OFFSET], 8, "the element is the first entry, at 0x80");
        // Type 0, no device handle.
        assert_eq!(seen.descriptor, (0, 1, 0), "{family:?}");
        // The element points at an MFI frame of 256 bytes.
        let (addr, len, flags) = (le64(f, SGL), le32(f, SGL + 8), f[SGL + 15]);
        assert!(inner.holds(addr), "{family:?}");
        assert_eq!(len, 256);
        assert_eq!(flags, 0x83, "{family:?}");
        let mfi = read_bytes(addr, 64);
        assert_eq!(mfi[0], 0x05, "a DCMD frame");
        assert_eq!(le32(&mfi, 8), 1, "its context is the SMID");
        assert_eq!(le32(&mfi, 24), 0x0501_0000);
        assert_eq!(&mfi[28..40], &mailbox);
        // Its status was written by the firmware, and the request's
        // context was left alone.
        assert_eq!(mfi[2], 0);
        assert_eq!(seen.opcode, 0x0501_0000);
    }
}

#[test]
fn a_management_command_has_a_chain_frame_for_each_slot() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        for _ in 0..3 {
            raid.submit_dcmd(1, &[0; 12], &[], Direction::None).unwrap();
        }
        let inner = sim.lock();
        let frames: Vec<u64> = inner.seen.iter().map(|s| le64(&s.frame, SGL)).collect();
        for pair in frames.windows(2) {
            assert_eq!(pair[1] - pair[0], 512, "{family:?}");
        }
        let smids: Vec<u16> = inner.seen.iter().map(|s| s.descriptor.1).collect();
        assert_eq!(smids, [1, 2, 3]);
    }
}

#[test]
fn a_member_command_goes_as_an_mfi_pass_frame_inside_a_pass_through() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        let memory = buffer(4096);
        let cdb = inquiry(36);
        let context =
            raid.submit_member(5, &cdb, &[(memory.phys(), 36)], Direction::FromDevice).unwrap();
        let (done, answer) = raid.complete().expect("the command ended");
        assert_eq!(done, context);
        assert!(answer.good(), "{family:?}: {}", answer.why());
        assert_eq!(answer.moved, 36);
        assert_eq!(&crate::tests::bytes_of(&memory, 16)[8..16], b"RADIXSIM");
        let inner = sim.lock();
        let seen = &inner.seen[0];
        let f = &seen.frame;
        assert_eq!(f[FUNCTION], 0xf0, "{family:?}");
        assert_eq!(seen.descriptor, (0, 1, 0), "{family:?}");
        let mfi = read_bytes(le64(f, SGL), 64);
        assert_eq!(mfi[0], 0x04, "a physical disk's pass frame");
        assert_eq!(mfi[4], 5, "its target is the device ID");
        assert_eq!(mfi[6], 6, "a six-byte command");
        assert_eq!(le32(&mfi, 8), 1, "its context is the SMID");
        assert!(!seen.volume);
        assert_eq!(seen.target, 5);
        assert_eq!(seen.cdb, cdb);
    }
}

#[test]
fn a_member_command_brings_the_disks_status_and_sense_back() {
    let sense = [0x70, 0, 0x05, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0x20, 0];
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().sense_next = Some(sense.to_vec());
        let memory = buffer(4096);
        let cdb = [0x4d, 0, 0x40, 0, 0, 0, 0, 0x02, 0, 0];
        raid.submit_member(5, &cdb, &[(memory.phys(), 512)], Direction::FromDevice).unwrap();
        let (_, answer) = raid.complete().expect("the command ended");
        assert!(answer.check_condition(), "{family:?}: {}", answer.why());
        assert_eq!(answer.moved, 0);
        assert_eq!(answer.sense(), &sense);
    }
}

#[test]
fn a_member_beyond_a_byte_is_refused() {
    for (family, _) in FUSION_FAMILIES {
        let (_, mut raid) = up(family);
        let err = raid.submit_member(0x100, &inquiry(36), &[], Direction::FromDevice);
        assert_eq!(err, Err(crate::Error::BadRequest), "{family:?}");
        assert_eq!(raid.in_flight(), 0);
    }
}
