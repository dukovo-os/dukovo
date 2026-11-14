// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a run of `hardware check` is going to do, from what the person
//! asked and what the machine has: the areas, the disks, the CPUs, how
//! much memory to test and how long the processors run, and from them
//! how long the run takes, which a full check says before it begins.

use std::io;
use std::time::Duration;

use super::check::Area;
use super::disks::{self, Disk};
use super::memory;
use crate::lscpu;
use crate::proc;
use crate::util::{self, Fail, fail};

/// How long the processors run in a quick check and a full one, seconds.
const QUICK_SECONDS: u64 = 60;
const FULL_SECONDS: u64 = 600;

/// What the person asked for on the command line.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Options {
    /// The areas, all of them when none is named.
    pub(crate) areas: Vec<Area>,
    pub(crate) full: bool,
    pub(crate) disk: Option<String>,
    /// How long the processors run, in seconds.
    pub(crate) time: Option<u64>,
    pub(crate) yes: bool,
    pub(crate) json: bool,
}

/// What a run does.
#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) areas: Vec<Area>,
    pub(crate) full: bool,
    /// The disks to check, each with the minutes its self-test takes.
    pub(crate) disks: Vec<(Disk, u32)>,
    pub(crate) cpus: usize,
    /// How much memory to test, in bytes.
    pub(crate) memory: u64,
    /// How long the processors run.
    pub(crate) time: Duration,
}

impl Plan {
    pub(crate) fn has(&self, area: Area) -> bool {
        self.areas.contains(&area)
    }

    /// How long the run takes: the memory test and the load, one after
    /// the other, while the disks run theirs at the same time.
    pub(crate) fn estimate(&self, disks: &[Duration]) -> Duration {
        let mut here = Duration::ZERO;
        if self.has(Area::Memory) {
            here += memory_time(self.memory);
        }
        if self.has(Area::Cpu) {
            here += self.time;
        }
        let disk = disks.iter().copied().max().unwrap_or(Duration::ZERO);
        here.max(disk) + Duration::from_secs(2)
    }

    /// How long each disk takes.
    fn disk_times(&self) -> Vec<Duration> {
        self.disks
            .iter()
            .map(|(disk, minutes)| disks::estimate(disk, *minutes, self.full))
            .collect()
    }

    /// What the run says first.
    pub(crate) fn heading(&self) -> String {
        let kind = if self.full { "full" } else { "quick" };
        let time = duration_text(self.estimate(&self.disk_times()));
        format!("Checking the hardware: a {kind} check, {time}. Ctrl-C stops it.")
    }

    /// What a full check asks before it begins.
    pub(crate) fn question(&self) -> String {
        let time = duration_text(self.estimate(&self.disk_times()));
        format!(
            "A full check takes {time}. The processors run at full load, the disks run their long \
             self-tests and are read from end to end, and most of the free memory is tested, so \
             the machine is slow meanwhile. Start it? [y/N] "
        )
    }
}

/// How long testing `bytes` of memory takes: each of the sweeps writes
/// the memory and reads it back twice, at about 2 GiB a second.
fn memory_time(bytes: u64) -> Duration {
    Duration::from_secs(bytes * memory::SWEEPS * 3 / (2 << 30) + 1)
}

/// `time` as a person says it: `about 3 minutes`, `about 5 hours 10
/// minutes`.
pub(crate) fn duration_text(time: Duration) -> String {
    let minutes = (time.as_secs() + 30) / 60;
    match minutes {
        0 | 1 => "about a minute".to_string(),
        2..=119 => format!("about {minutes} minutes"),
        _ => {
            let (hours, rest) = (minutes / 60, minutes % 60);
            let unit = if hours == 1 { "hour" } else { "hours" };
            if rest == 0 {
                format!("about {hours} {unit}")
            } else {
                format!("about {hours} {unit} {rest} minutes")
            }
        }
    }
}

/// The plan for `options` on a machine with `cpus` CPUs, `available_kib`
/// of memory to test a share of, and `disks`.
pub(crate) fn build(
    options: &Options,
    cpus: usize,
    available_kib: u64,
    disks: Vec<(Disk, u32)>,
) -> Plan {
    let areas = if options.areas.is_empty() { Area::ALL.to_vec() } else { options.areas.clone() };
    let default = if options.full { FULL_SECONDS } else { QUICK_SECONDS };
    Plan {
        areas,
        full: options.full,
        disks,
        cpus,
        memory: memory::tested_bytes(available_kib, options.full),
        time: Duration::from_secs(options.time.unwrap_or(default)),
    }
}

/// The plan for `options` on this machine: it reads what each area it
/// names needs.
pub(crate) fn gather(options: &Options) -> Result<Plan, Fail> {
    let wants = |area: Area| options.areas.is_empty() || options.areas.contains(&area);
    let cannot = |what: &str, error: io::Error| {
        fail(format!("cannot read {what}: {}", util::strip_os_error(&error)))
    };
    let cpus = if wants(Area::Cpu) || wants(Area::Memory) { lscpu::cpus()? as usize } else { 0 };
    let available = if wants(Area::Memory) {
        proc::memory().map_err(|error| cannot("the memory the system has", error))?.available()
    } else {
        0
    };
    let mut disks = Vec::new();
    if wants(Area::Disks) {
        for disk in disks::list(options.disk.as_deref())? {
            let minutes = disks::test_minutes(&disk, options.full);
            disks.push((disk, minutes));
        }
    }
    Ok(build(options, cpus, available, disks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(rotational: bool) -> Disk {
        Disk { name: "sda".into(), model: "Disk".into(), bytes: 2 << 40, sector: 512, rotational }
    }

    #[test]
    fn no_area_named_means_every_area_and_the_time_follows_the_kind_of_check() {
        let quick = build(&Options::default(), 4, 8 << 20, Vec::new());
        assert_eq!(quick.areas, Area::ALL);
        assert_eq!(quick.time, Duration::from_secs(60));
        // A quarter of 8 GiB is 2 GiB, over what a quick check tests at most.
        assert_eq!(quick.memory, 1 << 30);
        let full = Options { full: true, ..Options::default() };
        let full = build(&full, 4, 8 << 20, Vec::new());
        assert_eq!(full.time, Duration::from_secs(600));
        assert_eq!(full.memory, 6 << 30);
        let named = Options { areas: vec![Area::Cpu], time: Some(5), ..Options::default() };
        let named = build(&named, 4, 8 << 20, Vec::new());
        assert_eq!((named.areas.clone(), named.time), (vec![Area::Cpu], Duration::from_secs(5)));
        assert!(named.has(Area::Cpu) && !named.has(Area::Disks));
    }

    #[test]
    fn a_quick_check_takes_what_its_disk_and_its_own_work_take_together_at_most() {
        // The disk's 2-minute self-test and 1 GiB of reads outlast
        // 60 seconds of load and a few of memory.
        let plan = build(&Options::default(), 4, 8 << 20, vec![(disk(false), 2)]);
        let times = plan.disk_times();
        assert_eq!(times, [Duration::from_secs(122)]);
        assert_eq!(plan.estimate(&times), Duration::from_secs(124));
        assert_eq!(duration_text(plan.estimate(&times)), "about 2 minutes");
        // Without disks, the work here is memory then load.
        let alone = build(&Options::default(), 4, 8 << 20, Vec::new());
        let memory = memory_time(1 << 30);
        assert_eq!(alone.estimate(&[]), memory + Duration::from_secs(60 + 2));
    }

    #[test]
    fn the_time_of_a_full_check_is_said_in_hours_and_minutes() {
        assert_eq!(duration_text(Duration::from_secs(20)), "about a minute");
        assert_eq!(duration_text(Duration::from_secs(150)), "about 3 minutes");
        assert_eq!(duration_text(Duration::from_mins(119)), "about 119 minutes");
        assert_eq!(duration_text(Duration::from_hours(3)), "about 3 hours");
        assert_eq!(duration_text(Duration::from_mins(70)), "about 70 minutes");
        assert_eq!(duration_text(Duration::from_mins(314)), "about 5 hours 14 minutes");
        assert_eq!(duration_text(Duration::from_mins(160)), "about 2 hours 40 minutes");
    }

    #[test]
    fn a_full_check_asks_first_with_the_time_it_takes() {
        let options = Options { full: true, ..Options::default() };
        let plan = build(&options, 2, 4 << 20, vec![(disk(true), 180)]);
        let question = plan.question();
        assert!(question.starts_with("A full check takes about 8 hours 50 minutes"), "{question}");
        assert!(question.ends_with("Start it? [y/N] "));
        assert!(
            plan.heading()
                .starts_with("Checking the hardware: a full check, about 8 hours 50 minutes")
        );
    }
}
