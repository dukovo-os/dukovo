// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! keyring(1): your keyring, which authd keeps (ADR 0393): its entries
//! set, shown, listed and removed, and the keyring unlocked or locked.
//!
//! authd opens the keyring when you sign in with your password and holds
//! it open while you are signed in. A session that started without the
//! password, as one over SSH with a key, opens it with `keyring unlock`.
//! A secret is typed unseen at a terminal, or comes whole on the standard
//! input, as from a pipe, without its last newline. `keyring get` writes
//! the secret alone, with a newline only when the output is a terminal,
//! so that a script gets it as it was kept.
//!
//! Each error is authd's answer, which says what to do. authd's own log
//! says when a keyring opens and closes, and why one did not open.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Read, Write};

use accounts::request::{self, Answer, Request};
use accounts::wipe;

use crate::account::ask;
use crate::args::{Arg, Args};
use crate::prompt;
use crate::util::{self, Fail, fail, usage};

/// How it is used.
const HELP: &str = "Usage: keyring set NAME\n       \
                    keyring get NAME\n       \
                    keyring list\n       \
                    keyring remove NAME\n       \
                    keyring unlock\n       \
                    keyring lock";

/// keyring(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(long)) if long == "help" => {
                println!("{HELP}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => Err(usage(format!("which? set, get, list, remove, unlock or lock\n{HELP}"))),
        };
    };
    match subcommand.to_str() {
        Some("set") => set(&name(args)?),
        Some("get") => get(&name(args)?),
        Some("list") => none(args).and_then(|()| list()),
        Some("remove") => remove(&name(args)?),
        Some("unlock") => none(args).and_then(|()| unlock()),
        Some("lock") => none(args).and_then(|()| lock()),
        _ => Err(usage(format!("no such command: {}\n{HELP}", subcommand.to_string_lossy()))),
    }
}

/// The one operand, an entry's name.
fn name(args: &mut Args) -> Result<String, Fail> {
    let mut name: Option<OsString> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let name = name.ok_or_else(|| usage(format!("which entry?\n{HELP}")))?;
    let name = name.into_string().map_err(|_| fail("an entry's name is text".into()))?;
    if !request::keyring_name(&name) {
        return Err(fail(format!(
            "an entry's name is 1 to {} bytes of text, with no control characters",
            request::KEYRING_NAME_MAX
        )));
    }
    Ok(name)
}

/// No operands.
fn none(args: &mut Args) -> Result<(), Fail> {
    match args.next()? {
        Some(other) => Err(args.unknown(&other)),
        None => Ok(()),
    }
}

/// What authd said, or its refusal as the failure.
fn answered(answer: Answer) -> Result<String, Fail> {
    match answer {
        Answer::Done(text) => Ok(text),
        Answer::Refused(why) => Err(fail(why)),
    }
}

/// `keyring set NAME`: keeps a secret, typed or from the standard input.
fn set(name: &str) -> Result<i32, Fail> {
    let mut secret = if prompt::interactive() {
        match prompt::ask(&format!("Secret for {name}: "), false) {
            Ok(prompt::Answer::Line(line)) => line,
            Ok(prompt::Answer::Stopped) => return Err(fail("stopped; nothing kept".into())),
            Err(err) => return Err(util::failed(&"the terminal", err)),
        }
    } else {
        read_secret()?
    };
    if secret.len() > request::KEYRING_SECRET_MAX {
        wipe(&mut secret);
        return Err(fail(format!(
            "a secret is at most {} bytes; nothing kept",
            request::KEYRING_SECRET_MAX
        )));
    }
    let answer = ask(&Request::KeyringSet { name: name.into(), secret });
    answered(answer?)?;
    Ok(0)
}

/// The secret on the standard input, whole, without its last newline.
fn read_secret() -> Result<Vec<u8>, Fail> {
    let mut secret = Vec::new();
    // One byte past the most, to tell a secret too long.
    let limit = request::KEYRING_SECRET_MAX as u64 + 1;
    if let Err(err) = io::stdin().lock().take(limit).read_to_end(&mut secret) {
        wipe(&mut secret);
        return Err(util::failed(&"standard input", err));
    }
    if secret.ends_with(b"\n") {
        secret.pop();
    }
    Ok(secret)
}

/// `keyring get NAME`: writes the secret.
fn get(name: &str) -> Result<i32, Fail> {
    let hexed = answered(ask(&Request::KeyringGet { name: name.into() })?)?;
    let mut secret =
        request::unhex(&hexed).ok_or_else(|| fail(format!("authd answered `{hexed}`")))?;
    let mut out = io::stdout().lock();
    if out.is_terminal() {
        secret.push(b'\n');
    }
    let written = out.write_all(&secret).and_then(|()| out.flush());
    wipe(&mut secret);
    written.map_err(|err| util::failed(&"standard output", err))?;
    Ok(0)
}

/// `keyring list`: the entries' names, a line each.
fn list() -> Result<i32, Fail> {
    let names = answered(ask(&Request::KeyringList)?)?;
    for name in names.split('\t').filter(|name| !name.is_empty()) {
        println!("{name}");
    }
    Ok(0)
}

/// `keyring remove NAME`.
fn remove(name: &str) -> Result<i32, Fail> {
    answered(ask(&Request::KeyringRemove { name: name.into() })?)?;
    Ok(0)
}

/// `keyring unlock`: opens the keyring with your password, typed unseen,
/// or as a line of the standard input.
fn unlock() -> Result<i32, Fail> {
    let password = crate::account::Input::new().password("Password: ")?;
    answered(ask(&Request::KeyringUnlock { password })?)?;
    Ok(0)
}

/// `keyring lock`: closes the keyring until you unlock it or sign in
/// with your password again.
fn lock() -> Result<i32, Fail> {
    answered(ask(&Request::KeyringLock)?)?;
    Ok(0)
}
