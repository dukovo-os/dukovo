// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! When services start, as their files' `requires`, `wants` and `after`
//! say (ADR 0470), and when they are ready.
//!
//! A service that is wanted (enabled at boot, started by a request, or
//! pulled in by one that requires or wants it) is pending until each
//! service it names is ready or off; one it requires must be ready, and
//! one that is off refuses it. [`spark_api::graph`] holds the rules;
//! here Spark says what each service is doing. Once started, a service
//! is starting until it is ready as its `ready.when` says, which Spark
//! looks for every READY_POLL, up to its `ready.timeout`; a start that is
//! not ready in time has failed, and the service is stopped and started
//! again as its file says. A service that stops for good stops those that
//! require it.
//!
//! What to look for when a service does not start: Spark's log says
//! "not starting NAME: it requires …" and "NAME did not get ready in …",
//! and `service NAME status` says what it waits for. The `service-graph`
//! boot test covers this file.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::directory::FileKind;
use libradix::abi::process;
use libradix::abi::signal;
use libradix::abi::startup::SLOT_PROCESS;
use manifest::service::ReadyWhen;
use radix_rt::{SYS, println};
use spark_api::graph::{self, Link, Node, Peer, Verdict};
use spark_api::ready;

use super::watch::say;
use super::{MS, SECOND, STOP_WAIT, Service, Shutdown, State, Supervisor, now};

/// How often Spark looks whether a starting service is ready.
pub(super) const READY_POLL: u64 = 100 * MS;

/// The kernel's tables of TCP sockets (radix-proc(5)).
const TCP_TABLES: [&str; 2] = ["processes/net/tcp", "processes/net/tcp6"];

/// What made a service wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wanted {
    /// It is enabled, and the machine started.
    Boot,
    /// A `start` or `restart` request named it.
    Request,
}

impl Supervisor {
    /// Works out the stop ranks, and the services that wait for each
    /// other, which never start, from every service's file as last read.
    /// A circle is logged when it is first found.
    pub(super) fn settle_graph(&mut self) {
        let nodes: Vec<Node> = self
            .services
            .iter()
            .map(|service| Node {
                name: &service.spec.name,
                order: service.spec.order,
                links: links_of(service),
            })
            .collect();
        let ranks = graph::stop_ranks(&nodes);
        let circles = graph::circles(&nodes);
        let mut known = Vec::new();
        for (service, rank) in self.services.iter_mut().zip(ranks) {
            service.rank = rank;
            if service.circle.take().is_some() {
                known.push(service.spec.name.clone());
            }
        }
        for circle in circles {
            let names = and_list(&circle);
            if !circle.iter().all(|name| known.contains(name)) {
                println!(
                    "spark: {names} wait for each other, through requires, wants or after: none \
                     of them starts until one of their files is changed"
                );
            }
            for service in self.services.iter_mut().filter(|s| circle.contains(&s.spec.name)) {
                service.circle = Some(format!("it waits for itself, through {names}"));
            }
        }
    }

    /// Why Rescue or essentials leave service `name` off until it is
    /// started by a request, if they do.
    pub(super) fn left_off(&self, name: &str) -> Option<&'static str> {
        if self.essentials && !self.own(name) {
            Some("only the essentials start: it stays off until it is started")
        } else if self.rescue && !super::RESCUE_SERVICES.contains(&name) {
            Some("Rescue leaves it off until it is started")
        } else {
            None
        }
    }

    /// Makes service `index` wanted, and every service it requires or
    /// wants, at any depth, that is stopped; they start as
    /// [`Self::step_pending`] lets them. At boot, a service Rescue or
    /// essentials leave off is not pulled in; a request pulls it in, and
    /// one that was stopped by a request, and tries again one Spark gave
    /// up on.
    pub(super) fn want(&mut self, index: usize, wanted: Wanted) {
        let pending = |state| {
            matches!(state, State::Stopped | State::Failed | State::Done | State::Waiting(_))
        };
        // Spark read every file at boot; later, a file is read again as its
        // service is about to start.
        if wanted == Wanted::Request && pending(self.services[index].state) {
            self.reread(index);
        }
        let service = &mut self.services[index];
        if pending(service.state) {
            service.wanted_by = Some(String::from(match wanted {
                Wanted::Boot => "boot",
                Wanted::Request => "a request",
            }));
            if wanted == Wanted::Request {
                service.failures = 0;
            }
            service.held = false;
            service.set_why(None, None);
            service.set(State::Pending);
        }
        let root = service.spec.name.clone();
        let fresh = wanted == Wanted::Boot;
        for pull in graph::pulled_in(&root, |name| self.links_for_start(name, fresh)) {
            let Some(at) = self.services.iter().position(|s| s.spec.name == pull.name) else {
                // Its verdict says that it has no file.
                continue;
            };
            if wanted == Wanted::Boot && self.left_off(&pull.name).is_some() {
                continue;
            }
            let service = &mut self.services[at];
            if matches!(service.state, State::Stopped | State::Failed) {
                service.wanted_by = Some(format!("{}, which {} it", pull.by, pull.link.key()));
                if wanted == Wanted::Request {
                    service.failures = 0;
                }
                service.held = false;
                service.set_why(None, None);
                service.set(State::Pending);
            }
        }
    }

    /// The links of service `name`, for a start: its file is read again
    /// if it is about to start, unless Spark has just read it (`fresh`),
    /// and read for the first time if Spark has not read it since boot,
    /// as a start request does. None when there is no such service.
    fn links_for_start(&mut self, name: &str, fresh: bool) -> Option<Vec<(String, Link)>> {
        let at = match self.services.iter().position(|s| s.spec.name == name) {
            Some(at) => {
                if !fresh && matches!(self.services[at].state, State::Stopped | State::Failed) {
                    self.reread(at);
                }
                at
            }
            None => self.add_linked(name).ok()?,
        };
        Some(self.services[at].spec.links())
    }

    /// Adds service `name` from its file, which Spark did not read at
    /// boot, with every service it names, at any depth, that Spark has
    /// not read either, so that the graph knows what it waits for and any
    /// circle it is in; returns its index.
    pub(super) fn add_linked(&mut self, name: &str) -> Result<usize, String> {
        let at = self.add(name)?;
        let mut queue = alloc::vec![at];
        while let Some(index) = queue.pop() {
            for (link, _) in self.services[index].spec.links() {
                if self.services.iter().all(|s| s.spec.name != link)
                    && let Ok(added) = self.add(&link)
                {
                    queue.push(added);
                }
            }
        }
        self.settle_graph();
        Ok(at)
    }

    /// Starts each pending service whose links let it, lowest `order`
    /// first, and refuses those that a required service that is off holds
    /// back, until a pass changes nothing more; so a chain of services
    /// that are ready once started goes in one call.
    pub(super) fn step_pending(&mut self) {
        if matches!(self.shutdown, Some(Shutdown::Stopping { .. }))
            || !self.services.iter().any(|s| s.state == State::Pending)
        {
            return;
        }
        let mut queue: Vec<usize> = (0..self.services.len()).collect();
        queue.sort_by(|&a, &b| {
            let (a, b) = (&self.services[a].spec, &self.services[b].spec);
            (a.order, &a.name).cmp(&(b.order, &b.name))
        });
        // Each pass that changes something starts or settles a pending
        // service, which is pending again only through `want`, so at most
        // as many passes as services change anything.
        for _ in 0..=self.services.len() {
            let mut changed = false;
            for &index in &queue {
                if self.services[index].state == State::Pending {
                    changed |= self.try_start(index);
                }
            }
            if !changed {
                return;
            }
        }
    }

    /// Starts pending service `index` if its links let it, or refuses it;
    /// returns whether it is no longer pending.
    fn try_start(&mut self, index: usize) -> bool {
        let service = &self.services[index];
        if let Some(why) = &service.circle {
            self.refuse(index, why.clone(), None);
            return true;
        }
        let links = service.spec.links();
        let links: Vec<(&str, Link)> =
            links.iter().map(|(name, link)| (name.as_str(), *link)).collect();
        match graph::verdict(&links, |name| self.peer(name)) {
            Verdict::Go => {
                self.services[index].waiting_for.clear();
                self.start(index);
                true
            }
            Verdict::Wait(names) => {
                self.services[index].waiting_for = names;
                false
            }
            Verdict::Refuse { why, cause } => {
                self.refuse(index, why, Some(&cause));
                true
            }
        }
    }

    /// Refuses to start pending service `index`, for `why`, which names
    /// service `cause` if it names one, and stops what requires it.
    fn refuse(&mut self, index: usize, why: String, cause: Option<&str>) {
        let service = &mut self.services[index];
        say(&mut self.told, index, format!("not starting {}: {why}", service.spec.name));
        service.waiting_for.clear();
        service.set_why(Some(why), cause);
        service.set(State::Stopped);
        self.stop_dependents(index);
    }

    /// What service `name` is doing, as a service that names it sees it.
    fn peer(&self, name: &str) -> Peer<'static> {
        let Some(service) = self.services.iter().find(|s| s.spec.name == name) else {
            let file = format!("{}/{name}.toml", self.dir);
            return Peer::Off(if self.root.stat(&file).is_ok() {
                "has a service file that does not read"
            } else {
                "has no service file"
            });
        };
        if service.circle.is_some() {
            return Peer::Off("waits for itself");
        }
        match service.state {
            State::Running(_) | State::Done => Peer::Ready,
            State::Pending | State::Starting(_) | State::Waiting(_) | State::Draining(..) => {
                Peer::Coming
            }
            State::Failed => Peer::Off("Spark gave up on"),
            State::Stopped if service.held => Peer::Off("was stopped"),
            State::Stopped => match self.left_off(name) {
                Some(_) if self.essentials => Peer::Off("is not one of the essentials"),
                Some(_) => Peer::Off("Rescue leaves off"),
                None => Peer::Off("is stopped"),
            },
        }
    }

    /// Stops every service that requires service `index`, which is
    /// stopping for good, and those that require them, as a stop request
    /// stops each; returns their names, for a request's answer.
    pub(super) fn stop_dependents(&mut self, index: usize) -> Vec<String> {
        let mut stopped: Vec<String> = Vec::new();
        let mut queue = alloc::vec![index];
        while let Some(at) = queue.pop() {
            let name = self.services[at].spec.name.clone();
            let words = match self.peer(&name) {
                Peer::Off(words) => words,
                // Stopped by a request, or as one that requires another
                // that stopped, and ending.
                Peer::Ready | Peer::Coming if self.services[at].held => "was stopped",
                Peer::Ready | Peer::Coming => "is stopping",
            };
            for other in 0..self.services.len() {
                let service = &mut self.services[other];
                if service.held || !service.spec.requires.contains(&name) {
                    continue;
                }
                let why = format!("it requires {name}, which {words}");
                match service.state {
                    State::Pending | State::Waiting(_) => service.set(State::Stopped),
                    State::Starting(pid) | State::Running(pid) => {
                        say(
                            &mut self.told,
                            other,
                            format!("stopping {}: {why}", service.spec.name),
                        );
                        service.stop_by.get_or_insert(now() + STOP_WAIT);
                        // Its end comes through `ended`, as a stop request's
                        // does.
                        let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal::SIGTERM);
                    }
                    State::Draining(..) => {}
                    State::Stopped | State::Done | State::Failed => continue,
                }
                service.held = true;
                service.set_why(Some(why), Some(name.as_str()));
                stopped.push(service.spec.name.clone());
                queue.push(other);
            }
        }
        stopped
    }

    /// Moves on each starting service: ready once its `ready.when` says,
    /// or stopped as a failed start once its `ready.timeout` has passed.
    pub(super) fn step_ready(&mut self, time: u64) {
        let mut tables: Option<Vec<String>> = None;
        for index in 0..self.services.len() {
            let service = &self.services[index];
            if !matches!(service.state, State::Starting(_)) || service.stop_by.is_some() {
                continue;
            }
            let ready = match &service.spec.ready.when {
                ReadyWhen::Listening(port) => {
                    let ports = match port {
                        Some(port) => alloc::vec![(u32::from(*port), 1)],
                        None => service.spec.listen.clone(),
                    };
                    let tables = tables.get_or_insert_with(|| self.socket_tables());
                    tables.iter().any(|table| ready::listening(table, &ports))
                }
                ReadyWhen::Socket(path) => self.socket_made(path, service.socket_before),
                // A health check that passes says so, through
                // `check_passed`; an end through `schedule`.
                ReadyWhen::Healthy | ReadyWhen::Ended | ReadyWhen::Started => false,
            };
            if ready {
                self.became_ready(index);
            } else if time >= self.services[index].ready_by {
                self.not_ready_in_time(index);
            }
        }
    }

    /// The kernel's TCP socket tables; one that does not read is empty.
    fn socket_tables(&self) -> Vec<String> {
        TCP_TABLES
            .iter()
            .filter_map(|table| self.root.read_file(table).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .collect()
    }

    /// What is at `path` as a service starts, which a run before may have
    /// left: its number and change time, if anything is.
    pub(super) fn socket_there(&self, path: &str) -> Option<(u64, u64)> {
        let stat = self.root.stat(path.trim_start_matches('/')).ok()?;
        Some((stat.id, stat.changed))
    }

    /// Whether `path` is a socket other than what was there at the start,
    /// `before`.
    fn socket_made(&self, path: &str, before: Option<(u64, u64)>) -> bool {
        self.root.stat(path.trim_start_matches('/')).is_ok_and(|stat| {
            stat.kind == FileKind::Socket && Some((stat.id, stat.changed)) != before
        })
    }

    /// Starting service `index` is ready.
    pub(super) fn became_ready(&mut self, index: usize) {
        let service = &mut self.services[index];
        let State::Starting(pid) = service.state else { return };
        let time = now();
        service.ready_ns = Some(time);
        service.set_why(None, None);
        service.set(State::Running(pid));
        if let Some(health) = &service.spec.health {
            service.checks.next = time + u64::from(health.every) * SECOND;
        }
        let how = match &service.spec.ready.when {
            ReadyWhen::Listening(Some(port)) => format!(" (listening on {port})"),
            ReadyWhen::Listening(None) => String::from(" (listening)"),
            ReadyWhen::Socket(path) => format!(" ({path} is there)"),
            ReadyWhen::Healthy => String::from(" (its health check passes)"),
            ReadyWhen::Ended | ReadyWhen::Started => String::new(),
        };
        let after = time.saturating_sub(service.started_ns);
        let line = format!(
            "{} is ready{how} {}.{} s after its start",
            service.spec.name,
            after / SECOND,
            after % SECOND / (100 * MS)
        );
        say(&mut self.told, index, line);
    }

    /// Starting service `index` was not ready in time: it is stopped, and
    /// `schedule` counts its end as a failed start.
    fn not_ready_in_time(&mut self, index: usize) {
        let service = &mut self.services[index];
        let Some(pid) = service.state.pid() else { return };
        let missing = match &service.spec.ready.when {
            ReadyWhen::Listening(Some(port)) => format!("nothing listened on port {port}"),
            ReadyWhen::Listening(None) => String::from("nothing listened on its ports"),
            ReadyWhen::Socket(path) => format!("it made no socket at {path}"),
            ReadyWhen::Healthy => String::from("its health check did not pass"),
            ReadyWhen::Ended | ReadyWhen::Started => String::from("it had not ended"),
        };
        let seconds = service.spec.ready.timeout;
        let line = format!(
            "{} did not get ready in {seconds} s: {missing}; stopping it",
            service.spec.name
        );
        say(&mut self.told, index, line);
        service.set_why(Some(format!("it did not get ready in {seconds} s: {missing}")), None);
        service.stop_by = Some(now() + STOP_WAIT);
        let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal::SIGTERM);
    }
}

/// `service`'s links, by name, for the graph.
fn links_of(service: &Service) -> Vec<(&str, Link)> {
    let spec = &service.spec;
    let requires = spec.requires.iter().map(|name| (name.as_str(), Link::Requires));
    let wants = spec.wants.iter().map(|name| (name.as_str(), Link::Wants));
    let after = spec.after.iter().map(|name| (name.as_str(), Link::After));
    requires.chain(wants).chain(after).collect()
}

/// `names` as a list in words: "a", "a and b", "a, b and c".
pub(super) fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
