// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Services' health checks (ADR 0470): the command a service's
//! `[health]` names, run as a process of the service's own, in its domain
//! and view, with its account and environment, which passes when it
//! exits 0 within `health.timeout`.
//!
//! A running service is checked `health.every` seconds after it is
//! ready, then that long after each check ends; one whose `ready.when` is
//! `healthy` is checked at once as it starts, then each
//! READY_HEALTH_EVERY until a check passes. A check's output is not
//! logged, as a probe's is not; the last line of a failed one's is kept
//! for `show`. Spark logs a change only: the first failure, and a pass
//! after failures. After `health.failures` failed checks in a row the
//! service is stopped, as a restart request stops it, and its end counts
//! as a failed start, so that `give_up_after` gives up on a service that
//! never gets healthy.
//!
//! The `health-giveup` lab test covers this file.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::file as file_abi;
use libradix::abi::process;
use libradix::abi::signal;
use libradix::abi::startup::SLOT_PROCESS;
use manifest::service::ReadyWhen;
use radix_rt::fs::File;
use radix_rt::{SYS, println};

use super::{LINE_MAX, SECOND, STOP_WAIT, State, Supervisor, ending, now};

/// How often a service that is ready once healthy is checked while it
/// starts.
const READY_HEALTH_EVERY: u64 = SECOND;

/// A service's health checks.
#[derive(Default)]
pub(super) struct Checks {
    /// The check that runs, if one does.
    running: Option<CheckRun>,
    /// When the next check starts, while none runs; 0 while none is due,
    /// as before the service is ready.
    pub(super) next: u64,
    /// Failed checks in a row.
    pub(super) failures: u32,
    /// How the last failed check ended, and the last line it wrote.
    pub(super) last_failure: Option<String>,
    /// Whether the last check passed; None before the first since the
    /// service started.
    pub(super) passing: Option<bool>,
    /// When the last check ended.
    pub(super) checked_ns: Option<u64>,
}

/// A check's process.
struct CheckRun {
    pid: u64,
    /// When it is stopped if it has not ended.
    until: u64,
    output: Option<File>,
    /// The line being read, and the last whole line.
    line: Vec<u8>,
    last: Vec<u8>,
    /// It ran past its time, and was stopped.
    late: bool,
}

impl Checks {
    /// The running check's process.
    pub(super) fn pid(&self) -> Option<u64> {
        self.running.as_ref().map(|run| run.pid)
    }

    /// The running check's output, to poll.
    pub(super) fn output(&self) -> Option<&File> {
        self.running.as_ref().and_then(|run| run.output.as_ref())
    }

    /// When Spark must look at the checks again: the next one's start, or
    /// the running one's time limit.
    pub(super) fn next_look(&self) -> Option<u64> {
        match &self.running {
            Some(run) if !run.late => Some(run.until),
            Some(_) => None,
            None => (self.next > 0).then_some(self.next),
        }
    }

    /// Stops the running check, if one runs, and forgets it, so that its
    /// end is no check's; returns whether one ran.
    pub(super) fn stop(&mut self) -> bool {
        let Some(run) = self.running.take() else { return false };
        let _ = process::kill(&SYS, SLOT_PROCESS, run.pid as i64, signal::SIGKILL);
        true
    }

    /// Reads what the running check wrote, keeping its last line.
    pub(super) fn read_output(&mut self) {
        let Some(run) = &mut self.running else { return };
        let Some(output) = &run.output else { return };
        let mut buffer = [0u8; 512];
        loop {
            match file_abi::read(&SYS, output.slot(), file_abi::CURRENT, &mut buffer) {
                Ok(read) if read.count == 0 => {
                    run.output = None;
                    break;
                }
                Ok(read) => {
                    for &byte in &buffer[..read.count as usize] {
                        match byte {
                            b'\n' => run.last = core::mem::take(&mut run.line),
                            b'\r' => {}
                            _ if run.line.len() < LINE_MAX => run.line.push(byte),
                            _ => {}
                        }
                    }
                }
                // Would block, or gone.
                Err(_) => break,
            }
        }
        if !run.line.is_empty() && run.output.is_none() {
            run.last = core::mem::take(&mut run.line);
        }
    }
}

impl Supervisor {
    /// Starts the health checks that are due, and stops those that ran
    /// past their time.
    pub(super) fn step_health(&mut self, time: u64) {
        for index in 0..self.services.len() {
            let service = &mut self.services[index];
            let Some(health) = &service.spec.health else { continue };
            let checked = match service.state {
                State::Running(_) => true,
                State::Starting(_) => service.spec.ready.when == ReadyWhen::Healthy,
                _ => false,
            };
            if !checked || service.stop_by.is_some() {
                continue;
            }
            if let Some(run) = &mut service.checks.running {
                if !run.late && time >= run.until {
                    // Its end comes through `ended`, as a failure.
                    run.late = true;
                    let _ = process::kill(&SYS, SLOT_PROCESS, run.pid as i64, signal::SIGKILL);
                }
                continue;
            }
            if service.checks.next == 0 || time < service.checks.next {
                continue;
            }
            let command = health.command.clone();
            let timeout = u64::from(health.timeout) * SECOND;
            self.start_check(index, &command, time + timeout);
        }
    }

    /// Starts a health check of service `index`, running `command`, to be
    /// stopped at `until`.
    fn start_check(&mut self, index: usize, command: &[String], until: u64) {
        let Some((program, arguments)) = command.split_first() else { return };
        match self.spawn_as(index, program, arguments) {
            Ok((pid, output)) => {
                let checks = &mut self.services[index].checks;
                checks.next = 0;
                checks.running = Some(CheckRun {
                    pid,
                    until,
                    output: Some(output),
                    line: Vec::new(),
                    last: Vec::new(),
                    late: false,
                });
            }
            Err(why) => self.check_failed(index, format!("it could not start: {why}")),
        }
    }

    /// Whether process `pid` is a health check, which ended with `status`;
    /// if it is, moves its service on.
    pub(super) fn check_ended(&mut self, pid: u64, status: u32) -> bool {
        let Some(index) = self.services.iter().position(|s| s.checks.pid() == Some(pid)) else {
            return false;
        };
        let checks = &mut self.services[index].checks;
        checks.read_output();
        let Some(run) = checks.running.take() else { return true };
        checks.checked_ns = Some(now());
        if status == 0 && !run.late {
            self.check_passed(index);
            return true;
        }
        let timeout = self.services[index].spec.health.as_ref().map_or(0, |h| h.timeout);
        let mut words = if run.late { format!("it took over {timeout} s") } else { ending(status) };
        if !run.last.is_empty() {
            words.push_str(": ");
            words.push_str(&String::from_utf8_lossy(&run.last));
        }
        self.check_failed(index, words);
        true
    }

    /// Service `index`'s health check passed.
    fn check_passed(&mut self, index: usize) {
        let service = &mut self.services[index];
        let failed_before = service.checks.passing == Some(false);
        service.checks.passing = Some(true);
        service.checks.failures = 0;
        if matches!(service.state, State::Starting(_)) {
            // It schedules the next check.
            return self.became_ready(index);
        }
        if failed_before {
            println!("spark: {} passes its health check again", service.spec.name);
        }
        let every = service.spec.health.as_ref().map_or(0, |h| u64::from(h.every));
        service.checks.next = now() + every * SECOND;
    }

    /// Service `index`'s health check failed, as `words` say: it is
    /// checked again, or stopped once it has failed too many in a row.
    fn check_failed(&mut self, index: usize, words: String) {
        let service = &mut self.services[index];
        let Some(health) = &service.spec.health else { return };
        let checks = &mut service.checks;
        checks.passing = Some(false);
        checks.failures += 1;
        checks.last_failure = Some(words);
        let time = now();
        if matches!(service.state, State::Starting(_)) {
            // Not ready yet; its ready.timeout decides.
            checks.next = time + READY_HEALTH_EVERY;
            return;
        }
        let name = &service.spec.name;
        let words = checks.last_failure.as_deref().unwrap_or("");
        if checks.failures == 1 {
            println!("spark: {name} failed its health check ({words})");
        }
        if checks.failures < health.failures {
            checks.next = time + u64::from(health.every) * SECOND;
            return;
        }
        println!(
            "spark: {name} failed its health check {} times in a row; restarting it",
            checks.failures
        );
        checks.next = 0;
        let Some(pid) = service.state.pid() else { return };
        service.unhealthy = true;
        service.why = Some(format!(
            "it failed its health check {} times in a row ({words})",
            checks.failures
        ));
        service.stop_by.get_or_insert(time + STOP_WAIT);
        // Its end comes through `ended`, and `schedule` starts it again as
        // its file says.
        let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal::SIGTERM);
    }
}
