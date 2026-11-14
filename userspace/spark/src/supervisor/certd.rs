// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A changed certificate (ADR 0512): `certificate NAME`, which certd asks
//! through a socket of its own once it has made or changed one, and root
//! through the control socket, as `cert` does. Spark copies the
//! certificate anew for each running service that names it, and has each
//! read it again with SIGHUP.
//!
//! certd's socket, `/system/live/spark/certd/socket`, answers that one
//! request, from certd's own process alone: certd does not hold the
//! control socket, which could stop any service or the machine. The
//! socket's folder is in certd's view, read-only; one of its connections
//! is served at a time, beside the control socket's.
//!
//! SIGHUPs for certificates come RELOAD_SPACING apart to a service at
//! the most, and none sooner after its start: a change within that time
//! is copied at once, and the one SIGHUP for all such changes goes when
//! the time is up, once the service is ready.
//!
//! The `certificates` boot test covers this file.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::process;
use libradix::abi::signal;
use libradix::abi::startup::SLOT_PROCESS;
use manifest::service::{self, CERTD};
use radix_rt::{SYS, println};

use super::starting::and_list;
use super::{SECOND, State, Supervisor, account, now};
use crate::certificates;

/// The folder of certd's socket, beneath the root.
pub(super) const SOCKET_DIR: &str = "system/live/spark/certd";

/// certd's socket, beneath the root.
pub(super) const SOCKET: &str = "system/live/spark/certd/socket";

/// The least time between two SIGHUPs Spark sends a service for its
/// certificates, and between its start and the first.
pub(super) const RELOAD_SPACING: u64 = 10 * SECOND;

impl Supervisor {
    /// The answer to `request`, from process `peer`, at certd's socket.
    pub(super) fn answer_certd(&mut self, request: &str, peer: u64) -> String {
        let certd = self.services.iter().find(|s| s.spec.name == CERTD);
        if peer == 0 || certd.and_then(|s| s.state.pid()) != Some(peer) {
            let asked = request.escape_debug();
            audit!("refused `{asked}` at certd's socket from process {peer}, which is not certd");
            return String::from("error: only certd asks through this socket\n");
        }
        let mut words = request.split_whitespace();
        match words.next() {
            Some("certificate") => {
                let rest: Vec<&str> = words.collect();
                self.answer_certificate(&rest, peer)
            }
            _ => String::from("error: certd's socket answers `certificate NAME` alone\n"),
        }
    }

    /// The answer to `certificate NAME` from process `peer`: certificate
    /// NAME changed, so Spark copies it anew for each running service
    /// that names it, and has each read it again.
    pub(super) fn answer_certificate(&mut self, rest: &[&str], peer: u64) -> String {
        let name = match rest {
            [name] => *name,
            _ => return String::from("error: certificate takes a certificate's name\n"),
        };
        if let Err(err) = service::check_certificate(name) {
            return format!("error: {err}\n");
        }
        let time = now();
        let mut copied: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for index in 0..self.services.len() {
            let service = &self.services[index];
            if service.state.pid().is_none() || !service.spec.certificates.iter().any(|n| n == name)
            {
                continue;
            }
            let spec = &service.spec;
            let account = match &spec.account {
                Some(account_name) => match account(&self.root, self.names, account_name) {
                    Ok(account) => Some(account),
                    Err(err) => {
                        failed.push(format!("{}: {err}", spec.name));
                        continue;
                    }
                },
                None => None,
            };
            // A copy that fails leaves the one before in place.
            match certificates::copy(&self.root, &spec.name, name, account.as_ref()) {
                Ok(()) => {
                    copied.push(spec.name.clone());
                    self.reload_certificates(index, time);
                }
                Err(err) => failed.push(format!("{}: {err}", spec.name)),
            }
        }
        let services = if copied.is_empty() { String::from("none") } else { and_list(&copied) };
        audit!("certificate {name} changed: copied for {services}, by process {peer}");
        for failure in &failed {
            println!("spark: certificate {name} was not copied for {failure}");
        }
        if failed.is_empty() {
            let count = copied.len();
            let plural = if count == 1 { "" } else { "s" };
            format!("ok: {name} copied for {count} service{plural}\n")
        } else {
            format!("error: {name} was not copied for {}\n", failed.join("; "))
        }
    }

    /// Has service `index` read its certificates again, at `time`: now,
    /// if RELOAD_SPACING has passed since it last did or started and it
    /// is ready, and else once it has.
    fn reload_certificates(&mut self, index: usize, time: u64) {
        let service = &mut self.services[index];
        service.reload_due = true;
        if let State::Running(pid) = service.state
            && time >= service.reloaded_ns + RELOAD_SPACING
        {
            let _ = process::kill(&SYS, SLOT_PROCESS, pid as i64, signal::SIGHUP);
            service.reloaded_ns = time;
            service.reload_due = false;
        }
    }

    /// Sends the SIGHUPs for certificates that waited for RELOAD_SPACING,
    /// as of `time`.
    pub(super) fn step_reloads(&mut self, time: u64) {
        for index in 0..self.services.len() {
            if self.services[index].reload_due {
                self.reload_certificates(index, time);
            }
        }
    }
}
