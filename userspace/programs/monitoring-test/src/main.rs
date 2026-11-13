// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The helper of the `monitoring` boot test: the places alertd(8) sends
//! alerts to, and a way to ask alertd as a user who is not root.
//!
//! `monitoring-test serve`, which is also what it does with no
//! arguments, runs as a service. It listens on 127.0.0.1 for
//!
//! - a mail server on port 587 that offers STARTTLS and takes one
//!   account only after it, as a provider's submission server does;
//! - a webhook receiver on port 8443, in HTTPS, and on port 8080, in plain
//!   HTTP, which answers 403 to a path that begins with `/denied`.
//!
//! The certificate is tls-test's, for 127.0.0.1, from its test authority.
//! Each message and request is written, with its headers, to
//! `/shared/monitoring/mail.txt` and `hooks.txt`, a block each, which
//! `seen` and `wait` read back; `ready` is made in that folder when all
//! three listen.
//!
//! The other commands are what the test types, so that it types little.
//! As root (through elevate):
//!
//! - `monitoring-test setup` makes the receivers a service and starts it,
//!   makes the system trust the test authority, and writes alerts.toml,
//!   which names the receivers, at mode 600; alertd reads it.
//! - `monitoring-test settings good|broken|loose` writes alerts.toml again
//!   and has alertd read it: with a wrong password and an address the
//!   receiver refuses (*broken*), or readable by everyone (*loose*), which
//!   alertd refuses.
//!
//! As anyone:
//!
//! - `monitoring-test seen [--details] PHRASE...` counts, in a line for
//!   each PHRASE, how many times each receiver was sent it, in a
//!   message's body and not its headers; with `--details`, also how each
//!   came (TLS, the account, the bearer token).
//! - `monitoring-test wait PHRASE [SECONDS]` waits until the mail server
//!   has been sent PHRASE, and a few seconds more for the webhooks.
//! - `monitoring-test ask REQUEST` sends the line REQUEST to alertd's
//!   socket and prints its answer, as whoever runs it: a user who is not
//!   root sees alertd's own refusal.

mod base64;
mod config;
mod http;
mod records;
mod seen;
mod setup;
mod smtp;
mod tls;

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use config::{FOLDER, HTTP_PORT, HTTPS_PORT, SMTP_PORT};
use records::Records;

/// The program's name, which what it says starts with.
const NAME: &str = "monitoring-test";

/// The socket alertd answers on.
const SOCKET: &str = "/system/live/alerts/socket";

/// What a wrong command line is told.
const USAGE: &str = "use: monitoring-test [serve | setup | settings good|broken|loose | seen \
     [--details] PHRASE... | wait PHRASE [SECONDS] | ask REQUEST]";

/// How long `wait` waits for a message when the line gives no time: a
/// batch is sent 30 seconds after the change.
const WAIT_SECONDS: u64 = 90;

/// How long a client of the receivers, or alertd, may be silent.
const PATIENCE: Duration = Duration::from_secs(20);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        [] | ["serve"] => serve(),
        ["setup"] => setup::setup(),
        ["settings", which] => setup::Settings::parse(which)
            .ok_or_else(|| format!("settings are good, broken or loose, not {which}"))
            .and_then(setup::settings),
        ["seen", "--details", phrases @ ..] if !phrases.is_empty() => seen(phrases, true),
        ["seen", phrases @ ..] if !phrases.is_empty() => seen(phrases, false),
        ["wait", phrase] => seen::wait(Path::new(FOLDER), phrase, WAIT_SECONDS),
        ["wait", phrase, seconds] => seconds
            .parse()
            .map_err(|_| format!("wait takes a number of seconds, not {seconds}"))
            .and_then(|seconds| seen::wait(Path::new(FOLDER), phrase, seconds)),
        ["ask", request] => ask(request),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{NAME}: {why}");
            ExitCode::FAILURE
        }
    }
}

/// Prints what the receivers were sent each of `phrases` in.
fn seen(phrases: &[&str], details: bool) -> Result<(), String> {
    let lines = seen::report(Path::new(FOLDER), phrases, details)
        .map_err(|err| format!("{FOLDER}: {err}"))?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// Listens on 127.0.0.1 for the three receivers, says when it does, and
/// serves for ever.
fn serve() -> Result<(), String> {
    let tls = tls::server_config()?;
    let records = Records::start(Path::new(FOLDER)).map_err(|err| format!("{FOLDER}: {err}"))?;
    let records = Arc::new(records);
    let mail = {
        let (tls, records) = (tls.clone(), records.clone());
        listen(SMTP_PORT, move |tcp| smtp::serve(tcp, tls.clone(), &records))?
    };
    let https = {
        let (tls, records) = (tls, records.clone());
        listen(HTTPS_PORT, move |tcp| https(tcp, &tls, &records))?
    };
    let plain = {
        let records = records.clone();
        listen(HTTP_PORT, move |mut tcp| {
            tcp.set_read_timeout(Some(PATIENCE))?;
            http::serve(&mut tcp, false, &records)
        })?
    };
    records.ready().map_err(|err| format!("{FOLDER}: {err}"))?;
    println!("{NAME}: mail on {SMTP_PORT}, https on {HTTPS_PORT}, http on {HTTP_PORT}");
    for handle in [mail, https, plain] {
        let _ = handle.join();
    }
    Err("a listener ended".into())
}

/// Listens on `port` of 127.0.0.1 and serves each connection with
/// `handler` on a thread of its own, saying what fails.
fn listen<F>(port: u16, handler: F) -> Result<JoinHandle<()>, String>
where
    F: Fn(TcpStream) -> io::Result<()> + Send + Sync + 'static,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|err| format!("cannot listen on port {port}: {err}"))?;
    let handler = Arc::new(handler);
    Ok(thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let handler = handler.clone();
            thread::spawn(move || {
                if let Err(err) = handler(tcp) {
                    eprintln!("{NAME}: port {port}: {err}");
                }
            });
        }
    }))
}

/// One connection of the HTTPS receiver.
fn https(tcp: TcpStream, tls: &Arc<ServerConfig>, records: &Records) -> io::Result<()> {
    tcp.set_read_timeout(Some(PATIENCE))?;
    let connection = ServerConnection::new(tls.clone()).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(connection, tcp);
    http::serve(&mut stream, true, records)?;
    // The client may read to the end of the connection: it is closed as
    // TLS says.
    stream.conn.send_close_notify();
    stream.flush()
}

/// Sends `request`, a line, to alertd and prints each line it answers
/// with.
fn ask(request: &str) -> Result<(), String> {
    let mut stream =
        UnixStream::connect(SOCKET).map_err(|err| format!("cannot connect to {SOCKET}: {err}"))?;
    // Radix's Unix sockets take no SO_RCVTIMEO: the boot test's own time
    // limit bounds the wait for alertd's answer.
    writeln!(stream, "{request}").map_err(|err| format!("cannot write to alertd: {err}"))?;
    for line in BufReader::new(stream).lines() {
        println!("{}", line.map_err(|err| format!("cannot read alertd's answer: {err}"))?);
    }
    Ok(())
}
