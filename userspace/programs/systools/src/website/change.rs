// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The changes `website` makes to a machine's websites (ADR 0569): `add`
//! and `https`, and what `remove` shares with them.
//!
//! Every change is checked before anything is changed, and each step
//! that changes something is put in [`Done`] as it is taken, so that a
//! failure at any step takes every earlier step back, last first: the
//! site files are put back as they were, the override of nginx's service
//! file is restored and nginx restarted or told to read its settings
//! again, a service that was turned on is turned off, and the folders
//! and files made are removed. A change that is taken back says why it
//! failed and what could not be taken back.
//!
//! The order is the one the check needs. nginx's own check runs on the new
//! set of site files before it is live, so that settings nginx refuses
//! never reach a restart or a reload; the files go live one rename each;
//! and a file `website` put in the site's folder is asked for over this
//! machine's own address to see that nginx serves what was meant.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

use ring::rand::{SecureRandom, SystemRandom};

use super::answer::{self, Probe};
use super::conf::{self, Facts, Kind};
use super::folder::{self, CheckFiles, Made, Make, Owner};
use super::host::{BUNDLE_CONF, Host, NGINX, NGINX_BUNDLE, PHP_BUNDLE, PHP_FPM, Paths, Svc};
use super::plan::{
    self, DEFAULT_FILE, Main, SETTINGS, Spec, Wanted, default_text, named_certificates, site_text,
};
use super::stage::{self, Lock, Raw, Staged, conf_view};
use crate::cert::acme::steps::Steps;
use crate::settings_edit;
use crate::util::{self, Fail, strip_os_error};

/// nginx's service file.
pub const SERVICE: &str = "/system/settings/spark/services/nginx.toml";

/// The override of nginx's service file, which `website` changes.
pub const OVERRIDE: &str = "/system/settings/spark/services/settings.d/nginx.toml";

/// What a change is run with.
pub struct Ctx<'a> {
    /// The files.
    pub paths: &'a Paths,
    /// The machine.
    pub host: &'a dyn Host,
    /// The steps, told as they begin.
    pub steps: &'a Steps,
}

/// A change that does not go on, with the words for it.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The error's ID, `WEB-103` and the like, when it has one.
    pub id: Option<&'static str>,
    /// What is wrong, and what to do.
    pub words: String,
    /// Whether it is a mistake in what was asked.
    pub usage: bool,
}

impl Refusal {
    /// A refusal with the ID `id`.
    pub fn new(id: &'static str, words: impl Into<String>) -> Refusal {
        Refusal { id: Some(id), words: words.into(), usage: false }
    }

    /// A failure with no ID.
    pub fn plain(words: impl Into<String>) -> Refusal {
        Refusal { id: None, words: words.into(), usage: false }
    }

    /// A mistake in what was asked.
    pub fn usage(words: impl Into<String>) -> Refusal {
        Refusal { id: None, words: words.into(), usage: true }
    }

    /// The failure a tool ends with: its words, and its ID at the end of
    /// the last line, where the web console reads it.
    pub fn into_fail(self) -> Fail {
        let message = match self.id {
            Some(id) => format!("{} ({id})", self.words),
            None => self.words,
        };
        if self.usage { util::usage(message) } else { util::fail(message) }
    }

    /// This, with what could not be taken back after it.
    pub fn and_left(mut self, left: &[String]) -> Refusal {
        if !left.is_empty() {
            self.words.push_str(&format!("\nIt could not be taken back: {}", left.join("; ")));
        }
        self
    }
}

/// A site's file as it is.
#[derive(Clone, Debug)]
pub struct Site {
    /// The site's name.
    pub name: String,
    /// Its file's name.
    pub file: String,
    /// What it holds.
    pub text: String,
    /// What it states, or why it does not read.
    pub facts: Result<Facts, String>,
}

/// nginx's own settings as they are.
#[derive(Clone, Debug)]
pub struct Conf {
    /// What `nginx.conf` holds.
    pub text: String,
    /// What the websites need of it, or why it does not read.
    pub main: Result<Main, String>,
}

/// What a change finds before it changes anything.
#[derive(Clone, Debug)]
pub struct State {
    /// Every file nginx includes of the sites' folder.
    pub raw: Vec<Raw>,
    /// Those that are sites.
    pub sites: Vec<Site>,
    /// nginx's settings, none until it has started.
    pub conf: Option<Conf>,
    /// nginx's service file.
    pub service: String,
    /// The override of it.
    pub over: Option<String>,
    /// What Spark says of nginx.
    pub nginx: Option<Svc>,
    /// What Spark says of PHP-FPM.
    pub php: Option<Svc>,
}

/// A file's text, or none when it is not there.
fn read_text(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {}", path.display(), strip_os_error(&error))),
    }
}

/// The settings file `conf` as a [`Conf`].
fn conf_of(text: String) -> Conf {
    Conf { main: plan::read_main(&text), text }
}

/// Reads what a change needs to know.
///
/// # Errors
///
/// When a file cannot be read, or Spark cannot be asked.
pub fn load(ctx: &Ctx<'_>) -> Result<State, Refusal> {
    load_with(ctx, true)
}

/// [`load`], with nginx's own settings read only when `conf` says so: they
/// are in nginx's data folder, which only root reads, and what anyone may
/// list does not need them.
///
/// # Errors
///
/// When a file cannot be read, or Spark cannot be asked.
pub fn load_with(ctx: &Ctx<'_>, conf: bool) -> Result<State, Refusal> {
    let plain = Refusal::plain;
    let raw = stage::read_settings(ctx.paths).map_err(plain)?;
    let sites = raw
        .iter()
        .filter(|file| !file.file.starts_with('_'))
        .map(|file| {
            let text = String::from_utf8_lossy(&file.bytes).into_owned();
            let facts = conf::parse(&text).map(|read| conf::facts(&read));
            let name = file.file.trim_end_matches(".conf").to_string();
            Site { name, file: file.file.clone(), text, facts }
        })
        .collect();
    let conf = if conf { stage::read_conf(ctx.paths).map_err(plain)?.map(conf_of) } else { None };
    let service = read_text(&ctx.paths.real(SERVICE)).map_err(plain)?.unwrap_or_default();
    let over = read_text(&ctx.paths.real(OVERRIDE)).map_err(plain)?;
    Ok(State {
        raw,
        sites,
        conf,
        service,
        over,
        nginx: ctx.host.state(NGINX).map_err(plain)?,
        php: ctx.host.state(PHP_FPM).map_err(plain)?,
    })
}

/// A site file that was written live, and what was there before.
#[derive(Clone, Debug)]
pub struct Live {
    /// Its name.
    pub file: String,
    /// What it held; none when it was not there.
    pub old: Option<Vec<u8>>,
}

/// What a change has done, to be taken back when a later step fails.
#[derive(Debug, Default)]
pub struct Done {
    /// The folders and files made for a site.
    pub made: Made,
    /// Whether the sites' settings folder was made.
    pub settings_made: bool,
    /// The override as it was before it was written; none when it was
    /// not written.
    pub over: Option<Option<String>>,
    /// Whether nginx was told to restart, which it may not have done.
    pub restarted: bool,
    /// Whether nginx was told to read its settings again.
    pub reloaded: bool,
    /// The site files written or removed, in order.
    pub live: Vec<Live>,
    /// The services turned on, in order.
    pub turned_on: Vec<&'static str>,
}

/// Puts the override back as it was: its old text, or no file.
fn restore_override(paths: &Paths, old: Option<&str>) -> Result<(), String> {
    let path = paths.real(OVERRIDE);
    let result = match old {
        Some(text) => settings_edit::write(&path, text.as_bytes(), 0o644).map(|_| ()),
        None => fs::remove_file(&path),
    };
    match result {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(format!("{OVERRIDE}: {}", strip_os_error(&error)))
        }
        _ => Ok(()),
    }
}

/// Takes back what `done` holds, the last step first. What could not be
/// taken back is told.
pub fn rollback(ctx: &Ctx<'_>, done: &Done) -> Vec<String> {
    let mut left = Vec::new();
    for live in done.live.iter().rev() {
        let path = ctx.paths.real(&format!("{SETTINGS}/{}", live.file));
        let result = match &live.old {
            Some(bytes) => settings_edit::write(&path, bytes, 0o644).map(|_| ()),
            None => fs::remove_file(&path),
        };
        match result {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                left.push(format!("{SETTINGS}/{}: {}", live.file, strip_os_error(&error)));
            }
            _ => {}
        }
    }
    if let Some(old) = &done.over
        && let Err(why) = restore_override(ctx.paths, old.as_deref())
    {
        left.push(why);
    }
    // nginx that was turned on goes off with its service file back; any
    // other reads what is back as it restarts or reloads.
    if !done.turned_on.contains(&NGINX) {
        if done.restarted {
            if let Err(why) = ctx.host.restart(NGINX) {
                left.push(format!("nginx did not restart: {why}"));
            }
        } else if done.reloaded
            && let Err(why) = ctx.host.reload(NGINX)
        {
            left.push(format!("nginx did not read its settings again: {why}"));
        }
    }
    for service in done.turned_on.iter().rev() {
        if let Err(why) = ctx.host.turn(service, false) {
            left.push(format!("{service} was not turned off: {why}"));
        }
    }
    left.extend(folder::undo(ctx.paths, &done.made));
    if done.settings_made {
        // Only when nothing is in it.
        let _ = fs::remove_dir(ctx.paths.real(SETTINGS));
    }
    left
}

/// One result of step 1: what was checked, and how it came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// The error's ID, when a failure has one.
    pub id: Option<&'static str>,
    /// `passed`, `warning` or `failed`.
    pub result: &'static str,
    /// What was checked, and what it found.
    pub words: String,
    /// Whether a failure is a mistake in what was asked.
    pub usage: bool,
}

/// The checks of step 1 as they come, which stop the change at the first
/// that fails, or all go on to be told.
pub struct Checks {
    /// What was checked.
    pub list: Vec<Check>,
    stop: bool,
}

impl Checks {
    /// Checks that stop at the first failure.
    pub fn stopping() -> Checks {
        Checks { list: Vec::new(), stop: true }
    }

    /// Checks that all are made, for a plan.
    pub fn all() -> Checks {
        Checks { list: Vec::new(), stop: false }
    }

    /// A check that passed.
    pub fn pass(&mut self, words: impl Into<String>) {
        self.list.push(Check { id: None, result: "passed", words: words.into(), usage: false });
    }

    /// A check that passed with something to say.
    pub fn warn(&mut self, words: impl Into<String>) {
        self.list.push(Check { id: None, result: "warning", words: words.into(), usage: false });
    }

    /// A check that failed.
    ///
    /// # Errors
    ///
    /// The refusal, when the checks stop at a failure.
    pub fn fail(
        &mut self,
        id: Option<&'static str>,
        words: impl Into<String>,
        usage: bool,
    ) -> Result<(), Refusal> {
        let words = words.into();
        self.list.push(Check { id, result: "failed", words: words.clone(), usage });
        if self.stop { Err(Refusal { id, words, usage }) } else { Ok(()) }
    }

    /// Whether one failed.
    pub fn failed(&self) -> bool {
        self.list.iter().any(|check| check.result == "failed")
    }
}

/// What nginx's check said of its settings before any change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Now {
    /// They pass.
    Passed,
    /// They do not, with nginx's line.
    Failed(Option<String>),
    /// They were not checked.
    Skipped,
}

/// `names` as one list in words.
fn list_of(names: &[String]) -> String {
    names.join(", ")
}

/// Waits for nginx's first start to write its settings.
fn wait_for_conf(ctx: &Ctx<'_>) -> Result<Conf, Refusal> {
    let deadline = Instant::now() + ctx.host.patience() * 3;
    loop {
        if let Some(text) = stage::read_conf(ctx.paths).map_err(Refusal::plain)? {
            return Ok(conf_of(text));
        }
        if Instant::now() >= deadline {
            return Err(Refusal::plain(
                "nginx did not write its settings in its data folder after it was turned on",
            ));
        }
        ctx.host.pause();
    }
}

/// The checks of nginx's settings that every change begins with: `nginx.conf`
/// has the two lines the websites need, and passes nginx's check as it is.
/// When nginx has never started it is turned on first, as `done` takes
/// note of, unless `done` is none, which is a plan: then the bundle's own
/// settings are read and nginx's check is left for later.
///
/// # Errors
///
/// The first check that fails, when `checks` stop at a failure, and a
/// failure that stops any change.
pub fn conf_checks(
    ctx: &Ctx<'_>,
    state: &mut State,
    checks: &mut Checks,
    done: Option<&mut Done>,
) -> Result<(Option<Main>, Now), Refusal> {
    let mut path = conf_view();
    let mut live = true;
    if state.conf.is_none() {
        match done {
            Some(done) => {
                done.turned_on.push(NGINX);
                ctx.host.turn(NGINX, true).map_err(|why| {
                    Refusal::plain(format!(
                        "nginx could not be turned on to write its settings: {why}"
                    ))
                })?;
                state.conf = Some(wait_for_conf(ctx)?);
                state.nginx = Some(Svc { enabled: true, running: true });
            }
            None => {
                let bundled = read_text(&ctx.paths.real(BUNDLE_CONF)).map_err(Refusal::plain)?;
                state.conf = bundled.map(conf_of);
                path = BUNDLE_CONF.to_string();
                live = false;
                checks.warn(
                    "nginx has not started yet, so its settings are checked once it has: it \
                     writes them from the bundle's as it starts",
                );
            }
        }
    }
    let Some(conf) = state.conf.clone() else { return Ok((None, Now::Skipped)) };
    let main = match &conf.main {
        Ok(main) => main.clone(),
        Err(why) => {
            checks.fail(
                Some("WEB-102"),
                format!("nginx's settings, {path}, cannot be read here: {why}"),
                false,
            )?;
            return Ok((None, Now::Skipped));
        }
    };
    match plan::conf_words(&path, &main) {
        Some(words) => {
            checks.fail(Some("WEB-102"), words, false)?;
            return Ok((Some(main), Now::Skipped));
        }
        None => checks.pass("nginx's settings include the websites' folder"),
    }
    if !live {
        return Ok((Some(main), Now::Skipped));
    }
    match ctx.host.nginx_check(&conf_view()) {
        Ok(checked) if checked.passed => {
            checks.pass("nginx's settings pass its check");
            Ok((Some(main), Now::Passed))
        }
        Ok(checked) => {
            let line = checked.line.map(|line| stage::as_live(&line));
            let words = format!(
                "nginx's settings do not pass its check now, before any change: {}",
                line.as_deref().unwrap_or("it gave no line")
            );
            checks.fail(Some("WEB-103"), words, false)?;
            Ok((Some(main), Now::Failed(line)))
        }
        Err(why) => {
            checks.fail(None, format!("nginx's check could not be run: {why}"), false)?;
            Ok((Some(main), Now::Skipped))
        }
    }
}

/// What a person asked to add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddRequest {
    /// The names, the first being the site's name.
    pub names: Vec<String>,
    /// What it serves.
    pub kind: Kind,
    /// The folder, under `/shared/www/sites`.
    pub folder: String,
    /// The account the folder belongs to.
    pub owner: String,
    /// The certificate to serve HTTPS with.
    pub certificate: Option<String>,
}

impl AddRequest {
    /// The site's spec.
    pub fn spec(&self) -> Spec {
        Spec {
            name: self.names[0].clone(),
            names: self.names.clone(),
            kind: self.kind,
            folder: self.folder.clone(),
            certificate: self.certificate.clone(),
        }
    }
}

/// What the checks of an add found that its later steps use.
#[derive(Clone, Debug)]
pub struct Found {
    /// The account that owns the folder.
    pub owner: Option<accounts::User>,
    /// How nginx's settings fared before the change.
    pub now: Now,
    /// What is there of the folder.
    pub folder: folder::Seen,
}

/// The names of `names` that a certificate with the names `have` is not
/// valid for: a name is covered by itself, or by a `*.` name for one
/// label of it.
fn missing_names(have: &[String], names: &[String]) -> Vec<String> {
    let covers = |name: &String| {
        have.iter().any(|known| {
            known == name
                || known
                    .strip_prefix("*.")
                    .zip(name.split_once('.'))
                    .is_some_and(|(rest, (label, tail))| !label.is_empty() && tail == rest)
        })
    };
    names.iter().filter(|name| !covers(name)).cloned().collect()
}

/// The check of a certificate against the names of a site: WEB-106.
///
/// # Errors
///
/// As [`Checks::fail`].
pub fn certificate_check(
    ctx: &Ctx<'_>,
    certificate: &str,
    names: &[String],
    checks: &mut Checks,
) -> Result<(), Refusal> {
    match ctx.host.certificate_names(certificate) {
        Err(why) => checks.fail(
            Some("WEB-106"),
            format!("certd has no certificate {certificate} that can be read: {why}"),
            false,
        ),
        Ok(have) => {
            let lacking = missing_names(&have, names);
            if lacking.is_empty() {
                checks.pass(format!("The certificate {certificate} names every name of the site"));
                Ok(())
            } else {
                checks.fail(
                    Some("WEB-106"),
                    format!("The certificate {certificate} does not name {}", list_of(&lacking)),
                    false,
                )
            }
        }
    }
}

/// The checks of step 1 of an add, in the order that makes the cheap
/// ones first for the person: names, what is installed, nginx's
/// settings, the owner, the folder and the certificate.
///
/// # Errors
///
/// The first check that fails, when `checks` stop at a failure.
pub fn add_checks(
    ctx: &Ctx<'_>,
    state: &mut State,
    request: &AddRequest,
    checks: &mut Checks,
    done: Option<&mut Done>,
) -> Result<Found, Refusal> {
    let name = &request.names[0];
    let sites: Vec<(String, Vec<String>)> = state
        .sites
        .iter()
        .filter_map(|site| {
            let facts = site.facts.as_ref().ok()?;
            Some((format!("{SETTINGS}/{}", site.file), facts.names.clone()))
        })
        .collect();
    let live_main = state.conf.as_ref().and_then(|conf| conf.main.as_ref().ok());
    let in_conf = live_main.map(|main| (conf_view(), main));
    let own_file = format!("{name}.conf");
    let taken =
        plan::taken(&request.names, &sites, in_conf.as_ref().map(|(p, m)| (p.as_str(), *m)));
    match (taken, state.raw.iter().any(|file| file.file == own_file)) {
        (Some((who, file)), _) => checks.fail(
            Some("WEB-101"),
            format!("A website answers to {who} already: {file}"),
            false,
        )?,
        (None, true) => checks.fail(
            Some("WEB-101"),
            format!("A file of the site {name} is there already: {SETTINGS}/{own_file}"),
            false,
        )?,
        (None, false) => {
            checks.pass(format!("No website answers to {}", list_of(&request.names)));
        }
    }
    let nginx = ctx.host.installed(NGINX_BUNDLE, NGINX);
    if nginx {
        checks.pass("nginx is installed");
    } else {
        checks.fail(Some("WEB-105"), "nginx is not installed", false)?;
    }
    if request.kind == Kind::Php {
        if ctx.host.installed(PHP_BUNDLE, PHP_FPM) {
            checks.pass("PHP-FPM is installed");
        } else {
            checks.fail(Some("WEB-105"), "PHP-FPM is not installed", false)?;
        }
    }
    let (_, now) =
        if nginx { conf_checks(ctx, state, checks, done)? } else { (None, Now::Skipped) };
    let accounts = ctx.host.accounts();
    let owner = accounts.iter().find(|user| user.name == request.owner).cloned();
    match &owner {
        Some(user) if user.is_person() => {
            checks.pass(format!("{} is the account of a person", user.name));
        }
        Some(user) => checks.fail(
            None,
            format!("{} is not the account of a person: a site belongs to one", user.name),
            true,
        )?,
        None => {
            checks.fail(None, format!("There is no account {}", request.owner), true)?;
        }
    }
    let seen = folder_check(ctx, state, &request.folder, name, checks)?;
    if let Some(certificate) = &request.certificate {
        certificate_check(ctx, certificate, &request.names, checks)?;
    }
    Ok(Found { owner, now, folder: seen })
}

/// The check of the folder of a new site: it is a folder website may use,
/// and not in another site's.
fn folder_check(
    ctx: &Ctx<'_>,
    state: &State,
    folder_path: &str,
    name: &str,
    checks: &mut Checks,
) -> Result<folder::Seen, Refusal> {
    let others: Vec<(String, String)> = state
        .sites
        .iter()
        .filter(|site| site.name != name)
        .filter_map(|site| {
            let folder = site.facts.as_ref().ok()?.folder.clone()?;
            Some((site.name.clone(), folder))
        })
        .collect();
    if let Some(other) = folder::overlap(folder_path, &others) {
        checks.fail(
            None,
            format!(
                "The folder {folder_path} is in the folder of the site {other}, or has it in it"
            ),
            true,
        )?;
        return Ok(folder::Seen::default());
    }
    let seen =
        folder::check_sites(ctx.paths).and_then(|()| folder::inspect(ctx.paths, folder_path));
    match seen {
        Ok(seen) if seen.exists => {
            checks.pass(format!("The folder {folder_path} is there and is kept as it is"));
            Ok(seen)
        }
        Ok(seen) => {
            checks.pass(format!("The folder {folder_path} is made"));
            Ok(seen)
        }
        Err(why) => {
            checks.fail(None, why, true)?;
            Ok(folder::Seen::default())
        }
    }
}

/// What a change puts live, and what nginx's service file needs for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    /// The files to write, in order.
    pub put: Vec<Raw>,
    /// The files to remove.
    pub remove: Vec<String>,
    /// What nginx's service file has to hold afterwards.
    pub wanted: Wanted,
    /// Whether the site needs a certificate nginx's service file does not
    /// name yet, which nginx can see only after it restarts.
    pub new_certificate: bool,
    /// Whether nginx is turned on when it is off.
    pub starts: bool,
}

/// What putting `new` in the place of the site `name`, or taking the site
/// away when `new` is none, comes to: the files, and the keys nginx's
/// service file keeps for the sites that are then there.
pub fn intent(state: &State, name: &str, new: Option<&Spec>) -> Intent {
    let others: Vec<&Facts> = state
        .sites
        .iter()
        .filter(|site| site.name != name)
        .filter_map(|site| site.facts.as_ref().ok())
        .collect();
    let new_https = new.is_some_and(|spec| spec.certificate.is_some());
    let any_https = new_https || others.iter().any(|facts| facts.https);
    let default_file = state.raw.iter().find(|file| file.file == DEFAULT_FILE);
    let mut put = Vec::new();
    let mut remove = Vec::new();
    let default_after = match (any_https, default_file) {
        (true, None) => {
            put.push(Raw { file: DEFAULT_FILE.into(), bytes: default_text().as_bytes().to_vec() });
            true
        }
        (false, Some(file)) if file.bytes == default_text().as_bytes() => {
            remove.push(DEFAULT_FILE.to_string());
            false
        }
        (_, Some(_)) => true,
        (false, None) => false,
    };
    match new {
        Some(spec) => {
            put.push(Raw { file: format!("{name}.conf"), bytes: site_text(spec).into_bytes() });
        }
        None => remove.push(format!("{name}.conf")),
    }
    let main = state.conf.as_ref().and_then(|conf| conf.main.as_ref().ok());
    let https = default_after
        || new_https
        || main.is_some_and(|main| main.listens_443)
        || others.iter().any(|facts| facts.ports.contains(&443));
    let mut named: Vec<&str> =
        others.iter().filter_map(|facts| facts.certificate.as_deref()).collect();
    if let Some(main) = main {
        named.extend(main.certificates.iter().map(String::as_str));
    }
    let certificate = new.and_then(|spec| spec.certificate.clone());
    named.extend(certificate.as_deref());
    // A certificate leaves only when the site goes or goes back to HTTP.
    let old = state
        .sites
        .iter()
        .find(|site| site.name == name)
        .and_then(|site| site.facts.as_ref().ok())
        .and_then(|facts| facts.certificate.clone())
        .filter(|old| certificate.is_none() && !named.contains(&old.as_str()));
    let in_service = named_certificates(&state.service, state.over.as_deref()).unwrap_or_default();
    Intent {
        put,
        remove,
        new_certificate: certificate.as_ref().is_some_and(|c| !in_service.contains(c)),
        wanted: Wanted {
            add: certificate.into_iter().collect(),
            remove: old.into_iter().collect(),
            https,
        },
        starts: new.is_some(),
    }
}

/// The files nginx includes after `intent`: today's, without what leaves,
/// and what is put in them.
pub fn set_after(state: &State, intent: &Intent) -> Vec<Raw> {
    let mut files: Vec<Raw> =
        state.raw.iter().filter(|file| !intent.remove.contains(&file.file)).cloned().collect();
    for put in &intent.put {
        match files.iter_mut().find(|file| file.file == put.file) {
            Some(file) => file.bytes.clone_from(&put.bytes),
            None => files.push(put.clone()),
        }
    }
    files.sort_by(|a, b| a.file.cmp(&b.file));
    files
}

/// Step: nginx's service file gets what the sites need, when it lacks it,
/// and Spark's check passes it. Returns whether the file changed.
fn override_step(
    ctx: &Ctx<'_>,
    state: &State,
    wanted: &Wanted,
    done: &mut Done,
) -> Result<bool, Refusal> {
    ctx.steps.next("Changing nginx's service file");
    let new =
        plan::override_text(&state.service, state.over.as_deref(), wanted).map_err(|why| {
            Refusal::new("WEB-103", format!("nginx's service file cannot be changed: {why}"))
        })?;
    let Some(new) = new else { return Ok(false) };
    let path = ctx.paths.real(OVERRIDE);
    let words =
        |error: io::Error| Refusal::plain(format!("{OVERRIDE}: {}", strip_os_error(&error)));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(words)?;
    }
    done.over = Some(state.over.clone());
    settings_edit::write(&path, new.as_bytes(), 0o644).map_err(words)?;
    ctx.host.service_check(NGINX).map_err(|line| {
        Refusal::new("WEB-103", format!("nginx's service file does not pass Spark's check: {line}"))
    })?;
    Ok(true)
}

/// Step: nginx restarts when its service file changed and it runs.
fn restart_step(
    ctx: &Ctx<'_>,
    state: &State,
    changed: bool,
    done: &mut Done,
) -> Result<(), Refusal> {
    ctx.steps.next("Restarting nginx, when its service file changed");
    if !changed || !state.nginx.is_some_and(|svc| svc.running) {
        return Ok(());
    }
    // Taken note of first: a restart that fails is restarted again once
    // the old file is back.
    done.restarted = true;
    ctx.host.restart(NGINX).map_err(|why| {
        Refusal::new("WEB-104", format!("nginx did not run with its changed service file: {why}"))
    })
}

/// Step: nginx's own check of the new set of sites.
fn check_step(ctx: &Ctx<'_>, state: &State, intent: &Intent, done: &Done) -> Result<(), Refusal> {
    ctx.steps.next("Checking the new set of sites with nginx");
    // A certificate nginx has no copy of yet is in its view once it has
    // started; its start is the check then.
    if intent.new_certificate && !done.restarted {
        return Ok(());
    }
    let Some(conf) = &state.conf else { return Ok(()) };
    let Ok(main) = &conf.main else { return Ok(()) };
    if !main.includes {
        return Ok(());
    }
    let files = set_after(state, intent);
    let staged = Staged::make(ctx.paths, &files, &conf.text, main).map_err(Refusal::plain)?;
    let checked = ctx.host.nginx_check(&staged.conf());
    drop(staged);
    match checked {
        Ok(checked) if checked.passed => Ok(()),
        Ok(checked) => {
            let line = checked.line.map(|line| stage::as_live(&line));
            Err(Refusal::new(
                "WEB-103",
                format!(
                    "nginx refuses the new set of websites: {}",
                    line.as_deref().unwrap_or("it gave no line")
                ),
            ))
        }
        Err(why) => Err(Refusal::plain(format!("nginx's check could not be run: {why}"))),
    }
}

/// Writes the site file `file` live, and notes what it was.
fn write_site(ctx: &Ctx<'_>, raw: &Raw, done: &mut Done) -> Result<(), Refusal> {
    let path = ctx.paths.real(&format!("{SETTINGS}/{}", raw.file));
    let words = |error: io::Error| {
        Refusal::plain(format!("{SETTINGS}/{}: {}", raw.file, strip_os_error(&error)))
    };
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(words)?;
        done.settings_made = true;
    }
    let old = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(words(error)),
    };
    done.live.push(Live { file: raw.file.clone(), old });
    settings_edit::write(&path, &raw.bytes, 0o644).map(|_| ()).map_err(words)
}

/// Removes the site file `file` from the live set, and notes what it was.
fn remove_site(ctx: &Ctx<'_>, file: &str, done: &mut Done) -> Result<(), Refusal> {
    let path = ctx.paths.real(&format!("{SETTINGS}/{file}"));
    let words =
        |error: io::Error| Refusal::plain(format!("{SETTINGS}/{file}: {}", strip_os_error(&error)));
    match fs::read(&path) {
        Ok(bytes) => done.live.push(Live { file: file.to_string(), old: Some(bytes) }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(words(error)),
    }
    fs::remove_file(&path).map_err(words)
}

/// Step: the new set of site files goes live, one rename each, and nginx
/// reads it: reloaded when it runs, turned on when it is off and the
/// change is one that starts it, and PHP-FPM turned on for a PHP site.
fn live_step(
    ctx: &Ctx<'_>,
    state: &State,
    intent: &Intent,
    php: bool,
    done: &mut Done,
) -> Result<(), Refusal> {
    ctx.steps.next("Putting the sites live");
    for raw in &intent.put {
        write_site(ctx, raw, done)?;
    }
    for file in &intent.remove {
        remove_site(ctx, file, done)?;
    }
    let running = state.nginx.is_some_and(|svc| svc.running);
    if running {
        done.reloaded = true;
        ctx.host.reload(NGINX).map_err(|why| {
            Refusal::new("WEB-104", format!("nginx did not take the new settings: {why}"))
        })?;
    } else if intent.starts {
        done.turned_on.push(NGINX);
        ctx.host.turn(NGINX, true).map_err(|why| {
            Refusal::new("WEB-104", format!("nginx did not start with the new settings: {why}"))
        })?;
    }
    if php && !state.php.is_some_and(|svc| svc.running) {
        done.turned_on.push(PHP_FPM);
        ctx.host
            .turn(PHP_FPM, true)
            .map_err(|why| Refusal::new("WEB-104", format!("PHP-FPM did not start: {why}")))?;
    }
    Ok(())
}

/// How the steps of a change that changes nginx's service file go.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// The file first, as a site that needs more of nginx has it.
    ServiceFirst,
    /// The files first, as a site that gives some back has it.
    ServiceLast,
}

/// The steps that change what is live: nginx's service file, its restart,
/// nginx's check of the new set, and the new set live, in the order a
/// change needs.
///
/// # Errors
///
/// The step that failed; what was done is in `done`.
pub fn apply(
    ctx: &Ctx<'_>,
    state: &State,
    intent: &Intent,
    php: bool,
    order: Order,
    done: &mut Done,
) -> Result<(), Refusal> {
    match order {
        Order::ServiceFirst => {
            let changed = override_step(ctx, state, &intent.wanted, done)?;
            restart_step(ctx, state, changed, done)?;
            check_step(ctx, state, intent, done)?;
            live_step(ctx, state, intent, php, done)
        }
        Order::ServiceLast => {
            check_step(ctx, state, intent, done)?;
            live_step(ctx, state, intent, php, done)?;
            let changed = override_step(ctx, state, &intent.wanted, done)?;
            restart_step(ctx, state, changed, done)
        }
    }
}

/// `bytes` random bytes as hex.
fn random_hex(bytes: usize) -> Result<String, Refusal> {
    let mut raw = vec![0u8; bytes];
    SystemRandom::new()
        .fill(&mut raw)
        .map_err(|_| Refusal::plain("the system has no random numbers to name a check file"))?;
    let mut hex = String::with_capacity(bytes * 2);
    for byte in &raw {
        // Writing to a string cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// The last step: the site answers under each of its names.
///
/// # Errors
///
/// WEB-104 when it does not.
pub fn answer_step(ctx: &Ctx<'_>, spec: &Spec) -> Result<(), Refusal> {
    ctx.steps.next("Checking that the site answers");
    let hex = random_hex(16)?;
    let content = random_hex(16)?;
    let php = spec.kind == Kind::Php;
    let files =
        CheckFiles::make(ctx.paths, &spec.folder, &hex, &content, php).map_err(Refusal::plain)?;
    let probe = Probe {
        names: spec.names.clone(),
        certificate: spec.certificate.clone(),
        text: (format!("/website-check-{hex}.txt"), content.clone()),
        php: php.then(|| (format!("/website-check-{hex}.php"), content)),
    };
    let result = answer::check(ctx.host, &probe);
    drop(files);
    result.map_err(|why| Refusal::new("WEB-104", format!("The site did not answer: {why}")))
}

/// How the site is reached: `http` or `https`.
fn scheme(spec: &Spec) -> &'static str {
    if spec.certificate.is_some() { "https" } else { "http" }
}

/// How a site is said in the log.
fn describe(spec: &Spec) -> String {
    let kind = if spec.kind == Kind::Php { "php" } else { "static" };
    let how = match &spec.certificate {
        Some(certificate) => format!("HTTPS with {certificate}"),
        None => "HTTP only".to_string(),
    };
    format!("{} ({kind}, {}, {how})", spec.name, spec.folder)
}

/// The account PHP-FPM runs as.
fn php_owner(ctx: &Ctx<'_>) -> Result<Owner, Refusal> {
    ctx.host
        .accounts()
        .iter()
        .find(|user| user.name == "php")
        .map(|user| Owner { uid: user.uid, gid: user.gid })
        .ok_or_else(|| Refusal::plain("There is no account php, which PHP-FPM runs as"))
}

/// Adds a website.
///
/// # Errors
///
/// The refusal of a check, or the step that failed, with what was taken
/// back.
pub fn add(ctx: &Ctx<'_>, request: &AddRequest) -> Result<Vec<String>, Refusal> {
    let _lock = Lock::take(ctx.paths).map_err(Refusal::plain)?;
    let mut done = Done::default();
    match add_steps(ctx, request, &mut done) {
        Ok(lines) => Ok(lines),
        Err(refusal) => {
            let left = rollback(ctx, &done);
            Err(refusal.and_left(&left))
        }
    }
}

/// The steps of [`add`].
fn add_steps(ctx: &Ctx<'_>, request: &AddRequest, done: &mut Done) -> Result<Vec<String>, Refusal> {
    let mut state = load(ctx)?;
    ctx.steps.next("Checking the names, nginx and its settings");
    let found = add_checks(ctx, &mut state, request, &mut Checks::stopping(), Some(done))?;
    let spec = request.spec();
    let intent = intent(&state, &spec.name, Some(&spec));
    ctx.steps.next("Making the folder");
    let Some(owner) = found.owner else { return Err(Refusal::usage("There is no owner")) };
    let php = if spec.kind == Kind::Php {
        php_owner(ctx)?
    } else {
        Owner { uid: owner.uid, gid: owner.gid }
    };
    let what = Make {
        folder: &spec.folder,
        name: &spec.name,
        kind: spec.kind,
        owner: Owner { uid: owner.uid, gid: owner.gid },
        php,
    };
    folder::make(ctx.paths, &what, &mut done.made).map_err(Refusal::plain)?;
    apply(ctx, &state, &intent, spec.kind == Kind::Php, Order::ServiceFirst, done)?;
    answer_step(ctx, &spec)?;
    ctx.host.log(&format!("added {}", describe(&spec)));
    Ok(vec![format!("Added {}: {}://{}/", spec.name, scheme(&spec), spec.name)])
}

/// What a person asked of `https`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Https {
    /// Serve over HTTPS with this certificate.
    With(String),
    /// Serve over HTTP only.
    Off,
}

/// Takes the site `name` to HTTPS, or back to HTTP.
///
/// # Errors
///
/// The refusal of a check, or the step that failed, with what was taken
/// back.
pub fn https(ctx: &Ctx<'_>, name: &str, how: &Https) -> Result<Vec<String>, Refusal> {
    let _lock = Lock::take(ctx.paths).map_err(Refusal::plain)?;
    let mut done = Done::default();
    match https_steps(ctx, name, how, &mut done) {
        Ok(lines) => Ok(lines),
        Err(refusal) => {
            let left = rollback(ctx, &done);
            Err(refusal.and_left(&left))
        }
    }
}

/// The words for a site's file that `website` does not write.
const BY_HAND: &str = "Its file was changed by hand, so website does not write it: add the HTTPS \
                       server yourself, as website(8) shows";

/// The steps of [`https`].
fn https_steps(
    ctx: &Ctx<'_>,
    name: &str,
    how: &Https,
    done: &mut Done,
) -> Result<Vec<String>, Refusal> {
    let mut state = load(ctx)?;
    ctx.steps.next("Checking the site and nginx's settings");
    let mut checks = Checks::stopping();
    let Some(site) = state.sites.iter().find(|site| site.name == name).cloned() else {
        return Err(Refusal::plain(format!("No website {name}")));
    };
    let current = site
        .facts
        .as_ref()
        .ok()
        .filter(|facts| !plan::by_hand(name, &site.text, facts))
        .and_then(|facts| plan::spec_of(name, facts))
        .ok_or_else(|| Refusal::new("WEB-107", BY_HAND))?;
    if let Https::With(certificate) = how {
        certificate_check(ctx, certificate, &current.names, &mut checks)?;
    }
    if !ctx.host.installed(NGINX_BUNDLE, NGINX) {
        return Err(Refusal::new("WEB-105", "nginx is not installed"));
    }
    let seen = folder::inspect(ctx.paths, &current.folder).map_err(Refusal::plain)?;
    if !seen.exists {
        return Err(Refusal::plain(format!("Its folder {} is not there", current.folder)));
    }
    conf_checks(ctx, &mut state, &mut checks, Some(done))?;
    let new = Spec {
        certificate: match how {
            Https::With(certificate) => Some(certificate.clone()),
            Https::Off => None,
        },
        ..current.clone()
    };
    if new == current {
        return Ok(vec![match &new.certificate {
            Some(certificate) => format!("{name} serves HTTPS with {certificate} already"),
            None => format!("{name} is served over HTTP only already"),
        }]);
    }
    let intent = intent(&state, name, Some(&new));
    let order = if new.certificate.is_some() { Order::ServiceFirst } else { Order::ServiceLast };
    apply(ctx, &state, &intent, new.kind == Kind::Php, order, done)?;
    answer_step(ctx, &new)?;
    let said = match &new.certificate {
        Some(certificate) => format!("changed {name} to HTTPS with {certificate}"),
        None => format!("took {name} back to HTTP only"),
    };
    ctx.host.log(&said);
    Ok(vec![format!("{name} is served at {}://{name}/", scheme(&new))])
}

#[cfg(test)]
mod tests;
