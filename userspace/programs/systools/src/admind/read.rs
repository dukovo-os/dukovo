// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admind --read AREAS ACTION ARGUMENTS`: the one program authd starts
//! as an administrator for an MCP client (ADR 0691).
//!
//! admind is confined and asks authd for everything it does as an
//! administrator. For a session or an API token it sends the command to
//! run, and authd runs it, because a person unlocked the session or made
//! the token to change things. An MCP client was let in to read, so authd
//! takes no command for it: it starts this program, with the areas the
//! client was given whole (from the client's own record), and the action
//! and the arguments admind sent. This program finds the action in the
//! console's registry, and runs its command only when it is a read that
//! runs as an administrator, that a client may be offered
//! ([`offer`](super::mcp::offer)), in one of those areas. It then becomes
//! that command, so the read's output and its end are the command's own.
//!
//! So an admind that went wrong, holding a client's token, reads what the
//! client may read and no more. What a client asks to change never comes
//! this way: it runs by an administrator's own unlocked session.
//!
//! A refusal is one line on the error output, `ADM-NNN: words`, and the
//! status 125, which no read of the console's ends with.

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;

use serde_json::{Value, json};

use super::actions::{self, Registered};
use super::errors::{Failure, Kind};
use super::mcp::offer::{Offer, Reach};
use crate::args::Args;
use crate::elevate;
use crate::util::{self, Fail};

/// The status of a refusal.
pub const REFUSED: i32 = 125;

/// The command of the read `action` with `arguments`, its JSON, for a
/// client that was given `areas` whole.
///
/// # Errors
///
/// ADM-144 when the action is not a read that runs as an administrator
/// and that a client is offered, or is not in `areas`; ADM-108 or ADM-125
/// when the arguments are not the action's.
fn command(
    actions: &'static [&'static dyn Registered],
    areas: &str,
    action: &str,
    arguments: &[u8],
) -> Result<Vec<String>, Failure> {
    let may_not = |words: String| Failure::new(Kind::ClientMayNot, words);
    // authd sends the areas a client was given whole. A scope that gives
    // an area's reading alone gives nothing here.
    let reach = Reach::parse(areas)
        .filter(|_| !areas.contains(':'))
        .ok_or_else(|| may_not("the areas are not ones an AI client is given whole".into()))?;
    let found = actions
        .iter()
        .copied()
        .find(|registered| registered.id() == action)
        .ok_or_else(|| may_not(format!("the console has no action named {action}")))?;
    if reach.allows(found)? != Offer::ReadAll {
        return Err(may_not(format!("{action} is not a read that runs as an administrator")));
    }
    let arguments: Value = if arguments.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(arguments)
            .map_err(|_| Failure::new(Kind::BadRequest, "the arguments are not JSON"))?
    };
    let prepared = found.prepare(arguments)?;
    if prepared.input.is_some() {
        return Err(may_not(format!("{action} reads its input, which no AI client gives")));
    }
    Ok(prepared.argv)
}

/// Where the program `name` of an action's command is, among the
/// system's and the apps' commands.
fn program(name: &str) -> Option<Vec<u8>> {
    let path = elevate::resolve(name.as_bytes(), b"/")?;
    let text = String::from_utf8_lossy(&path);
    let plain = text.split('/').skip(1).all(|part| !matches!(part, "" | "." | ".."));
    let ours = elevate::PATH
        .split(':')
        .any(|folder| text.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/')));
    (plain && ours).then_some(path)
}

/// Says the refusal `failure` as authd's client reads it.
fn refuse(failure: &Failure) -> i32 {
    eprintln!("{}: {}", failure.kind.id(), failure.detail);
    REFUSED
}

/// `admind --read`.
///
/// # Errors
///
/// When the program of the action's command cannot be run.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let words = args.rest();
    let [areas, action, arguments] = words.as_slice() else {
        return Ok(refuse(&Failure::new(
            Kind::BadRequest,
            "admind --read takes the areas, the action and its arguments",
        )));
    };
    if util::own_user() != 0 {
        let why = "admind --read is authd's to start, for an AI client's read";
        return Ok(refuse(&Failure::new(Kind::ClientMayNot, why)));
    }
    let text = |word: &OsString| word.to_string_lossy().into_owned();
    let (areas, action) = (text(areas), text(action));
    let argv = match command(actions::offered(), &areas, &action, arguments.as_bytes()) {
        Ok(argv) => argv,
        Err(failure) => return Ok(refuse(&failure)),
    };
    let Some(path) = argv.first().and_then(|name| program(name)) else {
        let name = argv.first().map_or("", String::as_str);
        let failure = Failure::new(Kind::NoProgram, format!("{name}{}", super::errors::NOT_FOUND));
        return Ok(refuse(&failure));
    };
    let c = |bytes: &[u8]| CString::new(bytes).unwrap_or_default();
    let path = c(&path);
    let strings: Vec<CString> = argv.iter().map(|word| c(word.as_bytes())).collect();
    let mut pointers: Vec<*const libc::c_char> = strings.iter().map(|s| s.as_ptr()).collect();
    pointers.push(std::ptr::null());
    // SAFETY: execv takes NUL-terminated strings and a null-ended list,
    // which live until it returns, if it does.
    unsafe { libc::execv(path.as_ptr(), pointers.as_ptr()) };
    let err = io::Error::last_os_error();
    let code = if err.kind() == io::ErrorKind::NotFound { 127 } else { 126 };
    Err(util::fail_with(code, format!("{}: {}", argv[0], util::strip_os_error(&err))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::mcp::offer::offer;

    fn refused(areas: &str, action: &str, arguments: &[u8]) -> Failure {
        command(actions::offered(), areas, action, arguments).expect_err(action)
    }

    #[test]
    fn a_read_that_runs_as_an_administrator_runs_within_the_areas_given_whole() {
        let argv = command(actions::offered(), "logs,firewall", "firewall.status", b"");
        assert_eq!(argv.ok().and_then(|argv| argv.first().cloned()).as_deref(), Some("firewall"));
        let argv = command(actions::offered(), "*", "firewall.status", b"{}").expect("a command");
        assert_eq!(argv.first().map(String::as_str), Some("firewall"));
        let outside = refused("logs", "firewall.status", b"");
        assert_eq!(outside.kind, Kind::ClientMayNot);
        assert!(outside.detail.contains("may not read firewall"), "{}", outside.detail);
        assert_eq!(refused("", "firewall.status", b"").kind, Kind::ClientMayNot);
    }

    #[test]
    fn nothing_else_runs() {
        // A change, a read that runs as the person, an action with a
        // secret, and a name that is no action's.
        for action in ["service.restart", "service.list", "backups.key", "accounts.add", "no.such"]
        {
            let failure = refused("*", action, b"{}");
            assert_eq!(failure.kind, Kind::ClientMayNot, "{action}: {}", failure.detail);
        }
        // An area's reading alone is never passed on, and gives nothing if
        // it is.
        for areas in ["firewall:read", "*:read", "firewall,logs:read", "Firewall"] {
            let failure = refused(areas, "firewall.status", b"");
            assert_eq!(failure.kind, Kind::ClientMayNot, "{areas}");
            assert!(failure.detail.contains("given whole"), "{}", failure.detail);
        }
    }

    #[test]
    fn the_arguments_are_the_actions_own() {
        assert_eq!(refused("*", "firewall.status", b"{").kind, Kind::BadRequest);
        assert_eq!(refused("*", "firewall.status", b"[1]").kind, Kind::BadRequest);
        assert_eq!(refused("*", "firewall.status", br#"{"all":true}"#).kind, Kind::BadRequest);
        // A read with an argument takes it, and not another's.
        let argv = command(actions::offered(), "*", "storage.health", br#"{"disk":"nvme0"}"#);
        assert!(argv.is_ok_and(|argv| argv.iter().any(|word| word == "nvme0")));
        assert_eq!(refused("*", "storage.health", b"").kind, Kind::BadRequest);
    }

    #[test]
    fn it_runs_exactly_the_reads_a_client_is_offered_as_an_administrator() {
        let mut ran = 0;
        for action in actions::offered() {
            let offered = offer(*action) == Ok(Offer::ReadAll);
            let (examples, _, _) = action.example_values();
            let arguments = examples.first().cloned().unwrap_or_else(|| json!({}));
            let made =
                command(actions::offered(), "*", action.id(), arguments.to_string().as_bytes());
            match made {
                Ok(argv) => {
                    assert!(offered, "{} ran, and is not such a read", action.id());
                    assert!(!argv.is_empty());
                    ran += 1;
                }
                Err(failure) if offered => {
                    panic!("{} did not run with its own example: {}", action.id(), failure.detail)
                }
                Err(failure) => assert_eq!(failure.kind, Kind::ClientMayNot, "{}", action.id()),
            }
        }
        assert!(ran >= 20, "{ran}");
    }

    #[test]
    fn a_program_is_one_of_the_systems_or_the_apps_commands() {
        // Nothing is at these paths where the tests run, so each is None;
        // what this holds is that a name with a folder in it is never
        // taken as it stands.
        for name in ["/usr/bin/env", "../commands/log", "./log", "/system/commands/../x"] {
            assert_eq!(program(name), None, "{name}");
        }
    }
}
