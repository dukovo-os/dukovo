// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The run of `hardware check`: the areas that take no time (the sensors,
//! the battery and the RAID controllers) first, while the machine is at rest; then each disk
//! on a thread of its own, since a disk's self-test takes most of the
//! time and runs by itself, alongside the memory test and the load; and
//! last the network, whose counts are read at the start and at the end.
//! The hardware errors, which take no time either, are read with the
//! sensors and the battery.
//!
//! A check is given as soon as it ends, and the run keeps them in the
//! order of the areas for what it saves. When the person asks for the
//! steps, each check that ends also says how far the run is, as `step 4
//! of 11: disks sda self-test passed`.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;

use super::check::{Area, Check};
use super::plan::Plan;
use super::{battery, cpu, devices, disks, errors, memory, monitors, network, raid, ras};
use crate::raid::ROOT as RAID;
use crate::sensors::HWMON;

/// Where the lines that say how far a run is go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Steps {
    /// Nowhere: they are not asked for.
    Off,
    /// Standard output, with the lines of the checks.
    Output,
    /// Standard error, which leaves standard output to the JSON.
    Errors,
}

/// Where the checks of a run are gathered, and shown if `live`.
pub(crate) struct Sink {
    checks: Mutex<Vec<Check>>,
    live: bool,
    steps: Steps,
    /// How many checks the run is to give, which its steps count to.
    planned: AtomicUsize,
}

impl Sink {
    pub(crate) fn new(live: bool, steps: Steps) -> Sink {
        Sink { checks: Mutex::new(Vec::new()), live, steps, planned: AtomicUsize::new(0) }
    }

    /// Says how many checks the run is to give.
    fn expect(&self, total: usize) {
        self.planned.store(total, Ordering::Relaxed);
    }

    /// The line that says check `check` is the `ended`th of the run to
    /// end. A run that gives more than it counted on says the number it is
    /// at, never one past the total.
    fn step_line(&self, ended: usize, check: &Check) -> String {
        let total = self.planned.load(Ordering::Relaxed).max(ended);
        format!(
            "step {ended} of {total}: {} {} {}",
            check.area.name(),
            check.name,
            check.outcome.word()
        )
    }

    /// Keeps `check` and shows it, with its step if they are asked for. A
    /// closed output ends what is shown, not the run.
    pub(crate) fn emit(&self, check: Check) {
        // The lock is held while the lines are written, so that the steps
        // are shown in the order they are counted.
        let mut checks = self.checks.lock().unwrap_or_else(PoisonError::into_inner);
        let step = self.step_line(checks.len() + 1, &check);
        let _ = match self.steps {
            Steps::Off => Ok(()),
            Steps::Output => writeln!(io::stdout().lock(), "{step}"),
            Steps::Errors => writeln!(io::stderr().lock(), "{step}"),
        };
        if self.live {
            let _ = writeln!(io::stdout().lock(), "{}", check.line());
        }
        checks.push(check);
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

/// How many checks the areas of `plan` that take time give, with `cards`
/// network cards that have a wire. A disk gives each of its checks, and
/// an area with nothing to check gives the one that says so.
fn timed_checks(plan: &Plan, cards: usize) -> usize {
    let mut count = 0;
    if plan.has(Area::Disks) {
        count += if plan.disks.is_empty() {
            1
        } else {
            plan.disks.iter().map(|(disk, _)| disks::checks_of(disk)).sum()
        };
    }
    if plan.has(Area::Memory) {
        count += memory::CHECKS;
    }
    if plan.has(Area::Cpu) {
        count += cpu::CHECKS;
    }
    if plan.has(Area::Network) {
        count += cards.max(1);
    }
    count
}

/// Runs `plan`, giving its checks to `sink`. `stop` ends it early.
pub(crate) fn execute(plan: &Plan, stop: &AtomicBool, sink: &Sink) {
    let network_before = if plan.has(Area::Network) {
        network::counts(Path::new(network::CLASS_NET))
    } else {
        Vec::new()
    };
    // The sensors and the battery take no time, so the run knows what
    // they give before it shows its first line, and so how many checks it
    // is to give in all.
    let sensors =
        if plan.has(Area::Sensors) { monitors::run(Path::new(HWMON)) } else { Vec::new() };
    let battery = if plan.has(Area::Battery) {
        battery::run(Path::new(battery::POWER_SUPPLY))
    } else {
        Vec::new()
    };
    let hardware_errors = if plan.has(Area::Errors) {
        errors::run(Path::new(ras::DIR), &devices::Sources::system())
    } else {
        Vec::new()
    };
    let raid = if plan.has(Area::Raid) { raid::run(Path::new(RAID)) } else { Vec::new() };
    sink.expect(
        sensors.len()
            + battery.len()
            + hardware_errors.len()
            + raid.len()
            + timed_checks(plan, network_before.len()),
    );
    emit_all(sink, sensors);
    emit_all(sink, battery);
    emit_all(sink, hardware_errors);
    emit_all(sink, raid);
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
    use super::super::check::{Outcome, Problem};
    use super::super::disks::Disk;
    use super::super::plan::{self, Options};
    use super::*;

    #[test]
    fn the_checks_are_kept_in_the_order_of_the_areas() {
        let sink = Sink::new(false, Steps::Off);
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
            areas: vec![Area::Disks, Area::Sensors, Area::Battery, Area::Network, Area::Errors],
            ..Options::default()
        };
        let plan = plan::build(&options, 1, 0, Vec::new());
        let sink = Sink::new(false, Steps::Off);
        execute(&plan, &AtomicBool::new(false), &sink);
        // Each area gave the one check the run counted on.
        assert_eq!(sink.planned.load(Ordering::Relaxed), 5);
        let checks = sink.into_checks();
        let areas: Vec<Area> = checks.iter().map(|check| check.area).collect();
        assert_eq!(areas, [Area::Disks, Area::Sensors, Area::Battery, Area::Network, Area::Errors]);
        assert!(checks.iter().all(|check| check.outcome == Outcome::Skipped));
        assert_eq!(checks[0].detail, "this machine has no disk to check");
    }

    #[test]
    fn a_step_says_which_check_ended_of_how_many_with_its_area_name_and_result() {
        let sink = Sink::new(false, Steps::Errors);
        sink.expect(11);
        let check = Check::passed(Area::Disks, "sda self-test", String::new());
        assert_eq!(sink.step_line(4, &check), "step 4 of 11: disks sda self-test passed");
        let warned = Check::warning(Area::Cpu, "cpu heat", Problem::CpuHot, String::new());
        assert_eq!(sink.step_line(11, &warned), "step 11 of 11: cpu cpu heat warning");
        let failed = Check::failed(Area::Memory, "memory test", Problem::MemoryTest, String::new());
        assert_eq!(sink.step_line(1, &failed), "step 1 of 11: memory memory test FAILED");
        let skipped = Check::skipped(Area::Battery, "battery", String::new());
        assert_eq!(sink.step_line(2, &skipped), "step 2 of 11: battery battery skipped");
        // A run that gives more than it counted on is never past its end,
        // and one that did not count says the number it is at.
        assert_eq!(sink.step_line(12, &check), "step 12 of 12: disks sda self-test passed");
        let unplanned = Sink::new(false, Steps::Off);
        assert_eq!(unplanned.step_line(1, &check), "step 1 of 1: disks sda self-test passed");
    }

    #[test]
    fn what_the_areas_that_take_time_give_is_counted_before_they_run() {
        let disk = |name: &str| {
            let disk = Disk {
                name: name.into(),
                model: String::new(),
                bytes: 1 << 30,
                sector: 512,
                rotational: false,
            };
            (disk, 2)
        };
        let all = plan::build(&Options::default(), 4, 8 << 20, vec![disk("sda"), disk("sdb")]);
        // Two disks of three checks, two of the memory, three of the
        // processors and the two cards.
        assert_eq!(timed_checks(&all, 2), 6 + 2 + 3 + 2);
        // Without a disk the area gives one check that says so, and
        // without a card with a wire, as many.
        let bare = plan::build(&Options::default(), 4, 8 << 20, Vec::new());
        assert_eq!(timed_checks(&bare, 0), 1 + 2 + 3 + 1);
        let options = Options { areas: vec![Area::Sensors, Area::Battery], ..Options::default() };
        let quick = plan::build(&options, 4, 8 << 20, Vec::new());
        assert_eq!(timed_checks(&quick, 5), 0);
    }
}
