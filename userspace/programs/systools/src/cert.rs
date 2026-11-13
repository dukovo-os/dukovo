// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! cert(8): the machine's certificates, which certd(8) keeps (ADR 0512).
//!
//! `cert list` and `cert show` read the store as anyone may; `add`,
//! `remove` and `renew` change it, as root, under the store's lock, and tell
//! Spark through its control socket, which copies the certificate again for
//! the services that name it and has them read it. `cert check` is certd's
//! health check.
//!
//! - [`store`] is the folder of certificates and its lock;
//! - [`inspect`] reads PEM files and certificates, and checks a chain
//!   against the system's roots;
//! - [`machine`] makes `this-machine` and says when it needs making again;
//! - [`add`] is what `cert add` checks.
//!
//! When it misbehaves, `cert show NAME` names what is wrong with a
//! certificate's files, `log certd` shows what certd did, `spark`'s own
//! lines say why a service did not get its copy, and the `certificates` boot
//! test covers it all.

mod add;
#[cfg(test)]
mod fixtures;
pub mod inspect;
pub mod machine;
pub mod store;

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
    }
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
        let summaries: Vec<Summary> = certificates.iter().map(|read| summary(read, now)).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&summaries).map_err(|err| fail(err.to_string()))?
        );
    } else {
        let colour = util::is_terminal(1) && std::env::var_os("NO_COLOR").is_none();
        print!("{}", list_table(&certificates, now, colour));
    }
    Ok(i32::from(unreadable))
}

/// `list`'s table: a header, then a line a certificate, with the date a
/// certificate ran out in red when `colour` is on.
fn list_table(certificates: &[Certificate], now: i64, colour: bool) -> String {
    let titles = ["NAME", "KIND", "NAMES", "RUNS OUT", "USED BY"];
    let mut rows: Vec<[String; 5]> = vec![titles.map(String::from)];
    for read in certificates {
        let first = read.first();
        let mut names: Vec<&str> =
            first.names.iter().take(NAMES_SHOWN).map(String::as_str).collect();
        if names.is_empty() {
            names.push("-");
        }
        let more = first.names.len().saturating_sub(NAMES_SHOWN);
        let names =
            if more > 0 { format!("{} +{more}", names.join(", ")) } else { names.join(", ") };
        let used_by =
            if read.used_by.is_empty() { String::from("-") } else { read.used_by.join(", ") };
        rows.push([
            read.stored.name.clone(),
            read.stored.kind.word().to_string(),
            names,
            runs_out(first, now),
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
            } else {
                text.push_str(&format!("{cell}{padding}"));
            }
        }
        text.push('\n');
    }
    text
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

/// `cert add NAME --chain FILE --key FILE [--replace] [--untrusted]`.
fn add_certificate(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut chain, mut key) = (None, None, None);
    let (mut replace, mut untrusted) = (false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "chain" => chain = Some(PathBuf::from(args.value()?)),
            Arg::Long(option) if option == "key" => key = Some(PathBuf::from(args.value()?)),
            Arg::Long(option) if option == "replace" => replace = true,
            Arg::Long(option) if option == "untrusted" => untrusted = true,
            Arg::Operand(operand) if name.is_none() => name = Some(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let name = name.and_then(|name| name.into_string().ok());
    let (Some(name), Some(chain), Some(key)) = (name, chain, key) else {
        return Err(usage("add takes NAME, --chain FILE and --key FILE".into()));
    };
    need_root()?;
    let now = now();
    need_clock(now)?;
    let store = Store::system();
    let lock = store.lock().map_err(fail)?;
    let request = add::Request { name: &name, chain: &chain, key: &key, replace, untrusted };
    let roots = tls::roots(&[], "cert");
    let checked = add::check(&store, &request, &roots, now).map_err(fail)?;
    let files = Files { chain: &checked.chain, key: &checked.key, kind: Kind::Added, made: now };
    store.install(&lock, &name, &files).map_err(fail)?;
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
    drop(lock);
    println!("{name} removed");
    if let Err(why) = tell_spark(power::CONTROL, &name) {
        eprintln!("cert: {}", spark_warning(&why));
    }
    Ok(0)
}

/// `cert renew NAME`.
fn renew(args: &mut Args) -> Result<i32, Fail> {
    let name = one_name(args, "renew")?;
    need_root()?;
    service::check_certificate(&name).map_err(fail)?;
    let now = now();
    need_clock(now)?;
    let store = Store::system();
    let lock = store.lock().map_err(fail)?;
    if name != machine::NAME {
        return Err(fail(match store.generation(&name).map_err(fail)? {
            None => format!("there is no certificate {name}"),
            Some(_) => format!(
                "a certificate added by hand renews where it came from: add its new version \
                 with `cert add {name} --replace`"
            ),
        }));
    }
    let details = machine::renew(&store, &lock, &util::hostname(), now).map_err(fail)?;
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
            stored: Stored { name: name.into(), kind, made: NOW, chain: Vec::new() },
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
