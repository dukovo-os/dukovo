// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `service check`, `service new`, `service edit`, and `service NAME
//! enable` and `disable` (service(8)): the service files Spark reads
//! (service.toml(5), ADR 0216), checked with the same reader Spark uses.
//!
//! `service edit` works as visudo(8) does: it copies the file, opens the
//! copy in the editor, checks it, and puts it in place only if it reads;
//! otherwise it shows what is wrong and offers to edit it again.
//!
//! What is wrong is said as a compiler says it, with the line, the place
//! in it and what to do (`manifest::Error::report`), and what a file that
//! reads would do once it is put to use comes from `spark_api::dry_run`,
//! the words the web console says too (ADR 0471). Only root sees the dry
//! run: the control socket and the copy of each file Spark read last are
//! root's. The `service-graph` boot test and the lab's `services/check`
//! test cover this file.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use manifest::Source;
use manifest::service;
use spark_api::dry_run::{self, Change};
use spark_api::graph::Link;
use spark_api::show;

use crate::service::shown;
use crate::util::{self, Fail, fail, usage};
use crate::{power, prompt, servicetry};

/// Where Spark reads the service files.
pub const DIR: &str = "/system/settings/spark/services";

/// Where the system's own service files are kept, as Spark copies them.
const DEFAULTS: &str = "/system/resources/default-settings/spark/services";

/// Where Spark keeps the last copy of each service's file, and of its
/// override beneath `settings.d`, that it read: the copy a service that
/// runs runs.
const GOOD: &str = "/system/state/spark/good";

/// The editor `service edit` runs when neither VISUAL nor EDITOR names
/// one.
const EDITOR: &str = "nano";

/// The file `what` names: a path if it holds a `/` or ends in `.toml`,
/// else a service's name, whose file is in [`DIR`].
fn path_of(what: &str) -> Result<PathBuf, Fail> {
    if what.contains('/') || Path::new(what).extension().is_some_and(|e| e == "toml") {
        return Ok(PathBuf::from(what));
    }
    service::check_name(what).map_err(fail)?;
    Ok(Path::new(DIR).join(format!("{what}.toml")))
}

/// Every service file in [`DIR`] that reads, with its override over it, as
/// the service's name, its file and what it says; one that does not read
/// is left out, as `service check` reports it.
pub fn read_all() -> Vec<(String, PathBuf, service::Service)> {
    let Ok(entries) = fs::read_dir(DIR) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_string();
            let text = fs::read_to_string(&path).ok()?;
            let over = fs::read_to_string(override_path(&path))
                .ok()
                .and_then(|over| service::Override::from_toml(&over).ok());
            let read = service::from_toml_over(&text, over.as_ref()).ok()?;
            Some((name, path, read))
        })
        .collect()
}

/// The override file of the service file at `path` (ADR 0386):
/// `settings.d/NAME.toml` beside it.
fn override_path(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default();
    path.with_file_name(service::OVERRIDES).join(name)
}

/// A service file's override, as read from beside it.
struct Over {
    /// Where it is.
    path: PathBuf,
    /// What it says.
    text: String,
    /// What it says, read.
    read: service::Override,
}

/// A service file that reads, with what it was read from.
struct Checked {
    /// The service's name, its file's without `.toml`.
    name: String,
    /// What the file says.
    text: String,
    /// The file that sets keys over it, if there is one.
    over: Option<Over>,
    /// What the two say together.
    service: service::Service,
}

/// Why a file was not read.
enum Stop {
    /// It does not read: what to print of that.
    Broken(String),
    /// Something went wrong that is not the file's fault.
    Failed(Fail),
}

impl From<Fail> for Stop {
    fn from(failed: Fail) -> Stop {
        Stop::Failed(failed)
    }
}

/// The override of the service file at `path`, whose text is `text`, if
/// it has one that reads.
fn read_over(path: &Path, text: &str, brief: bool) -> Result<Option<Over>, Stop> {
    let over_path = override_path(path);
    let bytes = match fs::read(&over_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(util::failed(&over_path.display(), e).into()),
    };
    let Ok(over_text) = String::from_utf8(bytes) else {
        return Err(Stop::Broken(format!("{}: it is not UTF-8 text", over_path.display())));
    };
    match service::Override::from_toml(&over_text) {
        Ok(read) => Ok(Some(Over { path: over_path, text: over_text, read })),
        Err(err) => {
            // A mistake in the override's own text is in the override.
            let err = manifest::Error { in_override: true, ..err };
            let (file, shown) = (path.display().to_string(), over_path.display().to_string());
            let over = Source { path: &shown, text: &over_text };
            Err(Stop::Broken(said(&err, Source { path: &file, text }, Some(over), brief)))
        }
    }
}

/// `error` as `service check` prints it: where it is in `file` or in its
/// override `over`, what is wrong and the fix, all of it or, with `brief`,
/// on one line.
fn said(
    error: &manifest::Error,
    file: Source<'_>,
    over: Option<Source<'_>>,
    brief: bool,
) -> String {
    if brief { error.brief(file, over) } else { error.report(file, over) }
}

/// The service file at `path`, whose text is `bytes`, with its override
/// set over it. Its name must be a service's name too.
fn read_bytes(path: &Path, bytes: Vec<u8>, brief: bool) -> Result<Checked, Stop> {
    let shown = path.display();
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(Stop::Broken(format!("{shown}: it is not UTF-8 text")));
    };
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    if let Err(err) = service::check_name(name) {
        return Err(Stop::Broken(format!("{shown}: its name: {err}")));
    }
    let over = read_over(path, &text, brief)?;
    match service::from_toml_over(&text, over.as_ref().map(|over| &over.read)) {
        // What Spark checks against the service's own name too: that it
        // does not name itself, and that certd names no certificates.
        Ok(service) => {
            match service.check_links(name).and_then(|()| service.check_certificates(name)) {
                Ok(()) => Ok(Checked { name: name.into(), text, over, service }),
                Err(err) => Err(Stop::Broken(format!("{shown}: {err}"))),
            }
        }
        Err(err) => {
            let file = shown.to_string();
            let over_path = over.as_ref().map(|over| over.path.display().to_string());
            let over = over.as_ref().zip(over_path.as_deref());
            let over = over.map(|(over, path)| Source { path, text: &over.text });
            Err(Stop::Broken(said(&err, Source { path: &file, text: &text }, over, brief)))
        }
    }
}

/// The service file at `path`, read, or why not.
fn read_file(path: &Path, brief: bool) -> Result<Checked, Stop> {
    let bytes = fs::read(path).map_err(|e| util::failed(&path.display(), e))?;
    read_bytes(path, bytes, brief)
}

/// What Spark can say, to this session, of what putting a file to use
/// would do.
enum Spark {
    /// Not root's session: Spark's answers and its copies are root's.
    Hidden,
    /// Spark did not answer.
    Silent,
    /// Its answer, of every service.
    Shows(show::Show),
}

impl Spark {
    /// Asks Spark for its `show` document, if this session may.
    fn ask() -> Spark {
        if util::own_user() != 0 {
            return Spark::Hidden;
        }
        shown(None).map_or(Spark::Silent, Spark::Shows)
    }
}

/// What putting a file that reads to use would do, as far as Spark tells.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    /// This session is not root's.
    Hidden,
    /// Spark did not say, or its copy cannot be read.
    Unknown,
    /// What `spark_api::dry_run` says, or none when nothing changes.
    Known(Option<String>),
}

/// How the settings of a file compare with the copy Spark read last.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    /// These settings differ.
    Differ(Vec<String>),
    /// Spark has no copy: the file was added since it started.
    Absent,
    /// The copy cannot be read.
    Unreadable,
}

/// Compares the file `read` with the copy of its service's file, and of
/// its override, that Spark read last, in the folder `good`.
fn compare(good: &Path, read: &Checked) -> Held {
    let file = good.join(format!("{}.toml", read.name));
    let over = good.join(service::OVERRIDES).join(format!("{}.toml", read.name));
    let held = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Held::Absent,
        Err(_) => return Held::Unreadable,
    };
    let held_over = match fs::read_to_string(&over) {
        Ok(text) => match service::Override::from_toml(&text) {
            Ok(over) => Some(over),
            Err(_) => return Held::Unreadable,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Held::Unreadable,
    };
    let new = (read.text.as_str(), read.over.as_ref().map(|over| &over.read));
    match service::changed_settings((&held, held_over.as_ref()), new) {
        Ok(keys) => Held::Differ(keys),
        Err(_) => Held::Unreadable,
    }
}

/// What putting the file `read` to use would do, given the copies Spark
/// keeps in `good`.
fn effect(spark: &Spark, good: &Path, read: &Checked) -> Effect {
    let document = match spark {
        Spark::Hidden => return Effect::Hidden,
        Spark::Silent => return Effect::Unknown,
        Spark::Shows(document) => document,
    };
    let differ = match compare(good, read) {
        Held::Differ(keys) => Some(keys),
        Held::Absent => None,
        Held::Unreadable => return Effect::Unknown,
    };
    let service = &read.service;
    let links: Vec<(String, Link)> = [
        (&service.requires, Link::Requires),
        (&service.wants, Link::Wants),
        (&service.after, Link::After),
    ]
    .into_iter()
    .flat_map(|(names, link)| names.iter().map(move |name| (name.clone(), link)))
    .collect();
    let change = Change {
        name: &read.name,
        differ: differ.as_deref(),
        enabled: service.enabled,
        links: &links,
    };
    Effect::Known(dry_run::dry_run(document, &change))
}

/// What `service check` prints of a file that reads: that it does, `notes`
/// on its override and a newer default, and what putting it to use would
/// do. A check of every file, `whole`, leaves out that only root sees it,
/// which would repeat on each line.
fn ok_line(path: &str, notes: &str, effect: &Effect, whole: bool) -> String {
    let mut said = format!("{path}: ok{notes}");
    match effect {
        Effect::Known(Some(clause)) => said.push_str(&format!("; {clause}")),
        Effect::Hidden if !whole => said.push_str("; root sees what applying it does"),
        _ => {}
    }
    said
}

/// What `service edit` prints of a copy that reads, in place or about to
/// be tried: that it does, and what putting it to use does, or how to
/// apply it when that is not known.
fn edited_line(path: &str, name: &str, effect: &Effect) -> String {
    let clause = match effect {
        Effect::Known(Some(clause)) => clause.clone(),
        Effect::Known(None) => "nothing to apply".into(),
        Effect::Hidden => "root sees what applying it does".into(),
        Effect::Unknown => {
            format!("Spark reads it when the service next starts: `service {name} restart`")
        }
    };
    format!("{path}: ok; {clause}")
}

/// What `service check` says of a file that reads: its override, and a
/// newer default an update put beside it, with the keys that differ
/// (ADR 0386).
fn notes(path: &Path) -> String {
    let mut said = String::new();
    let over = override_path(path);
    if over.exists() {
        said.push_str(&format!(", with {} over it", over.display()));
    }
    let mut default = path.as_os_str().to_owned();
    default.push(".default");
    let default = PathBuf::from(default);
    if let (Ok(file), Ok(newer)) = (fs::read_to_string(path), fs::read_to_string(&default)) {
        let differ = match service::changed_keys(&file, &newer) {
            Ok(keys) if keys.is_empty() => "the same settings".into(),
            Ok(keys) => format!("these settings differ: {}", keys.join(", ")),
            Err(err) => format!("it does not read: {err}"),
        };
        said.push_str(&format!(
            "; an update left a newer default beside it, {}, and kept this one, which an \
             administrator changed; {differ}",
            default.display()
        ));
    }
    said
}

/// `service check [--brief] [NAME | FILE]...`: checks each file, or every
/// one in [`DIR`]; says where each that does not read is wrong and how
/// to mend it, or with `--brief` in one line, and for each that reads,
/// what putting it to use would do; fails if one does not read.
pub fn check(operands: &[String]) -> Result<i32, Fail> {
    let mut brief = false;
    let mut files = Vec::new();
    for operand in operands {
        match operand.as_str() {
            "--brief" => brief = true,
            other if other.starts_with('-') => {
                return Err(usage(format!("service check: unknown option `{other}'")));
            }
            _ => files.push(operand),
        }
    }
    let whole = files.is_empty();
    let mut paths = Vec::new();
    if whole {
        let entries = fs::read_dir(DIR).map_err(|e| util::failed(&DIR, e))?;
        let mut names: Vec<String> =
            entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for name in names {
            let extension = Path::new(&name).extension().and_then(|e| e.to_str());
            if extension == Some("toml") {
                paths.push(Path::new(DIR).join(name));
            } else if extension == Some("service") {
                println!(
                    "{DIR}/{name}: a file of before Phase 13; Spark converts it to TOML when it \
                     next starts"
                );
            }
        }
    }
    for file in files {
        paths.push(path_of(file)?);
    }
    let spark = Spark::ask();
    let mut bad = 0;
    for path in &paths {
        match read_file(path, brief) {
            Ok(read) => {
                let effect = effect(&spark, Path::new(GOOD), &read);
                println!("{}", ok_line(&path.display().to_string(), &notes(path), &effect, whole));
            }
            Err(Stop::Broken(report)) => {
                eprintln!("{report}");
                bad += 1;
            }
            Err(Stop::Failed(failed)) => {
                eprintln!("service: {}", failed.message);
                bad += 1;
            }
        }
    }
    Ok(i32::from(bad > 0))
}

/// `service new NAME`: writes the commented template for NAME, which is
/// not started at boot until its file says so.
pub fn new(operands: &[String]) -> Result<i32, Fail> {
    let [name] = operands else {
        return Err(usage("Usage: service new NAME".into()));
    };
    let path = path_of(name)?;
    if path.parent() != Some(Path::new(DIR)) {
        return Err(usage(format!("service new takes a service's name, not {name}")));
    }
    let mut file =
        OpenOptions::new().write(true).create_new(true).mode(0o644).open(&path).map_err(
            |e| match e.kind() {
                io::ErrorKind::AlreadyExists => fail(format!(
                    "{} is there already; edit it with `service edit {name}`",
                    path.display()
                )),
                _ => util::failed(&path.display(), e),
            },
        )?;
    file.write_all(service::template(name).as_bytes())
        .map_err(|e| util::failed(&path.display(), e))?;
    println!(
        "wrote {}: edit it with `service edit {name}`, then start it with `service {name} start`",
        path.display()
    );
    Ok(0)
}

/// What becomes of an edited copy that reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// It is put in place, for Spark to read when the service next
    /// starts.
    Now,
    /// It is tried for this many seconds (ADR 0472): Spark keeps the file
    /// it replaces, the service restarts, and the file comes back unless
    /// the change is kept.
    Trial(u32),
}

/// `service edit NAME`: edits NAME's file through a copy, put in place
/// only once it reads.
pub fn edit(operands: &[String]) -> Result<i32, Fail> {
    let [name] = operands else {
        return Err(usage("Usage: service edit NAME".into()));
    };
    edit_file(name, Place::Now)
}

/// Edits the file `name` names through a copy, which becomes what
/// `place` says once it reads.
pub fn edit_file(name: &str, place: Place) -> Result<i32, Fail> {
    let path = path_of(name)?;
    let original = fs::read(&path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => fail(format!(
            "{} is not there; write a new one with `service new {name}`",
            path.display()
        )),
        _ => util::failed(&path.display(), e),
    })?;
    // The copy is beside the file, so that putting it in place is a
    // rename; that it exists already means another edit is going on.
    let dir = path.parent().unwrap_or(Path::new("."));
    let file_name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let copy = dir.join(format!(".{file_name}.edit"));
    let mode = fs::metadata(&path).map_or(0o644, |m| m.permissions().mode() & 0o7777);
    let mut out = OpenOptions::new().write(true).create_new(true).mode(mode).open(&copy).map_err(
        |e| match e.kind() {
            io::ErrorKind::AlreadyExists => fail(format!(
                "{} is being edited already: {} is its copy",
                path.display(),
                copy.display()
            )),
            _ => util::failed(&copy.display(), e),
        },
    )?;
    let written = out.write_all(&original);
    drop(out);
    let result = written
        .map_err(|e| util::failed(&copy.display(), e))
        .and_then(|()| edit_copy(&path, &copy, &original, name, place));
    let _ = fs::remove_file(&copy);
    result
}

/// Runs the editor on `copy` until it reads or the user gives up, then
/// puts it in place of `path`, or tries it there, as `place` says.
fn edit_copy(
    path: &Path,
    copy: &Path,
    original: &[u8],
    name: &str,
    place: Place,
) -> Result<i32, Fail> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|e| !e.trim().is_empty()))
        .unwrap_or_else(|| EDITOR.into());
    let stdin = io::stdin();
    loop {
        // Through the shell, so that EDITOR may hold arguments, as
        // `vi -n`; the file is $1, never part of the command.
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(copy)
            .status()
            .map_err(|e| util::failed(&editor, e))?;
        if !status.success() {
            return Err(fail(format!("{editor} failed; {} is as it was", path.display())));
        }
        let edited = fs::read(copy).map_err(|e| util::failed(&copy.display(), e))?;
        if edited == original {
            println!("{}: no change", path.display());
            return Ok(0);
        }
        // The copy is read as the file will be, with the override beside
        // it, and what is wrong is said of the file's own path.
        match read_bytes(path, edited, false) {
            Ok(read) => {
                // What it would do is worked out before it is in place: a
                // trial restarts the service at once, and Spark keeps the
                // copy it reads then.
                let effect = effect(&Spark::ask(), Path::new(GOOD), &read);
                let line = edited_line(&path.display().to_string(), &read.name, &effect);
                if let Place::Trial(seconds) = place {
                    println!("{line}");
                    return servicetry::put_in_place(path, copy, name, seconds);
                }
                fs::rename(copy, path).map_err(|e| util::failed(&path.display(), e))?;
                println!("{line}");
                return Ok(0);
            }
            Err(Stop::Broken(report)) => eprintln!("{report}"),
            Err(Stop::Failed(failed)) => return Err(failed),
        }
        eprint!("What now? (e)dit again, or e(x)it without saving: ");
        let _ = io::stderr().flush();
        let mut answer = String::new();
        let read = stdin.lock().read_line(&mut answer).unwrap_or(0);
        if read == 0 || !answer.trim_start().starts_with('e') {
            eprintln!("{} is as it was", path.display());
            return Ok(1);
        }
    }
}

/// `service secret set NAME SECRET`, `service secret list NAME` and
/// `service secret remove NAME SECRET` (ADR 0217): the secrets Spark
/// hands service NAME as read-only files, which no command shows back.
pub fn secret(operands: &[String]) -> Result<i32, Fail> {
    const USAGE: &str = "Usage: service secret set NAME SECRET | list NAME | remove NAME SECRET";
    let (verb, rest) = operands.split_first().ok_or_else(|| usage(USAGE.into()))?;
    let (name, secret) = match (verb.as_str(), rest) {
        ("list", [name]) => (name, None),
        ("set" | "remove", [name, secret]) => (name, Some(secret.as_str())),
        _ => return Err(usage(USAGE.into())),
    };
    service::check_name(name).map_err(fail)?;
    if let Some(secret) = secret {
        service::check_secret(secret).map_err(fail)?;
    }
    let dir = PathBuf::from(service::secrets_dir(name));
    match (verb.as_str(), secret) {
        ("set", Some(secret)) => {
            let value = read_secret(name, secret)?;
            // Only root reads them; Spark hands each service its own.
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .map_err(|e| util::failed(&dir.display(), e))?;
            let path = dir.join(secret);
            let new = dir.join(format!(".{secret}.new"));
            let _ = fs::remove_file(&new);
            let written = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&new)
                .and_then(|mut file| file.write_all(&value).and_then(|()| file.sync_all()))
                .and_then(|()| fs::rename(&new, &path));
            if let Err(e) = written {
                let _ = fs::remove_file(&new);
                return Err(util::failed(&path.display(), e));
            }
            println!(
                "set {name}'s secret {secret}; {name} reads it when it next starts: `service \
                 {name} restart`"
            );
        }
        ("remove", Some(secret)) => {
            let path = dir.join(secret);
            fs::remove_file(&path).map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => fail(format!("{name} has no secret {secret}")),
                _ => util::failed(&path.display(), e),
            })?;
        }
        _ => {
            let mut names: Vec<String> = match fs::read_dir(&dir) {
                Ok(entries) => entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.'))
                    .collect(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(util::failed(&dir.display(), e)),
            };
            names.sort();
            for secret in names {
                println!("{secret}");
            }
        }
    }
    Ok(0)
}

/// The value of `name`'s secret `secret`, from the standard input: a line
/// typed at the terminal, which is not shown, or else everything there,
/// with one newline at its end taken off, as `echo` adds one.
fn read_secret(name: &str, secret: &str) -> Result<Vec<u8>, Fail> {
    let mut value = Vec::new();
    if prompt::interactive() {
        match prompt::ask(&format!("{name}'s secret {secret}: "), false) {
            Ok(prompt::Answer::Line(line)) => value = line,
            Ok(prompt::Answer::Stopped) => {
                return Err(fail(format!("stopped: {name}'s secret {secret} is as it was")));
            }
            Err(err) => return Err(util::failed(&"the terminal", err)),
        }
    } else {
        io::stdin()
            .lock()
            .read_to_end(&mut value)
            .map_err(|e| util::failed(&"standard input", e))?;
        if value.ends_with(b"\n") {
            value.pop();
        }
    }
    if value.is_empty() {
        return Err(fail(format!("{name}'s secret {secret} cannot be empty")));
    }
    Ok(value)
}

/// The path of service `name`'s file.
fn path(name: &str) -> String {
    format!("{DIR}/{name}.toml")
}

/// Service `name`'s file, from the system's own if it is not there.
pub fn text(name: &str) -> Result<String, Fail> {
    let path = path(name);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let default = format!("{DEFAULTS}/{name}.toml");
            fs::read_to_string(&default).map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => fail(format!("{name}: unrecognized service")),
                _ => util::failed(&default, e),
            })
        }
        Err(e) => Err(util::failed(&path, e)),
    }
}

// `with_enabled` is shared with the installer, which turns on the
// services of the features chosen at install.
pub(crate) use manifest::service::with_enabled;

/// Turns service `name` on or off: its file says whether it starts at
/// boot, and Spark starts or stops it now. The file changes only if it
/// still reads.
pub fn turn(name: &str, on: bool) -> Result<(), Fail> {
    let path = path(name);
    let text = with_enabled(&text(name)?, on);
    service::from_toml(&text).map_err(|e| fail(format!("{path}: {e}")))?;
    let new = format!("{DIR}/.{name}.toml.new");
    let _ = fs::remove_file(&new);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&new)
        .and_then(|mut file| file.write_all(text.as_bytes()).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&new, &path));
    if let Err(e) = written {
        let _ = fs::remove_file(&new);
        return Err(util::failed(&path, e));
    }
    let verb = if on { "start" } else { "stop" };
    let answer = power::request(&format!("{verb} {name}"))?;
    if let Some(why) = answer.strip_prefix("error: ") {
        return Err(fail(format!("cannot {verb} {name}: {}", why.trim_end())));
    }
    Ok(())
}

/// `service NAME enable` and `service NAME disable`: the service starts
/// now and at each boot, or stops now and no longer starts at boot, as
/// `systemctl enable --now` and `disable --now` do.
pub fn enable(name: &str, on: bool) -> Result<i32, Fail> {
    let verb = if on { "enable" } else { "disable" };
    if !name_ok(name) {
        return Err(usage(format!("service {verb} takes a service's name, not {name}")));
    }
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root may {verb} a service: elevate first, as `sudo service {name} {verb}`"
        )));
    }
    turn(name, on)?;
    if on {
        println!("{name} is running, and starts at each boot.");
    } else {
        println!("{name} is stopped, and no longer starts at boot.");
    }
    Ok(0)
}

/// Whether `name` names a service file in [`DIR`], not a path.
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_services_name_is_enabled() {
        assert!(name_ok("sshd"));
        assert!(name_ok("my-app_2"));
        for bad in ["", "../etc/passwd", "a/b", ".hidden", "a b"] {
            assert!(!name_ok(bad), "{bad}");
        }
    }

    /// A folder of its own for a test, empty.
    fn folder(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("servicefiles-{}-{what}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const WEB: &str = "service_version = 1\nprogram = \"/system/commands/true\"\n";

    /// The service file `web.toml` in `dir`, whose text is `text`, read.
    fn read_in(dir: &Path, text: &str) -> Result<Checked, Stop> {
        read_bytes(&dir.join("web.toml"), text.as_bytes().to_vec(), false)
    }

    /// What a file that does not read says, or panics.
    fn broken(read: Result<Checked, Stop>) -> String {
        match read {
            Err(Stop::Broken(report)) => report,
            Err(Stop::Failed(failed)) => panic!("failed: {}", failed.message),
            Ok(_) => panic!("it read"),
        }
    }

    #[test]
    fn a_file_that_reads_says_what_putting_it_to_use_does() {
        let runs = Effect::Known(Some("web runs an older copy: order differs".into()));
        assert_eq!(
            ok_line("/s/web.toml", "", &runs, false),
            "/s/web.toml: ok; web runs an older copy: order differs"
        );
        // The override and the newer default come before it.
        assert_eq!(
            ok_line("/s/web.toml", ", with /s/settings.d/web.toml over it", &runs, false),
            "/s/web.toml: ok, with /s/settings.d/web.toml over it; web runs an older copy: \
             order differs"
        );
        for whole in [false, true] {
            assert_eq!(ok_line("/s/web.toml", "", &Effect::Known(None), whole), "/s/web.toml: ok");
            assert_eq!(ok_line("/s/web.toml", "", &Effect::Unknown, whole), "/s/web.toml: ok");
            assert!(ok_line("/s/web.toml", "", &runs, whole).contains("; web runs"));
        }
    }

    #[test]
    fn only_root_sees_the_dry_run_and_a_check_of_every_file_says_it_once_not_per_file() {
        assert_eq!(
            ok_line("/s/web.toml", "", &Effect::Hidden, false),
            "/s/web.toml: ok; root sees what applying it does"
        );
        assert_eq!(ok_line("/s/web.toml", "", &Effect::Hidden, true), "/s/web.toml: ok");
    }

    #[test]
    fn an_edited_copy_that_reads_says_what_it_would_do_or_how_to_apply_it() {
        let runs = Effect::Known(Some("web runs an older copy: order differs".into()));
        assert_eq!(
            edited_line("/s/web.toml", "web", &runs),
            "/s/web.toml: ok; web runs an older copy: order differs"
        );
        assert_eq!(
            edited_line("/s/web.toml", "web", &Effect::Known(None)),
            "/s/web.toml: ok; nothing to apply"
        );
        assert_eq!(
            edited_line("/s/web.toml", "web", &Effect::Hidden),
            "/s/web.toml: ok; root sees what applying it does"
        );
        assert_eq!(
            edited_line("/s/web.toml", "web", &Effect::Unknown),
            "/s/web.toml: ok; Spark reads it when the service next starts: `service web restart`"
        );
    }

    #[test]
    fn a_file_that_does_not_read_is_said_as_a_compiler_says_it_with_its_own_path() {
        let dir = folder("broken");
        let text = format!("{WEB}discription = \"Web\"\n");
        let report = broken(read_in(&dir, &text));
        let path = dir.join("web.toml").display().to_string();
        assert_eq!(
            report,
            format!(
                "{path}:3:1: `discription` is not a setting here; service.toml(5) lists \
                 them\n  |\n3 | discription = \"Web\"\n  | ^^^^^^^^^^^\n  = fix: did you mean \
                 `description`?"
            )
        );
        let one = broken(read_bytes(&dir.join("web.toml"), text.into_bytes(), true));
        assert_eq!(
            one,
            format!(
                "{path}:3:1: `discription` is not a setting here; service.toml(5) lists them: \
                 did you mean `description`?"
            )
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_text_or_has_a_bad_name_is_said_in_a_line() {
        let dir = folder("lines");
        let not_text = read_bytes(&dir.join("web.toml"), vec![0xff, 0xfe], false);
        assert_eq!(broken(not_text), format!("{}/web.toml: it is not UTF-8 text", dir.display()));
        let name = read_bytes(&dir.join("a b.toml"), WEB.as_bytes().to_vec(), false);
        assert!(broken(name).starts_with(&format!("{}/a b.toml: its name: ", dir.display())));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mistake_in_an_override_is_said_in_the_override_and_of_which_file_it_is_set_over() {
        let dir = folder("override");
        fs::create_dir_all(dir.join(service::OVERRIDES)).unwrap();
        let over = dir.join(service::OVERRIDES).join("web.toml");
        fs::write(&over, "# a typo\ndiscription = \"x\"\n").unwrap();
        let (file, over_shown) =
            (dir.join("web.toml").display().to_string(), over.display().to_string());
        let report = broken(read_in(&dir, WEB));
        assert_eq!(
            report,
            format!(
                "{over_shown}:2:1: `discription` is not a setting here; service.toml(5) lists \
                 them\n  |\n2 | discription = \"x\"\n  | ^^^^^^^^^^^\n  = fix: did you mean \
                 `description`?\n  = note: set over {file}"
            )
        );
        // An override that is not TOML is in the override, as it is.
        fs::write(&over, "restart = \n").unwrap();
        let report = broken(read_in(&dir, WEB));
        assert!(report.starts_with(&format!("{over_shown}:1:")), "{report}");
        assert!(report.ends_with(&format!("= note: set over {file}")), "{report}");
        // One that reads is set over the file, which then reads.
        fs::write(&over, "restart = \"never\"\n").unwrap();
        let read = read_in(&dir, WEB).ok().unwrap();
        assert_eq!(read.name, "web");
        assert_eq!(read.service.restart, service::Restart::Never);
        assert_eq!(read.over.unwrap().path, over);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_is_compared_with_the_copy_spark_read_last() {
        let dir = folder("held");
        let good = dir.join("good");
        fs::create_dir_all(good.join(service::OVERRIDES)).unwrap();
        let read = read_in(&dir, &format!("{WEB}order = 7\n")).ok().unwrap();
        assert_eq!(compare(&good, &read), Held::Absent);
        fs::write(good.join("web.toml"), WEB).unwrap();
        assert_eq!(compare(&good, &read), Held::Differ(vec!["order".into()]));
        // The copy's override counts as the file's does.
        fs::write(good.join(service::OVERRIDES).join("web.toml"), "order = 7\n").unwrap();
        assert_eq!(compare(&good, &read), Held::Differ(vec![]));
        fs::write(good.join(service::OVERRIDES).join("web.toml"), "order = \n").unwrap();
        assert_eq!(compare(&good, &read), Held::Unreadable);
        // A copy that is in the way of being read is not one that is not there.
        let _ = fs::remove_file(good.join("web.toml"));
        fs::create_dir(good.join("web.toml")).unwrap();
        assert_eq!(compare(&good, &read), Held::Unreadable);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_a_file_would_do_comes_from_sparks_answer_and_its_copy() {
        let dir = folder("effect");
        let good = dir.join("good");
        fs::create_dir_all(&good).unwrap();
        let text = "service_version = 2\nprogram = \"/system/commands/true\"\n";
        fs::write(good.join("web.toml"), text).unwrap();
        let read = read_in(&dir, &format!("{text}requires = [\"db\"]\n")).ok().unwrap();
        let web = show::Service {
            name: "web".into(),
            state: show::State::Running,
            ..show::Service::default()
        };
        let db = show::Service {
            name: "db".into(),
            state: show::State::Stopped,
            ..show::Service::default()
        };
        let document = show::Show { services: vec![web, db], ..show::Show::default() };
        assert_eq!(
            effect(&Spark::Shows(document.clone()), &good, &read),
            Effect::Known(Some(
                "web runs an older copy: requires differs; `service web restart` applies the \
                 change: it restarts web and starts db, which it now requires"
                    .into()
            ))
        );
        assert_eq!(effect(&Spark::Hidden, &good, &read), Effect::Hidden);
        assert_eq!(effect(&Spark::Silent, &good, &read), Effect::Unknown);
        // No copy: Spark reads the file when the service first starts.
        let _ = fs::remove_file(good.join("web.toml"));
        assert_eq!(
            effect(&Spark::Shows(document), &good, &read),
            Effect::Known(Some("Spark reads it when web first starts and at each boot".into()))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
