// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! logforward(8): sends the system's log to syslog servers (ADR 0482), as
//! RFC 5424 messages over TLS (RFC 5425), TCP (RFC 6587's octet counting)
//! or UDP (RFC 5426), for what netlog(8), which sends the kernel's
//! records over UDP as they come, is not for: everything after the boot's
//! first seconds, kept until a server has it.
//!
//! The log's own day files, which logd(8) appends to, are the queue. Each
//! destination of `/system/settings/logforward.toml` (logforward.toml(5))
//! has a thread of its own, so that a server that is down holds up no
//! other, and a place in the log kept on the disk across restarts and
//! boots. The service never writes the log and never needs logd: it reads
//! the files, so that no reply of a server's TLS stack reaches the one
//! process that holds the whole log.
//!
//! - [`settings`] reads the destinations and says which records each is
//!   sent;
//! - [`message`] makes the RFC 5424 message of a record;
//! - [`reader`] reads the day files from a place in them;
//! - [`connection`] is the connection to a server, and [`position`] says
//!   what of the log counts as delivered;
//! - [`forward`] is the thread of one destination, which writes what
//!   [`status`] reads for `logforward status`.
//!
//! When it misbehaves, `logforward status` says where each destination
//! stands and why, `logforward test` sends a test record and says what the
//! TLS handshake found, `log logforward` shows what the service said, and
//! the `log-forward` boot test covers it against TLS, TCP and UDP servers.

mod connection;
mod forward;
mod message;
mod position;
mod reader;
mod settings;
mod status;

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use connection::{Connection, Transport};
use forward::Places;
use settings::Destination;

use crate::args::{Arg, Args};
use crate::logd;
use crate::util::{self, Fail, fail, usage};

/// How long a stopped service waits for its threads to save their
/// positions before it ends without them.
const STOP_WAIT: Duration = Duration::from_secs(5);
/// How often the service looks at whether a signal came.
const WATCH: Duration = Duration::from_millis(100);
/// How long `logforward test` waits to see whether a server that took the
/// test record closed the connection: a TLS 1.3 server tells a client
/// certificate it refuses after the handshake is done.
const TEST_WAIT: Duration = Duration::from_millis(250);

static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_hangup(_: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
}

/// logforward(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let help =
        "Usage: logforward run\n       logforward status [NAME]\n       logforward test [NAME]";
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(l)) if l == "help" => {
                println!("{help}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => Err(usage(format!("which? run, status or test\n{help}"))),
        };
    };
    match subcommand.to_str() {
        Some("run") => run(args),
        Some("status") => status(args),
        Some("test") => test(args),
        _ => Err(usage(format!("no such command: {}\n{help}", subcommand.to_string_lossy()))),
    }
}

/// The destination name after `status` or `test`, if there is one.
fn name_operand(args: &mut Args) -> Result<Option<String>, Fail> {
    let mut name = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => {
                let text = operand.into_string();
                name = Some(text.map_err(|_| {
                    usage("a destination's name is letters, digits, - and _".into())
                })?);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(name)
}

/// `logforward run`: the service.
fn run(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    for (signal, handler) in
        [(libc::SIGTERM, on_term as extern "C" fn(libc::c_int)), (libc::SIGHUP, on_hangup)]
    {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers only store to an atomic.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
    let places = Places::system();
    while !STOP.load(Ordering::Relaxed) {
        RELOAD.store(false, Ordering::Relaxed);
        let destinations = read_destinations();
        let names: Vec<&str> = destinations.iter().map(|d| d.name.as_str()).collect();
        status::remove_others(&places.status, &names);
        if destinations.is_empty() {
            println!(
                "logforward: no destinations in {}; waiting for service reload",
                settings::FILE
            );
            wait_for_signal();
        } else {
            serve(&destinations, &places);
        }
    }
    Ok(0)
}

/// The destinations of the settings file, after saying what is wrong in
/// it.
fn read_destinations() -> Vec<Destination> {
    let text = match fs::read_to_string(settings::FILE) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            eprintln!("<3>logforward: {}: {}", settings::FILE, util::strip_os_error(&error));
            String::new()
        }
    };
    let (destinations, wrong) = settings::read_settings(&text);
    for wrong in wrong {
        eprintln!("<3>logforward: {wrong}");
    }
    destinations
}

fn wait_for_signal() {
    while !STOP.load(Ordering::Relaxed) && !RELOAD.load(Ordering::Relaxed) {
        thread::sleep(WATCH);
    }
}

/// Runs a thread for each destination until SIGTERM or SIGHUP, then has
/// them save their positions and end. After SIGTERM it waits for them
/// [`STOP_WAIT`] at most: a thread that is in the middle of connecting
/// to a server that does not answer has nothing to save.
fn serve(destinations: &[Destination], places: &Places) {
    let stop = Arc::new(AtomicBool::new(false));
    let (ended, ends) = mpsc::channel();
    let mut running = 0;
    for destination in destinations {
        let name = destination.name.clone();
        let (destination, places) = (destination.clone(), places.clone());
        let (stop, ended) = (stop.clone(), ended.clone());
        let started = thread::Builder::new().name(format!("forward-{name}")).spawn(move || {
            forward::run(&destination, &places, &stop);
            // Nobody is left to tell once the service itself has ended.
            let _ = ended.send(());
        });
        match started {
            Ok(_) => running += 1,
            Err(error) => eprintln!(
                "<3>logforward: {name}: cannot start its thread: {}",
                util::strip_os_error(&error)
            ),
        }
    }
    wait_for_signal();
    stop.store(true, Ordering::Relaxed);
    let mut give_up = None;
    while running > 0 {
        if STOP.load(Ordering::Relaxed) {
            give_up.get_or_insert_with(|| Instant::now() + STOP_WAIT);
        }
        if give_up.is_some_and(|at| Instant::now() >= at) {
            return;
        }
        if ends.recv_timeout(WATCH).is_ok() {
            running -= 1;
        }
    }
}

/// `logforward status [NAME]`: where each destination stands.
fn status(args: &mut Args) -> Result<i32, Fail> {
    let name = name_operand(args)?;
    let mut all = status::read_all(Path::new(status::DIRECTORY));
    if let Some(name) = &name {
        if !all.iter().any(|status| status.name == *name) {
            return Err(fail(format!("no destination is called {name}")));
        }
        all.retain(|status| status.name == *name);
    }
    if all.is_empty() {
        println!("logforward: no destinations");
    } else {
        print!("{}", status::table(&all));
    }
    Ok(0)
}

/// `logforward test [NAME]`: sends a test record to each destination, or
/// to NAME, over a connection of its own.
fn test(args: &mut Args) -> Result<i32, Fail> {
    let name = name_operand(args)?;
    if util::own_user() != 0 {
        return Err(fail(
            "only root tests a destination, as it reads the client key: elevate first, as \
             `elevate logforward test`"
                .into(),
        ));
    }
    let text = match fs::read_to_string(settings::FILE) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(util::failed(&settings::FILE, error)),
    };
    let (mut destinations, wrong) = settings::read_settings(&text);
    for wrong in &wrong {
        eprintln!("logforward: {wrong}");
    }
    if let Some(name) = &name {
        destinations.retain(|destination| destination.name == *name);
        if destinations.is_empty() {
            return Err(fail(format!("no destination is called {name}")));
        }
    }
    if destinations.is_empty() {
        println!("logforward: no destinations");
    }
    let host = message::host_field(&util::hostname());
    let mut failed = !wrong.is_empty();
    for destination in &destinations {
        match send_test_record(destination, &host) {
            Ok(said) => println!("{}: {said}", destination.name),
            Err(why) => {
                println!("{}: {why}", destination.name);
                failed = true;
            }
        }
    }
    Ok(i32::from(failed))
}

/// Connects to `destination` afresh, sends one test record from machine
/// `host`, and ends the connection: what it found, or why it failed.
fn send_test_record(destination: &Destination, host: &str) -> Result<String, String> {
    let mut connection =
        Connection::connect(&destination.address, destination.transport, destination.tls.as_ref())?;
    let record = message::test_message(host, &destination.name, &logd::rfc3339(util::now_ns()));
    connection.send(record.as_bytes()).and_then(|()| connection.flush()).map_err(|error| {
        format!("cannot send to {}: {}", destination.endpoint, util::strip_os_error(&error))
    })?;
    if destination.transport != Transport::Udp {
        thread::sleep(TEST_WAIT);
        if !connection.open() {
            return Err(format!(
                "{} closed the connection after the test record",
                destination.endpoint
            ));
        }
    }
    let over = match (destination.transport, connection.describe()) {
        (_, Some(found)) => match found.split_once(", ") {
            Some((version, certificate)) => format!("over {version} ({certificate})"),
            None => format!("over {found}"),
        },
        (transport, None) => format!("over {}", transport.name().to_uppercase()),
    };
    connection.close();
    Ok(format!("sent a test record to {} {over}", destination.endpoint))
}
