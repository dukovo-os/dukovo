// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfsd(8): shares folders with Linux and other Unix machines over NFS
//! 4.2 (ADR 0341).
//!
//! - `nfsd`, as Spark runs it when NFS is on (`share nfs on`), listens on
//!   port 2049 and serves the shares of shares.toml(5) that name the
//!   networks their clients are on, with AUTH_SYS: a client says who
//!   calls, and is taken at its word.
//! - `nfsd --worker` serves one person's calls, as them, in a view of the
//!   shared folders alone: authd starts one for each person calls come
//!   as, and one as no one for root's.

mod attr;
mod fh;
mod ipc;
mod listener;
mod nfs;
mod ops;
mod rpc;
mod server;
mod shares;
mod state;
mod sys;
#[cfg(test)]
mod tests;
mod worker;
mod xdr;

use std::process::ExitCode;

const USAGE: &str = "usage: nfsd [--worker]";

const HELP: &str = "Shares folders with Linux and other Unix machines over NFS 4.2. Spark runs
it while NFS is on: see share(1).

  nfsd           listens on port 2049
  nfsd --worker  serves one person's calls, which authd starts";

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
            println!("nfsd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
