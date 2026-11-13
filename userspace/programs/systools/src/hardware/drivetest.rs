// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A disk's own self-test, started and waited for: the short test of a
//! quick check, the extended one of a full check. The kernel starts it
//! and reads the disk's log of results ([`crate::selftest`], radix-block(2)'s
//! SELF_TEST); a disk runs it by itself and answers reads meanwhile.
//!
//! The wait polls the disk's state every 5 seconds. A disk that has not
//! said it runs the test is given 15 seconds to log a result of its own,
//! since a disk may finish a short test, or QEMU's at once, between two
//! looks. Ctrl-C ends the wait and aborts the test this run started.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::check::{Area, Check, Problem};
use crate::selftest::{self, Entry, Outcome, State, Test};
use crate::util::Fail;

/// How often the disk is asked how far its test is.
const POLL: Duration = Duration::from_secs(5);

/// How long a disk that does not say it runs the test has to log one.
const GRACE: Duration = Duration::from_secs(15);

/// How the wait for a test ended.
#[derive(Debug)]
pub(crate) enum Ended {
    /// The disk is no longer running it, and its log's newest result is
    /// this.
    Newest(Option<Entry>),
    /// The test had not ended when the time was up.
    TimedOut,
    /// The person stopped the run.
    Stopped,
    /// The disk could not be asked.
    Unreadable(Fail),
}

/// How many minutes `test` takes on a disk: its own estimate, or the time
/// a disk usually takes where it gives none.
pub(crate) fn minutes(estimate: Option<(u32, u32)>, test: Test) -> u32 {
    let own = estimate.map(|(short, extended)| if test == Test::Short { short } else { extended });
    own.unwrap_or(if test == Test::Short { 2 } else { 180 })
}

/// How long to wait for `test`: three times what the disk takes and five
/// minutes more.
pub(crate) fn bound(estimate: Option<(u32, u32)>, test: Test) -> Duration {
    Duration::from_secs((3 * u64::from(minutes(estimate, test)) + 5) * 60)
}

/// Waits for the test that began after the log's newest result was
/// `before`: `state` asks the disk, `pause` waits the poll time and says
/// whether to go on (it does not once the person stopped the run).
pub(crate) fn wait(
    before: Option<&Entry>,
    time: Duration,
    state: &mut dyn FnMut() -> Result<Option<State>, Fail>,
    pause: &mut dyn FnMut(Duration) -> bool,
) -> Ended {
    let mut waited = Duration::ZERO;
    let mut seen_running = false;
    loop {
        match state() {
            Err(fail) => return Ended::Unreadable(fail),
            Ok(None) => return Ended::Newest(None),
            Ok(Some(now)) if now.running => seen_running = true,
            Ok(Some(now)) => {
                if seen_running || now.newest.as_ref() != before || waited >= GRACE {
                    return Ended::Newest(now.newest);
                }
            }
        }
        if waited >= time {
            return Ended::TimedOut;
        }
        if !pause(POLL) {
            return Ended::Stopped;
        }
        waited += POLL;
    }
}

/// Waits `time`, in short steps so that Ctrl-C ends it at once; false
/// if it did.
fn pause(time: Duration, stop: &AtomicBool) -> bool {
    let step = Duration::from_millis(100);
    let mut left = time;
    while !left.is_zero() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = left.min(step);
        thread::sleep(now);
        left -= now;
    }
    !stop.load(Ordering::Relaxed)
}

fn kind(test: Test) -> &'static str {
    if test == Test::Short { "short" } else { "extended" }
}

/// The check of a test that ended with `newest` as the log's newest
/// result.
pub(crate) fn verdict(name: &str, test: Test, newest: Option<Entry>) -> Check {
    let label = format!("{name} self-test");
    let kind = kind(test);
    let Some(entry) = newest else {
        return Check::skipped(
            Area::Disks,
            &label,
            format!("the disk's log holds no result of the {kind} self-test"),
        );
    };
    let hours = entry.power_on_hours.map_or(String::new(), |hours| format!(", at hour {hours}"));
    match entry.outcome {
        Outcome::Passed => Check::passed(
            Area::Disks,
            &label,
            format!("the {kind} self-test ended without an error{hours}"),
        ),
        Outcome::Failed { first_bad_sector } => {
            let sector = first_bad_sector
                .map_or(String::new(), |sector| format!("; first bad sector {sector}"));
            Check::failed(
                Area::Disks,
                &label,
                Problem::DiskSelfTest,
                format!("the {kind} self-test failed{sector}{hours}"),
            )
        }
        Outcome::Aborted => {
            Check::skipped(Area::Disks, &label, format!("the {kind} self-test was aborted"))
        }
        Outcome::Interrupted => Check::skipped(
            Area::Disks,
            &label,
            format!(
                "a reset or a command of the machine interrupted the {kind} self-test; run it \
                 again"
            ),
        ),
        Outcome::Other(code) => Check::skipped(
            Area::Disks,
            &label,
            format!(
                "the disk ended the {kind} self-test with status {code:#04x}, which is not one \
                 it knows"
            ),
        ),
    }
}

/// Runs the self-test of disk `name`, short or, in a full check,
/// extended, and gives its check.
pub(crate) fn run(name: &str, full: bool, stop: &AtomicBool) -> Check {
    let label = format!("{name} self-test");
    let skipped = |why: String| Check::skipped(Area::Disks, &label, why);
    let test = if full { Test::Extended } else { Test::Short };
    if stop.load(Ordering::Relaxed) {
        return skipped("stopped before it began".into());
    }
    let before = match selftest::state(name) {
        Ok(Some(state)) if state.running => {
            return skipped("a self-test is running already, which this check leaves alone".into());
        }
        Ok(Some(state)) => state,
        Ok(None) => return skipped("the disk runs no self-test here".into()),
        Err(fail) => return skipped(fail.message),
    };
    if let Err(fail) = selftest::start(name, test) {
        return skipped(fail.message);
    }
    let time = bound(before.minutes, test);
    let ended = wait(before.newest.as_ref(), time, &mut || selftest::state(name), &mut |time| {
        pause(time, stop)
    });
    match ended {
        Ended::Newest(newest) => verdict(name, test, newest),
        Ended::Unreadable(fail) => skipped(fail.message),
        Ended::Stopped => {
            abort(name);
            skipped("stopped; the self-test was aborted".into())
        }
        Ended::TimedOut => {
            abort(name);
            skipped(format!(
                "the {} self-test had not ended after {} minutes, and was aborted",
                kind(test),
                time.as_secs() / 60
            ))
        }
    }
}

/// Aborts the test this run started on disk `name`; a disk that will not
/// is told of on the error output, as the check has no line for it.
fn abort(name: &str) {
    if let Err(fail) = selftest::abort(name) {
        eprintln!("hardware: {}", fail.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::fail;

    fn entry(outcome: Outcome, hours: u64) -> Entry {
        Entry { test: Some(Test::Short), outcome, power_on_hours: Some(hours) }
    }

    fn state(running: bool, newest: Option<Entry>) -> State {
        State { running, percent_left: if running { 40 } else { 0 }, newest, minutes: None }
    }

    /// Waits on a disk that gives `states` one after another, and counts
    /// the pauses; `stopped_after` pauses, the person stops the run.
    fn waited(
        before: Option<&Entry>,
        time: Duration,
        states: Vec<Result<Option<State>, Fail>>,
        stopped_after: usize,
    ) -> (Ended, usize) {
        let mut states = states.into_iter();
        let mut pauses = 0;
        let ended = wait(
            before,
            time,
            &mut || states.next().unwrap_or_else(|| Err(fail("the script ran out".into()))),
            &mut |_| {
                pauses += 1;
                pauses <= stopped_after
            },
        );
        (ended, pauses)
    }

    #[test]
    fn a_test_that_runs_is_waited_for_until_the_disk_logs_its_result() {
        let done = entry(Outcome::Passed, 100);
        let states = vec![
            Ok(Some(state(true, None))),
            Ok(Some(state(true, None))),
            Ok(Some(state(false, Some(done.clone())))),
        ];
        let (ended, pauses) = waited(None, Duration::from_secs(600), states, usize::MAX);
        assert!(matches!(ended, Ended::Newest(Some(ref newest)) if *newest == done));
        assert_eq!(pauses, 2);
    }

    #[test]
    fn a_test_a_disk_finishes_at_once_is_taken_from_its_new_log_entry() {
        let done = entry(Outcome::Passed, 4660);
        let states = vec![Ok(Some(state(false, Some(done.clone()))))];
        let (ended, pauses) = waited(None, Duration::from_secs(600), states, usize::MAX);
        assert!(matches!(ended, Ended::Newest(Some(newest)) if newest == done));
        assert_eq!(pauses, 0);
    }

    #[test]
    fn a_disk_whose_log_did_not_change_is_given_fifteen_seconds() {
        let old = entry(Outcome::Passed, 4660);
        let states = (0..10).map(|_| Ok(Some(state(false, Some(old.clone()))))).collect();
        let (ended, pauses) = waited(Some(&old), Duration::from_secs(600), states, usize::MAX);
        assert!(matches!(ended, Ended::Newest(Some(newest)) if newest == old));
        assert_eq!(pauses, 3);
    }

    #[test]
    fn a_test_that_never_ends_times_out_and_a_stopped_run_is_stopped() {
        let states = || (0..100).map(|_| Ok(Some(state(true, None)))).collect();
        let (ended, pauses) = waited(None, Duration::from_secs(20), states(), usize::MAX);
        assert!(matches!(ended, Ended::TimedOut));
        assert_eq!(pauses, 4);
        let (ended, pauses) = waited(None, Duration::from_secs(600), states(), 2);
        assert!(matches!(ended, Ended::Stopped));
        assert_eq!(pauses, 3);
    }

    #[test]
    fn a_disk_that_cannot_be_asked_ends_the_wait() {
        let states = vec![Ok(Some(state(true, None))), Err(fail("sda: Input/output error".into()))];
        let (ended, _) = waited(None, Duration::from_secs(600), states, usize::MAX);
        assert!(matches!(ended, Ended::Unreadable(fail) if fail.message.contains("Input/output")));
    }

    #[test]
    fn the_time_to_wait_is_three_times_the_disks_estimate_and_five_minutes() {
        let estimate = Some((2, 120));
        assert_eq!(bound(estimate, Test::Short), Duration::from_mins(11));
        assert_eq!(bound(estimate, Test::Extended), Duration::from_mins(365));
        assert_eq!(bound(None, Test::Short), Duration::from_mins(11));
        assert_eq!(super::minutes(estimate, Test::Extended), 120);
        assert_eq!(super::minutes(None, Test::Short), 2);
        assert_eq!(super::minutes(None, Test::Extended), 180);
    }

    #[test]
    fn a_result_in_the_log_decides_the_check() {
        use super::super::check::Outcome as Found;
        let check = |outcome| verdict("sda", Test::Short, Some(entry(outcome, 77)));
        let passed = check(Outcome::Passed);
        assert_eq!(passed.outcome, Found::Passed);
        assert_eq!(passed.name, "sda self-test");
        assert_eq!(passed.detail, "the short self-test ended without an error, at hour 77");
        let failed = check(Outcome::Failed { first_bad_sector: Some(12345) });
        assert_eq!(failed.outcome, Found::Failed);
        assert_eq!(failed.id.as_deref(), Some("HW-112"));
        assert_eq!(failed.detail, "the short self-test failed; first bad sector 12345, at hour 77");
        let failed = check(Outcome::Failed { first_bad_sector: None });
        assert_eq!(failed.detail, "the short self-test failed, at hour 77");
        for outcome in [Outcome::Aborted, Outcome::Interrupted, Outcome::Other(0x25)] {
            assert_eq!(check(outcome).outcome, Found::Skipped, "{outcome:?}");
        }
        assert!(check(Outcome::Other(0x25)).detail.contains("status 0x25"));
        let none = verdict("nvme0n1", Test::Extended, None);
        assert_eq!(none.outcome, Found::Skipped);
        assert!(none.detail.contains("no result of the extended self-test"));
    }

    #[test]
    fn pausing_ends_at_once_for_a_run_that_is_stopped() {
        let stop = AtomicBool::new(true);
        assert!(!pause(Duration::from_secs(3600), &stop));
        let go = AtomicBool::new(false);
        assert!(pause(Duration::from_millis(20), &go));
    }
}
