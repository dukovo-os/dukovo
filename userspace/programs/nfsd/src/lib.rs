// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfsd(8)'s parts (ADR 0343), and NFS's protocol, which nfs(1), the
//! client, speaks with them (ADR 0344): XDR ([`xdr`]), NFS version 4's
//! numbers ([`nfs`]) and RPC's records and calls ([`rpc`]).

mod attr;
mod fh;
mod ipc;
mod listener;
pub mod nfs;
mod ops;
pub mod rpc;
mod server;
mod shares;
mod state;
mod sys;
#[cfg(feature = "testing")]
pub mod testing;
#[cfg(test)]
mod tests;
mod worker;
pub mod xdr;

use std::process::ExitCode;

/// `nfsd`: listens on port 2049, as Spark runs it.
pub fn listen() -> ExitCode {
    listener::run()
}

/// `nfsd --worker`: serves one person's calls, as authd starts it.
pub fn work() -> ExitCode {
    worker::run()
}
