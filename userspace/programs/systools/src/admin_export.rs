// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admin export DIR` (ADR 0516): copies the settings files that the web
//! console watches into `DIR`, a Git repository, so that a person keeps the
//! machine's settings history in their own Git.
//!
//! The files are those the console's journal watches under
//! `/system/settings` ([`journal::settings_files`]), put in `DIR` at the
//! path they have on the machine, under `system/settings`. A new `DIR`, or an
//! empty one, gets `git init`; a folder that has files and is not a
//! repository is refused, so that nothing of the person's is written
//! among them. Each export replaces what the last one put in
//! `system/settings`, so a file that was removed from the machine is
//! removed from the repository, and is one commit, "Settings of NAME at
//! TIME" with the machine's name and the time in UTC. An export that
//! changes nothing makes no commit and says so.
//!
//! It runs as the person who types it and reads with their rights. What
//! it cannot copy is left out and listed in `SECRETS.md` of the
//! repository, by its path on the machine: a secret, which is a file that
//! not everyone may read, whatever the person's own rights are (the
//! journal keeps the same files out of what everyone signed in reads); a
//! file or folder that the person cannot read. The contents of those are
//! never in the repository.
//!
//! The commits are made with an identity of the machine's, `Dukovo on NAME
//! <settings@NAME.invalid>`, and never the person's own, whatever their Git
//! settings say; their hooks and signing do not apply to it. The export is
//! the one thing it does in the repository, and it stages only its own
//! paths.
//!
//! When it fails, git's own words are in the message.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::admind::journal;
use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};

/// What a mistake in the command line is told.
const USAGE: &str = "usage: admin export DIR";

/// Where in `DIR` the settings go.
const TREE: &str = "system/settings";

/// The file in `DIR` that lists what was left out.
const LEFT_OUT: &str = "SECRETS.md";

/// The mode of a copy and of a folder made for it: the settings' own, which
/// everyone may read; the person's umask can take from them.
const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;

/// `admin export DIR`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut dir = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(value) if dir.is_none() => dir = Some(PathBuf::from(value)),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(dir) = dir else { return Err(usage(USAGE.into())) };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let mut out = std::io::stdout().lock();
    export(Path::new(journal::SETTINGS), &dir, &util::hostname(), now, &mut out)
}

/// What the export found in the settings.
#[derive(Debug, Default, PartialEq, Eq)]
struct Gathered {
    /// Each file that was read, by its path on the machine, with its text.
    files: Vec<(String, Vec<u8>)>,
    /// The paths left out as secrets: files that not everyone may read.
    secrets: Vec<String>,
    /// The paths this account could not read, and why.
    unreadable: Vec<(String, String)>,
}

/// Exports the settings under `source` to `dir`, as the module says, at
/// Unix time `now`, and says what it did on `out`.
fn export(
    source: &Path,
    dir: &Path,
    machine: &str,
    now: u64,
    out: &mut dyn Write,
) -> Result<i32, Fail> {
    git_works()?;
    prepare(source, dir, machine)?;
    let found = gather(source)?;
    write_tree(source, dir, &found)?;
    let text = left_out(machine, &found);
    let listed = dir.join(LEFT_OUT);
    fs::write(&listed, text).map_err(|err| util::failed(&listed.display(), err))?;

    let kept = found.files.len();
    say(out, &format!("{kept} {} copied to {}", plural(kept, "file", "files"), dir.display()))?;
    if !found.secrets.is_empty() {
        let count = found.secrets.len();
        say(
            out,
            &format!(
                "{count} {} left out, since not everyone may read {}: listed in {LEFT_OUT}",
                plural(count, "secret", "secrets"),
                if count == 1 { "it" } else { "them" },
            ),
        )?;
    }
    if !found.unreadable.is_empty() {
        let count = found.unreadable.len();
        say(
            out,
            &format!(
                "{count} could not be read by you and {} left out: listed in {LEFT_OUT}",
                if count == 1 { "is" } else { "are" },
            ),
        )?;
    }

    git(dir, machine, &["add", "-A", "-f", "--", TREE, LEFT_OUT])?;
    if git_status(dir, machine, &["diff", "--cached", "--quiet"])? == 0 {
        say(out, "nothing has changed since the last export, so there is no new commit")?;
        return Ok(0);
    }
    let message = format!("Settings of {machine} at {}", time_of(now));
    git(dir, machine, &["commit", "-q", "-m", &message])?;
    let commit = git(dir, machine, &["rev-parse", "--short", "HEAD"])?;
    say(out, &format!("committed {}: {message}", commit.trim()))?;
    Ok(0)
}

/// Says `line` on `out`.
fn say(out: &mut dyn Write, line: &str) -> Result<(), Fail> {
    writeln!(out, "{line}").map_err(|err| util::failed(&"standard output", err))
}

/// `word` for one of something, `words` for the rest.
fn plural<'a>(count: usize, word: &'a str, words: &'a str) -> &'a str {
    if count == 1 { word } else { words }
}

/// Unix time `now` as the commit says it: `2026-11-14 08:00:00 UTC`.
fn time_of(now: u64) -> String {
    let rfc = crate::logd::rfc3339(now.saturating_mul(1_000_000_000));
    format!("{} UTC", rfc[..19].replace('T', " "))
}

/// Makes `dir` ready: a new folder, or an empty one, becomes a Git
/// repository, and one that is one already stays. Refuses a folder that
/// has other files, and one inside the settings, which would be copied
/// into itself.
fn prepare(source: &Path, dir: &Path, machine: &str) -> Result<(), Fail> {
    if is_inside(dir, source) {
        return Err(fail(format!("{}: that is inside {}", dir.display(), source.display())));
    }
    match fs::metadata(dir) {
        Ok(md) if !md.is_dir() => {
            return Err(fail(format!("{}: not a folder", dir.display())));
        }
        Ok(_) if dir.join(".git").exists() => return Ok(()),
        Ok(_) => {
            let has_files = fs::read_dir(dir)
                .map_err(|err| util::failed(&dir.display(), err))?
                .next()
                .is_some();
            if has_files {
                return Err(fail(format!(
                    "{}: that folder has files and is not a Git repository; export into a new \
                     folder, an empty one, or a repository",
                    dir.display()
                )));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            DirBuilder::new()
                .mode(DIR_MODE)
                .create(dir)
                .map_err(|err| util::failed(&dir.display(), err))?;
        }
        Err(err) => return Err(util::failed(&dir.display(), err)),
    }
    git(dir, machine, &["init", "-q", "-b", "main"])?;
    Ok(())
}

/// Whether `path`, which need not be there yet, is `source` or in it.
fn is_inside(path: &Path, source: &Path) -> bool {
    let Ok(source) = fs::canonicalize(source) else { return false };
    let real = fs::canonicalize(path).or_else(|_| {
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
        fs::canonicalize(parent.unwrap_or(Path::new(".")))
            .map(|parent| parent.join(path.file_name().unwrap_or_default()))
    });
    real.is_ok_and(|real| real.starts_with(source))
}

/// Reads the settings under `source`: the files with their text, the
/// secrets by name and what could not be read.
fn gather(source: &Path) -> Result<Gathered, Fail> {
    let settings = journal::settings_files(source);
    if settings.closed.iter().any(|folder| Path::new(folder) == source) {
        return Err(fail(format!("{}: cannot be read", source.display())));
    }
    let mut found = Gathered::default();
    for path in settings.files {
        let read = fs::metadata(&path).and_then(|md| {
            if journal::is_hidden(&path, &md) { Ok(None) } else { fs::read(&path).map(Some) }
        });
        match read {
            Ok(Some(text)) => found.files.push((path, text)),
            Ok(None) => found.secrets.push(path),
            Err(err) => found.unreadable.push((path, util::strip_os_error(&err))),
        }
    }
    for folder in settings.closed {
        found.unreadable.push((format!("{folder}/"), "the folder cannot be opened".into()));
    }
    // The order of the files is the walk's, and the list is the same from
    // one export to the next.
    found.unreadable.sort();
    Ok(found)
}

/// Puts the files in `dir` under [`TREE`], in place of what the last export
/// put there.
fn write_tree(source: &Path, dir: &Path, found: &Gathered) -> Result<(), Fail> {
    let tree = dir.join(TREE);
    match fs::remove_dir_all(&tree) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(util::failed(&tree.display(), err)),
    }
    let folders = |path: &Path| {
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(path)
            .map_err(|err| util::failed(&path.display(), err))
    };
    folders(&tree)?;
    for (path, text) in &found.files {
        let relative = Path::new(path).strip_prefix(source).unwrap_or(Path::new(path));
        let target = tree.join(relative);
        if let Some(parent) = target.parent() {
            folders(parent)?;
        }
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&target)
            .and_then(|mut file| file.write_all(text))
            .map_err(|err| util::failed(&target.display(), err))?;
    }
    Ok(())
}

/// The text of `SECRETS.md`: what was left out and where it is on the
/// machine. It holds no time, so that an export that changed nothing is
/// the same as the one before.
fn left_out(machine: &str, found: &Gathered) -> String {
    let mut text = format!("# Left out of the export of {machine}'s settings\n\n");
    if found.secrets.is_empty() && found.unreadable.is_empty() {
        text.push_str("Nothing was left out: every settings file could be copied.\n");
        return text;
    }
    text.push_str("The contents of these are not in this repository.\n");
    if !found.secrets.is_empty() {
        text.push_str(
            "\n## Secrets\n\nThese files are not readable by everyone, so they are left out by \
             name. Each is where it says, on the machine.\n\n",
        );
        for path in &found.secrets {
            text.push_str(&format!("- `{path}`\n"));
        }
    }
    if !found.unreadable.is_empty() {
        text.push_str(
            "\n## Not readable by the account that exported\n\nThese could not be read by the \
             account that made the export, which may be able to read them as another.\n\n",
        );
        for (path, why) in &found.unreadable {
            text.push_str(&format!("- `{path}`: {why}\n"));
        }
    }
    text
}

/// The name and address the commits carry, `Dukovo on NAME` and
/// `settings@NAME.invalid`: a name of the machine and an address that is
/// never a person's. A machine's name is made of the letters, digits and
/// hyphens a host name has.
fn identity(machine: &str) -> (String, String) {
    let host: String = machine
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let host = if host.trim_matches('-').is_empty() { "localhost".to_string() } else { host };
    (format!("Dukovo on {machine}"), format!("settings@{host}.invalid"))
}

/// A `git` command in `dir` for `args`, as the export needs it: with the
/// machine's identity as author and committer, whatever the person's Git
/// settings are, and no hooks or signing of the person's own.
fn command(dir: &Path, machine: &str, args: &[&str]) -> Command {
    let (name, email) = identity(machine);
    let mut git = Command::new("git");
    git.current_dir(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_AUTHOR_NAME", &name)
        .env("GIT_AUTHOR_EMAIL", &email)
        .env("GIT_COMMITTER_NAME", &name)
        .env("GIT_COMMITTER_EMAIL", &email)
        .stdin(Stdio::null());
    git
}

/// Runs git for `args` in `dir`, and gives what it printed, or its own
/// words if it failed.
fn git(dir: &Path, machine: &str, args: &[&str]) -> Result<String, Fail> {
    let out = command(dir, machine, args).output().map_err(git_missing)?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    Err(fail(format!("git {}: {}", args.join(" "), why.trim())))
}

/// Runs git for `args` in `dir` and gives its exit status, for a question
/// that git answers with it.
fn git_status(dir: &Path, machine: &str, args: &[&str]) -> Result<i32, Fail> {
    let out = command(dir, machine, args).output().map_err(git_missing)?;
    match out.status.code() {
        Some(code @ (0 | 1)) => Ok(code),
        _ => Err(fail(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// What it is to find no git to run.
fn git_missing(err: std::io::Error) -> Fail {
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(
            "admin export keeps the settings in a Git repository, and git is not installed".into(),
        );
    }
    util::failed(&"git", err)
}

/// Checks that git runs, before anything is written.
fn git_works() -> Result<(), Fail> {
    let out =
        Command::new("git").arg("--version").stdin(Stdio::null()).output().map_err(git_missing)?;
    if out.status.success() { Ok(()) } else { Err(fail("git does not run".into())) }
}

#[cfg(test)]
mod tests;
