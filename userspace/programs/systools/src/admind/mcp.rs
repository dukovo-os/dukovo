// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's MCP server (ADR 0690): the console's actions for the
//! MCP clients an administrator lets in, which the pages call AI
//! clients.
//!
//! A client reads within the areas it was given, and changes nothing by
//! itself: what it asks to change waits for an administrator's yes on
//! the console's page, and runs as that administrator. The parts:
//!
//! - [`offer`] says what a client may be offered of the actions, and how
//!   far its scopes reach. The tools, the page and `admind --read`
//!   ([`read`](super::read)) all ask it, so that they cannot disagree.
//! - [`client`] asks authd whose client a credential is, on every
//!   message, and counts what each client has running.
//! - [`rpc`] reads and writes JSON-RPC, and [`protocol`] says what each
//!   method means in the revisions of MCP that are spoken (ADR 0692).
//! - [`server`] answers a client's bytes, whichever way they came in:
//!   by `/mcp`, or by [`local`], the socket of the clients that connect
//!   over SSH.
//! - [`tools`] are the six tools a client is given, and [`areas`] the
//!   console's areas as they and the page name them.
//! - [`requests`] holds what clients asked to change until an
//!   administrator answers, and [`history`] what the clients did.

pub(crate) mod areas;
pub(crate) mod client;
pub(crate) mod history;
pub(crate) mod local;
pub(crate) mod offer;
pub(crate) mod protocol;
pub(crate) mod requests;
pub(crate) mod rpc;
pub(crate) mod server;
pub(crate) mod tools;

use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use super::log;
use super::shared::{Shared, unix_now};
use crate::util;

/// Reads the history of what the clients did, and opens the socket of
/// those that connect over SSH. Neither keeps admind from starting:
/// without its file the history begins empty, and without the socket
/// the clients over HTTPS are let in all the same. The log says which.
///
/// # Panics
///
/// Outside a `LocalSet`.
pub fn start(shared: &Rc<Shared>) {
    {
        let mut history = shared.client_history.borrow_mut();
        if let Err(err) = history.load(Path::new(history::FILE)) {
            let why = util::strip_os_error(&err);
            log(4, &format!("cannot read {}: {why}: the history begins empty", history::FILE));
        }
        // A request's number is never one the history has had.
        shared.client_requests.borrow_mut().start_after(history.last_request());
    }
    match local::listen(Path::new(local::PATH)) {
        Ok(socket) => {
            tokio::task::spawn_local(local::accepting(Rc::clone(shared), socket));
        }
        Err(err) => {
            let why = util::strip_os_error(&err);
            let lost = "AI clients that connect over SSH are not let in";
            log(3, &format!("cannot listen at {}: {why}: {lost}", local::PATH));
        }
    }
}

/// Writes what the history holds and takes the socket away, as admind
/// stops.
pub fn stop(shared: &Shared) {
    shared.client_history.borrow_mut().flush();
    if let Err(err) = std::fs::remove_file(local::PATH)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        log(4, &format!("cannot remove {}: {}", local::PATH, util::strip_os_error(&err)));
    }
}

/// Lapses the requests that have waited their hour, and has the history
/// and the open pages hear of every request that lapsed since this last
/// looked.
pub fn lapse(shared: &Shared, now: Instant) {
    let lapsed = {
        let mut requests = shared.client_requests.borrow_mut();
        requests.lapse(now);
        requests.take_lapsed()
    };
    if lapsed.is_empty() {
        return;
    }
    let at = unix_now();
    {
        let mut history = shared.client_history.borrow_mut();
        for request in &lapsed {
            history.request_is(request, at);
        }
    }
    shared.changes.send_replace(());
}

/// Has the history and the open pages hear that the request `id` was
/// filed, or that its state changed.
pub fn moved(shared: &Shared, id: u64) {
    let request = shared.client_requests.borrow().get(id).cloned();
    if let Some(request) = request {
        shared.client_history.borrow_mut().request_is(&request, unix_now());
    }
    shared.changes.send_replace(());
}

/// What admind does for the clients every half minute: lapses requests,
/// forgets the counts of clients that are idle, and writes the reads
/// the history has joined since the last time.
pub fn upkeep(shared: &Shared, now: Instant) {
    lapse(shared, now);
    shared.client_calls.borrow_mut().tidy(now);
    shared.client_history.borrow_mut().flush();
}
