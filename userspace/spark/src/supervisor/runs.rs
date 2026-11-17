// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A program run once as a process of a service's own, for root (ADR
//! 0569, `spark_api::run`): the control socket's `run NAME PROGRAM
//! [ARGUMENT...]`, started as the service's health checks are, in its
//! domain and view, as its account, and answered once it ends with its
//! code and the first of what it wrote. It is killed after
//! `spark_api::run::SECONDS`. One runs at a time.
//!
//! A service with no account of its own, or one that holds the names
//! capability or the whole tree, is refused: what runs so never holds
//! more than an account's service does.
//!
//! The `admin-websites` boot test covers this file: website(8) has nginx
//! check its settings through it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::file as file_abi;
use libradix::abi::process;
use libradix::abi::signal;
use libradix::abi::startup::SLOT_PROCESS;
use radix_rt::fs::File;
use radix_rt::{SYS, println};
use spark_api::run::{self, Run};

use super::{CONNECTION_WAIT, Connection, SECOND, Supervisor, now};

/// The program that runs, and the connection that asked for it.
pub(super) struct Running {
    connection: Connection,
    /// The service's name, for the log.
    service: String,
    pid: u64,
    output: Option<File>,
    kept: Vec<u8>,
    /// When it is killed if it has not ended.
    until: u64,
    stopped: bool,
}

impl Running {
    /// Its output, to poll.
    pub(super) fn output(&self) -> Option<&File> {
        self.output.as_ref()
    }

    /// When the loop must look at it again: its time limit, until it is
    /// killed.
    pub(super) fn next_look(&self) -> Option<u64> {
        (!self.stopped).then_some(self.until)
    }

    /// Reads what it wrote, keeping what fits.
    pub(super) fn read_output(&mut self) {
        let Some(output) = &self.output else { return };
        let mut buffer = [0u8; 512];
        loop {
            match file_abi::read(&SYS, output.slot(), file_abi::CURRENT, &mut buffer) {
                Ok(read) if read.count == 0 => {
                    self.output = None;
                    return;
                }
                Ok(read) => run::keep(&mut self.kept, &buffer[..read.count as usize]),
                // Would block, or gone.
                Err(_) => return,
            }
        }
    }
}

impl Supervisor {
    /// Answers connection `index`'s `run` request, whose words after the
    /// verb are `words`: starts the program, keeping the connection to
    /// answer once it ends, or answers why not at once.
    pub(super) fn begin_run(&mut self, index: usize, words: &str) {
        let peer = self.connections[index].peer;
        match self.start_run(words) {
            Ok((service, pid, output)) => {
                audit!("running a program as {service}'s, process {pid}, for process {peer}");
                let connection = self.connections.remove(index);
                self.running = Some(Running {
                    connection,
                    service,
                    pid,
                    output: Some(output),
                    kept: Vec::new(),
                    until: now() + run::SECONDS * SECOND,
                    stopped: false,
                });
            }
            Err(why) => {
                let connection = &mut self.connections[index];
                connection.answered = true;
                connection.out = format!("error: {why}\n").into_bytes();
                connection.until = now() + CONNECTION_WAIT;
                self.write_answer(index);
            }
        }
    }

    /// Starts what `words` asks for: the service's name, the process's
    /// number and its output.
    fn start_run(&mut self, words: &str) -> Result<(String, u64, File), String> {
        if self.running.is_some() {
            return Err(run::BUSY.into());
        }
        let asked = Run::parse(words)?;
        let index = self
            .services
            .iter()
            .position(|s| s.spec.name == asked.service)
            .ok_or_else(|| format!("there is no service {}", asked.service))?;
        let spec = &self.services[index].spec;
        if spec.account.is_none() {
            return Err(format!(
                "{} has no account of its own: run runs only an account's service's programs",
                asked.service
            ));
        }
        if spec.names || spec.whole_tree {
            return Err(format!(
                "{} holds the names capability or the whole tree: run does not run its programs",
                asked.service
            ));
        }
        let (pid, output) = self.spawn_as(index, &asked.program, &asked.arguments, false)?;
        Ok((asked.service, pid, output))
    }

    /// Whether process `pid` is the program run for a `run` request; if
    /// it is, answers with how it ended.
    pub(super) fn run_ended(&mut self, pid: u64, status: u32) -> bool {
        if self.running.as_ref().is_none_or(|r| r.pid != pid) {
            return false;
        }
        let Some(mut running) = self.running.take() else { return true };
        running.read_output();
        let code = run::code(status);
        let ended = if running.stopped {
            format!("was killed after {} s", run::SECONDS)
        } else {
            format!("ended with code {code}")
        };
        println!("spark: the program run as {}'s, process {pid}, {ended}", running.service);
        let mut connection = running.connection;
        connection.out = [run::head(code, running.stopped).into_bytes(), running.kept].concat();
        connection.written = 0;
        connection.answered = true;
        connection.until = now() + CONNECTION_WAIT;
        self.connections.push(connection);
        true
    }

    /// Kills the program run for a `run` request once its time is up.
    pub(super) fn step_run(&mut self, time: u64) {
        let Some(running) = &mut self.running else { return };
        if running.stopped || time < running.until {
            return;
        }
        running.stopped = true;
        let _ = process::kill(&SYS, SLOT_PROCESS, running.pid as i64, signal::SIGKILL);
    }
}
