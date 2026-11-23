// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bring-up: the family of a device, the way to the ready state, and the
//! queues each interface is given.

use std::vec::Vec;

use crate::frame::{self, status};
use crate::regs::{REGISTERS, Registers, State};
use crate::sim::{Sim, fw, le16, pad2};
use crate::tests::{FUSION_FAMILIES, MFI_FAMILIES, all_families, try_new, up};
use crate::{Direction, Error, Family, MegaRaid};

#[test]
fn every_device_id_has_its_family() {
    let table: [(u16, Family); 24] = [
        (0x0060, Family::Mfi1078),
        (0x007c, Family::Mfi1078),
        (0x0078, Family::MfiGen2),
        (0x0079, Family::MfiGen2),
        (0x002f, Family::MfiGen2),
        (0x0071, Family::MfiSkinny),
        (0x0073, Family::MfiSkinny),
        (0x005b, Family::Thunderbolt),
        (0x005d, Family::Invader),
        (0x005f, Family::Invader),
        (0x00ce, Family::Invader),
        (0x00cf, Family::Invader),
        (0x0052, Family::Invader),
        (0x0053, Family::Invader),
        (0x0014, Family::Ventura),
        (0x0015, Family::Ventura),
        (0x0016, Family::Ventura),
        (0x0017, Family::Ventura),
        (0x001b, Family::Ventura),
        (0x001c, Family::Ventura),
        (0x10e1, Family::Aero),
        (0x10e2, Family::Aero),
        (0x10e5, Family::Aero),
        (0x10e6, Family::Aero),
    ];
    for (id, family) in table {
        assert_eq!(Family::of(id), Some(family), "device {id:#06x}");
    }
}

#[test]
fn devices_that_are_not_ours_have_no_family() {
    // The Aero parts whose secure boot did not pass, and strangers.
    for id in [0x10e0, 0x10e3, 0x10e4, 0x10e7, 0x0000, 0x1234, 0x0061, 0xffff] {
        assert_eq!(Family::of(id), None, "device {id:#06x}");
    }
}

#[test]
fn families_know_their_interface_and_layout() {
    for (family, _) in MFI_FAMILIES {
        assert!(!family.fusion() && !family.ventura(), "{family:?}");
    }
    assert!(Family::Thunderbolt.fusion() && !Family::Thunderbolt.ventura());
    assert!(Family::Invader.fusion() && !Family::Invader.ventura());
    assert!(Family::Ventura.fusion() && Family::Ventura.ventura());
    assert!(Family::Aero.fusion() && Family::Aero.ventura());
    assert_eq!(Family::Mfi1078.name(), "SAS1078");
    assert_eq!(Family::Aero.name(), "SAS3908");
}

#[test]
fn a_device_that_is_not_ours_is_refused() {
    let sim = Sim::new(Family::Mfi1078);
    let mut alloc = |pages: usize| sim.alloc(pages);
    let mut wait = sim.waiter();
    let result = MegaRaid::new(sim.regs(), 0x1234, false, &mut alloc, &mut wait);
    assert_eq!(result.err(), Some(Error::Device("not a MegaRAID controller")));
}

#[test]
fn a_firmware_that_is_ready_is_given_its_queues_without_help() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        let raid = try_new(&sim, false).unwrap();
        assert!(sim.lock().doorbell_log.is_empty(), "{family:?}");
        assert_eq!(sim.lock().state, fw::OPERATIONAL, "{family:?}");
        assert_eq!(raid.status().state(), State::Operational, "{family:?}");
        assert_eq!(raid.family(), family);
    }
}

#[test]
fn a_handshake_is_answered_with_the_bits_of_the_family() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        sim.lock().set_state(fw::WAIT_HANDSHAKE);
        try_new(&sim, false).unwrap();
        let expected = match family {
            Family::Mfi1078 | Family::MfiGen2 => 0x08,
            _ => 0x18,
        };
        assert_eq!(sim.lock().doorbell_log, [expected], "{family:?}");
    }
}

#[test]
fn a_handshake_with_too_few_bits_is_not_taken_by_the_newer_parts() {
    // The simulated firmware wants both bits on the newer parts: a driver
    // that rang one would wait out the whole 180 seconds.
    let sim = Sim::new(Family::Aero);
    sim.lock().set_state(fw::WAIT_HANDSHAKE);
    let regs = Registers::new(sim.regs(), Family::Aero).unwrap();
    regs.ring(0x08);
    assert_eq!(sim.lock().state, fw::WAIT_HANDSHAKE);
    regs.ring(0x18);
    assert_eq!(sim.lock().state, fw::READY);
}

#[test]
fn a_boot_message_is_answered() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        sim.lock().set_state(fw::BOOT_MESSAGE);
        try_new(&sim, false).unwrap();
        assert_eq!(sim.lock().doorbell_log, [0x10], "{family:?}");
    }
}

#[test]
fn a_firmware_that_takes_its_time_is_waited_for() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        {
            let mut inner = sim.lock();
            inner.set_state(fw::WAIT_HANDSHAKE);
            inner.after_handshake = std::vec![(100, fw::SCANNING), (250, fw::READY)];
        }
        try_new(&sim, false).unwrap();
        let now = sim.now();
        assert!((350..=370).contains(&now), "{family:?}: {now} ms");
    }
}

#[test]
fn an_operational_firmware_is_sent_back_to_ready() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        {
            let mut inner = sim.lock();
            inner.set_state(fw::OPERATIONAL);
            inner.reset_ms = 300;
            inner.after_reset = std::vec![(40, fw::READY)];
        }
        try_new(&sim, false).unwrap();
        let inner = sim.lock();
        assert_eq!(inner.doorbell_log, [0x07], "{family:?}");
        // It is operational again, with the driver's queues.
        assert_eq!(inner.state, fw::OPERATIONAL, "{family:?}");
        if family.fusion() {
            // The driver waited for the firmware to take the reset.
            assert_eq!(inner.now, 300, "{family:?}");
        }
    }
}

#[test]
fn a_fault_in_the_firmware_is_an_error_with_its_code() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        sim.lock().fault(0x1234);
        assert_eq!(try_new(&sim, false).err(), Some(Error::Fault(0x1234)), "{family:?}");
    }
}

#[test]
fn a_fault_while_waiting_is_found() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        {
            let mut inner = sim.lock();
            inner.set_state(fw::WAIT_HANDSHAKE);
            inner.fault_code = 0x77;
            inner.after_handshake = std::vec![(30, fw::FAULT)];
        }
        assert_eq!(try_new(&sim, false).err(), Some(Error::Fault(0x77)), "{family:?}");
    }
}

#[test]
fn a_firmware_stuck_in_a_state_is_not_ready_after_180_seconds() {
    for (family, _) in all_families() {
        for state in [fw::SCANNING, fw::UNDEFINED] {
            let sim = Sim::new(family);
            sim.lock().set_state(state);
            let word = sim.lock().status_word();
            assert_eq!(try_new(&sim, false).err(), Some(Error::NotReady(word)), "{family:?}");
            assert_eq!(sim.now(), 180_000, "{family:?}");
        }
    }
}

#[test]
fn a_handshake_nobody_takes_up_is_not_ready_after_180_seconds() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        {
            let mut inner = sim.lock();
            inner.set_state(fw::WAIT_HANDSHAKE);
            inner.after_handshake = Vec::new();
        }
        let word = sim.lock().status_word();
        assert_eq!(try_new(&sim, false).err(), Some(Error::NotReady(word)), "{family:?}");
        assert_eq!(sim.now(), 180_000);
    }
}

#[test]
fn a_state_the_driver_does_not_know_is_not_ready_at_once() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        sim.lock().set_state(0x3);
        let word = sim.lock().status_word();
        assert_eq!(try_new(&sim, false).err(), Some(Error::NotReady(word)), "{family:?}");
        assert_eq!(sim.now(), 0);
    }
}

#[test]
fn registers_too_short_are_refused() {
    for (family, id) in all_families() {
        let sim = Sim::new(family);
        let mut alloc = |pages: usize| sim.alloc(pages);
        let mut wait = sim.waiter();
        let short = sim.regs_of(REGISTERS - 4);
        let result = MegaRaid::new(short, id, false, &mut alloc, &mut wait);
        assert_eq!(result.err(), Some(Error::Registers), "{family:?}");
        // Exactly enough is enough.
        let enough = sim.regs_of(REGISTERS);
        assert!(MegaRaid::new(enough, id, false, &mut alloc, &mut wait).is_ok());
    }
}

#[test]
fn a_firmware_that_takes_no_commands_is_refused() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        // Fusion firmware says one more than it takes.
        sim.lock().commands = 1;
        let result = try_new(&sim, false);
        assert_eq!(
            result.err(),
            Some(Error::Device("its firmware takes no commands")),
            "{family:?}"
        );
    }
}

#[test]
fn memory_that_cannot_be_had_is_an_error() {
    for (family, id) in all_families() {
        let sim = Sim::new(family);
        let mut wait = sim.waiter();
        let mut none = |_: usize| None;
        let result = MegaRaid::new(sim.regs(), id, false, &mut none, &mut wait);
        assert_eq!(result.err(), Some(Error::OutOfMemory), "{family:?}");
        // Any one of the allocations may be the one that fails.
        for fail_at in 0..4 {
            let mut count = 0;
            let mut some_then_none = |pages: usize| {
                count += 1;
                if count > fail_at { None } else { sim.alloc(pages) }
            };
            let result = MegaRaid::new(sim.regs(), id, false, &mut some_then_none, &mut wait);
            assert_eq!(result.err(), Some(Error::OutOfMemory), "{family:?} at {fail_at}");
        }
    }
}

// ---- MFI

#[test]
fn the_mfi_queue_is_given_with_the_right_sizes() {
    // The firmware's command count, the reply queue's entries, and the
    // commands the driver keeps in flight.
    let table = [(64u16, 64u32, 63usize), (300, 300, 256), (1024, 1024, 256), (5000, 4096, 256)];
    for (family, _) in MFI_FAMILIES {
        for (fw_commands, entries, commands) in table {
            let sim = Sim::new(family);
            sim.lock().commands = fw_commands;
            let raid = try_new(&sim, false).unwrap();
            let inner = sim.lock();
            let queue = inner.queue.as_ref().expect("the queue was given");
            assert_eq!(queue.entries, entries, "{family:?} {fw_commands}");
            assert_eq!(raid.commands(), commands, "{family:?} {fw_commands}");
            assert_eq!(queue.flags, 0);
            assert!(inner.holds(queue.queue));
            assert!(inner.holds(queue.queue + u64::from(entries) * 4 - 1));
            assert!(inner.holds(queue.producer) && inner.holds(queue.consumer));
            assert_ne!(queue.producer, queue.consumer);
            // The INIT frame was answered well, and the firmware runs.
            assert_eq!(inner.init_frame[0], frame::cmd::INIT);
            assert_eq!(inner.state, fw::OPERATIONAL);
        }
    }
}

#[test]
fn the_mfi_interrupts_stay_off_until_asked() {
    for (family, _) in MFI_FAMILIES {
        let sim = Sim::new(family);
        let raid = try_new(&sim, false).unwrap();
        assert!(!sim.lock().interrupts_enabled(), "{family:?}");
        raid.enable_interrupts();
        let expected = match family {
            Family::Mfi1078 => !0x8000_0000u32,
            Family::MfiGen2 => !0x5,
            _ => !0x1,
        };
        assert_eq!(sim.lock().mask, expected, "{family:?}");
    }
}

#[test]
fn an_mfi_init_the_firmware_refuses_is_an_error() {
    for (family, _) in MFI_FAMILIES {
        let sim = Sim::new(family);
        sim.lock().init_status = Some(status::INVALID_PARAMETER);
        assert_eq!(
            try_new(&sim, false).err(),
            Some(Error::Device("its firmware refused its reply queue")),
            "{family:?}"
        );
    }
}

#[test]
fn an_mfi_init_nobody_answers_times_out() {
    for (family, _) in MFI_FAMILIES {
        let sim = Sim::new(family);
        sim.lock().ignore_init = true;
        assert_eq!(
            try_new(&sim, false).err(),
            Some(Error::Device("its firmware did not take its reply queue")),
            "{family:?}"
        );
        assert_eq!(sim.now(), 60_000, "{family:?}");
    }
}

#[test]
fn the_mfi_init_frame_is_posted_in_the_form_the_port_takes() {
    for (family, _) in MFI_FAMILIES {
        let sim = Sim::new(family);
        try_new(&sim, false).unwrap();
        let inner = sim.lock();
        assert_eq!(inner.unresolved, 0, "{family:?}");
        let (word, high) = inner.posts[0];
        // Bit 0 set, the extra frames in bits 4:1 (the INIT is one 64-byte
        // frame), and the rest of the low five bits clear.
        assert_eq!(word & 0x1f, 1, "{family:?}");
        // Only the skinny parts post a high half.
        if family == Family::MfiSkinny {
            assert!(inner.memory_with_high(high), "{family:?}");
        } else {
            assert_eq!(high, 0, "{family:?}");
        }
    }
}

#[test]
fn mfi_segments_follow_the_entry_size_and_the_firmware_limit() {
    for (family, _) in MFI_FAMILIES {
        // 512-byte frames: 12-byte entries fit 38, 16-byte ones 29.
        let (_, raid) = up(family);
        let expected = if family == Family::MfiSkinny { 29 } else { 32 };
        assert_eq!(raid.max_segments(), expected, "{family:?}");
        let sim = Sim::new(family);
        sim.lock().max_sge = 5;
        let raid = try_new(&sim, false).unwrap();
        assert_eq!(raid.max_segments(), 5, "{family:?}");
    }
}

#[test]
fn the_mfi_controller_never_syncs_a_volume_cache_itself() {
    for (family, _) in MFI_FAMILIES {
        let (_, raid) = up(family);
        assert!(!raid.volume_sync_cache(), "{family:?}");
    }
}

// ---- fusion

#[test]
fn the_ioc_init_carries_what_the_driver_is() {
    for (family, _) in FUSION_FAMILIES {
        for msix in [false, true] {
            let sim = Sim::new(family);
            try_new(&sim, msix).unwrap();
            let inner = sim.lock();
            let ioc = inner.ioc.as_ref().expect("the IOC INIT was taken");
            let raw = &ioc.raw;
            assert_eq!(raw[0], 4, "who initialised it: the host driver");
            assert_eq!(raw[3], 0x02, "the function");
            assert_eq!(le16(raw, 0x0c), 0x0200, "message version");
            assert_eq!(le16(raw, 0x0e), 0x1000, "header version");
            assert_eq!(raw[0x16], 12, "host page size, log 2");
            assert_eq!(raw[0x17], u8::from(msix), "MSI-X vectors");
            assert_eq!(le16(raw, 0x1a), 64, "request frame size, in words");
            // 64 commands: 2 * ceil16(65).
            assert_eq!(ioc.depth, 160);
            assert!(inner.memory_with_high(ioc.sense_high), "{family:?}");
            assert!(inner.holds(ioc.requests) && inner.holds(ioc.replies));
            assert_eq!(ioc.requests % 256, 0);
            assert_eq!(ioc.replies % 8, 0);
            // The capabilities: 255 volumes, and the second reply index
            // register set on every part but the first.
            let expected = if family == Family::Thunderbolt { 1 << 3 } else { 1 << 3 | 1 << 1 };
            assert_eq!(ioc.capabilities, expected, "{family:?}");
            // The INIT frame went as an MFA descriptor.
            let (low, _) = inner.descriptors[0];
            assert_eq!(low & 0xff, 0x02, "{family:?}");
            assert_eq!(inner.init_frame[0], frame::cmd::INIT);
        }
    }
}

#[test]
fn the_reply_ring_is_twice_the_commands_in_sixteens() {
    // The firmware's command count (one more than it takes), the commands
    // the driver keeps, and the ring's depth.
    let table = [
        (2u16, 1usize, 32u32),
        (16, 15, 32),
        (17, 16, 64),
        (18, 17, 64),
        (65, 64, 160),
        (129, 128, 288),
        (257, 256, 544),
        (300, 256, 544),
    ];
    for (family, _) in FUSION_FAMILIES {
        for (word, commands, depth) in table {
            let sim = Sim::new(family);
            sim.lock().commands = word;
            let raid = try_new(&sim, false).unwrap();
            assert_eq!(raid.commands(), commands, "{family:?} {word}");
            assert_eq!(sim.lock().ioc.as_ref().unwrap().depth, depth, "{family:?} {word}");
        }
    }
}

#[test]
fn an_ioc_init_the_firmware_refuses_is_an_error() {
    for (family, _) in FUSION_FAMILIES {
        let sim = Sim::new(family);
        sim.lock().init_status = Some(status::INVALID_CMD);
        assert_eq!(
            try_new(&sim, false).err(),
            Some(Error::Device("its firmware refused its queues")),
            "{family:?}"
        );
    }
}

#[test]
fn an_ioc_init_nobody_answers_times_out() {
    for (family, _) in FUSION_FAMILIES {
        let sim = Sim::new(family);
        sim.lock().ignore_init = true;
        assert_eq!(
            try_new(&sim, false).err(),
            Some(Error::Device("its firmware did not take its queues")),
            "{family:?}"
        );
        assert_eq!(sim.now(), 180_000, "{family:?}");
    }
}

#[test]
fn fusion_segments_follow_the_chain_frame_size() {
    // Scratch pad 2's chain size and what the driver makes of it: eight
    // entries fit the request frame, seven and a chain element when more.
    let table = [(0u32, 8usize), (128, 15), (256, 23), (384, 31), (512, 32), (1024, 32)];
    for (family, _) in FUSION_FAMILIES {
        for (chain, expected) in table {
            let sim = Sim::new(family);
            sim.lock().scratch2 = pad2(family, chain, 1, false);
            let raid = try_new(&sim, false).unwrap();
            assert_eq!(raid.max_segments(), expected, "{family:?} chain {chain}");
        }
    }
}

#[test]
fn a_chain_frame_size_may_be_in_units_of_512_bytes() {
    for (family, _) in FUSION_FAMILIES {
        // One unit of 512 bytes is the whole frame; of 128, a quarter.
        for (word, expected) in [(0x40_0000 | 1 << 5, 32usize), (1 << 5, 15), (0x40_0000, 8)] {
            let sim = Sim::new(family);
            sim.lock().scratch2 = word;
            let raid = try_new(&sim, false).unwrap();
            assert_eq!(raid.max_segments(), expected, "{family:?} {word:#x}");
        }
    }
}

#[test]
fn the_sync_cache_bit_says_whether_the_firmware_takes_it() {
    for (family, _) in FUSION_FAMILIES {
        for sync in [false, true] {
            let sim = Sim::new(family);
            sim.lock().scratch2 = pad2(family, 512, 1, sync);
            let raid = try_new(&sim, false).unwrap();
            assert_eq!(raid.volume_sync_cache(), sync, "{family:?}");
        }
    }
}

#[test]
fn the_reply_index_register_depends_on_the_vectors_and_msix() {
    use Family::{Aero, Invader, Thunderbolt, Ventura};
    let table = [
        (Thunderbolt, 16, true, 0x6c),
        (Invader, 8, true, 0x6c),
        (Invader, 9, true, 0x30c),
        (Invader, 9, false, 0x6c),
        (Ventura, 16, true, 0x6c),
        (Ventura, 17, true, 0x30c),
        (Ventura, 17, false, 0x6c),
        (Aero, 24, true, 0x30c),
        (Aero, 24, false, 0x6c),
        (Aero, 1, true, 0x6c),
    ];
    for (family, vectors, msix, register) in table {
        let sim = Sim::new(family);
        sim.lock().scratch2 = pad2(family, 512, vectors, false);
        let mut raid = try_new(&sim, msix).unwrap();
        raid.enable_interrupts();
        let mut wait = sim.waiter();
        raid.run_dcmd(0x0101_0000, &[0; 12], &[], Direction::None, &mut wait, 100).unwrap();
        // The reply is reported when the next read finds nothing.
        assert!(raid.complete().is_none());
        let writes = sim.lock().reply_index_writes.clone();
        assert_eq!(writes, [(register, 1)], "{family:?} {vectors} vectors, msix {msix}");
    }
}

#[test]
fn the_fusion_interrupts_are_masked_until_asked() {
    for (family, _) in FUSION_FAMILIES {
        let sim = Sim::new(family);
        let raid = try_new(&sim, false).unwrap();
        assert!(!sim.lock().interrupts_enabled(), "{family:?}");
        raid.enable_interrupts();
        assert_eq!(sim.lock().mask, !0x9, "{family:?}");
    }
}

// ---- acknowledging interrupts

#[test]
fn no_interrupt_is_acknowledged_when_none_was_raised() {
    for (family, _) in all_families() {
        let (_, raid) = up(family);
        assert!(!raid.acknowledge(), "{family:?}");
    }
}

#[test]
fn an_interrupt_a_completion_raised_is_acknowledged_once() {
    for (family, _) in all_families() {
        let (_, mut raid) = up(family);
        raid.submit_dcmd(0x0101_0000, &[0; 12], &[], Direction::None).unwrap();
        assert!(raid.acknowledge(), "{family:?}");
        assert!(!raid.acknowledge(), "{family:?}");
        assert!(raid.complete().is_some());
    }
}

#[test]
fn interrupt_bits_that_are_not_the_drivers_are_left_alone() {
    let table = [
        (Family::Mfi1078, 0x0000_0001u32),
        (Family::MfiGen2, 0x2),
        (Family::MfiSkinny, 0x2),
        (Family::Thunderbolt, 0x2),
        (Family::Invader, 0x4),
        (Family::Ventura, 0x2),
        (Family::Aero, 0x6),
    ];
    for (family, foreign) in table {
        let (sim, raid) = up(family);
        sim.lock().raise_bits(foreign);
        assert!(!raid.acknowledge(), "{family:?}");
        assert_eq!(sim.lock().raised(), foreign, "{family:?}");
    }
}

#[test]
fn each_family_acknowledges_the_bits_that_are_its_own() {
    // The SAS1078's interrupt is bit 31 of the status; the SAS2108's, bit
    // 0 or 2; the skinny parts' and the fusion parts', bit 0 (and 3).
    let table = [
        (Family::Mfi1078, 0x8000_0000u32),
        (Family::MfiGen2, 0x1),
        (Family::MfiGen2, 0x4),
        (Family::MfiSkinny, 0x1),
        (Family::Thunderbolt, 0x1),
        (Family::Thunderbolt, 0x8),
        (Family::Aero, 0x8),
    ];
    for (family, bits) in table {
        let (sim, raid) = up(family);
        sim.lock().raise_bits(bits);
        assert!(raid.acknowledge(), "{family:?} {bits:#x}");
        assert_eq!(sim.lock().raised() & bits, 0, "{family:?} {bits:#x}");
    }
}

#[test]
fn the_firmware_status_is_read_from_the_register_of_the_family() {
    for (family, _) in all_families() {
        let (sim, raid) = up(family);
        assert_eq!(raid.status().0, sim.lock().status_word(), "{family:?}");
        assert_eq!(raid.status().state(), State::Operational);
        assert_eq!(raid.status().max_commands(), sim.lock().commands);
    }
}

#[test]
fn the_status_word_says_the_limits_and_msix() {
    for (family, _) in all_families() {
        let sim = Sim::new(family);
        {
            let mut inner = sim.lock();
            inner.msix = true;
            inner.max_sge = 0x21;
        }
        let raid = try_new(&sim, false).unwrap();
        assert!(raid.status().msix(), "{family:?}");
        assert_eq!(raid.status().max_sge(), 0x21, "{family:?}");
    }
}
