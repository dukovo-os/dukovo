// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! cert(8): the machine's certificates, which certd(8) keeps (ADR 0512).
//!
//! `cert list`, `cert show` and `cert wanted` read the store as anyone may;
//! `add`, `get`, `remove`, `renew`, `revoke` and `forget` change it, as root,
//! under the store's lock, and tell Spark through its control socket, which
//! copies the certificate again for the services that name it and has them
//! read it. `cert check` is certd's health check.
//!
//! - [`store`] is the folder of certificates and its lock;
//! - [`history`] is what was done to them, which `cert history` shows;
//! - [`accounts`] is what anyone may know of the accounts at authorities;
//! - [`inspect`] reads PEM files and certificates, and checks a chain
//!   against the system's roots;
//! - [`machine`] makes `this-machine` and says when it needs making again;
//! - [`add`] is what `cert add` checks;
//! - [`acme`] gets certificates from an authority, renews and revokes them
//!   (ADR 0513);
//! - [`wanted`] is the certificates that could not be got, which certd asks
//!   for again.
//!
//! When it misbehaves, `cert show NAME` names what is wrong with a
//! certificate's files, `log certd` shows what certd did, `spark`'s own
//! lines say why a service did not get its copy, and the `certificates` boot
//! test covers it all.

pub mod accounts;
pub mod acme;
mod add;
// The certificates the tests of `cert` build on, which the console of a fuzz
// target (`crate::fuzz`) uses too. Outside the tests, that is all they are
// for, and most of the rest goes unused.
#[cfg(any(test, feature = "fuzz"))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod fixtures;
pub mod history;
pub mod inspect;
pub mod machine;
pub mod store;
pub mod wanted;

use std::path::PathBuf;

use serde::Serialize;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};
use crate::{power, servicefiles, tls};
use inspect::{Details, Trust};
use manifest::service::{self, Service};
use store::{Files, Kind, Store, Stored};

/// What a mistake in the command line is told.
const USAGE: &str = "usage: cert list [--json]\n       cert show NAME [--json] [--pem]\n       \
                     cert add NAME --chain FILE --key FILE [--replace] [--untrusted]\n       \
                     cert add NAME --from - [--replace] [--untrusted]\n       \
                     cert get NAME [--name DNS]... [--dns] [--staging] [--dry-run]\n                \
                     [--directory URL] [--email ADDRESS] [--agree-terms]\n                \
                     [--key ecdsa-p256|ecdsa-p384] [--profile PROFILE] [--yes]\n                \
                     [--progress]\n       \
                     cert get NAME [--name DNS]... [--dns] [--staging] [--directory URL]\n                \
                     --check [--json]\n       \
                     cert history [NAME] [-n NUMBER] [--json]\n       \
                     cert revoke NAME [--reason REASON]\n       \
                     cert wanted [--json]\n       cert forget NAME\n       \
                     cert remove NAME\n       cert renew NAME\n       cert check";

/// How many of a certificate's names `cert list` shows.
const NAMES_SHOWN: usize = 2;

/// The seconds since 1970 now.
pub fn now() -> i64 {
    i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(0)
}

/// Asks Spark on its socket `socket` to copy certificate `name` anew for the
/// services that name it: certd uses its own socket, `cert` the control
/// socket.
///
/// # Errors
///
/// With what Spark said when it refused, or why it could not be asked.
pub fn tell_spark(socket: &str, name: &str) -> Result<(), String> {
    let answer =
        power::request_at(socket, &format!("certificate {name}")).map_err(|fail| fail.message)?;
    if answer.starts_with("ok: ") {
        return Ok(());
    }
    let said = answer.trim_end();
    Err(said.strip_prefix("error: ").unwrap_or(said).to_string())
}

/// The words for a change Spark did not take up.
pub fn spark_warning(why: &str) -> String {
    format!(
        "certificates changed, but Spark did not copy them: {why}; restart the services that use it"
    )
}

/// Asks Spark to have certd look now, so that it asks for the certificates
/// that are wanted at the time they say, and its alerts follow what was
/// changed without waiting for its next look.
pub(crate) fn wake_certd() {
    if let Err(failed) = crate::service::ask("reload certd") {
        eprintln!("cert: certd was not asked to look now: {}", failed.message);
    }
}

/// cert(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(help)) if help == "help" => {
                println!("{USAGE}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => Err(usage(USAGE.into())),
        };
    };
    match subcommand.to_str() {
        Some("list") => list(args),
        Some("show") => show(args),
        Some("add") => add_certificate(args),
        Some("get") => get(args),
        Some("history") => history(args),
        Some("wanted") => wanted_certificates(args),
        Some("forget") => forget(args),
        Some("revoke") => revoke(args),
        Some("remove") => remove(args),
        Some("renew") => renew(args),
        Some("check") => check(args),
        _ => Err(usage(format!("no such command: {}\n{USAGE}", subcommand.to_string_lossy()))),
    }
}

/// A certificate as `list` and `show` have read it.
struct Certificate {
    stored: Stored,
    chain: Vec<Details>,
    trust: Trust,
    used_by: Vec<String>,
}

impl Certificate {
    fn first(&self) -> &Details {
        &self.chain[0]
    }
}

/// The services whose files name certificate `name`, with the file of each.
fn users(name: &str, services: &[(String, PathBuf, Service)]) -> Vec<(String, PathBuf)> {
    services
        .iter()
        .filter(|(_, _, file)| file.certificates.iter().any(|named| named == name))
        .map(|(service, path, _)| (service.clone(), path.clone()))
        .collect()
}

/// Reads certificate `name` and checks it against `roots` at time `now`.
fn read_certificate(
    store: &Store,
    name: &str,
    roots: &rustls::RootCertStore,
    services: &[(String, PathBuf, Service)],
    now: i64,
) -> Result<Certificate, String> {
    let stored = store.get(name)?;
    let text = String::from_utf8(stored.chain.clone())
        .map_err(|_| format!("{name}'s fullchain.pem is not text"))?;
    let certificates = inspect::chain_of(&text).map_err(|why| format!("{name}: {why}"))?;
    let chain: Vec<Details> = certificates
        .iter()
        .map(inspect::details)
        .collect::<Result<_, _>>()
        .map_err(|why| format!("{name}: {why}"))?;
    if chain.is_empty() {
        return Err(format!("{name}'s fullchain.pem holds no certificate"));
    }
    let trust = inspect::trust(&certificates, roots, now);
    let used_by = users(name, services).into_iter().map(|(service, _)| service).collect();
    Ok(Certificate { stored, chain, trust, used_by })
}

/// What `list --json` and `show --json` give of a certificate.
#[derive(Serialize)]
struct Summary {
    name: String,
    kind: &'static str,
    names: Vec<String>,
    not_before: String,
    not_after: String,
    days_left: i64,
    fingerprint_sha256: String,
    issuer: String,
    trusted: bool,
    used_by: Vec<String>,
    /// When certd renews a certificate from an authority, once it has said.
    #[serde(skip_serializing_if = "Option::is_none")]
    renews: Option<String>,
    /// Set for a certificate that was revoked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    revoked: bool,
}

fn summary(read: &Certificate, now: i64) -> Summary {
    let first = read.first();
    Summary {
        name: read.stored.name.clone(),
        kind: read.stored.kind.word(),
        names: first.names.clone(),
        not_before: util::time_text(first.not_before, "%FT%TZ", true),
        not_after: util::time_text(first.not_after, "%FT%TZ", true),
        days_left: first.days_left(now),
        fingerprint_sha256: first.fingerprint_hex(),
        issuer: first.issuer.clone(),
        trusted: read.trust == Trust::Trusted,
        used_by: read.used_by.clone(),
        renews: read
            .stored
            .acme
            .as_ref()
            .and_then(|acme| acme.renew_at)
            .map(|at| util::time_text(at, "%FT%TZ", true)),
        revoked: read.stored.acme.as_ref().is_some_and(|acme| acme.revoked),
    }
}

/// What `list --json` gives of a certificate: its summary, and the account
/// at the authority it came from.
#[derive(Serialize)]
struct Listed {
    #[serde(flatten)]
    summary: Summary,
    /// When the account at the certificate's authority was made and whether
    /// it has a contact; none for a certificate no authority issued, or an
    /// authority whose account is not known. Never the address.
    account: Option<accounts::Public>,
}

/// The account `read`'s authority keeps for this machine, from `accounts`.
fn account_of(
    read: &Certificate,
    accounts: &std::collections::BTreeMap<String, accounts::Public>,
) -> Option<accounts::Public> {
    let acme = read.stored.acme.as_ref()?;
    let directory = tuf::url::Url::parse(&acme.directory).ok()?;
    accounts.get(&acme::account_file(&directory).ok()?).cloned()
}

/// Every certificate of `store`, read, or what is wrong with each that
/// does not read.
fn read_all(store: &Store, now: i64) -> Result<Vec<Result<Certificate, String>>, Fail> {
    let names = store.names().map_err(fail)?;
    let roots = tls::roots(&[], "cert");
    let services = servicefiles::read_all();
    Ok(names.iter().map(|name| read_certificate(store, name, &roots, &services, now)).collect())
}

/// `cert list [--json]`.
fn list(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let now = now();
    let mut certificates = Vec::new();
    let mut unreadable = false;
    for found in read_all(&Store::system(), now)? {
        match found {
            Ok(certificate) => certificates.push(certificate),
            Err(why) => {
                eprintln!("cert: {why}");
                unreadable = true;
            }
        }
    }
    if json {
        let accounts = accounts::read(&Store::system());
        let entries: Vec<Listed> = certificates
            .iter()
            .map(|read| Listed {
                summary: summary(read, now),
                account: account_of(read, &accounts),
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).map_err(|err| fail(err.to_string()))?
        );
    } else {
        let colour = util::is_terminal(std::io::stdout()) && std::env::var_os("NO_COLOR").is_none();
        print!("{}", list_table(&certificates, now, colour));
    }
    Ok(i32::from(unreadable))
}

/// The first of a certificate's names that a table shows, and how many are left
/// out.
fn names_cell(names: &[String]) -> String {
    let mut shown: Vec<&str> = names.iter().take(NAMES_SHOWN).map(String::as_str).collect();
    if shown.is_empty() {
        shown.push("-");
    }
    let more = names.len().saturating_sub(NAMES_SHOWN);
    if more > 0 { format!("{} +{more}", shown.join(", ")) } else { shown.join(", ") }
}

/// `list`'s table: a header, then a line a certificate, with the date a
/// certificate ran out in red when `colour` is on.
fn list_table(certificates: &[Certificate], now: i64, colour: bool) -> String {
    let titles = ["NAME", "KIND", "NAMES", "RUNS OUT", "USED BY"];
    let mut rows: Vec<[String; 5]> = vec![titles.map(String::from)];
    for read in certificates {
        let first = read.first();
        let names = names_cell(&first.names);
        let used_by =
            if read.used_by.is_empty() { String::from("-") } else { read.used_by.join(", ") };
        rows.push([
            read.stored.name.clone(),
            read.stored.kind.word().to_string(),
            names,
            runs_out(first, now) + &renewal_note(&read.stored),
            used_by,
        ]);
    }
    let mut widths = [0; 4];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut text = String::new();
    for (number, row) in rows.iter().enumerate() {
        for (column, cell) in row.iter().enumerate() {
            if column == 4 {
                text.push_str(cell);
                continue;
            }
            let padding = " ".repeat(widths[column] - cell.chars().count() + 2);
            if colour && column == 3 && number > 0 && cell.starts_with("ran out") {
                text.push_str(&format!("\x1b[1;31m{cell}\x1b[0m{padding}"));
            } else if colour && column == 3 && number > 0 && cell.ends_with(REVOKED) {
                let (before, word) = cell.split_at(cell.len() - REVOKED.len());
                text.push_str(&format!("{before}\x1b[1;31m{word}\x1b[0m{padding}"));
            } else {
                text.push_str(&format!("{cell}{padding}"));
            }
        }
        text.push('\n');
    }
    text
}

/// What `list` says of a certificate that is shown revoked.
const REVOKED: &str = ", revoked";

/// What `list` adds after when a certificate runs out: when certd renews
/// one from an authority, or that it was revoked.
fn renewal_note(stored: &Stored) -> String {
    match &stored.acme {
        Some(acme) if acme.revoked => REVOKED.to_string(),
        Some(acme) => match acme.renew_at {
            Some(at) => format!(", renews {}", util::local_time(at, "%F")),
            None => String::new(),
        },
        None => String::new(),
    }
}

/// When a certificate runs out, as `list` says it.
fn runs_out(details: &Details, now: i64) -> String {
    let date = util::local_time(details.not_after, "%F");
    if details.not_after <= now {
        return format!("ran out {date}");
    }
    match details.days_left(now) {
        1 => format!("{date} (1 day)"),
        days => format!("{date} ({days} days)"),
    }
}

/// `cert show NAME [--json] [--pem]`.
fn show(args: &mut Args) -> Result<i32, Fail> {
    let (mut json, mut pem, mut name) = (false, false, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            Arg::Long(option) if option == "pem" => pem = true,
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name.and_then(|name| name.into_string().ok()) else {
        return Err(usage("show takes the name of a certificate".into()));
    };
    if json && pem {
        return Err(usage("--json and --pem are two ways to show it: give one".into()));
    }
    service::check_certificate(&name).map_err(fail)?;
    let store = Store::system();
    let now = now();
    if pem {
        let stored = store.get(&name).map_err(fail)?;
        print!("{}", String::from_utf8_lossy(&stored.chain));
        return Ok(0);
    }
    let roots = tls::roots(&[], "cert");
    let found =
        read_certificate(&store, &name, &roots, &servicefiles::read_all(), now).map_err(fail)?;
    let [chain, key] = store.files(&name);
    if json {
        let files = [chain.display().to_string(), key.display().to_string()];
        println!("{}", shown_json(&found, now, files)?);
    } else {
        print!(
            "{}",
            shown_text(&found, now, &chain.display().to_string(), &key.display().to_string())
        );
    }
    Ok(0)
}

#[derive(Serialize)]
struct Shown {
    #[serde(flatten)]
    summary: Summary,
    made: String,
    serial: String,
    key: String,
    chain: Vec<String>,
    /// The chain as PEM, the certificate first: public, as the file is.
    chain_pem: String,
    why_not_trusted: Option<String>,
    files: [String; 2],
}

fn shown_json(read: &Certificate, now: i64, files: [String; 2]) -> Result<String, Fail> {
    let first = read.first();
    let shown = Shown {
        summary: summary(read, now),
        made: util::time_text(read.stored.made, "%FT%TZ", true),
        serial: first.serial.clone(),
        key: first.key.clone(),
        chain: read.chain.iter().map(|details| details.subject.clone()).collect(),
        chain_pem: String::from_utf8_lossy(&read.stored.chain).into_owned(),
        why_not_trusted: read.trust.why_not(),
        files,
    };
    serde_json::to_string_pretty(&shown).map_err(|err| fail(err.to_string()))
}

/// What `show` says: a line for each thing, never the key.
fn shown_text(read: &Certificate, now: i64, chain_file: &str, key_file: &str) -> String {
    let first = read.first();
    let mut lines = vec![
        ("Name", read.stored.name.clone()),
        ("Kind", read.stored.kind.word().to_string()),
        ("Made", util::local_time(read.stored.made, "%F %T")),
        ("Names", first.names.join(", ")),
        ("Subject", first.subject.clone()),
        ("Issuer", first.issuer.clone()),
        ("Not before", util::time_text(first.not_before, "%F %T UTC", true)),
        ("Not after", util::time_text(first.not_after, "%F %T UTC", true)),
        ("Runs out", runs_out(first, now)),
        ("Serial", first.serial.clone()),
        ("SHA-256", first.fingerprint_colons()),
        ("Key", first.key.clone()),
    ];
    for (number, details) in read.chain.iter().enumerate() {
        lines.push(("Chain", format!("{}: {}", number + 1, details.subject)));
    }
    if let Some(acme) = &read.stored.acme {
        lines.push(("Authority", acme.directory.clone()));
        lines.push(("Proved by", acme.challenge.word().to_string()));
        lines.push(("Asked for", format!("{}, {}", acme.names.join(", "), acme.key.word())));
        if acme.revoked {
            lines.push(("Revoked", String::from("yes: get a new one with `cert get`")));
        } else if let Some(at) = acme.renew_at {
            lines.push(("Renews", util::local_time(at, "%F")));
        }
    }
    lines.push((
        "Trusted",
        match read.trust.why_not() {
            None => String::from("yes, by the system's roots"),
            Some(why) => format!("no: {why}"),
        },
    ));
    let used =
        if read.used_by.is_empty() { String::from("no service") } else { read.used_by.join(", ") };
    lines.push(("Used by", used));
    lines.push(("Files", format!("{chain_file}, {key_file}")));
    let mut text = String::new();
    for (label, value) in &lines {
        text.push_str(&format!("{:<11} {value}\n", format!("{label}:")));
    }
    text
}

/// `cert history [NAME] [-n NUMBER] [--json]`: what was done to the
/// certificates, the oldest first and so the newest last, of `NAME` alone
/// when it is given, and the newest NUMBER of them when `-n` says.
fn history(args: &mut Args) -> Result<i32, Fail> {
    let (mut json, mut name, mut limit) = (false, None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            Arg::Short('n') => limit = Some(args.number::<usize>()?),
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let name = match name {
        Some(name) => {
            let name = name
                .into_string()
                .map_err(|_| usage("history takes the name of a certificate".into()))?;
            service::check_certificate(&name).map_err(fail)?;
            Some(name)
        }
        None => None,
    };
    let mut entries = history::read(&Store::system()).map_err(fail)?;
    entries.retain(|entry| name.as_ref().is_none_or(|name| &entry.name == name));
    if let Some(limit) = limit {
        entries.drain(..entries.len().saturating_sub(limit));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).map_err(|err| fail(err.to_string()))?
        );
    } else {
        print!("{}", history_text(&entries));
    }
    Ok(0)
}

/// `history`'s lines: when, the certificate, what happened and the words
/// of it, and who did it, and how long it took when that is known.
fn history_text(entries: &[history::Entry]) -> String {
    let names = entries.iter().map(|entry| entry.name.chars().count()).max().unwrap_or(0);
    let events = entries.iter().map(|entry| entry.event.word().len()).max().unwrap_or(0);
    let mut text = String::new();
    for entry in entries {
        let time = util::local_time(entry.at_ms / 1_000, "%F %T");
        let who = entry.by.word();
        let by = match entry.took_ms {
            None => who.to_string(),
            Some(ms) if ms < 1_000 => format!("{who}, under 1 s"),
            Some(ms) => format!("{who}, {} s", ms / 1_000),
        };
        text.push_str(&format!(
            "{time}  {:<names$}  {:<events$}  {}  ({by})\n",
            entry.name,
            entry.event.word(),
            entry.detail
        ));
    }
    text
}

/// `cert wanted [--json]`: the certificates that could not be got, which
/// certd asks for again, as anyone may read them.
fn wanted_certificates(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let wanted = wanted::read_public(&Store::system()).map_err(fail)?;
    if json {
        let document = wanted::Document { format: wanted::FORMAT, wanted };
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|err| fail(err.to_string()))?
        );
    } else {
        print!("{}", wanted_table(&wanted));
    }
    Ok(0)
}

/// `wanted`'s table: a header, then a line a certificate, which says when the
/// first try was, when the next is or that certd tries no more, and why the
/// last failed.
fn wanted_table(wanted: &[wanted::Public]) -> String {
    if wanted.is_empty() {
        return String::from("no certificate is being tried again\n");
    }
    let time = |ms: i64| util::local_time(ms.div_euclid(1_000), "%F %T");
    let titles = ["NAME", "NAMES", "TRIES", "FIRST TRY", "NEXT TRY", "WHY"];
    let mut rows: Vec<[String; 6]> = vec![titles.map(String::from)];
    for each in wanted {
        let next = if each.given_up { String::from("given up") } else { time(each.next_ms) };
        rows.push([
            each.name.clone(),
            names_cell(&each.names),
            each.tries.to_string(),
            time(each.first_ms),
            next,
            each.why.clone(),
        ]);
    }
    let mut widths = [0; 5];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut text = String::new();
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            text.push_str(cell);
            if let Some(width) = widths.get(column) {
                text.push_str(&" ".repeat(width - cell.chars().count() + 2));
            }
        }
        text.push('\n');
    }
    text
}

/// `cert forget NAME`: certd does not ask for certificate `NAME` again.
fn forget(args: &mut Args) -> Result<i32, Fail> {
    let name = one_name(args, "forget")?;
    need_root()?;
    service::check_certificate(&name).map_err(fail)?;
    let store = Store::system();
    let lock = store.lock().map_err(fail)?;
    if !wanted::remove(&store, &lock, &name).map_err(fail)? {
        return Err(fail(format!(
            "certd is not trying to get {name} again: `cert wanted` lists what it tries"
        )));
    }
    drop(lock);
    println!("{name} forgotten: certd does not try to get it again");
    wake_certd();
    Ok(0)
}

/// Refuses a change a person other than root asks for.
fn need_root() -> Result<(), Fail> {
    if util::own_user() == 0 {
        return Ok(());
    }
    Err(fail("run it with elevate: changing certificates needs root".into()))
}

/// Refuses a change while the clock is wrong, since no certificate can
/// be judged by it.
fn need_clock(now: i64) -> Result<(), Fail> {
    match machine::wrong_clock(now) {
        Some(why) => Err(fail(format!("{why}: set it first, as `timedatectl` does"))),
        None => Ok(()),
    }
}

/// Takes the one name `args` has left, for `remove` and `renew`.
fn one_name(args: &mut Args, what: &str) -> Result<String, Fail> {
    let mut name = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let name = name.and_then(|name| name.into_string().ok());
    name.ok_or_else(|| usage(format!("{what} takes the name of a certificate")))
}

/// Where `cert add` takes the certificate and its key from.
enum Source {
    /// A file of the chain and a file of the key.
    Files { chain: PathBuf, key: PathBuf },
    /// One PEM stream on standard input.
    Input,
}

/// `cert add NAME --chain FILE --key FILE [--replace] [--untrusted]` or
/// `cert add NAME --from - [--replace] [--untrusted]`.
fn add_certificate(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut chain, mut key, mut from) = (None, None, None, false);
    let (mut replace, mut untrusted) = (false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "chain" => chain = Some(PathBuf::from(args.value()?)),
            Arg::Long(option) if option == "key" => key = Some(PathBuf::from(args.value()?)),
            Arg::Long(option) if option == "from" => {
                if args.value()? != "-" {
                    return Err(usage(
                        "--from takes `-`, the standard input, which holds the key and the chain"
                            .into(),
                    ));
                }
                from = true;
            }
            Arg::Long(option) if option == "replace" => replace = true,
            Arg::Long(option) if option == "untrusted" => untrusted = true,
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let name = name.and_then(|name| name.into_string().ok());
    let source = match (from, chain, key) {
        (true, None, None) => Some(Source::Input),
        (false, Some(chain), Some(key)) => Some(Source::Files { chain, key }),
        _ => None,
    };
    let (Some(name), Some(source)) = (name, source) else {
        return Err(usage("add takes NAME, and --chain FILE and --key FILE or --from -".into()));
    };
    need_root()?;
    let now = now();
    need_clock(now)?;
    let store = Store::system();
    let roots = tls::roots(&[], "cert");
    // The input is read before the lock is taken: a person at a terminal
    // may take as long as they like to paste it.
    let stream = match &source {
        Source::Input => Some(add::read_stream(&mut std::io::stdin().lock()).map_err(fail)?),
        Source::Files { .. } => None,
    };
    let lock = store.lock().map_err(fail)?;
    let replaced = replace && store.generation(&name).map_err(fail)?.is_some();
    let checked = match (&source, &stream) {
        (Source::Files { chain, key }, _) => {
            let request = add::Request { name: &name, chain, key, replace, untrusted };
            add::check(&store, &request, &roots, now)
        }
        (Source::Input, Some(stream)) => {
            add::check_stream(&store, &name, replace, untrusted, stream, &roots, now)
        }
        (Source::Input, None) => Err(String::from("there is no input")),
    }
    .map_err(fail)?;
    let files = Files {
        chain: &checked.chain,
        key: &checked.key,
        kind: Kind::Added,
        made: now,
        acme: None,
    };
    store.install(&lock, &name, &files).map_err(fail)?;
    let event = if replaced { history::Event::Replaced } else { history::Event::Added };
    let detail = format!(
        "{}, runs out {}",
        checked.details.names.join(", "),
        util::time_text(checked.details.not_after, "%F", true)
    );
    history::record(
        &store,
        &lock,
        &history::Entry::new(now, &name, event, &detail, history::By::Cert),
    );
    drop(lock);
    println!(
        "{name} added: {}, runs out {}",
        checked.details.names.join(", "),
        runs_out(&checked.details, now)
    );
    for note in &checked.notes {
        eprintln!("cert: {note}");
    }
    if let Err(why) = tell_spark(power::CONTROL, &name) {
        eprintln!("cert: {}", spark_warning(&why));
    }
    Ok(0)
}

/// `cert remove NAME`.
fn remove(args: &mut Args) -> Result<i32, Fail> {
    let name = one_name(args, "remove")?;
    need_root()?;
    service::check_certificate(&name).map_err(fail)?;
    if name == machine::NAME {
        return Err(fail(format!(
            "{name} is the machine's own certificate, which certd makes: it is not removed"
        )));
    }
    let store = Store::system();
    let lock = store.lock().map_err(fail)?;
    if store.generation(&name).map_err(fail)?.is_none() {
        // A certificate that could not be got is not in the store, and is
        // removed all the same.
        if wanted::remove(&store, &lock, &name).map_err(fail)? {
            drop(lock);
            println!("{name} removed: certd does not try to get it again");
            wake_certd();
            return Ok(0);
        }
        return Err(fail(format!("there is no certificate {name}")));
    }
    let named = users(&name, &servicefiles::read_all());
    if !named.is_empty() {
        let each: Vec<String> = named
            .iter()
            .map(|(service, path)| format!("{service} names it in {}", path.display()))
            .collect();
        return Err(fail(format!("{}: remove it there first", each.join("; "))));
    }
    store.remove(&lock, &name).map_err(fail)?;
    let entry = history::Entry::new(
        now(),
        &name,
        history::Event::Removed,
        "taken out of the store",
        history::By::Cert,
    );
    history::record(&store, &lock, &entry);
    let was_wanted = wanted::remove(&store, &lock, &name).unwrap_or_else(|why| {
        eprintln!("cert: the record certd tried {name} again from cannot be taken away: {why}");
        false
    });
    drop(lock);
    println!("{name} removed");
    if let Err(why) = tell_spark(power::CONTROL, &name) {
        eprintln!("cert: {}", spark_warning(&why));
    }
    if was_wanted {
        println!("certd does not try to get {name} again");
        wake_certd();
    }
    Ok(0)
}

/// `cert get NAME [--name DNS]... [--dns] [--staging] [--dry-run] [--directory URL]
/// [--email ADDRESS] [--agree-terms] [--key TYPE] [--profile PROFILE] [--yes]
/// [--progress]`, or with `--check [--json]` what the checks before it find.
fn get(args: &mut Args) -> Result<i32, Fail> {
    let mut options = acme::Get {
        name: String::new(),
        names: Vec::new(),
        dns: false,
        staging: false,
        dry_run: false,
        directory: None,
        email: None,
        agree_terms: false,
        key: None,
        profile: None,
        yes: false,
        check: false,
        progress: false,
        json: false,
    };
    let mut name = None;
    let text = |args: &mut Args| -> Result<String, Fail> {
        args.value()?.into_string().map_err(|_| usage("an option's value is not text".into()))
    };
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "name" => options.names.push(text(args)?),
            Arg::Long(option) if option == "dns" => options.dns = true,
            Arg::Long(option) if option == "staging" => options.staging = true,
            Arg::Long(option) if option == "dry-run" => options.dry_run = true,
            Arg::Long(option) if option == "directory" => options.directory = Some(text(args)?),
            Arg::Long(option) if option == "email" => options.email = Some(text(args)?),
            Arg::Long(option) if option == "agree-terms" => options.agree_terms = true,
            Arg::Long(option) if option == "key" => {
                options.key = Some(acme::key_kind(&text(args)?).map_err(usage)?);
            }
            Arg::Long(option) if option == "profile" => options.profile = Some(text(args)?),
            Arg::Long(option) if option == "yes" => options.yes = true,
            Arg::Long(option) if option == "check" => options.check = true,
            Arg::Long(option) if option == "progress" => options.progress = true,
            Arg::Long(option) if option == "json" => options.json = true,
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name.and_then(|name| name.into_string().ok()) else {
        return Err(usage("get takes the name of a certificate".into()));
    };
    if options.directory.is_some() && options.staging {
        return Err(usage(
            "--staging and --directory each name an authority: give one of them".into(),
        ));
    }
    if options.json && !options.check {
        return Err(usage("--json is for --check: it tells what the checks found".into()));
    }
    if options.check && options.progress {
        return Err(usage(
            "--check orders nothing, so it has no steps: leave out --progress".into(),
        ));
    }
    if let Some(email) = &options.email {
        acme::check_email(email).map_err(usage)?;
    }
    options.name = name;
    acme::get(&options)
}

/// `cert revoke NAME [--reason REASON]`.
fn revoke(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut reason) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "reason" => {
                reason = Some(
                    args.value()?
                        .into_string()
                        .map_err(|_| usage("the reason is not text".into()))?,
                );
            }
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name.and_then(|name| name.into_string().ok()) else {
        return Err(usage("revoke takes the name of a certificate".into()));
    };
    acme::revoke(&name, reason.as_deref())
}

/// `cert renew NAME`.
fn renew(args: &mut Args) -> Result<i32, Fail> {
    let name = one_name(args, "renew")?;
    need_root()?;
    service::check_certificate(&name).map_err(fail)?;
    let now = now();
    need_clock(now)?;
    let store = Store::system();
    if name != machine::NAME {
        let kind = {
            let _lock = store.lock().map_err(fail)?;
            match store.generation(&name).map_err(fail)? {
                None => return Err(fail(format!("there is no certificate {name}"))),
                Some(_) => store.get(&name).map_err(fail)?.kind,
            }
        };
        if kind == Kind::Acme {
            return renew_from_authority(&store, &name, now);
        }
        return Err(fail(format!(
            "a certificate added by hand renews where it came from: add its new version with \
             `cert add {name} --replace`"
        )));
    }
    let lock = store.lock().map_err(fail)?;
    let details = machine::renew(&store, &lock, &util::hostname(), now).map_err(fail)?;
    let entry = history::Entry::new(
        now,
        &name,
        history::Event::Made,
        &format!("for {}", util::hostname()),
        history::By::Cert,
    );
    history::record(&store, &lock, &entry);
    drop(lock);
    println!(
        "{name} made anew: SHA-256 {}, runs out {}",
        details.fingerprint_colons(),
        runs_out(&details, now)
    );
    if let Err(why) = tell_spark(power::CONTROL, &name) {
        eprintln!("cert: {}", spark_warning(&why));
    }
    Ok(0)
}

/// `cert renew` of a certificate from an authority: asks again, now,
/// whatever certd's time for it is.
fn renew_from_authority(store: &Store, name: &str, now: i64) -> Result<i32, Fail> {
    let mut say = |line: &str| println!("{line}");
    let (details, notes) =
        acme::renew(store, name, acme::Who::Administrator, now, &mut say).map_err(fail)?;
    println!("{name} renewed: {}, runs out {}", details.names.join(", "), runs_out(&details, now));
    for note in &notes {
        eprintln!("cert: {note}");
    }
    if let Err(why) = tell_spark(power::CONTROL, name) {
        eprintln!("cert: {}", spark_warning(&why));
    }
    Ok(0)
}

/// `cert check`: certd's health check, which fails when the clock is
/// wrong or `this-machine` is not there, does not read, or has 30 days or
/// fewer left.
fn check(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let loaded = machine::load(&Store::system());
    match machine::unhealthy(&loaded, now()) {
        Some(why) => Err(fail(why)),
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixtures::{DAY, ECDSA, KNOWN_END, NOW};

    fn details_of_known() -> Details {
        let chain = inspect::chain_of(ECDSA.pem).unwrap();
        inspect::details(&chain[0]).unwrap()
    }

    fn certificate(name: &str, kind: Kind, used_by: &[&str], trust: Trust) -> Certificate {
        Certificate {
            stored: Stored { name: name.into(), kind, made: NOW, acme: None, chain: Vec::new() },
            chain: vec![details_of_known()],
            trust,
            used_by: used_by.iter().map(|service| (*service).to_string()).collect(),
        }
    }

    #[test]
    fn a_certificate_says_when_it_runs_out_in_days_or_that_it_ran_out() {
        let details = details_of_known();
        assert!(runs_out(&details, KNOWN_END - 4 * DAY - 60).ends_with("(4 days)"));
        assert!(runs_out(&details, KNOWN_END - DAY / 2).ends_with("(0 days)"));
        assert!(runs_out(&details, KNOWN_END - DAY - 60).ends_with("(1 day)"));
        assert!(runs_out(&details, KNOWN_END).starts_with("ran out 2126-"));
        assert!(runs_out(&details, KNOWN_END + 9 * DAY).starts_with("ran out "));
    }

    #[test]
    fn the_table_has_a_header_and_a_row_for_each_certificate_in_columns() {
        let rows = [
            certificate("alpha.example", Kind::Added, &["nginx", "admind"], Trust::Trusted),
            certificate("this-machine", Kind::SelfSigned, &[], Trust::SelfSigned),
        ];
        let table = list_table(&rows, NOW, false);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("NAME") && lines[0].ends_with("USED BY"));
        assert!(lines[1].starts_with("alpha.example  added"));
        assert!(lines[1].contains("ec.example, www.ec.example +2"));
        assert!(lines[1].ends_with("nginx, admind"));
        assert!(lines[2].starts_with("this-machine   self-signed"));
        assert!(lines[2].ends_with(" -"));
        let used_by = lines[0].find("USED BY").unwrap();
        assert_eq!(lines[1].find("nginx").unwrap(), used_by);
    }

    fn wanted_one(name: &str, tries: u32, given_up: bool) -> wanted::Public {
        wanted::Public {
            name: name.into(),
            names: vec![format!("{name}.example"), format!("www.{name}.example")],
            directory: String::from("https://acme.example/dir"),
            first_ms: NOW * 1_000,
            last_ms: (NOW + 3_600) * 1_000,
            next_ms: (NOW + 7_200) * 1_000,
            tries,
            why: String::from("port 80 is closed (CRT-103)"),
            given_up,
        }
    }

    #[test]
    fn the_table_of_wanted_certificates_says_when_each_is_tried_and_why_it_failed() {
        let table = wanted_table(&[wanted_one("shop", 2, false), wanted_one("blog", 31, true)]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3, "{table}");
        assert!(lines[0].starts_with("NAME  NAMES"), "{table}");
        assert!(lines[0].contains("TRIES") && lines[0].ends_with("WHY"), "{table}");
        assert!(lines[1].starts_with("shop  shop.example, www.shop.example  2  "), "{table}");
        assert!(lines[1].contains("2027-01-15 08:00:00  2027-01-15 10:00:00  port 80"), "{table}");
        assert!(lines[2].contains("31     2027-01-15 08:00:00  given up"), "{table}");
        let why = lines[0].find("WHY").unwrap();
        assert_eq!(lines[1].find("port 80").unwrap(), why);
        assert_eq!(lines[2].find("port 80").unwrap(), why);
    }

    #[test]
    fn with_nothing_wanted_the_table_says_so() {
        assert_eq!(wanted_table(&[]), "no certificate is being tried again\n");
    }

    #[test]
    fn a_table_shows_two_names_and_counts_the_rest() {
        let names = |count: usize| -> Vec<String> {
            (1..=count).map(|number| format!("n{number}.example")).collect()
        };
        assert_eq!(names_cell(&[]), "-");
        assert_eq!(names_cell(&names(2)), "n1.example, n2.example");
        assert_eq!(names_cell(&names(5)), "n1.example, n2.example +3");
    }

    #[test]
    fn a_certificate_that_ran_out_is_red_only_where_there_is_colour() {
        let rows = [certificate("old.example", Kind::Added, &[], Trust::Trusted)];
        let later = KNOWN_END + DAY;
        assert!(list_table(&rows, later, true).contains("\x1b[1;31mran out 2126-"));
        assert!(!list_table(&rows, later, false).contains('\x1b'));
        assert!(!list_table(&rows, NOW, true).contains('\x1b'));
    }

    #[test]
    fn the_json_of_a_certificate_has_the_fields_in_the_documented_order() {
        let found = certificate("nas.example", Kind::Added, &["nginx"], Trust::Trusted);
        let json = serde_json::to_string(&summary(&found, KNOWN_END - 3 * DAY - 9)).unwrap();
        assert_eq!(
            json,
            format!(
                "{{\"name\":\"nas.example\",\"kind\":\"added\",\"names\":[\"ec.example\",\"www.ec.example\",\
                 \"192.0.2.7\",\"2001:db8::1\"],\"not_before\":\"2026-11-13T18:22:33Z\",\
                 \"not_after\":\"2126-10-20T18:22:33Z\",\"days_left\":3,\
                 \"fingerprint_sha256\":\"{}\",\"issuer\":\"CN=ec.example\",\"trusted\":true,\
                 \"used_by\":[\"nginx\"]}}",
                ECDSA.fingerprint.replace(':', "").to_lowercase()
            )
        );
        let untrusted = certificate("self.example", Kind::Added, &[], Trust::SelfSigned);
        assert!(!summary(&untrusted, NOW).trusted);
    }

    #[test]
    fn show_gives_every_field_a_line_and_the_fingerprint_with_colons_and_never_a_key() {
        let found = certificate("nas.example", Kind::Added, &["nginx"], Trust::SelfSigned);
        let text =
            shown_text(&found, NOW, "/s/nas.example/fullchain.pem", "/s/nas.example/key.pem");
        for label in [
            "Name:",
            "Kind:",
            "Made:",
            "Names:",
            "Subject:",
            "Issuer:",
            "Not before:",
            "Not after:",
            "Runs out:",
            "Serial:",
            "SHA-256:",
            "Key:",
            "Chain:",
            "Trusted:",
            "Used by:",
            "Files:",
        ] {
            assert!(text.lines().any(|line| line.starts_with(label)), "{label}\n{text}");
        }
        assert!(text.contains(ECDSA.fingerprint));
        assert!(text.contains("ECDSA P-256"));
        assert!(text.contains("Trusted:    no: self-signed: browsers warn"));
        assert!(text.contains("Chain:      1: CN=ec.example"));
        assert!(text.contains("/s/nas.example/key.pem"));
        assert!(!text.contains("BEGIN"));
    }

    #[test]
    fn show_gives_the_chain_as_pem_beside_its_subjects_and_never_a_key() {
        let mut found = certificate("nas.example", Kind::Added, &[], Trust::Trusted);
        found.stored.chain = ECDSA.pem.as_bytes().to_vec();
        let json = shown_json(&found, NOW, ["/c".into(), "/k".into()]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["chain_pem"], ECDSA.pem);
        assert!(value["chain_pem"].as_str().unwrap().starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(!json.contains("PRIVATE KEY"));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        let at = |key: &str| keys.iter().position(|each| *each == key).unwrap();
        assert!(at("chain") < at("chain_pem") && at("chain_pem") < at("why_not_trusted"));
        assert_eq!(value["fingerprint_sha256"], summary(&found, NOW).fingerprint_sha256);
    }

    #[test]
    fn list_gives_the_account_of_the_authority_a_certificate_came_from_and_never_its_address() {
        let accounts = std::collections::BTreeMap::from([(
            String::from("acme-v02.api.letsencrypt.org.json"),
            accounts::Public { made_ms: 1_793_613_600_000, contact: true },
        )]);
        let mut from_authority = certificate("shop.example", Kind::Acme, &[], Trust::Trusted);
        from_authority.stored.acme = Some(store::Acme {
            directory: acme::LETS_ENCRYPT.into(),
            names: vec!["shop.example".into()],
            challenge: store::Challenge::Http01,
            key: store::KeyKind::EcdsaP256,
            profile: None,
            renew_at: None,
            revoked: false,
        });
        let listed = Listed {
            summary: summary(&from_authority, NOW),
            account: account_of(&from_authority, &accounts),
        };
        let json = serde_json::to_string(&listed).unwrap();
        assert!(
            json.ends_with("\"account\":{\"made_ms\":1793613600000,\"contact\":true}}"),
            "{json}"
        );
        // A certificate nobody issued, and an authority with no account known.
        let added = certificate("nas.example", Kind::Added, &[], Trust::Trusted);
        assert_eq!(account_of(&added, &accounts), None);
        assert_eq!(account_of(&from_authority, &std::collections::BTreeMap::new()), None);
        let none = Listed { summary: summary(&added, NOW), account: None };
        assert!(serde_json::to_string(&none).unwrap().ends_with("\"account\":null}"));
    }

    #[test]
    fn history_is_a_line_for_each_event_with_the_oldest_first() {
        use history::{By, Entry, Event};
        let entries = [
            Entry::new(NOW, "a.example", Event::Renewed, "Let's Encrypt", By::Certd).took(4_200),
            Entry::new(
                NOW + 60,
                "photos.example.org",
                Event::Failed,
                "port 80 was closed (CRT-103)",
                By::Cert,
            ),
            Entry::new(NOW + 90, "a.example", Event::Got, "from a private authority", By::Cert)
                .took(400),
        ];
        let text = history_text(&entries);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("renewed  Let's Encrypt  (certd, 4 s)"), "{text}");
        assert!(
            lines[1]
                .contains("  photos.example.org  failed   port 80 was closed (CRT-103)  (cert)")
        );
        assert!(lines[2].ends_with("(cert, under 1 s)"));
        assert_eq!(lines[0].find("renewed"), lines[1].find("failed "));
        assert!(history_text(&[]).is_empty());
    }

    #[test]
    fn the_services_that_name_a_certificate_are_found_in_their_files() {
        let named = |names: &[&str]| {
            let list: Vec<String> = names.iter().map(|name| format!("\"{name}\"")).collect();
            let text = format!(
                "service_version = 2\ndescription = \"x\"\nprogram = \"/system/commands/x\"\n\
                 certificates = [{}]\n",
                list.join(", ")
            );
            service::from_toml(&text).unwrap()
        };
        let services = [
            (String::from("nginx"), PathBuf::from("/s/nginx.toml"), named(&["this-machine"])),
            (
                String::from("admind"),
                PathBuf::from("/s/admind.toml"),
                named(&["this-machine", "a.example"]),
            ),
            (String::from("sshd"), PathBuf::from("/s/sshd.toml"), named(&[])),
        ];
        let found = users("this-machine", &services);
        assert_eq!(
            found,
            [("nginx".into(), "/s/nginx.toml".into()), ("admind".into(), "/s/admind.toml".into())]
        );
        assert_eq!(users("a.example", &services).len(), 1);
        assert!(users("none.example", &services).is_empty());
    }
}
