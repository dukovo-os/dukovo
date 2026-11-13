// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The cpu area of `hardware check`: all the CPUs under load for a while,
//! each of which must give the same answer as the first, how hot the
//! processor gets, and the machine checks in the kernel's log.
//!
//! The load is one thread for each CPU, pinned to it, which repeats the
//! same work: a chain of SHA-256 hashes and a floating-point matrix
//! product, folded into one number. The work is exact (no instruction
//! whose result differs between CPUs), so every round of every thread
//! must give the number the first round gave; a CPU that computes wrongly
//! under load, from heat, voltage or an overclock, does not. Meanwhile the
//! temperatures are read every 2 seconds, and their peaks judged against
//! the limits the monitor gives.

use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::check::{Area, Check, Problem, counted};
use super::monitors::{self, Temperature};
use crate::diskhealth;
use crate::dmesg::{self, Line};
use crate::priority;
use crate::sensors::HWMON;
use crate::util::Fail;

/// How often the temperatures are read.
pub(crate) const SAMPLE: Duration = Duration::from_secs(2);

/// Hashes in the chain of a round.
const CHAIN: usize = 2048;

/// The side of the matrices a round multiplies.
const SIDE: usize = 48;

/// How close to its high limit a processor may get before it warns.
const MARGIN: f64 = 5.0;

/// The monitors of processors: Intel's and AMD's package sensors and
/// the sensor of an ARM board's processor.
const CHIPS: [&str; 4] = ["coretemp", "k10temp", "zenpower", "cpu_thermal"];

/// One round of the work: a chain of hashes and a matrix product, folded
/// into one number that depends on every step.
pub(crate) fn round() -> u64 {
    let mut digest = [0u8; 32];
    digest[0] = black_box(1);
    for _ in 0..CHAIN {
        let next = ring::digest::digest(&ring::digest::SHA256, &digest);
        digest.copy_from_slice(next.as_ref());
    }
    let mut folded = u64::from_le_bytes(digest[..8].try_into().expect("a digest has 32 bytes"));
    let matrix = |seed: usize| -> Vec<f64> {
        (0..SIDE * SIDE)
            .map(|at| (((at * 7 + seed * 3) % 11) as f64 + black_box(0.25)) / 4.0)
            .collect()
    };
    let (a, b) = (matrix(1), matrix(2));
    for i in 0..SIDE {
        for j in 0..SIDE {
            let mut sum = 0.0f64;
            for k in 0..SIDE {
                sum += a[i * SIDE + k] * b[k * SIDE + j];
            }
            folded = folded.rotate_left(7) ^ sum.to_bits();
        }
    }
    folded
}

/// A CPU that gave another answer than the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Wrong {
    pub(crate) cpu: usize,
    /// Which of its rounds, from 1.
    pub(crate) round: u64,
    pub(crate) wanted: u64,
    pub(crate) got: u64,
}

/// How the load went.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Load {
    /// Rounds that agreed, over all the threads.
    pub(crate) rounds: u64,
    pub(crate) wrong: Option<Wrong>,
    pub(crate) stopped: bool,
}

/// Runs `work` on `threads` threads for `time`, calling `sample` every
/// `tick` meanwhile. `work` gets the thread's number, and `start` runs
/// first on the thread (it pins the thread to its CPU). A thread whose
/// answer differs from the first one given ends the load.
pub(crate) fn load(
    threads: usize,
    time: Duration,
    tick: Duration,
    work: &(dyn Fn(usize) -> u64 + Sync),
    start: &(dyn Fn(usize) + Sync),
    stop: &AtomicBool,
    sample: &mut dyn FnMut(),
) -> Load {
    let done = AtomicBool::new(false);
    let first = OnceLock::new();
    let wrong = Mutex::new(None);
    let rounds = AtomicU64::new(0);
    thread::scope(|scope| {
        for cpu in 0..threads {
            let (done, first, wrong, rounds) = (&done, &first, &wrong, &rounds);
            scope.spawn(move || {
                start(cpu);
                let mut mine = 0u64;
                while !done.load(Ordering::Relaxed) {
                    let got = work(cpu);
                    mine += 1;
                    let wanted = *first.get_or_init(|| got);
                    if got != wanted {
                        let mut slot = wrong.lock().unwrap_or_else(PoisonError::into_inner);
                        slot.get_or_insert(Wrong { cpu, round: mine, wanted, got });
                        done.store(true, Ordering::Relaxed);
                        return;
                    }
                    rounds.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        let began = Instant::now();
        let mut next = began + tick;
        let step = tick.min(Duration::from_millis(50));
        while !done.load(Ordering::Relaxed)
            && !stop.load(Ordering::Relaxed)
            && began.elapsed() < time
        {
            if Instant::now() >= next {
                sample();
                next += tick;
            }
            thread::sleep(step);
        }
        done.store(true, Ordering::Relaxed);
    });
    Load {
        rounds: rounds.into_inner(),
        wrong: wrong.into_inner().unwrap_or_else(PoisonError::into_inner),
        stopped: stop.load(Ordering::Relaxed),
    }
}

/// The check of how the load went.
pub(crate) fn load_check(threads: usize, time: Duration, found: &Load) -> Check {
    const NAME: &str = "load";
    if let Some(wrong) = found.wrong {
        let detail = format!(
            "the thread of CPU {} gave {:#018x} in its round {} where the first answer was \
             {:#018x}",
            wrong.cpu, wrong.got, wrong.round, wrong.wanted
        );
        return Check::failed(Area::Cpu, NAME, Problem::CpuWrongResult, detail);
    }
    if found.stopped {
        return Check::skipped(Area::Cpu, NAME, "stopped before it ended".into());
    }
    let cpus = if threads == 1 { "1 CPU".to_string() } else { format!("{threads} CPUs") };
    let detail = format!(
        "{cpus} ran {} rounds in {} seconds, and every answer agreed",
        diskhealth::grouped(u128::from(found.rounds)),
        time.as_secs()
    );
    Check::passed(Area::Cpu, NAME, detail)
}

/// The highest temperature each sensor read.
#[derive(Default)]
pub(crate) struct Peaks {
    highest: Vec<Temperature>,
}

impl Peaks {
    /// Takes what the sensors read now.
    pub(crate) fn add(&mut self, readings: Vec<Temperature>) {
        for reading in readings {
            let same = |t: &Temperature| t.chip == reading.chip && t.label == reading.label;
            match self.highest.iter_mut().find(|t| same(t)) {
                Some(kept) => {
                    let (high_alarm, critical_alarm) = (
                        kept.high_alarm || reading.high_alarm,
                        kept.critical_alarm || reading.critical_alarm,
                    );
                    if reading.celsius > kept.celsius {
                        *kept = reading;
                    }
                    kept.high_alarm = high_alarm;
                    kept.critical_alarm = critical_alarm;
                }
                None => self.highest.push(reading),
            }
        }
    }

    /// The check of the peaks: how hot the processor got.
    pub(crate) fn check(&self) -> Check {
        const NAME: &str = "temperature";
        if self.highest.is_empty() {
            let why = "the kernel shows no sensor of the processor's temperature";
            return Check::skipped(Area::Cpu, NAME, why.into());
        }
        monitors::heat_check(Area::Cpu, NAME, &self.highest, MARGIN, Problem::CpuHot)
    }
}

/// Reads the processors' temperatures in `hwmon`.
fn processor_temperatures(hwmon: &Path) -> Vec<Temperature> {
    let keep = |name: &str| CHIPS.iter().any(|chip| name.starts_with(&format!("{chip}-")));
    monitors::temperatures(hwmon, &keep)
}

/// The corrected machine-check errors the kernel's log holds: a record
/// for each, or the number it counts for a burst.
pub(crate) fn corrected(log: &[Line]) -> u64 {
    log.iter()
        .filter(|line| line.level == 4 && line.text.contains("Machine check: CPU "))
        .map(|line| {
            let burst = line
                .text
                .split("; ")
                .find_map(|part| part.strip_suffix(" of them since the bank was read"));
            burst.and_then(|count| count.parse().ok()).unwrap_or(1)
        })
        .sum()
}

/// The check of the machine checks: `before` and `after` count what the
/// log held when the load began and ended.
pub(crate) fn machine_check_check(before: &Result<u64, Fail>, after: &Result<u64, Fail>) -> Check {
    const NAME: &str = "machine checks";
    let (before, after) = match (before, after) {
        (Ok(before), Ok(after)) => (*before, *after),
        (Err(fail), _) | (_, Err(fail)) => {
            return Check::skipped(Area::Cpu, NAME, fail.message.clone());
        }
    };
    if after == 0 {
        return Check::passed(Area::Cpu, NAME, "none in the kernel's log since the start".into());
    }
    let during = after.saturating_sub(before);
    let now = if during > 0 { format!(", {during} during this check") } else { String::new() };
    let errors = counted(after, "corrected error");
    let detail = format!("{errors} in the kernel's log since the start{now}");
    Check::warning(Area::Cpu, NAME, Problem::CpuCorrected, detail)
}

fn machine_checks() -> Result<u64, Fail> {
    dmesg::records().map(|log| corrected(&log))
}

/// The checks of the area: the load of `cpus` CPUs for `time`, the
/// temperatures it reached, and the machine checks.
pub(crate) fn run(cpus: usize, time: Duration, stop: &AtomicBool) -> Vec<Check> {
    let before = machine_checks();
    let mut peaks = Peaks::default();
    peaks.add(processor_temperatures(Path::new(HWMON)));
    let found = load(
        cpus,
        time,
        SAMPLE,
        &|_| round(),
        // A thread that cannot be pinned runs where the scheduler puts it,
        // which loads the CPUs all the same.
        &|cpu| {
            let _ = priority::pin_to(cpu);
        },
        stop,
        &mut || peaks.add(processor_temperatures(Path::new(HWMON))),
    );
    peaks.add(processor_temperatures(Path::new(HWMON)));
    vec![
        load_check(cpus, time, &found),
        peaks.check(),
        machine_check_check(&before, &machine_checks()),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::*;
    use crate::util;

    const SHORT: Duration = Duration::from_millis(200);
    const TICK: Duration = Duration::from_millis(20);

    fn nothing(_: usize) {}

    #[test]
    fn a_round_gives_the_same_number_every_time() {
        assert_eq!(round(), round());
        assert_ne!(round(), 0);
    }

    #[test]
    fn threads_that_agree_run_rounds_until_the_time_is_up() {
        let stop = AtomicBool::new(false);
        let mut samples = 0;
        let found = load(3, SHORT, TICK, &|_| 42, &nothing, &stop, &mut || samples += 1);
        assert_eq!(found.wrong, None);
        assert!(!found.stopped);
        assert!(found.rounds > 0);
        // One reading about every tick of the 200 ms.
        assert!((5..=10).contains(&samples), "{samples}");
        let check = load_check(3, SHORT, &found);
        assert_eq!(check.outcome, Outcome::Passed);
        assert!(check.detail.starts_with("3 CPUs ran "), "{}", check.detail);
        assert!(check.detail.ends_with("rounds in 0 seconds, and every answer agreed"));
    }

    #[test]
    fn a_thread_that_gives_another_answer_fails_the_load() {
        // CPU 1 starts late, so that CPU 0's answer is the first.
        let late = |cpu: usize| {
            if cpu == 1 {
                thread::sleep(Duration::from_millis(30));
            }
        };
        let stop = AtomicBool::new(false);
        let planted = |cpu: usize| if cpu == 1 { 7 } else { 42 };
        let started = Instant::now();
        let found = load(3, Duration::from_secs(30), TICK, &planted, &late, &stop, &mut || {});
        assert!(started.elapsed() < Duration::from_secs(10), "the load ended at once");
        let wrong = found.wrong.expect("the planted answer is found");
        assert_eq!((wrong.cpu, wrong.wanted, wrong.got), (1, 42, 7));
        assert_eq!(wrong.round, 1);
        let check = load_check(3, SHORT, &found);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Failed, Some("HW-122")));
        assert_eq!(
            check.detail,
            "the thread of CPU 1 gave 0x0000000000000007 in its round 1 where the first answer \
             was 0x000000000000002a"
        );
    }

    #[test]
    fn a_stop_ends_the_load_early_and_the_check_says_so() {
        let stop = AtomicBool::new(true);
        let found = load(2, Duration::from_secs(30), TICK, &|_| 1, &nothing, &stop, &mut || {});
        assert!(found.stopped);
        let check = load_check(2, SHORT, &found);
        assert_eq!(check.outcome, Outcome::Skipped);
        // What a wrong answer found before the stop still fails.
        let wrong = Wrong { cpu: 0, round: 4, wanted: 1, got: 2 };
        let found = Load { wrong: Some(wrong), ..found };
        assert_eq!(load_check(2, SHORT, &found).outcome, Outcome::Failed);
    }

    fn reading(label: &str, celsius: f64) -> Temperature {
        Temperature {
            chip: "coretemp-isa-0000".into(),
            label: label.into(),
            celsius,
            high: Some(90.0),
            critical: Some(100.0),
            high_alarm: false,
            critical_alarm: false,
        }
    }

    #[test]
    fn the_peaks_keep_the_highest_reading_of_each_sensor() {
        let mut peaks = Peaks::default();
        assert_eq!(peaks.check().outcome, Outcome::Skipped);
        peaks.add(vec![reading("Package id 0", 50.0), reading("Core 0", 48.0)]);
        peaks.add(vec![reading("Package id 0", 70.0), reading("Core 0", 40.0)]);
        peaks.add(vec![reading("Package id 0", 60.0)]);
        let check = peaks.check();
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(check.detail, "hottest is Package id 0 at 70.0 °C (high limit 90.0 °C)");
        peaks.add(vec![reading("Core 0", 86.0)]);
        let check = peaks.check();
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-123")));
        peaks.add(vec![reading("Core 0", 100.0)]);
        assert_eq!(peaks.check().outcome, Outcome::Failed);
    }

    fn record(level: u8, text: &str) -> Line {
        dmesg::parse(format!("{level},1,1000,-;{text}\n").as_bytes()).unwrap()
    }

    #[test]
    fn corrected_machine_checks_are_counted_from_the_log() {
        let log = [
            record(6, "Machine check: 8 banks on each CPU, read at boot and every 300 s"),
            record(4, "Machine check: CPU 0 bank 9, at boot: corrected error"),
            record(
                4,
                "Machine check: CPU 1 bank 4: corrected error; 5 of them since the bank was read",
            ),
            record(3, "Machine check: CPU 0 bank 2: uncorrected error"),
            record(4, "Machine check: the page at 0x1000 is out of use for good"),
            record(4, "mm: something else"),
        ];
        assert_eq!(corrected(&log), 6);
        assert_eq!(corrected(&[]), 0);
    }

    #[test]
    fn the_machine_check_check_warns_when_the_log_holds_corrected_errors() {
        let ok = |n| Ok::<u64, Fail>(n);
        let check = machine_check_check(&ok(0), &ok(0));
        assert_eq!(check.outcome, Outcome::Passed);
        let check = machine_check_check(&ok(2), &ok(2));
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-121")));
        assert_eq!(check.detail, "2 corrected errors in the kernel's log since the start");
        let one = machine_check_check(&ok(0), &ok(1));
        assert_eq!(
            one.detail,
            "1 corrected error in the kernel's log since the start, 1 during this check"
        );
        let check = machine_check_check(&ok(2), &ok(5));
        assert_eq!(
            check.detail,
            "5 corrected errors in the kernel's log since the start, 3 during this check"
        );
        let refused =
            Err(util::fail("the kernel log is for administrators: run elevate dmesg".into()));
        let check = machine_check_check(&refused, &ok(0));
        assert_eq!(check.outcome, Outcome::Skipped);
        assert_eq!(check.detail, "the kernel log is for administrators: run elevate dmesg");
    }
}
