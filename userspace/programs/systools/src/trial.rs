// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A settings file changed with an undo (ADR 0472): the order that every
//! trial keeps, whether `service try`, `network set --try` or `firewall
//! --try` begins it. Spark is asked first to keep the file and to put it
//! back unless the change is kept in time; only then is the new file
//! renamed over the old and put to use, by a restart or a reload of the
//! service that reads it. When the rename or the use fails, Spark is asked
//! to undo at once. Spark's timer, not this command, undoes a change that
//! is not kept, so the change comes undone when the terminal, the session
//! or the network that began it goes.
//!
//! The new text is written beside the file it replaces, in a file that
//! only this writer makes, and synced before the rename, so that the file
//! is always whole: the old one or the new one.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use spark_api::show;
use spark_api::trial::Request;

use crate::service::{self as command, ask, said};
use crate::util::{self, Fail, usage};

/// The seconds of `--try SECONDS`, which Spark takes: 10 to 3600.
///
/// # Errors
///
/// A usage error, for words that are not such a number.
pub fn seconds(text: &str) -> Result<u32, Fail> {
    let range = spark_api::trial::SECONDS;
    text.parse::<u32>().ok().filter(|seconds| range.contains(seconds)).ok_or_else(|| {
        usage(format!("--try takes {} to {} seconds, not {text}", range.start(), range.end()))
    })
}

/// The time of day, on this machine's clock and in its zone, when a trial
/// of `seconds` begun now is undone unless it is kept.
pub fn undone_at(seconds: u32) -> String {
    let at = util::now_ns() / 1_000_000_000 + u64::from(seconds);
    util::time_text(i64::try_from(at).unwrap_or(i64::MAX), "%H:%M:%S", false)
}

/// How the service that reads a tried file takes the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToUse<'a> {
    /// The service starts again, as a service's own file needs.
    Restart(&'a str),
    /// The service reads its settings again and runs on, as netd does.
    Reload(&'a str),
    /// The service stops, as its file now says it does not run: Spark's
    /// undo starts it again, since it ran when the trial began.
    Stop(&'a str),
}

impl ToUse<'_> {
    /// The service's name.
    fn name(&self) -> &str {
        match self {
            ToUse::Restart(name) | ToUse::Reload(name) | ToUse::Stop(name) => name,
        }
    }
}

/// What came of a trial that Spark began.
#[derive(Debug, PartialEq, Eq)]
pub enum Tried {
    /// The new file is in use, and Spark's timer runs.
    InUse,
    /// The service did not take the new file: `why`, as the restart or
    /// the reload said, and `undone`, Spark's words of the undo.
    Undone {
        /// Why the use failed.
        why: String,
        /// What Spark said of the undo.
        undone: String,
    },
}

/// The request that asks Spark to try `path` as trial `id` for
/// `seconds`, putting it back with `to_use`.
#[cfg(test)]
fn request(id: &str, path: &Path, seconds: u32, to_use: ToUse<'_>) -> Request {
    request_of(id, &[path], seconds, to_use)
}

/// [`request`] for the files at `paths`, which one trial keeps together.
fn request_of(id: &str, paths: &[&Path], seconds: u32, to_use: ToUse<'_>) -> Request {
    let (restart, reload) = match to_use {
        // Spark restarts a service named here when it undoes the trial if
        // it ran when the trial began, and stops it if it did not, which
        // puts back a service that the change stopped as it was.
        ToUse::Restart(name) | ToUse::Stop(name) => (vec![name.to_string()], Vec::new()),
        ToUse::Reload(name) => (Vec::new(), vec![name.to_string()]),
    };
    let files = paths.iter().map(|path| path.display().to_string()).collect();
    Request { id: id.into(), seconds, files, restart, reload }
}

/// The name of the file beside `path` that holds its new text: hidden,
/// and this process's own.
fn beside(path: &Path) -> PathBuf {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    path.with_file_name(format!(".{name}.new-{}", std::process::id()))
}

/// Writes `text` beside `path`, in a file made new for it, with the mode
/// `path` has (`mode` when it is not there), and syncs it. Gives its path.
///
/// # Errors
///
/// When the file cannot be made or written; none is left behind.
pub fn write_beside(path: &Path, text: &[u8], mode: u32) -> Result<PathBuf, Fail> {
    let copy = beside(path);
    let mode = fs::metadata(path).map_or(mode, |m| m.permissions().mode() & 0o7777);
    let written = OpenOptions::new().write(true).create_new(true).mode(mode).open(&copy).and_then(
        |mut file| {
            file.write_all(text)?;
            file.sync_all()
        },
    );
    match written {
        Ok(()) => Ok(copy),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                let _ = fs::remove_file(&copy);
            }
            Err(util::failed(&copy.display(), error))
        }
    }
}

/// Syncs the folder that holds `path`, so that a rename in it lasts.
fn sync_folder(path: &Path) {
    if let Some(folder) = path.parent() {
        // A folder that cannot be synced loses nothing that is not lost
        // already: the rename is done, and the file is whole.
        let _ = File::open(folder).and_then(|folder| folder.sync_all());
    }
}

/// Puts `text` in place of `path` at once, with no trial: written beside
/// it, synced, and renamed over it.
///
/// # Errors
///
/// When the new file cannot be written or renamed; `path` is then as it
/// was.
pub fn replace(path: &Path, text: &[u8], mode: u32) -> Result<(), Fail> {
    let copy = write_beside(path, text, mode)?;
    if let Err(error) = fs::rename(&copy, path) {
        let _ = fs::remove_file(&copy);
        return Err(util::failed(&path.display(), error));
    }
    sync_folder(path);
    Ok(())
}

/// Tries `text` as `path`'s for `seconds`, as trial `id`: written beside
/// it, then [`try_file`].
///
/// # Errors
///
/// As [`write_beside`] and [`try_file`].
pub fn try_text(
    id: &str,
    path: &Path,
    text: &[u8],
    mode: u32,
    seconds: u32,
    to_use: ToUse<'_>,
) -> Result<Tried, Fail> {
    let copy = write_beside(path, text, mode)?;
    try_file(id, path, &copy, seconds, to_use)
}

/// Tries `copy`, a file beside `path` that holds its new text, in place
/// of `path` for `seconds`, as trial `id`: asks Spark to keep `path` and
/// to put it back unless the trial is kept, renames `copy` over `path`,
/// says so, and puts it to use as `to_use` says. A use that fails is
/// undone at once. `copy` is gone after the rename, and removed when the
/// trial does not begin.
///
/// # Errors
///
/// Spark's refusal of the trial, when nothing has changed; or a rename
/// that failed, once Spark is asked to undo.
pub fn try_file(
    id: &str,
    path: &Path,
    copy: &Path,
    seconds: u32,
    to_use: ToUse<'_>,
) -> Result<Tried, Fail> {
    try_files(id, &[(path, copy)], seconds, to_use)
}

/// Tries the new texts of several files as one trial, `id`: each `(path,
/// text, mode)` is written beside its file, then [`try_files`].
///
/// # Errors
///
/// As [`write_beside`] and [`try_files`]; a copy made before a write that
/// failed is removed.
pub fn try_texts(
    id: &str,
    changes: &[(&Path, &[u8], u32)],
    seconds: u32,
    to_use: ToUse<'_>,
) -> Result<Tried, Fail> {
    let mut copies: Vec<PathBuf> = Vec::new();
    for &(path, text, mode) in changes {
        match write_beside(path, text, mode) {
            Ok(copy) => copies.push(copy),
            Err(failed) => {
                for copy in &copies {
                    let _ = fs::remove_file(copy);
                }
                return Err(failed);
            }
        }
    }
    let pairs: Vec<(&Path, &Path)> =
        changes.iter().zip(&copies).map(|(&(path, _, _), copy)| (path, copy.as_path())).collect();
    try_files(id, &pairs, seconds, to_use)
}

/// [`try_file`] for several files, which Spark keeps and puts back
/// together: each `(path, copy)` is renamed in the order given, and a
/// rename that fails has Spark undo the trial, which puts back those
/// renamed before it.
///
/// # Errors
///
/// As [`try_file`].
pub fn try_files(
    id: &str,
    files: &[(&Path, &Path)],
    seconds: u32,
    to_use: ToUse<'_>,
) -> Result<Tried, Fail> {
    let remove_copies = |from: usize| {
        for &(_, copy) in &files[from..] {
            let _ = fs::remove_file(copy);
        }
    };
    let paths: Vec<&Path> = files.iter().map(|&(path, _)| path).collect();
    if let Err(refused) = ask(&request_of(id, &paths, seconds, to_use).line()) {
        remove_copies(0);
        return Err(refused);
    }
    for (at, &(path, copy)) in files.iter().enumerate() {
        if let Err(error) = fs::rename(copy, path) {
            remove_copies(at);
            let _ = ask(&format!("undo {id}"));
            return Err(util::failed(&path.display(), error));
        }
        sync_folder(path);
    }
    for path in &paths {
        println!("{}: trying the change for {seconds} s", path.display());
    }
    let path = paths.first().copied().unwrap_or(Path::new("/"));
    let used = match to_use {
        ToUse::Restart(name) => command::change("restart", name, &[]).map(drop),
        ToUse::Reload(name) => ask(&format!("reload {name}")).map(drop),
        ToUse::Stop(name) => command::change("stop", name, &[]).map(drop),
    };
    let Err(failed) = used else {
        return Ok(Tried::InUse);
    };
    // Spark undoes a trial whose restart failed by itself, so that the
    // undo asked here finds none; a refused reload is undone here.
    let undone = match ask(&format!("undo {id}")) {
        Ok(answer) => said(&answer).to_string(),
        Err(_) => undone_words(path, to_use),
    };
    Ok(Tried::Undone { why: failed.message, undone })
}

/// What Spark did when it undid the trial of `path` by itself.
fn undone_words(path: &Path, to_use: ToUse<'_>) -> String {
    match to_use {
        ToUse::Restart(name) | ToUse::Stop(name) if path.starts_with(crate::servicefiles::DIR) => {
            format!("Spark undid the change: {name} runs its file as it was")
        }
        _ => format!(
            "Spark undid the change: {} is as it was, and {} uses it",
            path.display(),
            to_use.name()
        ),
    }
}

/// The trial `id`, while Spark runs it.
///
/// # Errors
///
/// When Spark cannot be read.
pub fn running(id: &str) -> Result<Option<show::Trial>, Fail> {
    Ok(command::shown(None)?.trials.into_iter().find(|trial| trial.id == id))
}

/// Asks Spark to keep trial `id`, or to undo it now, as `verb` says, and
/// gives Spark's words. `none` is the refusal when no such trial runs.
///
/// # Errors
///
/// `none()`, or Spark's refusal.
pub fn end(verb: &str, id: &str, none: impl FnOnce() -> Fail) -> Result<String, Fail> {
    debug_assert!(matches!(verb, "keep" | "undo"), "{verb}");
    match ask(&format!("{verb} {id}")) {
        Ok(answer) => Ok(said(&answer).to_string()),
        Err(refused) if refused.message.starts_with(&format!("no trial called {id} runs")) => {
            Err(none())
        }
        Err(refused) => Err(refused),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trial_names_its_file_and_how_the_service_takes_it() {
        let path = Path::new("/system/settings/network");
        let reload = request("network", path, 300, ToUse::Reload("netd"));
        assert_eq!(reload.line(), "try network 300 file=/system/settings/network reload=netd\n");
        let path = Path::new("/system/settings/firewall");
        let restart = request("firewall", path, 60, ToUse::Restart("firewall"));
        assert_eq!(
            restart.line(),
            "try firewall 60 file=/system/settings/firewall restart=firewall\n"
        );
        // What Spark takes, it parses back the same.
        let line = restart.line();
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        assert_eq!(Request::parse(&words), Ok(restart));
        // A service the change stops is restarted by the undo, which
        // starts it again, since it ran when the trial began.
        let path = Path::new("/system/settings/spark/services/sshd.toml");
        let stop = request("ssh", path, 120, ToUse::Stop("sshd"));
        assert_eq!(stop.line(), format!("try ssh 120 file={} restart=sshd\n", path.display()));
    }

    #[test]
    fn a_trial_lasts_ten_seconds_to_an_hour() {
        assert_eq!(seconds("10").unwrap(), 10);
        assert_eq!(seconds("3600").unwrap(), 3600);
        for text in ["9", "3601", "-5", "1m", ""] {
            let refused = seconds(text).unwrap_err();
            assert!(refused.usage, "{text}");
            assert_eq!(refused.message, format!("--try takes 10 to 3600 seconds, not {text}"));
        }
    }

    #[test]
    fn the_new_text_is_beside_the_file_hidden_and_this_process_s() {
        let copy = beside(Path::new("/system/settings/network"));
        let name = format!(".network.new-{}", std::process::id());
        assert_eq!(copy, Path::new("/system/settings").join(name));
    }

    /// A new folder for test `name`.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trial-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_replace_keeps_the_file_s_mode_and_leaves_nothing_beside_it() {
        let dir = scratch("replace");
        let path = dir.join("settings");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        replace(&path, b"new\n", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o600);
        // A file that is not there takes the mode given.
        let fresh = dir.join("fresh");
        replace(&fresh, b"made\n", 0o640).unwrap();
        assert_eq!(fs::metadata(&fresh).unwrap().permissions().mode() & 0o7777, 0o640);
        let left: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_that_is_there_already_is_not_written_over_or_removed() {
        let dir = scratch("beside");
        let path = dir.join("settings");
        fs::write(beside(&path), "another writer's\n").unwrap();
        let refused = write_beside(&path, b"mine\n", 0o644).unwrap_err();
        assert!(refused.message.contains("File exists"), "{}", refused.message);
        assert_eq!(fs::read_to_string(beside(&path)).unwrap(), "another writer's\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spark_s_own_undo_is_told_in_words_of_the_file() {
        let service = Path::new(crate::servicefiles::DIR).join("sshd.toml");
        assert_eq!(
            undone_words(&service, ToUse::Restart("sshd")),
            "Spark undid the change: sshd runs its file as it was"
        );
        assert_eq!(
            undone_words(Path::new("/system/settings/firewall"), ToUse::Restart("firewall")),
            "Spark undid the change: /system/settings/firewall is as it was, and firewall uses it"
        );
    }
}
