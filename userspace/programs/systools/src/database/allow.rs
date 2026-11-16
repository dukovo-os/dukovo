// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `database allow` and `deny`: other machines let in to a database over
//! TLS with its role's password, and shut out again.
//!
//! A change reaches four files, PostgreSQL's service override (which
//! certificates Spark copies for it), `pg_hba.conf` (who may connect),
//! `postgresql.auto.conf` (where it listens and with what certificate,
//! through `ALTER SYSTEM`) and the firewall's rules, which hold the block
//! that opens port 5432 to the networks of every database and to none
//! else ([`port`]). Before any of it takes effect PostgreSQL reads its
//! files as they are now, and a mistake it finds puts them all back. Then
//! the firewall has the rules, and the server reads its files again, or
//! restarts when it must listen elsewhere or read another certificate;
//! one that does not answer as it should, and with the certificate, has
//! the files put back, the firewall has them again, and the server is
//! started again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::add::{built, ready, run};
use super::names::Network;
use super::server::{Host, PORT, RELOAD_WAIT, value};
use super::settings::{self, Change, Settings};
use super::{errors, hba, port, sql};
use crate::cert::acme::steps::Steps;
use crate::settings_edit;
use crate::util::{Fail, fail};

/// What `allow` is asked to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Allow {
    /// The database.
    pub name: String,
    /// The networks let in.
    pub networks: Vec<Network>,
    /// The certificate the server takes TLS with.
    pub certificate: String,
}

/// What `deny` is asked to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Deny {
    /// The database.
    pub name: String,
    /// The networks shut out, or every one of the database's.
    pub networks: Option<Vec<Network>>,
}

/// How many steps `allow` tells of.
pub const ALLOW_STEPS: usize = 8;

/// How many steps `deny` tells of.
pub const DENY_STEPS: usize = 6;

/// The mode of `pg_hba.conf`, when it has to be made again: the owner,
/// postgres, is kept as it is.
const HBA_MODE: u32 = 0o600;

/// The mode of a service's override.
const OVERRIDE_MODE: u32 = 0o644;

/// The mode of the firewall's rules, when the file has to be made:
/// everyone may read them, as `firewall list` does.
const FIREWALL_MODE: u32 = 0o644;

/// The files of the server that a change touches.
struct Server {
    /// `pg_hba.conf`.
    hba: PathBuf,
    /// `postgresql.auto.conf`, which `ALTER SYSTEM` writes.
    auto: PathBuf,
}

/// Where the server keeps its files, as it says.
fn locate(host: &mut dyn Host) -> Result<Server, Fail> {
    let hba = run(host, "SHOW hba_file")?;
    let data = run(host, "SHOW data_directory")?;
    let (Some(hba), Some(data)) = (value(&hba), value(&data)) else {
        return Err(fail("PostgreSQL did not say where its files are".into()));
    };
    Ok(Server { hba: PathBuf::from(hba), auto: Path::new(data).join("postgresql.auto.conf") })
}

/// The text of the file `path`, or none when it is not there.
pub(super) fn read_optional(path: &Path) -> Result<Option<String>, Fail> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(crate::util::failed(&path.display(), error)),
    }
}

/// The text of `pg_hba.conf`.
fn read_hba(path: &Path) -> Result<String, Fail> {
    read_optional(path)?
        .ok_or_else(|| fail(format!("{} is not there, though PostgreSQL reads it", path.display())))
}

/// Puts `text` in the file at `path`, which keeps its owner and mode.
fn put(path: &Path, text: &[u8], mode: u32) -> Result<(), Fail> {
    settings_edit::write(path, text, mode)
        .map(|_| ())
        .map_err(|error| crate::util::failed(&path.display(), error))
}

/// Puts the file at `path` back as it was: its old text, or none.
fn restore(path: &Path, before: Option<&[u8]>, mode: u32) -> Result<(), String> {
    match before {
        Some(text) => settings_edit::write(path, text, mode).map(|_| ()).map_err(|e| e.to_string()),
        None => match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        },
    }
}

/// What the server's settings are, as its files say them.
fn read_settings(host: &mut dyn Host) -> Result<Settings, Fail> {
    let files = run(host, sql::FILE_SETTINGS)?;
    let now = run(host, sql::SETTING_NOW)?;
    match (files.first(), now.first()) {
        (Some(files), Some(now)) => Ok(Settings::read(files, now)),
        _ => Err(fail("PostgreSQL did not say what its settings are".into())),
    }
}

/// What the server runs with now for the setting `name`.
fn running(host: &mut dyn Host, name: &str) -> Result<String, Fail> {
    let answers = run(host, &format!("SHOW {name}"))?;
    value(&answers)
        .map(String::from)
        .ok_or_else(|| fail(format!("PostgreSQL did not say what {name} is")))
}

/// The mistakes PostgreSQL finds in `pg_hba.conf` and its settings as
/// they are on disk now. A setting that waits for a restart is no
/// mistake, though it is listed as one.
fn mistakes(host: &mut dyn Host) -> Result<Vec<String>, Fail> {
    let mut found = Vec::new();
    let hba = run(host, sql::HBA_ERRORS)?;
    for row in hba.first().map_or_default(|answer| answer.rows.as_slice()) {
        let cell = |at: usize| row.get(at).and_then(|cell| cell.as_deref());
        found.push(errors::problem_words(
            "pg_hba.conf",
            cell(0),
            None,
            cell(1).unwrap_or("it cannot be read"),
        ));
    }
    let settings = run(host, sql::SETTINGS_ERRORS)?;
    for row in settings.first().map_or_default(|answer| answer.rows.as_slice()) {
        let cell = |at: usize| row.get(at).and_then(|cell| cell.as_deref());
        let error = cell(3).unwrap_or("it cannot be read");
        if error == WAITS {
            continue;
        }
        let file = cell(0).map_or("the settings", |path| path.rsplit('/').next().unwrap_or(path));
        found.push(errors::problem_words(file, cell(1), cell(2), error));
    }
    Ok(found)
}

/// What `pg_file_settings` says of a setting that cannot be applied while
/// the server runs, as `listen_addresses` cannot: it is not a mistake.
const WAITS: &str = "setting could not be applied";

/// Asks the server to read its files again, and waits until it has.
fn reload(host: &mut dyn Host) -> Result<(), Fail> {
    let before = run(host, sql::LOADED_AT)?;
    let before = value(&before).map(String::from);
    run(host, sql::RELOAD)?;
    let step = std::time::Duration::from_millis(100);
    let mut waited = std::time::Duration::ZERO;
    while waited < RELOAD_WAIT {
        let now = run(host, sql::LOADED_AT)?;
        if value(&now).map(String::from) != before {
            return Ok(());
        }
        host.pause(step);
        waited += step;
    }
    Err(fail(format!(
        "PostgreSQL did not read its files again within {} seconds",
        RELOAD_WAIT.as_secs()
    )))
}

/// What happens to the server once the files are put back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Then {
    /// Nothing: nothing took effect.
    Nothing,
    /// The server reads its files again.
    Reload,
    /// The server starts again.
    Restart,
}

/// What a change has done to the files and the settings, so that it can
/// be taken back.
struct Attempt {
    /// The server's files.
    server: Server,
    /// The service's override.
    over: PathBuf,
    /// The text of `pg_hba.conf` before.
    hba: String,
    /// The text of the override before, none for a file not there.
    over_before: Option<String>,
    /// The firewall's rules.
    firewall: PathBuf,
    /// The text of the firewall's rules before, none for a file not there.
    firewall_before: Option<String>,
    /// The text of `postgresql.auto.conf` before, none for no file.
    auto: Option<Vec<u8>>,
    /// The settings before.
    settings: Settings,
    /// Whether the override has been written.
    wrote_over: bool,
    /// Whether `pg_hba.conf` has been written.
    wrote_hba: bool,
    /// Whether the firewall's rules have been written.
    wrote_firewall: bool,
    /// The settings that were set or reset.
    applied: Vec<Change>,
}

impl Attempt {
    /// Puts the firewall's rules back as they were and has the firewall
    /// load them. Gives what went wrong, as words that follow "the change
    /// was taken back, but".
    fn put_firewall_back(&self, host: &mut dyn Host) -> Option<String> {
        let before = self.firewall_before.as_deref().map(str::as_bytes);
        if let Err(why) = restore(&self.firewall, before, FIREWALL_MODE) {
            return Some(format!("the firewall's rules could not be put back ({why})"));
        }
        host.load_firewall()
            .err()
            .map(|why| format!("the firewall could not load the rules put back ({why})"))
    }

    /// Puts the files and the settings back as they were, and then does
    /// what `then` says. Gives how it went, as words that follow
    /// "the change was taken back": empty when it all went well.
    fn take_back(&mut self, host: &mut dyn Host, then: Then) -> String {
        let mut trouble = Vec::new();
        if self.wrote_hba
            && let Err(why) = restore(&self.server.hba, Some(self.hba.as_bytes()), HBA_MODE)
        {
            trouble.push(format!("pg_hba.conf could not be put back ({why})"));
        }
        if self.wrote_over
            && let Err(why) =
                restore(&self.over, self.over_before.as_deref().map(str::as_bytes), OVERRIDE_MODE)
        {
            trouble.push(format!("the service's override could not be put back ({why})"));
        }
        if self.wrote_firewall {
            trouble.extend(self.put_firewall_back(host));
        }
        if !self.applied.is_empty() {
            if then == Then::Restart {
                // The server may not be there to be asked.
                if let Err(why) = restore(&self.server.auto, self.auto.as_deref(), HBA_MODE) {
                    trouble.push(format!("postgresql.auto.conf could not be put back ({why})"));
                }
            } else {
                for change in self.settings.to_take_back(&self.applied) {
                    let said = change.statement().map_err(fail).and_then(|s| run(host, &s));
                    if let Err(why) = said {
                        trouble.push(format!(
                            "{} could not be put back ({})",
                            change.name, why.message
                        ));
                    }
                }
            }
        }
        let started = match then {
            Then::Nothing => Ok(()),
            Then::Reload => reload(host).map_err(|failed| failed.message),
            Then::Restart => host.restart(),
        };
        match (started, trouble.is_empty()) {
            (Ok(()), true) if then == Then::Restart => ", and PostgreSQL runs again".into(),
            (Ok(()), true) => String::new(),
            (Ok(()), false) => format!(", but {}", trouble.join(", and ")),
            (Err(why), true) => format!(", but PostgreSQL could not be started again: {why}"),
            (Err(why), false) => format!(
                ", but {}, and PostgreSQL could not be started again: {why}",
                trouble.join(", and ")
            ),
        }
    }
}

/// `database allow`: lets the networks in to the database over TLS, and
/// opens port 5432 in the firewall to the networks of every database.
///
/// # Errors
///
/// DB-101, DB-104, DB-105, DB-107 and DB-108 as the command says; a
/// plain failure for a database that is not there, or whose role has no
/// password to sign in with.
pub fn allow(host: &mut dyn Host, ask: &Allow, steps: &Steps) -> Result<Vec<String>, Fail> {
    steps.next("checking the database, the networks and the certificate");
    ready(host)?;
    let found = run(host, &built(sql::the_database(&ask.name))?)?;
    let Some(answer) = found.first().filter(|answer| !answer.rows.is_empty()) else {
        return Err(fail(format!(
            "there is no database called {}; `database list` shows them",
            ask.name
        )));
    };
    let owner = answer.first("owner").unwrap_or_default().to_string();
    if answer.first("password") != Some("t") {
        return Err(fail(format!(
            "the role {owner} signs in as the account of that name and has no password, which \
             other machines sign in with: `database password {}` is not for it either",
            ask.name
        )));
    }
    // A line of pg_hba.conf names them in double quotes.
    super::names::check_name(&owner).map_err(|why| {
        fail(format!("the role {owner} cannot be named in pg_hba.conf by database: {why}"))
    })?;
    if !host.has_certificate(&ask.certificate).map_err(fail)? {
        return Err(errors::no_certificate(&ask.certificate));
    }
    let server = locate(host)?;
    let hba_before = read_hba(&server.hba)?;
    let settings = read_settings(host)?;
    let listening = running(host, settings::LISTEN)?;
    let rules: Vec<hba::Rule> = ask
        .networks
        .iter()
        .map(|network| hba::Rule {
            database: ask.name.clone(),
            role: owner.clone(),
            network: *network,
        })
        .collect();
    let hba_after =
        hba::with_rules(&hba_before, &rules).map_err(|why| errors::not_passed(&why, ""))?;
    let open_to = hba::rules(&hba_after).map_err(|why| errors::not_passed(&why, ""))?;
    let (firewall_before, wanted) = firewall_plan(host, &open_to)?;
    let files = host.files().clone();
    let service_file = fs::read_to_string(&files.service)
        .map_err(|error| crate::util::failed(&files.service.display(), error))?;
    let over_before = read_optional(&files.over)?;
    let over_after =
        settings::override_with(&service_file, over_before.as_deref(), &ask.certificate)
            .map_err(|why| fail(format!("{}: {why}", files.over.display())))?;
    let changes = settings.to_allow(&ask.certificate);
    let mut attempt = Attempt {
        auto: read_optional(&server.auto)?.map(String::into_bytes),
        server,
        over: files.over.clone(),
        hba: hba_before.clone(),
        over_before,
        firewall: files.firewall.clone(),
        firewall_before,
        settings,
        wrote_over: false,
        wrote_hba: false,
        wrote_firewall: false,
        applied: Vec::new(),
    };

    steps.next("naming the certificate in PostgreSQL's service file");
    if let Some(text) = &over_after {
        let made = files
            .over
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .map_err(|error| crate::util::failed(&files.over.display(), error))
            .and_then(|()| put(&files.over, text.as_bytes(), OVERRIDE_MODE));
        attempt.wrote_over = true;
        if let Err(why) = made {
            return Err(stopped(host, &mut attempt, &why.message));
        }
    }
    steps.next("writing the lines of pg_hba.conf");
    if hba_after != hba_before {
        attempt.wrote_hba = true;
        if let Err(why) = put(&attempt.server.hba, hba_after.as_bytes(), HBA_MODE) {
            return Err(stopped(host, &mut attempt, &why.message));
        }
    }
    steps.next("setting what PostgreSQL listens with");
    if let Err(why) = apply(host, &mut attempt, &changes) {
        return Err(stopped(host, &mut attempt, &why.message));
    }

    steps.next("having PostgreSQL check its files");
    let found = mistakes(host);
    let found = match found {
        Ok(found) => found,
        Err(why) => return Err(stopped(host, &mut attempt, &why.message)),
    };
    if !found.is_empty() {
        let back = attempt.take_back(host, Then::Nothing);
        return Err(errors::not_passed(
            &format!("PostgreSQL's check found {}", found.join("; ")),
            &back,
        ));
    }

    steps.next(&format!("opening port {PORT} to these networks only"));
    if let Err(why) = write_firewall(host, &mut attempt, &wanted.text) {
        return Err(stopped(host, &mut attempt, &why));
    }

    let after =
        read_settings(host).map_err(|why| stopped_with(host, &mut attempt, &why, Then::Nothing))?;
    let restarts = over_after.is_some() || after.needs_restart(&changes, &listening);
    steps.next(if restarts {
        "restarting PostgreSQL"
    } else {
        "asking PostgreSQL to read its files again"
    });
    let settled = if restarts { host.restart() } else { reload(host).map_err(|why| why.message) };
    let then = if restarts { Then::Restart } else { Then::Reload };
    if let Err(why) = settled {
        return Err(taken_back(host, &mut attempt, &why, then));
    }

    steps.next("checking that PostgreSQL answers over TLS");
    if let Err(why) = answers_over_tls(host, &ask.certificate) {
        return Err(taken_back(host, &mut attempt, &why, then));
    }
    let networks: Vec<String> = ask.networks.iter().map(ToString::to_string).collect();
    host.log(&format!(
        "let {} connect to {} over TLS with {}'s certificate",
        networks.join(", "),
        ask.name,
        ask.certificate
    ));
    let mut words = vec![format!(
        "{} may connect to {} over TLS, as {owner}, with the password",
        networks.join(", "),
        ask.name
    )];
    if restarts {
        words.push("PostgreSQL restarted, and its open sessions ended".into());
    }
    if let Some(rule) = &wanted.earlier {
        words.push(format!(
            "the firewall rule `{rule}` comes before database's rules for port {PORT} and decides \
             first for the connections it names; database does not move it"
        ));
    }
    Ok(words)
}

/// The firewall's rules as they are, and as they are to be once the
/// server is open to the networks of `open_to`, the lines of
/// `pg_hba.conf`'s block that will be there. Refused while the firewall's
/// rules are being tried, and when the file cannot take the block.
///
/// # Errors
///
/// DB-107 and DB-108; a plain failure when the file cannot be read.
fn firewall_plan(
    host: &mut dyn Host,
    open_to: &[hba::Rule],
) -> Result<(Option<String>, port::Wanted), Fail> {
    if host.firewall_is_tried() {
        return Err(errors::firewall_is_tried());
    }
    let before = read_optional(&host.files().firewall)?;
    let wanted = port::wanted(before.as_deref().unwrap_or_default(), open_to)?;
    Ok((before, wanted))
}

/// Writes `text` as the firewall's rules, unless the file holds it, and
/// has the firewall load it.
fn write_firewall(host: &mut dyn Host, attempt: &mut Attempt, text: &str) -> Result<(), String> {
    if attempt.firewall_before.as_deref().unwrap_or_default() == text {
        return Ok(());
    }
    attempt.wrote_firewall = true;
    put(&attempt.firewall, text.as_bytes(), FIREWALL_MODE).map_err(|why| why.message)?;
    host.load_firewall().map_err(|why| format!("the firewall did not take its rules: {why}"))
}

/// Sets and resets `changes`, one statement each, noting in `attempt`
/// what was done.
fn apply(host: &mut dyn Host, attempt: &mut Attempt, changes: &[Change]) -> Result<(), Fail> {
    for change in changes {
        let statement = change.statement().map_err(fail)?;
        attempt.applied.push(change.clone());
        run(host, &statement)?;
    }
    Ok(())
}

/// The failure for a step that failed before the check: what failed, with
/// the files and settings put back.
fn stopped(host: &mut dyn Host, attempt: &mut Attempt, why: &str) -> Fail {
    let back = attempt.take_back(host, Then::Nothing);
    errors::not_passed(why, &back)
}

/// [`stopped`] for a failure that is already one.
fn stopped_with(host: &mut dyn Host, attempt: &mut Attempt, why: &Fail, then: Then) -> Fail {
    let back = attempt.take_back(host, then);
    errors::not_passed(&why.message, &back)
}

/// The failure for a server that did not settle or answer as it should:
/// everything put back, and the server made to take it.
fn taken_back(host: &mut dyn Host, attempt: &mut Attempt, why: &str, then: Then) -> Fail {
    let back = attempt.take_back(host, then);
    errors::taken_back(why, &back)
}

/// Checks that the server listens on the network, takes TLS, and does
/// with the certificate `certificate` holds.
fn answers_over_tls(host: &mut dyn Host, certificate: &str) -> Result<(), String> {
    let said = |why: Fail| why.message;
    let listen = running(host, settings::LISTEN).map_err(said)?;
    if listen != settings::NETWORK {
        return Err(format!("PostgreSQL listens on {listen}, not on the network"));
    }
    let ssl = running(host, settings::SSL).map_err(said)?;
    if ssl != "on" {
        return Err(format!("PostgreSQL has ssl {ssl}, not on"));
    }
    host.probe(certificate)
}

/// `database deny`: shuts the networks, or every one, out of the
/// database, shuts port 5432 in the firewall to the networks that no
/// database names, and has the server listen on this machine only again
/// when no line of `database`'s is left.
///
/// # Errors
///
/// DB-101, DB-104, DB-107 and DB-108 as the command says.
pub fn deny(host: &mut dyn Host, ask: &Deny, steps: &Steps) -> Result<Vec<String>, Fail> {
    steps.next("reading pg_hba.conf");
    ready(host)?;
    let server = locate(host)?;
    let hba_before = read_hba(&server.hba)?;
    let rules = hba::rules(&hba_before).map_err(|why| errors::not_passed(&why, ""))?;
    let goes = |rule: &hba::Rule| {
        rule.database == ask.name
            && ask.networks.as_ref().is_none_or(|networks| networks.contains(&rule.network))
    };
    let going: Vec<&hba::Rule> = rules.iter().filter(|rule| goes(rule)).collect();
    if going.is_empty() {
        return Ok(vec![format!("{} has no line in pg_hba.conf for other machines", ask.name)]);
    }
    let gone: Vec<String> = going.iter().map(|rule| rule.network.to_string()).collect();
    let hba_after =
        hba::without_rules(&hba_before, goes).map_err(|why| errors::not_passed(&why, ""))?;
    let open_to = hba::rules(&hba_after).map_err(|why| errors::not_passed(&why, ""))?;
    let left = open_to.len();
    let (firewall_before, wanted) = firewall_plan(host, &open_to)?;
    let settings = read_settings(host)?;
    let listening = running(host, settings::LISTEN)?;
    let changes = if left == 0 { settings.to_deny() } else { Vec::new() };
    let mut attempt = Attempt {
        auto: read_optional(&server.auto)?.map(String::into_bytes),
        server,
        over: host.files().over.clone(),
        hba: hba_before,
        over_before: None,
        firewall: host.files().firewall.clone(),
        firewall_before,
        settings,
        wrote_over: false,
        wrote_hba: false,
        wrote_firewall: false,
        applied: Vec::new(),
    };

    steps.next("taking the lines out of pg_hba.conf");
    attempt.wrote_hba = true;
    if let Err(why) = put(&attempt.server.hba, hba_after.as_bytes(), HBA_MODE) {
        return Err(stopped(host, &mut attempt, &why.message));
    }
    steps.next("setting what PostgreSQL listens with");
    if let Err(why) = apply(host, &mut attempt, &changes) {
        return Err(stopped(host, &mut attempt, &why.message));
    }
    steps.next("having PostgreSQL check its files");
    let found = match mistakes(host) {
        Ok(found) => found,
        Err(why) => return Err(stopped(host, &mut attempt, &why.message)),
    };
    if !found.is_empty() {
        let back = attempt.take_back(host, Then::Nothing);
        return Err(errors::not_passed(
            &format!("PostgreSQL's check found {}", found.join("; ")),
            &back,
        ));
    }
    steps.next(&format!("closing port {PORT} to the networks no database names"));
    if let Err(why) = write_firewall(host, &mut attempt, &wanted.text) {
        return Err(stopped(host, &mut attempt, &why));
    }
    let after =
        read_settings(host).map_err(|why| stopped_with(host, &mut attempt, &why, Then::Nothing))?;
    let restarts = after.needs_restart(&changes, &listening);
    steps.next(if restarts {
        "restarting PostgreSQL"
    } else {
        "asking PostgreSQL to read its files again"
    });
    let then = if restarts { Then::Restart } else { Then::Reload };
    let settled = if restarts { host.restart() } else { reload(host).map_err(|why| why.message) };
    if let Err(why) = settled {
        return Err(taken_back(host, &mut attempt, &why, then));
    }
    let wanted = after.effective(settings::LISTEN).unwrap_or(settings::LOCAL).to_string();
    match running(host, settings::LISTEN) {
        Ok(now) if now == wanted => {}
        Ok(now) => {
            let why = format!("PostgreSQL listens on {now}, not on {wanted}");
            return Err(taken_back(host, &mut attempt, &why, then));
        }
        Err(why) => return Err(taken_back(host, &mut attempt, &why.message, then)),
    }
    host.log(&format!("shut {} out of {}", gone.join(", "), ask.name));
    let mut words = vec![format!("{} may no longer connect to {}", gone.join(", "), ask.name)];
    if left == 0 && restarts {
        words.push("PostgreSQL listens on this machine only again, and restarted".into());
    }
    Ok(words)
}

/// `pg_hba.conf` as it will be with the lines of `database` gone, and
/// where it is.
pub struct Without {
    /// The file.
    path: PathBuf,
    /// Its new text.
    text: String,
    /// The database whose lines go.
    database: String,
}

/// `pg_hba.conf` without the lines of `database`, or none when it has
/// none there. Nothing is written.
///
/// # Errors
///
/// DB-104 when the block is not whole.
pub fn hba_without(host: &mut dyn Host, database: &str) -> Result<Option<Without>, Fail> {
    let server = locate(host)?;
    let text = read_hba(&server.hba)?;
    let rules = hba::rules(&text).map_err(|why| errors::not_passed(&why, ""))?;
    if !rules.iter().any(|rule| rule.database == database) {
        return Ok(None);
    }
    let new = hba::without_rules(&text, |rule| rule.database == database)
        .map_err(|why| errors::not_passed(&why, ""))?;
    Ok(Some(Without { path: server.hba, text: new, database: database.to_string() }))
}

/// Writes `pg_hba.conf` without the lines and has the server read it
/// again. The database is gone by now, and a failure says so.
///
/// # Errors
///
/// When the file cannot be written or the server does not read it again.
pub fn write_without(host: &mut dyn Host, without: &Without) -> Result<Vec<String>, Fail> {
    let done = put(&without.path, without.text.as_bytes(), HBA_MODE).and_then(|()| reload(host));
    match done {
        Ok(()) => Ok(vec![format!(
            "took {}'s lines out of pg_hba.conf, and PostgreSQL reads it again",
            without.database
        )]),
        Err(why) => Err(fail(format!(
            "the database {} was removed, but its lines stay in pg_hba.conf: {}",
            without.database, why.message
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::Fake;
    use super::*;

    fn net(text: &str) -> Network {
        Network::parse(text).unwrap()
    }

    fn lets(name: &str, networks: &[&str]) -> Allow {
        Allow {
            name: name.into(),
            networks: networks.iter().map(|text| net(text)).collect(),
            certificate: "this-machine".into(),
        }
    }

    fn shop() -> Fake {
        Fake::new().with_database("shop", "shop")
    }

    const CHAIN: &str = "/system/live/certificates/postgresql/this-machine/fullchain.pem";
    const KEY: &str = "/system/live/certificates/postgresql/this-machine/key.pem";

    #[test]
    fn a_network_is_let_in_by_the_lines_the_settings_and_a_restart() {
        let mut fake = shop();
        let original = fake.hba_text();
        let words = allow(&mut fake, &lets("shop", &["192.168.1.0/24"]), &Steps::off()).unwrap();
        assert_eq!(
            words,
            [
                "192.168.1.0/24 may connect to shop over TLS, as shop, with the password",
                "PostgreSQL restarted, and its open sessions ended"
            ]
        );
        assert_eq!(fake.hba_rules(), ["hostssl \"shop\" \"shop\" 192.168.1.0/24 scram-sha-256"]);
        assert!(fake.hba_text().starts_with(&original));
        assert_eq!(
            fake.changes(),
            [
                "ALTER SYSTEM SET listen_addresses = '*'".to_string(),
                "ALTER SYSTEM SET ssl = 'on'".to_string(),
                format!("ALTER SYSTEM SET ssl_cert_file = '{CHAIN}'"),
                format!("ALTER SYSTEM SET ssl_key_file = '{KEY}'"),
            ]
        );
        let over = fake.override_text().unwrap();
        assert!(over.contains("certificates = [\"this-machine\"]"), "{over}");
        // The server was started again, not asked to read its files again,
        // and was then asked how it listens and whether it takes TLS.
        assert_eq!((fake.restarts(), fake.reloads()), (1, 0));
        assert_eq!((fake.running("listen_addresses"), fake.running("ssl")), ("*", "on"));
        assert!(fake.sent().contains(&"(a TLS check with this-machine)".to_string()));
        assert_eq!(
            fake.logged(),
            ["let 192.168.1.0/24 connect to shop over TLS with this-machine's certificate"]
        );
    }

    #[test]
    fn a_setting_that_waits_for_the_restart_is_no_mistake() {
        let mut fake = shop();
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        // The server said so of listen_addresses, which only a restart sets.
        let said = fake.sent().iter().filter(|sql| sql.contains("pg_file_settings")).count();
        assert!(said >= 1);
        assert_eq!(fake.restarts(), 1);
    }

    #[test]
    fn another_network_for_a_server_that_listens_already_is_a_reload() {
        let mut fake = shop().listening_with("this-machine").with_hba_lines(&[(
            "shop",
            "shop",
            "192.168.1.0/24",
        )]);
        // The override names the certificate already.
        fs::create_dir_all(fake.files().over.parent().unwrap()).unwrap();
        fs::write(&fake.files().over, "certificates = [\"this-machine\"]\n").unwrap();
        let words = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        assert_eq!(words, ["10.0.2.0/24 may connect to shop over TLS, as shop, with the password"]);
        assert!(fake.changes().is_empty(), "{:?}", fake.changes());
        assert_eq!((fake.restarts(), fake.reloads()), (0, 1));
        assert_eq!(
            fake.hba_rules(),
            [
                "hostssl \"shop\" \"shop\" 192.168.1.0/24 scram-sha-256",
                "hostssl \"shop\" \"shop\" 10.0.2.0/24 scram-sha-256"
            ]
        );
    }

    #[test]
    fn the_same_again_changes_nothing_and_asks_for_no_restart() {
        let mut fake = shop();
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        let (hba, over, auto) = (fake.hba_text(), fake.override_text(), fake.auto_file());
        let restarts = fake.restarts();
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        assert_eq!((fake.hba_text(), fake.override_text(), fake.auto_file()), (hba, over, auto));
        assert_eq!(fake.restarts(), restarts);
        assert_eq!(fake.reloads(), 1);
    }

    #[test]
    fn a_certificate_that_certd_has_not_is_db_105_and_nothing_is_changed() {
        let mut fake = shop();
        let ask = Allow { certificate: "nas".into(), ..lets("shop", &["10.0.2.0/24"]) };
        let failure = allow(&mut fake, &ask, &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("(DB-105)"), "{}", failure.message);
        assert!(fake.changes().is_empty());
        assert_eq!(fake.override_text(), None);
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
    }

    #[test]
    fn a_database_that_is_not_there_or_has_no_password_is_refused_before_a_change() {
        let mut fake = Fake::new();
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("no database called shop"), "{}", failure.message);
        assert!(!failure.message.contains("DB-1"));
        let mut fake = Fake::new().with_database("notes", "webdev").without_password("webdev");
        let failure =
            allow(&mut fake, &lets("notes", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("has no password"), "{}", failure.message);
        assert!(fake.changes().is_empty());
    }

    #[test]
    fn a_server_that_is_not_there_is_db_101() {
        let mut fake = shop().off();
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("(DB-101)"), "{}", failure.message);
    }

    #[test]
    fn a_mistake_in_pg_hba_conf_puts_every_file_back_and_is_db_104() {
        let mut fake = shop();
        let original = format!("{}host all all BROKEN scram-sha-256\n", fake.hba_text());
        fake.write_hba(&original);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(
            failure.message.contains("pg_hba.conf line 7: invalid IP address \"BROKEN\""),
            "{}",
            failure.message
        );
        assert!(failure.message.ends_with("nothing was changed (DB-104)"), "{}", failure.message);
        assert_eq!(fake.hba_text(), original);
        assert_eq!(fake.override_text(), None);
        assert!(fake.auto().is_empty(), "{:?}", fake.auto());
        // Each setting set was reset.
        let changes = fake.changes();
        assert_eq!(changes.iter().filter(|sql| sql.starts_with("ALTER SYSTEM SET")).count(), 4);
        assert_eq!(changes.iter().filter(|sql| sql.starts_with("ALTER SYSTEM RESET")).count(), 4);
        // Nothing took effect: no reload, no restart.
        assert_eq!((fake.restarts(), fake.reloads()), (0, 0));
        assert!(fake.logged().is_empty());
    }

    #[test]
    fn a_mistake_in_the_settings_is_found_the_same_way() {
        let mut fake =
            shop().with_settings_error("invalid value for parameter \"max_connections\"");
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(
            failure.message.contains("postgresql.conf line 7 (max_connections): invalid value"),
            "{}",
            failure.message
        );
        assert!(failure.message.ends_with("(DB-104)"));
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
        assert_eq!(fake.override_text(), None);
    }

    #[test]
    fn what_alter_system_had_before_is_what_a_failed_allow_leaves() {
        // The server listened with another certificate before.
        let mut fake = shop().listening_with("old").with_settings_error("no");
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert_eq!(
            fake.auto().get("ssl_cert_file").map(String::as_str),
            Some("/system/live/certificates/postgresql/old/fullchain.pem")
        );
        assert_eq!(fake.auto().get("listen_addresses").map(String::as_str), Some("*"));
        assert_eq!(
            fake.auto().get("ssl_key_file").map(String::as_str),
            Some("/system/live/certificates/postgresql/old/key.pem")
        );
    }

    #[test]
    fn a_server_that_does_not_come_back_has_everything_put_back_and_is_started_again() {
        let mut fake = shop().failing_to_restart(1);
        let original = fake.hba_text();
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("did not come back"), "{}", failure.message);
        assert!(
            failure.message.contains("the change was taken back, and PostgreSQL runs again"),
            "{}",
            failure.message
        );
        assert!(failure.message.ends_with("(DB-104)"));
        assert_eq!(fake.hba_text(), original);
        assert_eq!(fake.override_text(), None);
        // The file ALTER SYSTEM writes is put back too, as the server was
        // not there to be asked.
        assert!(fake.auto().is_empty(), "{:?}", fake.auto());
        assert_eq!(fake.restarts(), 2);
        assert_eq!(fake.running("listen_addresses"), "localhost");
        assert!(fake.logged().is_empty());
    }

    #[test]
    fn a_server_that_cannot_be_started_again_either_says_so() {
        let mut fake = shop().failing_to_restart(2);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("could not be started again"), "{}", failure.message);
        assert!(failure.message.ends_with("(DB-104)"));
        assert_eq!(fake.restarts(), 2);
    }

    #[test]
    fn a_certificate_that_does_not_load_is_found_by_the_tls_check_and_taken_back() {
        let mut fake = shop().failing_the_tls_check(
            "TLS with PostgreSQL at 127.0.0.1:5432 for localhost failed: bad certificate",
        );
        let original = fake.hba_text();
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("bad certificate"), "{}", failure.message);
        assert!(failure.message.contains("taken back"));
        assert_eq!(fake.hba_text(), original);
        assert!(fake.auto().is_empty());
        assert_eq!(fake.running("listen_addresses"), "localhost");
        assert_eq!(fake.restarts(), 2);
        assert!(fake.logged().is_empty());
    }

    #[test]
    fn a_server_that_does_not_read_its_files_again_is_told() {
        let mut fake = shop()
            .listening_with("this-machine")
            .with_hba_lines(&[("shop", "shop", "192.168.1.0/24")])
            .deaf();
        fs::create_dir_all(fake.files().over.parent().unwrap()).unwrap();
        fs::write(&fake.files().over, "certificates = [\"this-machine\"]\n").unwrap();
        let before = fake.hba_text();
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(
            failure.message.contains("did not read its files again within 10 seconds"),
            "{}",
            failure.message
        );
        assert!(failure.message.ends_with("(DB-104)"));
        assert_eq!(fake.hba_text(), before);
    }

    #[test]
    fn a_role_that_cannot_be_named_in_the_file_is_not_let_in() {
        let mut fake = Fake::new().with_database("shop", "Shop Owner");
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("cannot be named in pg_hba.conf"), "{}", failure.message);
        assert!(fake.changes().is_empty());
    }

    #[test]
    fn a_hand_edit_of_the_override_and_the_file_is_kept() {
        let mut fake = shop().with_hba_lines(&[("blog", "blog", "10.0.0.0/8")]);
        fake.write_hba(&format!("# mine\n{}", fake.hba_text()));
        fs::create_dir_all(fake.files().over.parent().unwrap()).unwrap();
        fs::write(&fake.files().over, "# my own\ncertificates = [\"mine.example\"]\n").unwrap();
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        let over = fake.override_text().unwrap();
        assert!(
            over.starts_with("# my own\ncertificates = [\"mine.example\", \"this-machine\"]"),
            "{over}"
        );
        assert!(fake.hba_text().starts_with("# mine\n"));
        assert_eq!(fake.hba_rules().len(), 2);
    }

    /// A server that lets `shop` in from `networks`.
    fn open_to(networks: &[&str]) -> Fake {
        let mut fake = shop();
        allow(&mut fake, &lets("shop", networks), &Steps::off()).unwrap();
        fake
    }

    fn all_of(name: &str) -> Deny {
        Deny { name: name.into(), networks: None }
    }

    #[test]
    fn denying_the_last_lines_has_the_server_listen_on_this_machine_only_again() {
        let mut fake = shop();
        let original = fake.hba_text();
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        let restarts = fake.restarts();
        let words = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(
            words,
            [
                "10.0.2.0/24 may no longer connect to shop",
                "PostgreSQL listens on this machine only again, and restarted"
            ]
        );
        // The block and its blank line are gone, the file as it was.
        assert_eq!(fake.hba_text(), original);
        assert_eq!(fake.running("listen_addresses"), "localhost");
        assert_eq!(fake.restarts(), restarts + 1);
        let changes = fake.changes();
        assert_eq!(changes.last().unwrap(), "ALTER SYSTEM RESET listen_addresses");
        // TLS and the certificate stay: the server starts with them, and the
        // copies of the certificate stay for it.
        assert_eq!(fake.auto().get("ssl").map(String::as_str), Some("on"));
        assert!(fake.auto().contains_key("ssl_cert_file"));
        assert!(fake.override_text().unwrap().contains("this-machine"));
        assert_eq!(fake.logged().last().unwrap(), "shut 10.0.2.0/24 out of shop");
    }

    #[test]
    fn denying_one_network_of_two_is_a_reload_and_keeps_the_rest() {
        let mut fake = open_to(&["10.0.2.0/24", "192.168.1.0/24"]);
        let (restarts, reloads) = (fake.restarts(), fake.reloads());
        let ask = Deny { name: "shop".into(), networks: Some(vec![net("10.0.2.77/24")]) };
        let words = deny(&mut fake, &ask, &Steps::off()).unwrap();
        assert_eq!(words, ["10.0.2.0/24 may no longer connect to shop"]);
        assert_eq!(fake.hba_rules(), ["hostssl \"shop\" \"shop\" 192.168.1.0/24 scram-sha-256"]);
        assert_eq!((fake.restarts(), fake.reloads()), (restarts, reloads + 1));
        assert_eq!(fake.running("listen_addresses"), "*");
    }

    #[test]
    fn denying_a_database_while_another_is_still_let_in_is_a_reload() {
        let mut fake = open_to(&["10.0.2.0/24"]);
        fake = fake.with_database("blog", "blog");
        allow(&mut fake, &lets("blog", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        let restarts = fake.restarts();
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(fake.hba_rules(), ["hostssl \"blog\" \"blog\" 10.0.2.0/24 scram-sha-256"]);
        assert_eq!(fake.restarts(), restarts);
        assert_eq!(fake.running("listen_addresses"), "*");
        assert!(!fake.changes().iter().any(|sql| sql.starts_with("ALTER SYSTEM RESET")));
    }

    #[test]
    fn denying_what_is_not_there_changes_nothing() {
        let mut fake = shop();
        let words = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(words, ["shop has no line in pg_hba.conf for other machines"]);
        assert_eq!((fake.changes(), fake.restarts(), fake.reloads()), (vec![], 0, 0));
        assert_eq!((fake.firewall_text(), fake.firewall_loads()), (None, 0));
        let mut fake = open_to(&["10.0.2.0/24"]);
        let (reloads, loads) = (fake.reloads(), fake.firewall_loads());
        let ask = Deny { name: "shop".into(), networks: Some(vec![net("10.9.0.0/16")]) };
        let words = deny(&mut fake, &ask, &Steps::off()).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!((fake.reloads(), fake.firewall_loads()), (reloads, loads));
        assert_eq!(fake.hba_rules().len(), 1);
    }

    #[test]
    fn a_deny_that_the_check_refuses_puts_the_file_back() {
        let mut fake = open_to(&["10.0.2.0/24"]);
        let broken = format!("{}host all all BROKEN scram-sha-256\n", fake.hba_text());
        fake.write_hba(&broken);
        let failure = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("nothing was changed (DB-104)"), "{}", failure.message);
        assert_eq!(fake.hba_text(), broken);
        assert_eq!(fake.running("listen_addresses"), "*");
        // The setting was set back to what the file had.
        assert_eq!(fake.auto().get("listen_addresses").map(String::as_str), Some("*"));
    }

    #[test]
    fn a_deny_whose_server_does_not_come_back_is_taken_back() {
        let mut fake = open_to(&["10.0.2.0/24"]);
        let hba = fake.hba_text();
        fake = fake.failing_to_restart(1);
        let failure = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("the change was taken back"), "{}", failure.message);
        assert!(failure.message.ends_with("(DB-104)"));
        assert_eq!(fake.hba_text(), hba);
        assert_eq!(fake.auto().get("listen_addresses").map(String::as_str), Some("*"));
        assert_eq!(fake.running("listen_addresses"), "*");
    }

    #[test]
    fn a_block_that_is_not_whole_is_db_104_for_both() {
        let mut fake = shop();
        fake.write_hba(&format!("local all all peer\n{}\n", hba::BEGIN));
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("(DB-104)"), "{}", failure.message);
        let failure = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("(DB-104)"), "{}", failure.message);
        assert!(fake.changes().is_empty());
    }

    /// The block of the firewall's rules that opens port 5432 to
    /// `networks`, as `database` writes it.
    fn block_for(networks: &[&str]) -> String {
        let mut text =
            String::from("# database(8): who reaches PostgreSQL from other machines (ADR 0570)\n");
        for network in networks {
            text.push_str(&format!("allow tcp 5432 from {network}\n"));
        }
        text.push_str("deny tcp 5432\n# database(8): end\n");
        text
    }

    /// A server with `shop` and `blog`, neither let in.
    fn two() -> Fake {
        shop().with_database("blog", "blog")
    }

    #[test]
    fn a_network_that_is_let_in_gets_port_5432_in_the_firewall_and_the_firewall_loads_it() {
        let mut fake = shop();
        allow(&mut fake, &lets("shop", &["192.168.1.0/24"]), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), block_for(&["192.168.1.0/24"]));
        assert_eq!(fake.firewall_loads(), 1);
        // The same again writes nothing and loads nothing.
        allow(&mut fake, &lets("shop", &["192.168.1.0/24"]), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), block_for(&["192.168.1.0/24"]));
        assert_eq!(fake.firewall_loads(), 1);
    }

    #[test]
    fn the_block_is_the_union_of_every_database_s_networks_each_once_in_the_order_seen() {
        let mut fake = two();
        allow(&mut fake, &lets("shop", &["192.168.1.0/24"]), &Steps::off()).unwrap();
        allow(&mut fake, &lets("blog", &["10.0.0.0/8", "192.168.1.0/24"]), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), block_for(&["192.168.1.0/24", "10.0.0.0/8"]));
        assert_eq!(fake.firewall_loads(), 2);
    }

    #[test]
    fn what_the_administrator_wrote_in_the_firewall_s_rules_is_kept_as_it_was() {
        let before = "# Mine.\nallow tcp 22   # ssh\n\nallow tcp 8080";
        let mut fake = shop().with_firewall_rules(before);
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        assert_eq!(
            fake.firewall_text().unwrap(),
            format!("{before}\n\n{}", block_for(&["10.0.2.0/24"]))
        );
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), format!("{before}\n"));
    }

    #[test]
    fn denying_one_database_keeps_the_networks_of_the_other() {
        let mut fake = two();
        allow(&mut fake, &lets("shop", &["192.168.1.0/24"]), &Steps::off()).unwrap();
        allow(&mut fake, &lets("blog", &["10.0.0.0/8"]), &Steps::off()).unwrap();
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), block_for(&["10.0.0.0/8"]));
        assert_eq!(fake.firewall_loads(), 3);
    }

    #[test]
    fn denying_a_network_that_another_database_has_too_keeps_it_in_the_block() {
        let mut fake = two();
        allow(&mut fake, &lets("shop", &["10.0.2.0/24", "192.168.1.0/24"]), &Steps::off()).unwrap();
        allow(&mut fake, &lets("blog", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        let ask = Deny { name: "shop".into(), networks: Some(vec![net("10.0.2.0/24")]) };
        deny(&mut fake, &ask, &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), block_for(&["192.168.1.0/24", "10.0.2.0/24"]));
    }

    #[test]
    fn denying_the_last_network_takes_the_block_away_and_leaves_the_rest() {
        let before = "# Mine.\nallow tcp 22\n";
        let mut fake = shop().with_firewall_rules(before);
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), before);
        assert_eq!(fake.firewall_loads(), 2);
        // A second deny has nothing to take out and loads nothing.
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_loads(), 2);
    }

    #[test]
    fn a_server_opened_before_the_firewall_had_a_block_gets_one_from_the_next_deny() {
        let mut fake = shop()
            .listening_with("this-machine")
            .with_hba_lines(&[("shop", "shop", "10.0.2.0/24"), ("shop", "shop", "192.168.1.0/24")]);
        assert_eq!(fake.firewall_text(), None);
        let ask = Deny { name: "shop".into(), networks: Some(vec![net("10.0.2.0/24")]) };
        deny(&mut fake, &ask, &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), block_for(&["192.168.1.0/24"]));
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!(fake.firewall_text().unwrap(), "");
        // With no block before and none wanted, the file is not made.
        let mut fake = shop().listening_with("this-machine").with_hba_lines(&[(
            "shop",
            "shop",
            "10.0.2.0/24",
        )]);
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap();
        assert_eq!((fake.firewall_text(), fake.firewall_loads()), (None, 0));
    }

    #[test]
    fn a_firewall_trial_refuses_an_allow_with_db_107_before_anything_is_changed() {
        let mut fake = shop().with_a_firewall_trial();
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("nothing was changed (DB-107)"), "{}", failure.message);
        assert!(failure.message.contains("elevate firewall keep"), "{}", failure.message);
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
        assert_eq!((fake.override_text(), fake.firewall_text()), (None, None));
        assert!(fake.auto().is_empty() && fake.changes().is_empty());
        assert_eq!((fake.restarts(), fake.reloads(), fake.firewall_loads()), (0, 0, 0));
        assert!(fake.logged().is_empty());
    }

    #[test]
    fn a_firewall_trial_refuses_a_deny_that_has_something_to_do_and_no_other() {
        let mut fake = shop()
            .listening_with("this-machine")
            .with_hba_lines(&[("shop", "shop", "10.0.2.0/24")])
            .with_a_firewall_trial();
        let before = fake.hba_text();
        let failure = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("nothing was changed (DB-107)"), "{}", failure.message);
        assert_eq!(fake.hba_text(), before);
        assert_eq!((fake.restarts(), fake.reloads(), fake.firewall_loads()), (0, 0, 0));
        assert_eq!(fake.running("listen_addresses"), "*");
        let words = deny(&mut fake, &all_of("blog"), &Steps::off()).unwrap();
        assert_eq!(words, ["blog has no line in pg_hba.conf for other machines"]);
    }

    #[test]
    fn a_block_that_is_not_whole_is_db_108_for_both_and_nothing_is_changed() {
        let broken = "allow tcp 22\n# database(8): end\n";
        let mut fake = shop().with_firewall_rules(broken);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("(DB-108)"), "{}", failure.message);
        assert!(failure.message.contains("line 2 ends a block that no line starts"));
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
        assert_eq!((fake.override_text(), fake.firewall_text().as_deref()), (None, Some(broken)));
        assert!(fake.changes().is_empty());
        assert_eq!((fake.restarts(), fake.reloads(), fake.firewall_loads()), (0, 0, 0));
        let mut fake = shop()
            .listening_with("this-machine")
            .with_hba_lines(&[("shop", "shop", "10.0.2.0/24")])
            .with_firewall_rules(broken);
        let before = fake.hba_text();
        let failure = deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap_err();
        assert!(failure.message.ends_with("(DB-108)"), "{}", failure.message);
        assert_eq!(fake.hba_text(), before);
        assert_eq!((fake.restarts(), fake.reloads(), fake.firewall_loads()), (0, 0, 0));
    }

    #[test]
    fn a_file_that_would_hold_more_rules_than_the_firewall_does_is_db_108() {
        let full = "allow tcp 22\n".repeat(127);
        let mut fake = shop().with_firewall_rules(&full);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("129 rules"), "{}", failure.message);
        assert!(failure.message.ends_with("(DB-108)"));
        assert_eq!(fake.firewall_text().as_deref(), Some(full.as_str()));
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
    }

    #[test]
    fn a_rule_of_the_administrator_s_that_decides_first_is_told_and_not_moved() {
        let mut fake = shop().with_firewall_rules("allow tcp 5432\n");
        let words = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap();
        assert_eq!(words.len(), 3, "{words:?}");
        assert_eq!(
            words[2],
            "the firewall rule `allow tcp 5432` comes before database's rules for port 5432 and \
             decides first for the connections it names; database does not move it"
        );
        assert_eq!(
            fake.firewall_text().unwrap(),
            format!("allow tcp 5432\n\n{}", block_for(&["10.0.2.0/24"]))
        );
    }

    #[test]
    fn a_failure_after_the_firewall_step_puts_its_file_back_and_has_it_loaded_again() {
        let before = "# Mine.\nallow tcp 22\n";
        let mut fake = shop().with_firewall_rules(before).failing_to_restart(1);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(failure.message.contains("the change was taken back"), "{}", failure.message);
        assert!(failure.message.ends_with("(DB-104)"));
        assert_eq!(fake.firewall_text().as_deref(), Some(before));
        assert_eq!(fake.firewall_loads(), 2, "written and loaded, then put back and loaded");
        // A file that was not there is not there.
        let mut fake = shop().failing_the_tls_check("TLS with PostgreSQL failed: bad certificate");
        allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert_eq!((fake.firewall_text(), fake.firewall_loads()), (None, 2));
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
    }

    #[test]
    fn a_deny_that_fails_after_the_firewall_step_has_the_block_back() {
        let mut fake = open_to(&["10.0.2.0/24"]);
        let open = fake.firewall_text().unwrap();
        fake = fake.failing_to_restart(1);
        deny(&mut fake, &all_of("shop"), &Steps::off()).unwrap_err();
        assert_eq!(fake.firewall_text().unwrap(), open);
        assert_eq!(fake.firewall_loads(), 3);
    }

    #[test]
    fn a_firewall_that_does_not_take_its_rules_has_its_file_put_back_before_any_restart() {
        let before = "allow tcp 22\n";
        let mut fake = shop().with_firewall_rules(before).failing_to_load_the_firewall(1);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(
            failure.message.contains("the firewall did not take its rules: the kernel refused"),
            "{}",
            failure.message
        );
        assert!(failure.message.ends_with("nothing was changed (DB-104)"), "{}", failure.message);
        assert_eq!(fake.firewall_text().as_deref(), Some(before));
        assert_eq!(fake.hba_rules(), Vec::<String>::new());
        assert!(fake.auto().is_empty());
        assert_eq!((fake.restarts(), fake.reloads(), fake.firewall_loads()), (0, 0, 2));
        // The rules put back are loaded again, and the firewall says if it
        // cannot.
        let mut fake = shop().failing_to_load_the_firewall(2);
        let failure = allow(&mut fake, &lets("shop", &["10.0.2.0/24"]), &Steps::off()).unwrap_err();
        assert!(
            failure.message.contains("but the firewall could not load the rules put back"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn the_steps_counted_are_those_each_command_tells() {
        assert_eq!((ALLOW_STEPS, DENY_STEPS), (8, 6));
    }
}
