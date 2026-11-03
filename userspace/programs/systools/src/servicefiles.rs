// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `service check`, `service new`, `service edit`, and `service NAME
//! enable` and `disable` (service(8)): the service files Spark reads
//! (service.toml(5), ADR 0216), checked with the same reader Spark uses.
//!
//! `service edit` works as visudo(8) does: it copies the file, opens the
//! copy in the editor, checks it, and puts it in place only if it reads;
//! otherwise it shows what is wrong and offers to edit it again.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use manifest::service;

use crate::util::{self, Fail, fail, usage};
use crate::{power, prompt};

/// Where Spark reads the service files.
pub const DIR: &str = "/system/settings/spark/services";

/// Where the system's own service files are kept, as Spark copies them.
const DEFAULTS: &str = "/system/resources/default-settings/spark/services";

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

/// The override file of the service file at `path` (ADR 0386):
/// `settings.d/NAME.toml` beside it.
fn override_path(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default();
    path.with_file_name(service::OVERRIDES).join(name)
}

/// What is wrong with the service file at `path`, with its override set
/// over it, or None if they read. Its name must be a service's name too.
fn check_file(path: &Path) -> Result<Option<String>, Fail> {
    let text = fs::read(path).map_err(|e| util::failed(&path.display(), e))?;
    let Ok(text) = String::from_utf8(text) else {
        return Ok(Some("it is not UTF-8 text".into()));
    };
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    if let Err(err) = service::check_name(name) {
        return Ok(Some(format!("its name: {err}")));
    }
    let over_path = override_path(path);
    let over = match fs::read(&over_path) {
        Ok(bytes) => {
            let Ok(over) = String::from_utf8(bytes) else {
                return Ok(Some(format!("{}: it is not UTF-8 text", over_path.display())));
            };
            match service::Override::from_toml(&over) {
                Ok(over) => Some(over),
                Err(err) => return Ok(Some(format!("{}: {err}", over_path.display()))),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(util::failed(&over_path.display(), e)),
    };
    Ok(service::from_toml_over(&text, over.as_ref()).err().map(|err| match over {
        Some(_) => format!("with {} over it: {err}", over_path.display()),
        None => err.to_string(),
    }))
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

/// `service check [NAME | FILE]...`: checks each file, or every one in
/// [`DIR`]; fails if one does not read.
pub fn check(operands: &[String]) -> Result<i32, Fail> {
    let mut paths = Vec::new();
    if operands.is_empty() {
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
    for operand in operands {
        paths.push(path_of(operand)?);
    }
    let mut bad = 0;
    for path in &paths {
        match check_file(path) {
            Ok(None) => println!("{}: ok{}", path.display(), notes(path)),
            Ok(Some(why)) => {
                eprintln!("{}: {why}", path.display());
                bad += 1;
            }
            Err(failed) => {
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

/// `service edit NAME`: edits NAME's file through a copy, put in place
/// only once it reads.
pub fn edit(operands: &[String]) -> Result<i32, Fail> {
    let [name] = operands else {
        return Err(usage("Usage: service edit NAME".into()));
    };
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
        .and_then(|()| edit_copy(&path, &copy, &original, name));
    let _ = fs::remove_file(&copy);
    result
}

/// Runs the editor on `copy` until it reads or the user gives up, then
/// puts it in place of `path`.
fn edit_copy(path: &Path, copy: &Path, original: &[u8], name: &str) -> Result<i32, Fail> {
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
        let why = match String::from_utf8(edited) {
            Ok(text) => service::from_toml(&text).err().map(|err| err.to_string()),
            Err(_) => Some("it is not UTF-8 text".into()),
        };
        let Some(why) = why else {
            fs::rename(copy, path).map_err(|e| util::failed(&path.display(), e))?;
            println!(
                "{}: ok; Spark reads it when the service next starts: `service {name} restart`",
                path.display()
            );
            return Ok(0);
        };
        eprintln!("{}: {why}", path.display());
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

/// `text`, a service file, with its `enabled` line saying `on`, or one
/// added after its `program` line.
pub(crate) fn with_enabled(text: &str, on: bool) -> String {
    let line = format!("enabled = {on}");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    // The top-level keys: those before the first table.
    let top = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let key_of = |l: &str| l.split_once('=').map(|(key, _)| key.trim().to_string());
    if let Some(at) = lines[..top].iter().position(|l| key_of(l).as_deref() == Some("enabled")) {
        lines[at] = line;
    } else {
        let at = lines[..top]
            .iter()
            .position(|l| key_of(l).as_deref() == Some("program"))
            .map_or(0, |at| at + 1);
        lines.insert(at, line);
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

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
    fn enabled_is_set_in_place_or_after_program() {
        let file = "# smbd\nservice_version = 1\nprogram = \"/system/commands/smbd\"\nenabled = false\n\n[permissions]\nenabled = 1\n";
        let on = with_enabled(file, true);
        assert!(on.contains("\nenabled = true\n\n[permissions]\nenabled = 1\n"), "{on}");
        assert_eq!(with_enabled(&on, false), file);
        let without = "service_version = 1\nprogram = \"/x\"\norder = 40\n";
        assert_eq!(
            with_enabled(without, true),
            "service_version = 1\nprogram = \"/x\"\nenabled = true\norder = 40\n"
        );
    }

    #[test]
    fn only_a_services_name_is_enabled() {
        assert!(name_ok("sshd"));
        assert!(name_ok("my-app_2"));
        for bad in ["", "../etc/passwd", "a/b", ".hidden", "a b"] {
            assert!(!name_ok(bad), "{bad}");
        }
    }
}
