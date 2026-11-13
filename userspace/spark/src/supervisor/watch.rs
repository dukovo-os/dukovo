// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Watches (spark(8), ADR 0471): `start`, `stop` and `restart` with
//! `--wait` keep the connection open once answered, and Spark writes to
//! it each of its log lines about the services the request touches, as
//! it says them, and a last line, `ok:` or `error:`, once the service has
//! settled, so that service(8) shows a start along the service graph as
//! it happens and returns once it is over.
//!
//! A start or restart has settled once the service is ready or done, or
//! has ended well, or a start of it or of a service it waits for has
//! failed, was refused or was given up on; a stop, once the service and those that require it
//! have ended. Each of these comes within the ready timeouts and the
//! stop wait of what the watch follows, so a watch always ends; WATCH_MAX
//! is a guard. A watch's lines are the log's, word for word.
//!
//! At most WATCHES_MAX watches run at once, so that other requests always
//! find room among the connections; a request for another is answered as
//! if it had not asked to wait. A watch whose reader has gone, or that has
//! UNREAD_MAX bytes its reader has not taken, is closed. The
//! `service-graph` boot test covers this file.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use radix_rt::println;

use super::{Connection, MS, SECOND, Shutdown, State, Supervisor, ending, now};

/// The most watches at once.
pub(super) const WATCHES_MAX: usize = 4;

/// How long a watch may last.
const WATCH_MAX: u64 = 15 * 60 * SECOND;

/// The most a watch's reader may leave unread.
const UNREAD_MAX: usize = 64 * 1024;

/// A line Spark said about a service in this turn of its loop.
pub(super) struct Told {
    index: usize,
    line: String,
}

/// Logs `line`, a line of Spark's about service `index`, and keeps it for
/// the watches that follow that service.
pub(super) fn say(told: &mut Vec<Told>, index: usize, line: String) {
    println!("spark: {line}");
    told.push(Told { index, line });
}

/// What a `start`, `stop` or `restart` request asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verb {
    Start,
    Stop,
    Restart,
}

impl Verb {
    /// The request's word.
    pub(super) fn word(self) -> &'static str {
        match self {
            Verb::Start => "start",
            Verb::Stop => "stop",
            Verb::Restart => "restart",
        }
    }
}

/// A request that waits until its service has settled.
pub(super) struct Watch {
    /// The service it named.
    index: usize,
    verb: Verb,
    /// Whether the service counts as started only once it has started
    /// since the request: a restart, or a start of one that is stopping.
    fresh: bool,
    /// Each service's starts when the request came, by index; a service
    /// added since had none.
    attempts: Vec<u32>,
    /// The services whose lines it gets, by index, the one it named
    /// among them.
    follows: Vec<usize>,
    /// The lines said in this turn before the request, which are not its.
    skip: usize,
    /// When it ends, settled or not.
    until: u64,
}

impl Watch {
    /// The starts service `index` had when the request came.
    fn attempts_before(&self, index: usize) -> u32 {
        self.attempts.get(index).copied().unwrap_or(0)
    }

    /// When it ends, settled or not.
    pub(super) fn until(&self) -> u64 {
        self.until
    }
}

impl Supervisor {
    /// A watch for a `verb` request of service `index`, as things are
    /// before the request is answered; None when WATCHES_MAX run already.
    pub(super) fn watch(&self, index: usize, verb: Verb) -> Option<Watch> {
        let watching = self.connections.iter().filter(|c| c.watch.is_some()).count();
        if watching >= WATCHES_MAX {
            return None;
        }
        let service = &self.services[index];
        let stopping = match service.state {
            State::Starting(_) | State::Running(_) => service.stop_by.is_some(),
            State::Draining(..) => true,
            _ => false,
        };
        Some(Watch {
            index,
            verb,
            fresh: verb == Verb::Restart || stopping,
            attempts: self.services.iter().map(|s| s.attempts).collect(),
            follows: Vec::new(),
            skip: self.told.len(),
            until: now() + WATCH_MAX,
        })
    }

    /// Fills in what `watch` follows, now that its request is answered:
    /// for a start or restart, its service and every service that one
    /// names, at any depth; for a stop, its service and every service
    /// that requires it, at any depth.
    pub(super) fn follow(&self, watch: &mut Watch) {
        let mut follows = alloc::vec![watch.index];
        let mut next = 0;
        while let Some(&at) = follows.get(next) {
            next += 1;
            let spec = &self.services[at].spec;
            let names: Vec<String> = match watch.verb {
                Verb::Start | Verb::Restart => {
                    spec.links().into_iter().map(|(name, _)| name).collect()
                }
                Verb::Stop => self
                    .services
                    .iter()
                    .filter(|s| s.spec.requires.contains(&spec.name))
                    .map(|s| s.spec.name.clone())
                    .collect(),
            };
            for name in names {
                if let Some(other) = self.services.iter().position(|s| s.spec.name == name)
                    && !follows.contains(&other)
                {
                    follows.push(other);
                }
            }
        }
        watch.follows = follows;
    }

    /// Hands each watch the lines said in this turn about what it follows,
    /// ends each that has settled with its last line, and writes what each
    /// has to write. Called once a turn, after the services have moved on.
    pub(super) fn step_watches(&mut self, time: u64) {
        let told = core::mem::take(&mut self.told);
        for at in (0..self.connections.len()).rev() {
            let Some(watch) = &mut self.connections[at].watch else { continue };
            let skip = core::mem::take(&mut watch.skip);
            let mut lines = String::new();
            for told in told.iter().skip(skip).filter(|t| watch.follows.contains(&t.index)) {
                lines.push_str("spark: ");
                lines.push_str(&told.line);
                lines.push('\n');
            }
            let last = self.settled(at, time);
            let connection = &mut self.connections[at];
            connection.out.extend_from_slice(lines.as_bytes());
            if let Some(last) = last {
                connection.out.extend_from_slice(last.as_bytes());
                connection.watch = None;
                connection.until = time + super::CONNECTION_WAIT;
            }
            if connection.out.len() - connection.written > UNREAD_MAX {
                self.connections.remove(at);
            } else if connection.written < connection.out.len() {
                self.write_answer(at);
            }
        }
    }

    /// The last line of connection `at`'s watch, once what it waits for
    /// has settled or its time is up.
    fn settled(&self, at: usize, time: u64) -> Option<String> {
        let watch = self.connections[at].watch.as_ref()?;
        let service = &self.services[watch.index];
        let name = &service.spec.name;
        if matches!(self.shutdown, Some(Shutdown::Stopping { .. })) {
            return Some(String::from("error: the machine is shutting down\n"));
        }
        if time >= watch.until {
            return Some(format!("ok: still waiting; `service {name} status` says more\n"));
        }
        match watch.verb {
            Verb::Stop => self.stopped(watch),
            Verb::Start | Verb::Restart => self.started_since(watch),
        }
    }

    /// The last line of a stop's watch, once what it follows has ended.
    fn stopped(&self, watch: &Watch) -> Option<String> {
        let running = watch.follows.iter().any(|&at| {
            matches!(
                self.services[at].state,
                State::Starting(_) | State::Running(_) | State::Draining(..)
            )
        });
        let name = &self.services[watch.index].spec.name;
        (!running).then(|| format!("ok: {name} is stopped\n"))
    }

    /// The last line of a start's or restart's watch, once the service is
    /// ready or a start it waits on has failed.
    fn started_since(&self, watch: &Watch) -> Option<String> {
        let service = &self.services[watch.index];
        let name = &service.spec.name;
        let since = service.attempts > watch.attempts_before(watch.index);
        match service.state {
            State::Running(pid) if service.stop_by.is_none() && (since || !watch.fresh) => {
                Some(format!("ok: {name} is ready, as process {pid}\n"))
            }
            State::Done => Some(format!("ok: {name} ran and is done\n")),
            // Started, and ended well before its watch looked, as a job
            // that is not to be started again does.
            State::Stopped if since && service.why.is_none() && service.last == Some(0) => {
                Some(format!("ok: {name} ran and ended\n"))
            }
            State::Failed => {
                Some(format!("error: Spark gave up on {name}: {}\n", self.failure(watch.index)))
            }
            State::Stopped => {
                Some(format!("error: could not start {name}: {}\n", self.failure(watch.index)))
            }
            State::Waiting(at) if since => Some(format!(
                "error: could not start {name}: {}; Spark tries again in {}\n",
                self.failure(watch.index),
                seconds(at.saturating_sub(now()))
            )),
            State::Pending => self.blocked(watch, watch.index, 0),
            State::Running(_) | State::Starting(_) | State::Waiting(_) | State::Draining(..) => {
                None
            }
        }
    }

    /// The last line of a watch on a start that waits, through service
    /// `index`, which is pending, for a service whose start has failed
    /// since the request, or was given up on, if one does.
    fn blocked(&self, watch: &Watch, index: usize, depth: usize) -> Option<String> {
        if depth > watch.follows.len() {
            return None;
        }
        let pending = &self.services[index];
        for name in &pending.waiting_for {
            let Some(at) = self.services.iter().position(|s| &s.spec.name == name) else {
                continue;
            };
            let other = &self.services[at];
            let failed_since = other.attempts > watch.attempts_before(at);
            let words = match other.state {
                State::Waiting(when) if failed_since => format!(
                    "{}; Spark tries {name} again in {}",
                    self.failure(at),
                    seconds(when.saturating_sub(now()))
                ),
                State::Failed => self.failure(at),
                State::Pending => match self.blocked(watch, at, depth + 1) {
                    Some(line) => return Some(line),
                    None => continue,
                },
                _ => continue,
            };
            let waiter = &self.services[watch.index].spec.name;
            return Some(format!(
                "error: {waiter} waits for {name}, which could not start: {words}\n"
            ));
        }
        None
    }

    /// Why service `index`'s last start failed, as a clause after its
    /// name: why Spark says it failed, or how it ended, with the last line
    /// its output wrote.
    pub(super) fn failure(&self, index: usize) -> String {
        let service = &self.services[index];
        let mut words = match (&service.why, service.last) {
            (Some(why), _) => why.clone(),
            (None, Some(status)) => format!("it {}", ending(status)),
            (None, None) => String::from("it could not start"),
        };
        if let Some(line) = &service.last_line {
            words.push_str(&format!(" (\"{line}\")"));
        }
        words
    }
}

/// A wait in seconds, to the tenth, as Spark's lines write one: "0.4 s".
fn seconds(ns: u64) -> String {
    format!("{}.{} s", ns / SECOND, ns % SECOND / (100 * MS))
}

impl Connection {
    /// Whether it waits for something to be said: a watch with nothing
    /// left to write.
    pub(super) fn idle(&self) -> bool {
        self.watch.is_some() && self.written == self.out.len()
    }
}
