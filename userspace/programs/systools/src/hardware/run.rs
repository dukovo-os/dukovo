// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The run of `hardware check`: the areas that take no time (the sensors
//! and the battery) first, while the machine is at rest; then each disk
//! on a thread of its own, since a disk's self-test takes most of the
//! time and runs by itself, alongside the memory test and the load; and
//! last the network, whose counts are read at the start and at the end.
//!
//! A check is given as soon as it ends, and the run keeps them in the
//! order of the areas for what it saves.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};
use std::thread;

use super::check::{Area, Check};
use super::plan::Plan;
use super::{battery, cpu, disks, memory, monitors, network};
use crate::sensors::HWMON;

/// Where the checks of a run are gathered, and shown if `live`.
pub(crate) struct Sink {
    checks: Mutex<Vec<Check>>,
    live: bool,
}

impl Sink {
    pub(crate) fn new(live: bool) -> Sink {
        Sink { checks: Mutex::new(Vec::new()), live }
    }

    /// Keeps `check` and shows it. A closed output ends what is shown,
    /// not the run.
    pub(crate) fn emit(&self, check: Check) {
        if self.live {
            let _ = writeln!(io::stdout().lock(), "{}", check.line());
        }
        self.checks.lock().unwrap_or_else(PoisonError::into_inner).push(check);
    }

    /// The checks, in the order of the areas, and in the order they
    /// ended within one.
    pub(crate) fn into_checks(self) -> Vec<Check> {
        let mut checks = self.checks.into_inner().unwrap_or_else(PoisonError::into_inner);
        checks.sort_by_key(|check| check.area);
        checks
    }
}

fn emit_all(sink: &Sink, checks: Vec<Check>) {
    for check in checks {
        sink.emit(check);
    }
}

/// Runs `plan`, giving its checks to `sink`. `stop` ends it early.
pub(crate) fn execute(plan: &Plan, stop: &AtomicBool, sink: &Sink) {
    let network_before = if plan.has(Area::Network) {
        network::counts(Path::new(network::CLASS_NET))
    } else {
        Vec::new()
    };
    if plan.has(Area::Sensors) {
        emit_all(sink, monitors::run(Path::new(HWMON)));
    }
    if plan.has(Area::Battery) {
        emit_all(sink, battery::run(Path::new(battery::POWER_SUPPLY)));
    }
    thread::scope(|scope| {
        if plan.has(Area::Disks) {
            if plan.disks.is_empty() {
                let why = "this machine has no disk to check";
                sink.emit(Check::skipped(Area::Disks, "disks", why.into()));
            }
            for (disk, _) in &plan.disks {
                scope.spawn(|| disks::check(disk, plan.full, stop, &|check| sink.emit(check)));
            }
        }
        if plan.has(Area::Memory) {
            emit_all(sink, memory::run(plan.memory, plan.cpus, stop));
        }
        if plan.has(Area::Cpu) {
            emit_all(sink, cpu::run(plan.cpus, plan.time, stop));
        }
    });
    if plan.has(Area::Network) {
        emit_all(sink, network::run(Path::new(network::CLASS_NET), &network_before));
    }
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::super::plan::{self, Options};
    use super::*;

    #[test]
    fn the_checks_are_kept_in_the_order_of_the_areas() {
        let sink = Sink::new(false);
        sink.emit(Check::passed(Area::Network, "eth0", String::new()));
        sink.emit(Check::passed(Area::Disks, "sda read scan", String::new()));
        sink.emit(Check::passed(Area::Memory, "memory test", String::new()));
        sink.emit(Check::passed(Area::Disks, "sda health", String::new()));
        let names: Vec<String> = sink.into_checks().into_iter().map(|check| check.name).collect();
        assert_eq!(names, ["memory test", "sda read scan", "sda health", "eth0"]);
    }

    #[test]
    fn areas_that_find_nothing_to_check_are_skipped_and_said_so() {
        // A machine that shows none of /kernel's classes, as a build
        // machine does, has nothing for these areas.
        let options = Options {
            areas: vec![Area::Disks, Area::Sensors, Area::Battery, Area::Network],
            ..Options::default()
        };
        let plan = plan::build(&options, 1, 0, Vec::new());
        let sink = Sink::new(false);
        execute(&plan, &AtomicBool::new(false), &sink);
        let checks = sink.into_checks();
        let areas: Vec<Area> = checks.iter().map(|check| check.area).collect();
        assert_eq!(areas, [Area::Disks, Area::Sensors, Area::Battery, Area::Network]);
        assert!(checks.iter().all(|check| check.outcome == Outcome::Skipped));
        assert_eq!(checks[0].detail, "this machine has no disk to check");
    }
}
