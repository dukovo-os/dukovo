// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A firmware that faults or stops answering while the driver runs.

use crate::regs::State;
use crate::tests::{all_families, buffer, read10, up};
use crate::{Direction, Error, Target};

#[test]
fn a_fault_while_a_management_command_runs_is_an_error_with_its_code() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        {
            let mut inner = sim.lock();
            inner.holding = true;
            inner.fault_at = Some((inner.now + 5, 0x1bad));
        }
        let mut wait = sim.waiter();
        let result = raid.run_dcmd(1, &[0; 12], &[], Direction::None, &mut wait, 1000);
        assert_eq!(result.err(), Some(Error::Fault(0x1bad)), "{family:?}");
        // It took the 5 milliseconds to find, not the whole timeout.
        assert_eq!(sim.now(), 5, "{family:?}");
    }
}

#[test]
fn a_firmware_that_faulted_before_the_command_is_found_at_once() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().fault(0x0042);
        let mut wait = sim.waiter();
        let result = raid.run_dcmd(1, &[0; 12], &[], Direction::None, &mut wait, 1000);
        assert_eq!(result.err(), Some(Error::Fault(0x42)), "{family:?}");
        assert_eq!(sim.now(), 0);
        assert_eq!(raid.status().state(), State::Fault);
        assert_eq!(raid.status().fault_code(), 0x42);
    }
}

#[test]
fn a_management_command_that_never_ends_times_out() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().holding = true;
        let mut wait = sim.waiter();
        let result = raid.run_dcmd(1, &[0; 12], &[], Direction::None, &mut wait, 100);
        assert_eq!(
            result.err(),
            Some(Error::Device("a management command did not end")),
            "{family:?}"
        );
        assert_eq!(sim.now(), 100, "{family:?}");
        // The command is still in flight: the caller decides what to do.
        assert_eq!(raid.in_flight(), 1);
    }
}

#[test]
fn commands_sent_to_a_faulted_firmware_do_not_end() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().fault(0x0042);
        let memory = buffer(4096);
        let run = [(memory.phys(), 512)];
        let context =
            raid.submit(Target::Disk(5), 0, &read10(0, 1), &run, Direction::FromDevice).unwrap();
        assert_eq!(context, 0);
        assert!(raid.complete().is_none(), "{family:?}");
        assert_eq!(raid.in_flight(), 1);
        assert!(sim.lock().seen.is_empty(), "the firmware never saw it");
    }
}

#[test]
fn a_management_command_that_ends_before_the_fault_is_still_answered() {
    for (family, _) in all_families() {
        let (sim, mut raid) = up(family);
        sim.lock().fault_at = Some((50, 0x1bad));
        let mut wait = sim.waiter();
        let answer = raid.run_dcmd(1, &[0; 12], &[], Direction::None, &mut wait, 1000).unwrap();
        assert!(answer.good(), "{family:?}");
    }
}
