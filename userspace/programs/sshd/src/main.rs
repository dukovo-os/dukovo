// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sshd(8): lets people sign in to this computer over SSH, with a key
//! their `~/.ssh/authorized_keys` lists, or a password once
//! sshd_config(5) lets them, each session served as them by authd(8)
//! (ADRs 0347 and 0348): a shell, a command or SFTP.
//!
//! - `sshd`, as Spark runs it once `service enable sshd` turns it on,
//!   reads sshd_config, makes the host's key the first time, listens on
//!   port 22, and starts a process for each connection.
//! - `sshd --connection --settings TEXT` serves one connection, its
//!   standard input, with the settings the listener read.
//! - `sshd -t` checks sshd_config, and `sshd -T` shows what it says.

mod authd;
mod connection;
mod hostkey;
mod listener;
mod pty;
mod session;
mod sys;

use std::process::ExitCode;

use accounts::sshd_config;

const USAGE: &str = "usage: sshd [-t | -T]";

const HELP: &str = "Lets people sign in to this computer over SSH with a key, or a password
once sshd_config(5) lets them. Spark runs it once `service sshd enable`
turns it on: see sshd(8).

  sshd     reads /system/settings/ssh/sshd_config, makes the host key,
           listens on port 22, and starts a process for each connection
  sshd -t  checks sshd_config: says each mistake, and ends with 1 if any
  sshd -T  shows the settings sshd_config makes";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => listener::run(),
        ["--connection", "--settings", settings] => match sshd_config::parse(settings) {
            Ok(settings) => connection::run(settings),
            Err(_) => {
                eprintln!("sshd: the listener's settings do not read");
                ExitCode::FAILURE
            }
        },
        ["-t"] => match sshd_config::read() {
            Ok(_) => ExitCode::SUCCESS,
            Err(why) => {
                eprintln!("{why}");
                ExitCode::FAILURE
            }
        },
        ["-T"] => match sshd_config::read() {
            Ok(settings) => {
                print!("{}", settings.text());
                ExitCode::SUCCESS
            }
            Err(why) => {
                eprintln!("{why}");
                ExitCode::FAILURE
            }
        },
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
