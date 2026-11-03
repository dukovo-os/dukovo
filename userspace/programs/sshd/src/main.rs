// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sshd(8): lets people sign in to this computer over SSH, with a key
//! their `~/.ssh/authorized_keys` lists, each session served as them by
//! authd(8) (ADR 0347).
//!
//! - `sshd`, as Spark runs it once `service enable sshd` turns it on,
//!   makes the host's key the first time, listens on port 22, and starts
//!   a process for each connection.
//! - `sshd --connection` serves one connection, its standard input.

mod authd;
mod connection;
mod hostkey;
mod listener;
mod pty;
mod session;
mod sys;

use std::process::ExitCode;

const USAGE: &str = "usage: sshd [--connection]";

const HELP: &str = "Lets people sign in to this computer over SSH with a key. Spark runs it
once `service enable sshd` turns it on: see sshd(8).

  sshd               makes the host key, listens on port 22, and starts a
                     process for each connection
  sshd --connection  serves the connection on standard input";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => listener::run(),
        ["--connection"] => connection::run(),
        ["--help" | "-h"] => {
            println!("{USAGE}\n\n{HELP}");
            ExitCode::SUCCESS
        }
        ["--version"] => {
            println!("sshd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
