// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! admind(8): the web console's server, Dukovo Admin (ADR 0514).
//!
//! `admind` listens for HTTPS on the port admin.toml(5) names, 9090 unless
//! it says another, with the certificate certd(8) gives it. It signs
//! people in through authd(8) and keeps their sessions, serves the
//! console's own files from `/system/resources/admin`, and answers the
//! pages' calls under `/api/1`. It holds no secret of anyone's: every
//! password, code, passkey and token is checked by authd, which also
//! decides who may unlock a session and what an unlocked session runs.
//!
//! `admind --worker` is what authd starts for each signed-in session, as
//! the person, without the bypass right, with its link to admind as
//! descriptor 0. It does what the person may do with their own rights.
//!
//! The parts:
//!
//! - [`serve`] listens, checks who may connect, speaks TLS and HTTP, and
//!   ends the sessions whose time is up;
//! - [`routes`] answers each request: the order of its checks, the sign-in
//!   and the session routes, the account's second step;
//! - [`guard`], [`reach`] and [`sessions`] are the security core: whom to
//!   answer, which requests may change anything, and what a session is;
//! - [`limits`] counts what keeps admind usable while anyone on the network
//!   may knock, and [`shared`] holds what the connections share;
//! - [`authd`], [`link`] and [`worker`] are the asking: authd's requests
//!   and the frames between admind and a session's worker;
//! - [`errors`] is the table of error IDs, `ADM-101` and on, each with its
//!   status; [`files`], [`tls`] and [`webauthn`] read the console's files,
//!   the certificate and a browser's passkey JSON.
//!
//! When something is wrong, `log admind` has a line for each start, sign-in,
//! sign-out and refusal, with the browser's address and never a password,
//! code, cookie, token, ticket or challenge. A page that says
//! `ADM-NNN` has the error's ID in [`errors`]; the line says what authd or
//! the worker answered.
//!
//! SIGHUP (`service admind reload`) reads admin.toml and the certificate
//! again, for the connections that come after. SIGTERM ends the sessions
//! and stops. The tests of what the console must refuse are in
//! `lab/tests/web-console`.

mod authd;
mod errors;
mod files;
#[cfg(test)]
mod fixture;
mod guard;
mod limits;
mod link;
mod reach;
mod routes;
mod serve;
mod sessions;
mod shared;
mod tls;
mod webauthn;
mod who;
mod worker;

use crate::args::{Arg, Args};
use crate::util::Fail;

/// Says `text` in the log at syslog level `level`.
fn log(level: u8, text: &str) {
    eprintln!("<{level}>admind: {text}");
}

/// admind(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let help = "Usage: admind [--worker]";
    let Some(arg) = args.next()? else { return serve::run() };
    match arg {
        Arg::Long(long) if long == "help" => {
            println!("{help}");
            Ok(0)
        }
        Arg::Long(long) if long == "worker" => match args.next()? {
            None => worker::run(),
            Some(other) => Err(args.unknown(&other)),
        },
        other => Err(args.unknown(&other)),
    }
}
