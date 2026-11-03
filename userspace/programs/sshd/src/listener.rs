// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sshd as Spark runs it, as root: it makes the host's key the first
//! time, listens on port 22, and starts `sshd --connection` for each
//! client, with the connection as its standard input, so that each
//! connection is a process of its own, as Linux's sshd forks one.
//!
//! At most 30 connections sign in at once, 10 of them from one address,
//! as OpenSSH's MaxStartups and PerSourceMaxStartups keep them, and 100
//! connections are open at most. A connection says on a pipe once
//! someone signed in, and counts as signing in until then.

use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::os::fd::OwnedFd;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::hostkey;

/// SSH's port.
const PORT: u16 = 22;
/// This program, which serves each connection.
const PROGRAM: &str = "/system/commands/sshd";
/// The most connections signing in at once, and from one address.
const SIGNING_IN_MAX: usize = 30;
const SIGNING_IN_PER_ADDRESS: usize = 10;
/// The most connections open at once.
const CONNECTIONS_MAX: usize = 100;

/// The connections open, and those signing in.
#[derive(Default)]
struct Counts {
    open: usize,
    signing_in: usize,
    from: HashMap<IpAddr, usize>,
}

impl Counts {
    /// Counts a connection from `address` as open and signing in, if the
    /// limits let it; otherwise why not.
    fn take(&mut self, address: IpAddr) -> Result<(), String> {
        let from = self.from.get(&address).copied().unwrap_or(0);
        if self.open >= CONNECTIONS_MAX {
            return Err(format!("{CONNECTIONS_MAX} connections are open"));
        }
        if self.signing_in >= SIGNING_IN_MAX {
            return Err(format!("{SIGNING_IN_MAX} connections are signing in"));
        }
        if from >= SIGNING_IN_PER_ADDRESS {
            return Err(format!("{SIGNING_IN_PER_ADDRESS} connections from it are signing in"));
        }
        self.open += 1;
        self.signing_in += 1;
        self.from.insert(address, from + 1);
        Ok(())
    }

    /// A connection from `address` signed in, or ended before it did.
    fn signed_in(&mut self, address: IpAddr) {
        self.signing_in -= 1;
        match self.from.get_mut(&address) {
            Some(1) | None => {
                self.from.remove(&address);
            }
            Some(count) => *count -= 1,
        }
    }

    /// A connection ended.
    fn closed(&mut self) {
        self.open -= 1;
    }
}

type Shared = Arc<Mutex<Counts>>;

fn counts(shared: &Shared) -> std::sync::MutexGuard<'_, Counts> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts `sshd --connection` for `stream`, from `address`, and a thread
/// that hears when someone signed in on it and when it ends.
fn start(stream: TcpStream, address: IpAddr, shared: &Shared) -> Result<(), String> {
    let mut child = Command::new(PROGRAM)
        .arg("--connection")
        .stdin(Stdio::from(OwnedFd::from(stream)))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("cannot start {PROGRAM} --connection: {err}"))?;
    let shared = Arc::clone(shared);
    let notify = child.stdout.take();
    thread::Builder::new()
        .name("connection".into())
        .spawn(move || watch(child, notify, address, &shared))
        .map(drop)
        .map_err(|err| format!("cannot start a thread for a connection: {err}"))
}

/// Waits for the connection `child` to say someone signed in, or to end,
/// and then for it to end, counting each.
fn watch(
    mut child: Child,
    notify: Option<std::process::ChildStdout>,
    address: IpAddr,
    shared: &Shared,
) {
    if let Some(mut notify) = notify {
        // A line once someone signed in; the end of the pipe if the
        // connection ends first.
        let mut byte = [0u8; 1];
        let _ = notify.read(&mut byte);
    }
    counts(shared).signed_in(address);
    let _ = child.wait();
    counts(shared).closed();
}

/// Takes connections from `listener` while the limits let them in.
fn accept(listener: &TcpListener, shared: &Shared) {
    loop {
        let (stream, from) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(err) => {
                eprintln!("sshd: cannot accept a connection: {err}");
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let address = from.ip().to_canonical();
        if let Err(why) = counts(shared).take(address) {
            eprintln!("sshd: refused a connection from {address}: {why}");
            continue;
        }
        if let Err(why) = start(stream, address, shared) {
            eprintln!("sshd: {why}");
            let mut counts = counts(shared);
            counts.signed_in(address);
            counts.closed();
        }
    }
}

/// sshd: makes the host key if there is none, listens on port 22, and
/// serves each connection in a process of its own.
pub fn run() -> ExitCode {
    match hostkey::load_or_make() {
        Ok((key, made)) => {
            let fingerprint = hostkey::fingerprint(&key);
            let what = if made { "made the host key" } else { "the host key is" };
            eprintln!("sshd: {what} {fingerprint} ({})", hostkey::PUBLIC);
        }
        Err(why) => {
            eprintln!("sshd: {why}");
            return ExitCode::FAILURE;
        }
    }
    let ipv4 = match TcpListener::bind(("0.0.0.0", PORT)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("sshd: cannot listen on port {PORT}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let shared = Shared::default();
    if let Ok(ipv6) = TcpListener::bind(("::", PORT)) {
        let shared = Arc::clone(&shared);
        let _ =
            thread::Builder::new().name("listen-ipv6".into()).spawn(move || accept(&ipv6, &shared));
    }
    eprintln!("sshd: listening on port {PORT}");
    accept(&ipv4, &shared);
    ExitCode::SUCCESS
}
