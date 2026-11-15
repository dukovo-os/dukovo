// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The worker's `run`: an action's command, run as the person (ADR 0515).
//!
//! The command is a list of words that admind's action made, never text
//! for a shell. Its program is found only in `/system/commands` and
//! `/applications/commands`, as authd finds an elevated one, so a word
//! that names a path, or a program of the person's own, is refused. It
//! runs with the worker's own environment, no terminal, and input that
//! holds the secrets the action takes, if any, and then ends (ADR 0561);
//! its output and its error output are kept as admind's elevated ones are.
//! At most 8 run at once for a worker, and each has 30 seconds, after
//! which it is killed. A program that is in neither folder is ADM-129,
//! said as authd says it of an elevated one.

use std::cell::Cell;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::unix::pipe::Receiver;

use crate::admind::actions::Secret;
use crate::admind::actions::secret::INPUT_MAX;
use crate::admind::errors::{Kind, NOT_FOUND};
use crate::admind::link::Fault;
use crate::admind::run::{RUN_TIME, exit_code, read_both, typed_and_ended};

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
    /// What the command reads on its input: the secrets of the action.
    #[serde(default)]
    input: Option<Secret>,
}

/// `run {argv, input?}` -> `{status, output, errors}`.
pub async fn run(params: Value) -> Result<Value, Fault> {
    // serde's words are not given: they could quote the input.
    let Params { argv, input } = serde_json::from_value(params).map_err(|_| {
        Fault::new(Kind::BadRequest, "run takes argv, a list of words, and input, text")
    })?;
    run_in(&argv, input.as_ref(), &PROGRAM_FOLDERS).await
}

/// Runs `argv`, its program found in `folders`, with `input` on its
/// standard input.
async fn run_in(argv: &[String], input: Option<&Secret>, folders: &[&str]) -> Result<Value, Fault> {
    check(argv)?;
    if input.is_some_and(|input| input.bytes().len() > INPUT_MAX) {
        return Err(Fault::new(
            Kind::BadRequest,
            format!("a command's input is at most {INPUT_MAX} bytes"),
        ));
    }
    let program = find(&argv[0], folders)
        .ok_or_else(|| Fault::new(Kind::NoProgram, format!("{}{NOT_FOUND}", argv[0])))?;
    let _place = Place::take()
        .ok_or_else(|| Fault::new(Kind::Wait, "too many commands at once for this session"))?;
    let internal = |err: std::io::Error| Fault::new(Kind::Internal, format!("{}: {err}", argv[0]));
    // The input is written to a pipe of its own before the command starts,
    // which holds it all (INPUT_MAX), and the pipe is closed.
    let stdin = match input {
        Some(input) => {
            let (reading, typed) = std::io::pipe().map_err(internal)?;
            typed_and_ended(typed, Some(input)).map_err(internal)?;
            Stdio::from(reading)
        }
        None => Stdio::null(),
    };
    let mut child = Command::new(&program)
        .args(&argv[1..])
        .stdin(stdin)
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
    fn the_lab_s_missing_command_is_in_neither_folder_of_commands() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let program = crate::admind::actions::lab::MISSING_PROGRAM;
        let fault = runtime.block_on(run(json!({ "argv": [program, "--json"] }))).unwrap_err();
        assert_eq!(fault.id, Kind::NoProgram.id());
        assert_eq!(fault.detail, format!("{program}: command not found"));
    }

    #[test]
    fn the_checks_command_is_in_neither_folder_of_commands_yet() {
        // checks.last runs it. This test is the one to change, with the
        // route's in routes/tests/act.rs, when the command is installed.
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let program = crate::admind::actions::checks::PROGRAM;
        let fault =
            runtime.block_on(run(json!({ "argv": [program, "--last", "--json"] }))).unwrap_err();
        assert_eq!(fault.id, Kind::NoProgram.id());
        assert_eq!(fault.detail, "dukovo-check: command not found");
    }

    #[test]
    fn a_command_reads_its_input_and_then_its_end_and_without_one_reads_its_end_at_once() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("admind-input-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("lines");
        // Each line read, and how the input ended: `cat` waits for its end.
        std::fs::write(
            &tool,
            "#!/bin/sh\nwhile read -r line; do echo \"[$line]\"; done\ncat\necho end\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let folders = [dir.to_str().unwrap()];
        let argv = vec!["lines".to_string()];
        let (current, new) =
            (Secret::from("old one".to_string()), Secret::from("n\u{e9}w".to_string()));
        let input = Secret::lines(&[&current, &new]);
        let ran = runtime.block_on(run_in(&argv, Some(&input), &folders)).unwrap();
        assert_eq!(ran["output"], "[old one]\n[n\u{e9}w]\nend\n");
        let ran = runtime.block_on(run_in(&argv, None, &folders)).unwrap();
        assert_eq!(ran["output"], "end\n");
        // The call admind makes: its input is text in the JSON.
        let call = json!({ "argv": ["lines"], "input": "one\n" });
        let params: Params = serde_json::from_value(call).unwrap();
        assert_eq!(params.input.unwrap().bytes(), b"one\n");
        // One that is more than a pipe holds is refused before anything runs.
        let long = Secret::from("x".repeat(INPUT_MAX + 1));
        let refused = runtime.block_on(run_in(&argv, Some(&long), &folders)).unwrap_err();
        assert_eq!(refused.id, Kind::BadRequest.id());
        // What run does not take is refused without quoting it.
        let wrong = runtime.block_on(run(json!({ "argv": ["lines"], "input": 5 }))).unwrap_err();
        assert!(!wrong.detail.contains('5'), "{}", wrong.detail);
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
        let ran = runtime.block_on(run_in(&argv(&["tool", "a b"]), None, &folders)).unwrap();
        assert_eq!(ran, json!({ "status": 3, "output": "out a b\n", "errors": "err\n" }));
        let missing = runtime.block_on(run_in(&argv(&["nothing"]), None, &folders)).unwrap_err();
        assert_eq!(missing.id, Kind::NoProgram.id());
        assert_eq!(missing.detail, "nothing: command not found");
        // A path is no program's name, and is said the same.
        let path = runtime.block_on(run_in(&argv(&["/bin/sh"]), None, &folders)).unwrap_err();
        assert_eq!(path.id, Kind::NoProgram.id());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
