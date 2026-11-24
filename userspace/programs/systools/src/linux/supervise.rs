// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `linux run` decides while it waits for the program: how often
//! linuxd may be started again, and what the status a wait gives means.
//!
//! The waiting itself is in `sandbox.rs`, which only runs on Dukovo; the
//! rules are here so that they are tested.

use std::collections::VecDeque;
use std::time::Duration;

/// The most times linuxd is started again in [`WITHIN`].
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const MOST: usize = 5;
/// The time the restarts are counted in.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const WITHIN: Duration = Duration::from_secs(60);

/// The times linuxd was started again, to keep a server that ends at
/// once from being started in a loop for as long as the program runs.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
#[derive(Debug, Default)]
pub struct Restarts {
    times: VecDeque<Duration>,
}

impl Restarts {
    /// Whether linuxd may be started again at `now`, a time on any clock
    /// that does not go back; if it may, the restart is counted.
    #[cfg_attr(not(target_os = "radix"), allow(dead_code))]
    pub fn allow(&mut self, now: Duration) -> bool {
        while self.times.front().is_some_and(|&time| time + WITHIN <= now) {
            self.times.pop_front();
        }
        if self.times.len() >= MOST {
            return false;
        }
        self.times.push_back(now);
        true
    }
}

/// How a process that `waitpid` reported has ended, as a shell gives its
/// status: the exit status, or 128 and the signal's number for one a
/// signal ended. `None` for a process that stopped or went on.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub fn exit_status(status: i32) -> Option<i32> {
    if libc::WIFEXITED(status) {
        Some(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        Some(128 + libc::WTERMSIG(status))
    } else {
        None
    }
}

/// Whether `waitpid` reported that the process stopped.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub fn stopped(status: i32) -> bool {
    libc::WIFSTOPPED(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn linuxd_is_started_again_five_times_a_minute() {
        let mut restarts = Restarts::default();
        for at in [0, 1, 2, 3, 4] {
            assert!(restarts.allow(secs(at)), "{at}");
        }
        assert!(!restarts.allow(secs(5)));
        assert!(!restarts.allow(secs(59)));
        // The first of the five is a minute old.
        assert!(restarts.allow(secs(60)));
        assert!(!restarts.allow(secs(60)));
        assert!(restarts.allow(secs(61)));
    }

    #[test]
    fn a_restart_that_was_refused_is_not_counted() {
        let mut restarts = Restarts::default();
        for _ in 0..MOST {
            assert!(restarts.allow(secs(10)));
        }
        for at in 11..40 {
            assert!(!restarts.allow(secs(at)));
        }
        // All five were made at 10, and nothing since counts against it.
        assert!(restarts.allow(secs(70)));
    }

    #[test]
    fn restarts_spread_over_the_minutes_are_all_allowed() {
        let mut restarts = Restarts::default();
        for at in (0..600).step_by(20) {
            assert!(restarts.allow(secs(at)), "{at}");
        }
    }

    #[test]
    fn the_status_is_the_exit_status_or_128_and_the_signal() {
        assert_eq!(exit_status(0), Some(0));
        assert_eq!(exit_status(3 << 8), Some(3));
        assert_eq!(exit_status(255 << 8), Some(255));
        assert_eq!(exit_status(libc::SIGKILL), Some(137));
        assert_eq!(exit_status(libc::SIGSEGV), Some(139));
        // A core dump, as the C library's macros see it.
        assert_eq!(exit_status(libc::SIGABRT | 0x80), Some(134));
    }

    #[test]
    fn a_stop_is_no_end() {
        let stop = (libc::SIGSTOP << 8) | 0x7f;
        assert!(stopped(stop));
        assert_eq!(exit_status(stop), None);
        assert!(!stopped(3 << 8));
        assert!(!stopped(libc::SIGTERM));
    }
}
