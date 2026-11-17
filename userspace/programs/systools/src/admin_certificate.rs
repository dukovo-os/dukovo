// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admin certificate [NAME] [--try SECONDS]` (admin(8), ADR 0567): says
//! which certificate the console serves, or makes it serve certificate
//! NAME of those certd keeps, as the console's `console.certificate`
//! action does.
//!
//! Two files change together: `certificate` in admin.toml(5), which says
//! which certificate admind serves, and `certificates` in admind's service
//! file, which has Spark copy that certificate for it, keeping
//! `this-machine` and dropping the one served before. Spark's reload of
//! admind copies the certificates its file names now and sends it SIGHUP,
//! on which admind reads both again and serves the new certificate to the
//! connections that come after; the command then waits for the issue file
//! to name the new certificate's fingerprint, which admind writes once it
//! serves it. With `--try SECONDS` both files are Spark's trial `console`,
//! as `admin reach --try` makes it: Spark puts both back, and reloads
//! admind, unless `admin keep` comes in time.
//!
//! It is root's, as both files are. A certificate certd does not keep is
//! refused before anything changes, and so is a change while the trial
//! `console` runs (ADM-138).

use std::path::Path;
use std::time::{Duration, Instant};

use crate::admin_settings::{self, Settings};
use crate::admind::ISSUE;
use crate::args::{Arg, Args};
use crate::cert::{inspect, machine, store::Store};
use crate::trial::{self, ToUse, Tried};
use crate::util::{self, Fail, fail, usage};
use crate::{admin, service, servicefiles};

/// The service that serves the console, whose file names the certificate.
const ADMIND: &str = "admind";

/// The mode of admin.toml and of a service file when either is made.
const MODE: u32 = 0o644;

/// How long the command waits for admind to say it serves the new
/// certificate: its reload reads two small files and a certificate.
const SERVED_WAIT: Duration = Duration::from_secs(10);

/// How often the issue file is read while it waits.
const SERVED_EVERY: Duration = Duration::from_millis(250);

/// `admin certificate [NAME] [--try SECONDS]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut seconds) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            Arg::Long(option) if option == "try" && seconds.is_none() => {
                seconds = Some(trial::seconds(&args.text()?)?);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let path = Path::new(admin_settings::PATH);
    let Some(name) = name else {
        if seconds.is_some() {
            return Err(usage("--try goes with a certificate's name: cert list names them".into()));
        }
        println!("{}", Settings::read_at(path).map_err(fail)?.console.certificate);
        return Ok(0);
    };
    manifest::service::check_certificate(&name)
        .map_err(|why| usage(format!("{name} is not a certificate's name: {why}")))?;
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root may change the console's certificate: run it with elevate, as `elevate \
             admin certificate {name}`"
        )));
    }
    admin::console_not_tried(seconds)?;
    let fingerprint = fingerprint_of(&Store::system(), &name)?;
    let served = Settings::read_at(path).map_err(fail)?.console.certificate;
    let settings =
        if served == name { None } else { admin::console_text(path, "certificate", &name)? };
    let (service_file, service_text) =
        servicefiles::certificates_file(ADMIND, |had| certificates(had, &served, &name))?;
    let mut changes: Vec<(&Path, &[u8], u32)> = Vec::new();
    // The service file first: the certificate is copied before admin.toml
    // names it, though admind reads neither until its reload.
    if let Some(text) = &service_text {
        changes.push((&service_file, text.as_bytes(), MODE));
    }
    if let Some(text) = &settings {
        changes.push((path, text.as_bytes(), MODE));
    }
    if changes.is_empty() {
        println!("The console serves {name}'s certificate already: nothing is changed.");
        return Ok(0);
    }
    match seconds {
        Some(seconds) => tried(&name, &fingerprint, &changes, seconds),
        None => set(&name, &fingerprint, &changes),
    }
}

/// The certificates admind's file names for it to serve `name`, from the
/// list it has (`had`) and the one it serves (`served`): `name` added, and
/// `served` dropped unless it is the machine's own, which admind's file
/// always names.
fn certificates(had: &[String], served: &str, name: &str) -> Vec<String> {
    let mut list: Vec<String> = had
        .iter()
        .filter(|kept| kept.as_str() != served || kept.as_str() == machine::NAME)
        .cloned()
        .collect();
    for wanted in [machine::NAME, name] {
        if !list.iter().any(|kept| kept == wanted) {
            list.push(wanted.to_string());
        }
    }
    list
}

/// The fingerprint, as admind's issue file writes it, of certificate
/// `name` of `store`, which must be there and read.
fn fingerprint_of(store: &Store, name: &str) -> Result<String, Fail> {
    let stored = store.get(name).map_err(|why| {
        fail(format!(
            "{why}: `cert list` names the certificates certd keeps, and `cert add` adds one"
        ))
    })?;
    let text = String::from_utf8(stored.chain)
        .map_err(|_| fail(format!("{name}'s fullchain.pem is not text")))?;
    let chain = inspect::chain_of(&text).map_err(|why| fail(format!("{name}: {why}")))?;
    let first = chain.first().ok_or_else(|| fail(format!("{name}'s fullchain.pem holds none")))?;
    let details = inspect::details(first).map_err(|why| fail(format!("{name}: {why}")))?;
    Ok(details.fingerprint_colons())
}

/// Writes `changes` at once, each whole, and has admind take them.
fn set(name: &str, fingerprint: &str, changes: &[(&Path, &[u8], u32)]) -> Result<i32, Fail> {
    for &(path, text, mode) in changes {
        trial::replace(path, text, mode)?;
    }
    service::ask(&format!("reload {ADMIND}")).map_err(|why| {
        fail(format!(
            "{} names {name} now, but admind was not told to read it again ({}): it serves it \
             once it starts again, with `elevate service admind restart`",
            admin_settings::PATH,
            why.message
        ))
    })?;
    served(name, fingerprint)?;
    println!("admin: the console serves {name}'s certificate");
    Ok(0)
}

/// Tries `changes` for `seconds` as the trial `console`, and has admind
/// take them. A reload that fails, or a certificate admind does not serve,
/// is undone at once, and says so, with 1.
fn tried(
    name: &str,
    fingerprint: &str,
    changes: &[(&Path, &[u8], u32)],
    seconds: u32,
) -> Result<i32, Fail> {
    match trial::try_texts("console", changes, seconds, ToUse::Reload(ADMIND))? {
        Tried::InUse => {}
        Tried::Undone { why, undone } => {
            eprintln!("admin: {why}");
            println!("{undone}");
            return Ok(1);
        }
    }
    if let Err(why) = served(name, fingerprint) {
        eprintln!("admin: {}", why.message);
        let undone = trial::end("undo", "console", || fail("the trial console has ended".into()))?;
        println!("{undone}");
        return Ok(1);
    }
    println!("admin: the console serves {name}'s certificate");
    println!(
        "Spark undoes it at {} unless it is kept, with `admin keep`.",
        trial::undone_at(seconds)
    );
    Ok(0)
}

/// Waits for admind to say, in its issue file, that it serves the
/// certificate whose fingerprint is `fingerprint`.
fn served(name: &str, fingerprint: &str) -> Result<(), Fail> {
    let line = format!("SHA-256 fingerprint {fingerprint}");
    let until = Instant::now() + SERVED_WAIT;
    loop {
        let issue = std::fs::read_to_string(ISSUE).unwrap_or_default();
        if issue.lines().any(|said| said == line) {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(fail(format!(
                "admind did not serve {name}'s certificate within {} s: `log admind` says why",
                SERVED_WAIT.as_secs()
            )));
        }
        std::thread::sleep(SERVED_EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::fixtures;
    use crate::cert::store::{Files, Kind};

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_list_gains_the_name_and_loses_the_one_served_before_but_never_the_machine_s() {
        let had = list(&["this-machine"]);
        assert_eq!(
            certificates(&had, "this-machine", "nas.example.org"),
            ["this-machine", "nas.example.org"]
        );
        let had = list(&["this-machine", "old.example.org"]);
        assert_eq!(
            certificates(&had, "old.example.org", "nas.example.org"),
            ["this-machine", "nas.example.org"]
        );
        // Back to the machine's own: the one served before goes.
        assert_eq!(certificates(&had, "old.example.org", "this-machine"), ["this-machine"]);
        // A list that lost the machine's own gets it back; one named for
        // another reason stays.
        let had = list(&["mail.example.org"]);
        assert_eq!(
            certificates(&had, "this-machine", "nas.example.org"),
            ["mail.example.org", "this-machine", "nas.example.org"]
        );
        // Named already: nothing moves.
        let had = list(&["this-machine", "nas.example.org"]);
        assert_eq!(certificates(&had, "nas.example.org", "nas.example.org"), had);
    }

    #[test]
    fn a_certificate_certd_does_not_keep_is_refused_with_where_to_look() {
        let scratch = fixtures::Scratch::new("admin-certificate-none");
        let store = Store::at(&scratch.0, Duration::from_secs(1));
        let refused = fingerprint_of(&store, "nas.example.org").unwrap_err();
        assert_eq!(
            refused.message,
            "there is no certificate nas.example.org: `cert list` names the certificates certd \
             keeps, and `cert add` adds one"
        );
    }

    #[test]
    fn the_fingerprint_is_the_issue_file_s_of_the_first_certificate_of_the_chain() {
        let scratch = fixtures::Scratch::new("admin-certificate-one");
        let store = Store::at(&scratch.0, Duration::from_secs(1));
        let leaf = fixtures::self_signed(&["nas.example.org"]);
        let chain = fixtures::chain_file(&[&leaf.der]);
        let files = Files {
            chain: &chain,
            key: &leaf.key,
            kind: Kind::Added,
            made: fixtures::NOW,
            acme: None,
        };
        store.install(&store.lock().unwrap(), "nas.example.org", &files).unwrap();
        let fingerprint = fingerprint_of(&store, "nas.example.org").unwrap();
        assert_eq!(fingerprint, inspect::details(&leaf.der).unwrap().fingerprint_colons());
        assert_eq!(fingerprint.len(), 32 * 3 - 1);
    }

    #[test]
    fn a_name_that_is_not_a_certificate_s_or_a_try_alone_is_a_mistake_in_the_command_line() {
        let run = |items: &[&str]| {
            let mut args = Args::new("admin", items.iter().map(std::ffi::OsString::from).collect());
            main(&mut args)
        };
        let refused = run(&["../key"]).unwrap_err();
        assert!(refused.usage, "{}", refused.message);
        assert!(refused.message.starts_with("../key is not a certificate's name"));
        let refused = run(&["--try", "60"]).unwrap_err();
        assert_eq!(
            (refused.usage, refused.message.as_str()),
            (true, "--try goes with a certificate's name: cert list names them")
        );
        for items in [&["a", "b"][..], &["a", "--try", "5"], &["a", "--try", "60", "--try", "60"]] {
            assert!(run(items).unwrap_err().usage, "{items:?}");
        }
    }
}
