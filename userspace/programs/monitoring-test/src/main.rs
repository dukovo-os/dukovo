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
//! `/shared/monitoring/mail.txt` and `hooks.txt`, a block each that the
//! test counts and reads with `grep`; `ready` is made in that folder when
//! all three listen.
//!
//! - `monitoring-test config [broken]` prints the alerts.toml that names
//!   the receivers (with a wrong password and a refused address when
//!   `broken`).
//! - `monitoring-test ca` prints the test authority's certificate.
//! - `monitoring-test ask REQUEST` sends the line REQUEST to alertd's
//!   socket and prints its answer, as whoever runs it: a user who is not
//!   root sees alertd's own refusal.

mod base64;
mod config;
mod http;
mod records;
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

/// How long a client of the receivers, or alertd, may be silent.
const PATIENCE: Duration = Duration::from_secs(20);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        [] | ["serve"] => serve(),
        ["config"] => print(config::alerts_toml(false).as_bytes()),
        ["config", "broken"] => print(config::alerts_toml(true).as_bytes()),
        ["ca"] => print(tls::authority()),
        ["ask", request] => ask(request),
        _ => Err("use: monitoring-test [serve | config [broken] | ca | ask REQUEST]".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{NAME}: {why}");
            ExitCode::FAILURE
        }
    }
}

/// Writes `bytes` to the standard output.
fn print(bytes: &[u8]) -> Result<(), String> {
    io::stdout().write_all(bytes).map_err(|err| format!("cannot write: {err}"))
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
    stream.set_read_timeout(Some(PATIENCE)).map_err(|err| err.to_string())?;
    writeln!(stream, "{request}").map_err(|err| format!("cannot write to alertd: {err}"))?;
    for line in BufReader::new(stream).lines() {
        println!("{}", line.map_err(|err| format!("cannot read alertd's answer: {err}"))?);
    }
    Ok(())
}
