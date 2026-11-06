// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! kinit(1), klist(1), kvno(1) and kdestroy(1): Kerberos tickets (ADR
//! 0397) got with a password or a keytab, listed, taken for services,
//! and thrown away, on the `kerberos` crate.
//!
//! Tickets live in your keyring (ADR 0393), each the entry
//! `krb5 CLIENT SERVER`, not in a file under /tmp as on Linux: the
//! keyring is sealed under your password and closes when you sign out,
//! so a ticket left behind is not left readable. kinit replaces the
//! tickets of whoever held them before, as MIT's kinit starts its cache
//! afresh. The file systems' `sec=krb5` takes its service tickets from
//! the same entries.
//!
//! Where to start when kinit fails: its error names what the KDC
//! refused, or which KDC did not answer and why; krb5.conf(5) says where
//! the KDCs are; `keyring list` shows the entries.

use std::ffi::OsString;

use accounts::request::{Answer, Request};
use accounts::wipe;
use kerberos::client::{self, Options, Secret};
use kerberos::config::Config;
use kerberos::creds::{self, Keyring};
use kerberos::messages::{Principal, Ticket};
use kerberos::{keytab, time};

use crate::account::{Input, ask};
use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};

/// Your keyring, through authd.
struct Authd;

/// authd's answer to `request`, or its refusal as Kerberos's error.
fn asked(request: &Request) -> kerberos::Result<String> {
    match ask(request).map_err(|fail| kerberos::Error::Other(fail.message))? {
        Answer::Done(text) => Ok(text),
        Answer::Refused(why) => Err(kerberos::Error::Other(why)),
    }
}

impl Keyring for Authd {
    fn names(&mut self) -> kerberos::Result<Vec<String>> {
        let names = asked(&Request::KeyringList)?;
        Ok(names.split('\t').filter(|name| !name.is_empty()).map(str::to_string).collect())
    }

    fn get(&mut self, name: &str) -> kerberos::Result<Option<Vec<u8>>> {
        crate::keyring::entry(name).map_err(kerberos::Error::Other)
    }

    fn set(&mut self, name: &str, secret: &[u8]) -> kerberos::Result<()> {
        crate::keyring::keep(name, secret).map_err(kerberos::Error::Other)
    }

    fn remove(&mut self, name: &str) -> kerberos::Result<()> {
        asked(&Request::KeyringRemove { name: name.into() }).map(drop)
    }
}

/// A Kerberos error as a failure: its own words, which say what to do.
fn failed(err: kerberos::Error) -> Fail {
    match err {
        kerberos::Error::Other(text) | kerberos::Error::Kdc { text, .. } => fail(text),
        other => fail(other.to_string()),
    }
}

/// How kinit is used.
const KINIT: &str =
    "Usage: kinit [-V] [-l LIFETIME] [-r RENEWABLE] [-f | -F] [-k [-t KEYTAB]] [PRINCIPAL]";

/// kinit(1).
pub fn kinit(args: &mut Args) -> Result<i32, Fail> {
    let config = Config::load().map_err(failed)?;
    let mut options = Options::from(&config);
    let (mut with_keytab, mut keytab_path, mut verbose) = (false, None::<String>, false);
    let mut principal: Option<OsString> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('l') => options.lifetime = lifetime(&args.text()?)?,
            Arg::Short('r') => options.renewable = Some(lifetime(&args.text()?)?),
            Arg::Short('f') => options.forwardable = true,
            Arg::Short('F') => options.forwardable = false,
            Arg::Short('k') => with_keytab = true,
            Arg::Short('t') => keytab_path = Some(args.text()?),
            Arg::Short('V') => verbose = true,
            Arg::Long(long) if long == "help" => {
                println!("{KINIT}");
                return Ok(0);
            }
            Arg::Operand(operand) if principal.is_none() => principal = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    if keytab_path.is_some() && !with_keytab {
        return Err(usage(format!("-t names a keytab for -k\n{KINIT}")));
    }
    let realm = config.default_realm.as_deref();
    let named = principal
        .map(|p| p.into_string().map_err(|_| fail("a principal's name is text".into())))
        .transpose()?;
    let mut keyring = Authd;
    let credential = if with_keytab {
        let path = keytab_path.unwrap_or_else(keytab::path);
        let entries = keytab::read(&path).map_err(failed)?;
        let client = match named {
            Some(name) => Principal::parse(&name, realm).map_err(failed)?,
            None => entries
                .first()
                .map(|e| e.principal.clone())
                .ok_or_else(|| fail(format!("the keytab {path} holds no keys")))?,
        };
        let keys = keytab::keys_for(&entries, &client, &config.enctypes);
        if keys.is_empty() {
            return Err(fail(format!("the keytab {path} holds no key of {client}")));
        }
        client::initial(&config, &client, &Secret::Keys(&keys), &options).map_err(failed)?
    } else {
        let client = match named {
            Some(name) => Principal::parse(&name, realm).map_err(failed)?,
            None => default_client(&mut keyring, realm)?,
        };
        let mut password = Input::new().password(&format!("Password for {client}: "))?;
        let got = client::initial(&config, &client, &Secret::Password(&password), &options);
        wipe(&mut password);
        got.map_err(failed)?
    };
    creds::start(&mut keyring, &credential)
        .map_err(|err| fail(format!("the KDC gave a ticket, but keeping it failed: {err}")))?;
    if verbose {
        println!("Authenticated to Kerberos v5 as {}", credential.client);
    }
    Ok(0)
}

/// A lifetime on kinit's command line.
fn lifetime(text: &str) -> Result<i64, Fail> {
    time::parse_duration(text)
        .filter(|&seconds| seconds > 0)
        .ok_or_else(|| usage(format!("'{text}' is not a lifetime, such as 10h, 7d or 36000")))
}

/// Whom kinit asks for without a name: the keyring's client, else you
/// in the default realm.
fn default_client(keyring: &mut Authd, realm: Option<&str>) -> Result<Principal, Fail> {
    if let Ok(Some(client)) = creds::client(keyring) {
        return Ok(client);
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let name = util::user_name(unsafe { libc::getuid() });
    Principal::parse(&name, realm).map_err(failed)
}

/// How klist is used.
const KLIST: &str = "Usage: klist [-e] [-s]\n       klist -k [-e] [KEYTAB]";

/// klist(1).
pub fn klist(args: &mut Args) -> Result<i32, Fail> {
    let (mut keytab_wanted, mut silent, mut enctypes) = (false, false, false);
    let mut path: Option<String> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('k') => keytab_wanted = true,
            Arg::Short('s') => silent = true,
            Arg::Short('e') => enctypes = true,
            Arg::Long(long) if long == "help" => {
                println!("{KLIST}");
                return Ok(0);
            }
            Arg::Operand(operand) if path.is_none() => {
                path = Some(
                    operand.into_string().map_err(|_| fail("a path that is not text".into()))?,
                );
            }
            other => return Err(args.unknown(&other)),
        }
    }
    if keytab_wanted {
        return list_keytab(&path.unwrap_or_else(keytab::path), enctypes);
    }
    if path.is_some() {
        return Err(usage(format!("tickets are in your keyring, not a file\n{KLIST}")));
    }
    let mut all = creds::all(&mut Authd).map_err(failed)?;
    let (now, _) = time::now();
    if silent {
        let good = all.iter().any(|c| c.server.is_tgs() && c.valid_at(now));
        return Ok(if good { 0 } else { 1 });
    }
    if all.is_empty() {
        return Err(fail("your keyring holds no Kerberos tickets: kinit gets them".into()));
    }
    // The ticket-granting ticket first, then the rest as they started.
    all.sort_by_key(|c| (!c.server.is_tgs(), c.starttime));
    println!("Ticket cache: keyring\nDefault principal: {}\n", all[0].client);
    println!("{:<25}{:<25}Service principal", "Valid starting", "Expires");
    for credential in &all {
        let mut line = credential.listing();
        if !enctypes {
            // The flags' line names the type; without -e only the flags.
            line = line.replace(&format!(", Etype: {}", credential.key.enctype.name()), "");
        }
        let ended = if credential.endtime <= now { "  (expired)" } else { "" };
        println!("{line}{ended}");
    }
    Ok(0)
}

/// `klist -k`: the keys of the keytab at `path`.
fn list_keytab(path: &str, enctypes: bool) -> Result<i32, Fail> {
    let entries = keytab::read(path).map_err(failed)?;
    println!("Keytab name: FILE:{path}\nKVNO Principal\n---- {}", "-".repeat(60));
    for entry in &entries {
        let kind =
            if enctypes { format!(" ({})", entry.key.enctype.name()) } else { String::new() };
        println!("{:>4} {}{kind}", entry.kvno, entry.principal);
    }
    Ok(0)
}

/// How kvno is used.
const KVNO: &str = "Usage: kvno SERVICE...";

/// kvno(1): a ticket for each service, taken with your ticket-granting
/// ticket and kept, and the version of the service's key it is under.
pub fn kvno(args: &mut Args) -> Result<i32, Fail> {
    let mut services = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(long) if long == "help" => {
                println!("{KVNO}");
                return Ok(0);
            }
            Arg::Operand(operand) => services
                .push(operand.into_string().map_err(|_| fail("a service's name is text".into()))?),
            other => return Err(args.unknown(&other)),
        }
    }
    if services.is_empty() {
        return Err(usage(format!("which service?\n{KVNO}")));
    }
    let config = Config::load().map_err(failed)?;
    let mut keyring = Authd;
    let mut status = 0;
    for name in &services {
        let got = Principal::parse(name, config.default_realm.as_deref())
            .and_then(|service| client::ticket_for(&config, &mut keyring, &service));
        match got.and_then(|credential| Ticket::decode(&credential.ticket)) {
            Ok(ticket) => match ticket.enc_part.kvno {
                Some(kvno) => println!("{}: kvno = {kvno}", ticket.server),
                None => println!("{}: no key version", ticket.server),
            },
            Err(err) => {
                eprintln!("kvno: {name}: {err}");
                status = 1;
            }
        }
    }
    Ok(status)
}

/// How kdestroy is used.
const KDESTROY: &str = "Usage: kdestroy [-q]";

/// kdestroy(1): every ticket out of your keyring.
pub fn kdestroy(args: &mut Args) -> Result<i32, Fail> {
    while let Some(arg) = args.next()? {
        match arg {
            // Quiet is all kdestroy is: it says nothing either way.
            Arg::Short('q') => {}
            Arg::Long(long) if long == "help" => {
                println!("{KDESTROY}");
                return Ok(0);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    creds::destroy(&mut Authd).map_err(failed)?;
    Ok(0)
}
