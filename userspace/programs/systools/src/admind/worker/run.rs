// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The worker's `run`: an action's command, run as the person (ADR 0515).
//!
//! The command is a list of words that admind's action made, never text
//! for a shell. Its program is found only in `/system/commands` and
//! `/applications/commands`, as authd finds an elevated one, so a word
//! that names a path, or a program of the person's own, is refused. It
//! runs with the worker's own environment, no terminal, and input that
//! ends at once; its output and its error output are kept as admind's
//! elevated ones are. At most 8 run at once for a worker, and each has 30
//! seconds, after which it is killed.

use std::cell::Cell;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::unix::pipe::Receiver;

use crate::admind::errors::Kind;
use crate::admind::link::Fault;
use crate::admind::run::{RUN_TIME, exit_code, read_both};

/// Where a command's program may be, in the order they are looked in.
const PROGRAM_FOLDERS: [&str; 2] = ["/system/commands", "/applications/commands"];

/// The most commands a worker runs at once.
const RUNNING_MAX: usize = 8;

/// The most words of a command, and bytes of them, as authd allows an
/// elevated one.
const WORDS_MAX: usize = 64;
const BYTES_MAX: usize = 32 * 1024;

thread_local! {
    /// The commands this worker runs now. The worker is one thread.
    static RUNNING: Cell<usize> = const { Cell::new(0) };
}

/// A place among the commands that run at once.
struct Place;

impl Place {
    fn take() -> Option<Place> {
        RUNNING.with(|running| {
            (running.get() < RUNNING_MAX).then(|| {
                running.set(running.get() + 1);
                Place
            })
        })
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        RUNNING.with(|running| running.set(running.get().saturating_sub(1)));
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    argv: Vec<String>,
}

/// `run {argv}` -> `{status, output, errors}`.
pub async fn run(params: Value) -> Result<Value, Fault> {
    let Params { argv } = serde_json::from_value(params)
        .map_err(|err| Fault::new(Kind::BadRequest, format!("run does not take that: {err}")))?;
    run_in(&argv, &PROGRAM_FOLDERS).await
}

/// Runs `argv`, its program found in `folders`.
async fn run_in(argv: &[String], folders: &[&str]) -> Result<Value, Fault> {
    check(argv)?;
    let program = find(&argv[0], folders)
        .ok_or_else(|| Fault::new(Kind::BadRequest, format!("there is no command {}", argv[0])))?;
    let _place = Place::take()
        .ok_or_else(|| Fault::new(Kind::Wait, "too many commands at once for this session"))?;
    let internal = |err: std::io::Error| Fault::new(Kind::Internal, format!("{}: {err}", argv[0]));
    let mut child = Command::new(&program)
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(internal)?;
    let output = child.stdout.take().map(OwnedFd::from).expect("piped");
    let errors = child.stderr.take().map(OwnedFd::from).expect("piped");
    let output = Receiver::from_file(File::from(output)).map_err(internal)?;
    let errors = Receiver::from_file(File::from(errors)).map_err(internal)?;
    let (output, errors) = match tokio::time::timeout(RUN_TIME, read_both(output, errors)).await {
        Ok(read) => read.map_err(internal)?,
        Err(_) => {
            // Killed and waited for, so that no process of it is left.
            let _ = child.kill();
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
            let why = format!("{} did not end in {} seconds", argv[0], RUN_TIME.as_secs());
            return Err(Fault::new(Kind::Internal, why));
        }
    };
    // Its output has closed, so it has ended or is ending: the wait is
    // short, and off the thread that answers the link.
    let status = tokio::task::spawn_blocking(move || child.wait())
        .await
        .map_err(|err| Fault::new(Kind::Internal, format!("waiting for {}: {err}", argv[0])))?
        .map_err(internal)?;
    let status = std::os::unix::process::ExitStatusExt::into_raw(status);
    Ok(json!({
        "status": exit_code(status),
        "output": String::from_utf8_lossy(&output),
        "errors": String::from_utf8_lossy(&errors),
    }))
}

/// Whether `argv` can be a command: a program's name and at most 63
/// arguments, 32 KiB in all, none holding a NUL.
fn check(argv: &[String]) -> Result<(), Fault> {
    let bad = |why: &str| Err(Fault::new(Kind::BadRequest, why.to_string()));
    if argv.is_empty() || argv.len() > WORDS_MAX {
        return bad("a command is a program and at most 63 arguments");
    }
    if argv.iter().map(String::len).sum::<usize>() > BYTES_MAX {
        return bad("a command is at most 32 KiB");
    }
    if argv.iter().any(|word| word.contains('\0')) {
        return bad("a command's words hold no NUL");
    }
    Ok(())
}

/// The program `name` names in `folders`: a bare name, found as a file in
/// the first folder that has it. A name with a `/`, `.` or `..` is no
/// program's.
fn find(name: &str, folders: &[&str]) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return None;
    }
    folders.iter().map(|folder| Path::new(folder).join(name)).find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_a_bare_name_in_the_folders_only() {
        let dir = std::env::temp_dir().join(format!("admind-run-{}", std::process::id()));
        let (first, second) = (dir.join("first"), dir.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("tool"), b"").unwrap();
        std::fs::write(first.join("both"), b"").unwrap();
        std::fs::write(second.join("both"), b"").unwrap();
        std::fs::create_dir_all(first.join("folder")).unwrap();
        let folders = [first.to_str().unwrap(), second.to_str().unwrap()];
        assert_eq!(find("tool", &folders), Some(second.join("tool")));
        assert_eq!(find("both", &folders), Some(first.join("both")));
        for name in ["", ".", "..", "missing", "folder", "../second/tool", "/bin/sh", "first/both"]
        {
            assert_eq!(find(name, &folders), None, "{name:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_command_has_a_program_and_limits() {
        let words = |n: usize| vec!["w".to_string(); n];
        assert!(check(&words(1)).is_ok());
        assert!(check(&words(64)).is_ok());
        assert!(check(&[]).is_err());
        assert!(check(&words(65)).is_err());
        assert!(check(&["a".repeat(BYTES_MAX + 1)]).is_err());
        assert!(check(&["a\0b".to_string()]).is_err());
    }

    #[test]
    fn a_command_s_status_output_and_errors_come_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("admind-ran-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("tool");
        std::fs::write(&tool, "#!/bin/sh\necho out \"$1\"\necho err >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let folders = [dir.to_str().unwrap()];
        let argv = |words: &[&str]| words.iter().map(ToString::to_string).collect::<Vec<_>>();
        let ran = runtime.block_on(run_in(&argv(&["tool", "a b"]), &folders)).unwrap();
        assert_eq!(ran, json!({ "status": 3, "output": "out a b\n", "errors": "err\n" }));
        let missing = runtime.block_on(run_in(&argv(&["nothing"]), &folders)).unwrap_err();
        assert_eq!(missing.id, Kind::BadRequest.id());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
