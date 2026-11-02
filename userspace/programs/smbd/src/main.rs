// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! smbd(8): shares folders with Windows PCs, Macs and Linux machines
//! over SMB 3, signed and encrypted (ADR 0228).
//!
//! - `smbd`, as Spark runs it when sharing is on (`share on`), listens
//!   on port 445 and signs PCs in through authd(8), with each person's
//!   sharing password (`share password`).
//! - `smbd --worker` is a person's share session, which authd starts as
//!   them for each PC that signs in: it serves the shares shares.toml(5)
//!   lists, with their permissions.

mod auth;
mod commands;
mod crypto;
mod fs;
mod info;
mod listener;
mod negotiate;
mod rpc;
mod state;
mod sys;
#[cfg(test)]
mod tests;
mod wire;
mod worker;

use std::process::ExitCode;

const USAGE: &str = "usage: smbd [--worker]";

const HELP: &str = "Shares folders with Windows PCs and other computers over SMB 3, signed and
encrypted. Spark runs it while sharing is on: see share(1).

  smbd           listens on port 445 and signs PCs in
  smbd --worker  a person's share session, which authd starts";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => listener::run(),
        ["--worker"] => worker::run(),
        ["--help" | "-h"] => {
            println!("{USAGE}\n\n{HELP}");
            ExitCode::SUCCESS
        }
        ["--version"] => {
            println!("smbd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
