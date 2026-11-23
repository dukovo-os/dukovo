// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The reply queues and the slots: filling every slot, wrapping the
//! queues, the replies the driver passes over, the reply index, and reset.

use std::boxed::Box;
use std::vec::Vec;

use crate::frame::status;
use crate::sim::fw;
use crate::tests::{
    FUSION_FAMILIES, MFI_FAMILIES, all_families, buffer, exec, read10, up, up_with,
};
use crate::{Direction, Error, Target};

/// Submits `count` one-block reads of the disk into `memory` and returns
/// their contexts.
fn submit_reads(raid: &mut crate::MegaRaid, count: usize, run: [(u64, u32); 1]) -> Vec<u16> {
    (0..count)
        .map(|_| {
            raid.submit(Target::Disk(5), 0, &read10(0, 1), &run, Direction::FromDevice).unwrap()
        })
        .collect()
}

/// Takes `count` answers, which must all be good; their contexts.
fn take(raid: &mut crate::MegaRaid, count: usize) -> Vec<u16> {
    (0..count)
        .map(|_| {
            let (context, answer) = raid.complete().expect("a command ended");
            assert!(answer.good());
            context
        })
        .collect()
}

#[test]
fn every_command_can_be_in_flight_and_the_next_is_refused() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let total = raid.commands();
        let mut contexts = Vec::new();
        for n in 0..total {
            assert_eq!(raid.room(), total - n, "{family:?}");
            assert_eq!(raid.in_flight(), n);
            contexts.extend(submit_reads(&mut raid, 1, run));
        }
        assert_eq!(raid.room(), 0, "{family:?}");
        assert_eq!(
            raid.submit(Target::Disk(5), 0, &read10(0, 1), &run, Direction::FromDevice),
            Err(Error::Full)
        );
        assert_eq!(raid.submit_dcmd(1, &[0; 12], &[], Direction::None), Err(Error::Full));
        contexts.sort_unstable();
        contexts.dedup();
        assert_eq!(contexts.len(), total, "{family:?}: every context is its own");
        sim.release_all();
        let mut done = take(&mut raid, total);
        done.sort_unstable();
        assert_eq!(done, contexts, "{family:?}");
        assert!(raid.complete().is_none());
        assert_eq!(raid.room(), total, "{family:?}");
        // The slots are free again.
        assert!(
            raid.submit(Target::Disk(5), 0, &read10(0, 1), &run, Direction::FromDevice).is_ok()
        );
    }
}

#[test]
fn no_more_than_256_commands_are_kept_in_flight() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up_with(family, |inner| inner.commands = 400);
        sim.lock().holding = true;
        assert_eq!(raid.commands(), 256, "{family:?}");
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        submit_reads(&mut raid, 256, run);
        assert_eq!(
            raid.submit(Target::Disk(5), 0, &read10(0, 1), &run, Direction::FromDevice),
            Err(Error::Full),
            "{family:?}"
        );
        sim.release_all();
        take(&mut raid, 256);
        assert_eq!(raid.room(), 256);
    }
}

#[test]
fn the_mfi_reply_queue_wraps_at_its_entries() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up_with(family, |inner| inner.commands = 8);
        assert_eq!(raid.commands(), 7);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        // One at a time, around the queue several times.
        for n in 1..=20u32 {
            exec(&mut raid, Target::Disk(5), &read10(0, 1), &run, Direction::FromDevice);
            let head = sim.lock().queue.as_ref().unwrap().head;
            assert_eq!(head, n % 8, "{family:?}");
        }
        // Then in bursts that end across the wrap.
        sim.lock().holding = true;
        for _ in 0..10 {
            let contexts = submit_reads(&mut raid, 7, run);
            sim.release_all();
            assert_eq!(take(&mut raid, 7), contexts, "{family:?}");
            assert!(raid.complete().is_none());
        }
        let inner = sim.lock();
        assert_eq!(inner.queue.as_ref().unwrap().head, (20 + 70) % 8, "{family:?}");
        assert_eq!(inner.overruns, 0, "{family:?}");
    }
}

#[test]
fn the_fusion_reply_ring_wraps_at_its_depth() {
    for (family, _) in FUSION_FAMILIES {
        // Nine from the firmware is eight commands and a ring of 32.
        let (sim, mut raid) = up_with(family, |inner| inner.commands = 9);
        assert_eq!(raid.commands(), 8);
        assert_eq!(sim.lock().ioc.as_ref().unwrap().depth, 32);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        for _ in 0..25 {
            let contexts = submit_reads(&mut raid, 8, run);
            sim.release_all();
            assert_eq!(take(&mut raid, 8), contexts, "{family:?}");
            assert!(raid.complete().is_none());
        }
        let inner = sim.lock();
        assert_eq!(inner.posted_total, 200);
        assert_eq!(inner.fw_index, 200 % 32, "{family:?}");
        // The firmware never wrote over a reply the host had not cleared.
        assert_eq!(inner.overwrites, 0, "{family:?}");
        assert_eq!(inner.reported_total, 200);
    }
}

#[test]
fn replies_the_host_has_not_reported_are_not_reused() {
    // The ring is 32 deep. A driver that reads replies and never reports
    // them would stop the firmware after 32: here the driver reports, so
    // many times the depth go through.
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up_with(family, |inner| inner.commands = 9);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        for _ in 0..100 {
            exec(&mut raid, Target::Disk(5), &read10(0, 1), &run, Direction::FromDevice);
            // Another read finds nothing and tells the firmware.
            assert!(raid.complete().is_none());
        }
        let inner = sim.lock();
        assert_eq!(inner.held(), 0, "{family:?}: no reply waits for room");
        assert_eq!(inner.overwrites, 0);
    }
}

#[test]
fn the_reply_index_is_written_every_32_replies_and_when_nothing_is_left() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        submit_reads(&mut raid, 40, run);
        sim.release_all();
        take(&mut raid, 31);
        assert!(sim.lock().reply_index_writes.is_empty(), "{family:?}: not yet");
        take(&mut raid, 1);
        assert_eq!(sim.lock().reply_index_writes, [(0x6c, 32)], "{family:?}: the 32nd");
        take(&mut raid, 8);
        assert_eq!(sim.lock().reply_index_writes.len(), 1);
        // Nothing left: the rest are told.
        assert!(raid.complete().is_none());
        assert_eq!(sim.lock().reply_index_writes, [(0x6c, 32), (0x6c, 40)], "{family:?}");
        // Nothing read since: nothing to tell.
        assert!(raid.complete().is_none());
        assert_eq!(sim.lock().reply_index_writes.len(), 2);
    }
}

#[test]
fn the_reply_index_follows_the_ring_around() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        // 160 deep: 5 rounds of 40 go round it once and a quarter.
        for _ in 0..5 {
            submit_reads(&mut raid, 40, run);
            sim.release_all();
            take(&mut raid, 40);
            assert!(raid.complete().is_none());
        }
        let inner = sim.lock();
        assert!(
            inner
                .reply_index_writes
                .iter()
                .all(|&(register, value)| register == 0x6c && value < 160)
        );
        assert_eq!(inner.reply_index_writes.last().unwrap().1, 200 % 160, "{family:?}");
        assert_eq!(inner.fw_index, 40);
        assert_eq!(inner.reported_total, 200);
    }
}

#[test]
fn mfi_contexts_the_driver_never_gave_out_are_passed_over() {
    for (family, _) in MFI_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let context = submit_reads(&mut raid, 1, run)[0];
        {
            let mut inner = sim.lock();
            // Zero is the INIT frame's, and these are in no slot.
            for junk in [0, 9999, 0xffff_ffff] {
                inner.inject_context(junk);
            }
        }
        sim.release_all();
        let (done, answer) = raid.complete().unwrap();
        assert_eq!(done, context, "{family:?}");
        assert!(answer.good());
        assert!(raid.complete().is_none());
        assert_eq!(raid.in_flight(), 0);
        // A context that was already answered is passed over too.
        sim.lock().inject_context(u32::from(context) + 1);
        assert!(raid.complete().is_none(), "{family:?}");
        assert_eq!(raid.room(), raid.commands());
    }
}

#[test]
fn fusion_descriptors_for_commands_the_driver_never_gave_out_are_passed_over() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let context = submit_reads(&mut raid, 1, run)[0];
        {
            let mut inner = sim.lock();
            // SMID 0, a SMID beyond the commands, and the last SMID there is.
            for smid in [0u32, 200, 0xffff] {
                inner.inject_reply(smid << 16, 0);
            }
        }
        sim.release_all();
        let (done, answer) = raid.complete().unwrap();
        assert_eq!(done, context, "{family:?}");
        assert!(answer.good());
        assert!(raid.complete().is_none());
        assert_eq!(raid.in_flight(), 0, "{family:?}");
        // Four descriptors were read, so the index went to four.
        assert_eq!(sim.lock().reply_index_writes.last().unwrap().1, 4);
        // One for a command that already ended.
        sim.lock().inject_reply(u32::from(context + 1) << 16, 0);
        assert!(raid.complete().is_none(), "{family:?}");
        assert_eq!(raid.room(), raid.commands());
    }
}

#[test]
fn an_unused_fusion_descriptor_is_the_end_of_the_replies() {
    for (family, _) in FUSION_FAMILIES {
        let (sim, mut raid) = up(family);
        // A slot that still holds all ones, and one of type 15, are unused.
        assert!(raid.complete().is_none());
        sim.lock().inject_reply(0x0000_000f | 3 << 16, 0);
        assert!(raid.complete().is_none(), "{family:?}");
        assert_eq!(raid.in_flight(), 0);
    }
}

#[test]
fn management_and_scsi_commands_in_flight_together_each_get_their_own_answer() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        sim.lock().answer_with(Box::new(|_, _| (status::OK, std::vec![0xd7; 100])));
        let memory = buffer(4096);
        let data = buffer(4096);
        let run = [(memory.phys(), 512)];
        let first = submit_reads(&mut raid, 1, run)[0];
        let dcmd =
            raid.submit_dcmd(7, &[0; 12], &[(data.phys(), 100)], Direction::FromDevice).unwrap();
        let second = submit_reads(&mut raid, 1, run)[0];
        assert_eq!(raid.in_flight(), 3);
        sim.release_all();
        let mut got = Vec::new();
        while let Some((context, answer)) = raid.complete() {
            assert!(answer.good());
            got.push((context, answer.moved));
        }
        assert_eq!(got, [(first, 512), (dcmd, 100), (second, 512)], "{family:?}");
        assert!(crate::tests::bytes_of(&data, 100).iter().all(|&b| b == 0xd7));
    }
}

#[test]
fn a_reset_drops_every_command_in_flight() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        submit_reads(&mut raid, 5, run);
        assert_eq!(raid.in_flight(), 5);
        raid.reset();
        assert_eq!(raid.in_flight(), 0, "{family:?}");
        assert_eq!(raid.room(), raid.commands());
        let inner = sim.lock();
        assert_eq!(inner.state, fw::READY, "{family:?}");
        assert_eq!(inner.held(), 0, "the firmware dropped them too");
        assert!(inner.queue.is_none() && inner.ioc.is_none(), "{family:?}");
        assert!(!inner.interrupts_enabled());
        // The doorbell took the reset: ready, MFI mode and abort.
        assert_eq!(*inner.doorbell_log.last().unwrap(), 0x07, "{family:?}");
        drop(inner);
        sim.release_all();
        assert!(raid.complete().is_none(), "{family:?}");
    }
}

#[test]
fn replies_wait_for_room_in_the_ring_until_the_host_reports() {
    for (family, _) in FUSION_FAMILIES {
        // A ring of 32 with 28 of its slots taken by replies the driver
        // passes over: the last four commands' replies have room, the
        // other four wait.
        let (sim, mut raid) = up_with(family, |inner| inner.commands = 9);
        sim.lock().holding = true;
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let contexts = submit_reads(&mut raid, 8, run);
        {
            let mut inner = sim.lock();
            for _ in 0..28 {
                inner.inject_reply(0, 0);
            }
        }
        sim.release_all();
        assert_eq!(sim.lock().held(), 4, "{family:?}");
        assert_eq!(take(&mut raid, 8), contexts, "{family:?}");
        let inner = sim.lock();
        assert_eq!(inner.held(), 0, "{family:?}");
        assert_eq!(inner.overwrites, 0, "{family:?}");
        assert_eq!(inner.fw_index, 36 % 32);
    }
}
