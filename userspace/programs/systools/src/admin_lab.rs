// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admin lab-steps [SECONDS]` (lab builds only, ADR 0516): the command of
//! the lab's long action, `lab.long`, which the tests of the console's
//! tasks run, since no real action is long yet.
//!
//! It prints `step N of 4: WORDS` and `progress P%` lines over SECONDS
//! seconds (20 when it is not said, up to 120), with a log line each
//! second, and changes nothing. When it is sent SIGTERM it says it stops
//! and ends well at once, as a command that puts back what it did does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::admind::actions::lab::{SECONDS_DEFAULT, SECONDS_MAX};
use crate::args::{Arg, Args};
use crate::util::{self, Fail, usage};

/// What a mistake in the command line is told.
const USAGE: &str = "usage: admin lab-steps [SECONDS]";

/// What the four steps are.
const STEPS: [&str; 4] = ["Getting ready", "Copying", "Checking", "Finishing"];

/// How long it sleeps between looks for the signal.
const SLICE: Duration = Duration::from_millis(50);

/// Set by SIGTERM.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// The lines of second `second` (from 0) of a run of `seconds` seconds:
/// the step when it starts one, the progress in it, and a log line.
pub fn lines(second: u32, seconds: u32) -> Vec<String> {
    let at = u64::from(second) * 400 / u64::from(seconds.max(1));
    let step = usize::try_from(at / 100).unwrap_or(0).min(STEPS.len() - 1);
    let before = u64::from(second.saturating_sub(1)) * 400 / u64::from(seconds.max(1));
    let mut lines = Vec::new();
    if second == 0 || usize::try_from(before / 100).unwrap_or(0) != step {
        lines.push(format!("step {} of {}: {}", step + 1, STEPS.len(), STEPS[step]));
    }
    lines.push(format!("progress {}%", at % 100));
    lines.push(format!("lab-steps: second {} of {seconds}", second + 1));
    lines
}

/// `admin lab-steps [SECONDS]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut seconds = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if seconds.is_none() => {
                seconds = Some(
                    operand
                        .to_str()
                        .and_then(|text| text.parse::<u32>().ok())
                        .filter(|seconds| (1..=SECONDS_MAX).contains(seconds))
                        .ok_or_else(|| usage(format!("SECONDS is 1 to {SECONDS_MAX}\n{USAGE}")))?,
                );
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let seconds = seconds.unwrap_or(SECONDS_DEFAULT);
    // SAFETY: all zeros is a valid sigaction: numbers, a signal set and a
    // function address.
    let mut action: libc::sigaction = unsafe { util::zeroed() };
    action.sa_sigaction = on_term as *const () as usize;
    // SAFETY: the handler only stores to an atomic.
    unsafe { libc::sigaction(libc::SIGTERM, &raw const action, std::ptr::null_mut()) };
    let start = Instant::now();
    for second in 0..seconds {
        for line in lines(second, seconds) {
            println!("{line}");
        }
        let until = start + Duration::from_secs(u64::from(second) + 1);
        while Instant::now() < until {
            if STOP.load(Ordering::Relaxed) {
                println!("lab-steps: stopped; there is nothing to put back");
                return Ok(0);
            }
            std::thread::sleep(SLICE);
        }
    }
    println!("progress 100%");
    println!("lab-steps: finished");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_says_each_step_once_and_its_progress_each_second() {
        let all: Vec<String> = (0..20).flat_map(|second| lines(second, 20)).collect();
        let steps: Vec<&String> = all.iter().filter(|line| line.starts_with("step ")).collect();
        assert_eq!(
            steps,
            [
                "step 1 of 4: Getting ready",
                "step 2 of 4: Copying",
                "step 3 of 4: Checking",
                "step 4 of 4: Finishing"
            ]
        );
        let percents: Vec<&str> =
            all.iter().filter(|line| line.starts_with("progress ")).map(String::as_str).collect();
        assert_eq!(percents.len(), 20);
        assert_eq!(percents[0], "progress 0%");
        assert_eq!(percents[1], "progress 20%");
        assert_eq!(percents[5], "progress 0%");
        assert_eq!(percents[19], "progress 80%");
        assert_eq!(lines(0, 20).last().unwrap(), "lab-steps: second 1 of 20");
    }

    #[test]
    fn a_run_of_one_second_is_one_second_long_and_still_says_a_step() {
        assert_eq!(
            lines(0, 1),
            ["step 1 of 4: Getting ready", "progress 0%", "lab-steps: second 1 of 1"]
        );
    }

    #[test]
    fn the_seconds_are_one_number_in_range() {
        for bad in [&["0"][..], &["121"], &["x"], &["1", "2"], &["--now"]] {
            let items = bad.iter().map(Into::into).collect();
            let failed = main(&mut Args::new("admin", items)).unwrap_err();
            assert!(failed.usage || failed.message.contains("usage"), "{bad:?}");
        }
    }
}
