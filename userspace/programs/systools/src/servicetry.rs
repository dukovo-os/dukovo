// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `service try NAME`, `service keep NAME` and `service undo NAME`
//! (service(8), ADR 0472): a change to a service's file tried with an
//! undo. Spark keeps the file the change replaces, the service restarts
//! with the change, and unless the change is kept within the time, here
//! or from another session, Spark puts the file back and restarts the
//! service as it was. A start that fails with the change undoes it at
//! once, and so does a start of the machine before it was kept, so a
//! change that cuts the administrator off comes undone by itself.

use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use manifest::service;
use spark_api::trial::{self, Request};

use crate::service::{self as command, ask, said};
use crate::servicefiles::{self, Place};
use crate::util::{self, Fail, fail, fail_with, usage};

const USAGE: &str = "Usage: service try NAME [--for SECONDS] | keep NAME | undo NAME";

/// The trial `service try NAME` begins: Spark's name for it.
pub fn trial_id(name: &str) -> String {
    format!("service.{name}")
}

/// `service try NAME [--for SECONDS]`: edits NAME's file as `service
/// edit` does, then tries the change.
pub fn try_change(operands: &[String]) -> Result<i32, Fail> {
    let (name, seconds) = match operands {
        [name] => (name, trial::SECONDS_DEFAULT),
        [name, flag, seconds] if flag == "--for" => {
            let seconds =
                seconds.parse::<u32>().ok().filter(|s| trial::SECONDS.contains(s)).ok_or_else(
                    || {
                        usage(format!(
                            "--for takes {} to {} seconds, not {seconds}",
                            trial::SECONDS.start(),
                            trial::SECONDS.end()
                        ))
                    },
                )?;
            (name, seconds)
        }
        _ => return Err(usage(USAGE.into())),
    };
    // A service's name, not a path: what is tried is the file Spark reads.
    service::check_name(name).map_err(fail)?;
    // Spark restarts only a service it knows, which one never started
    // since its file was written is not yet.
    command::shown(Some(name))?;
    servicefiles::edit_file(name, Place::Trial(seconds))
}

/// Puts the checked `copy` in place of `path`, service `name`'s file, as
/// a trial of `seconds`: asks Spark to keep the file, renames the copy
/// over it, restarts the service and waits for it, then asks whether to
/// keep the change.
pub fn put_in_place(path: &Path, copy: &Path, name: &str, seconds: u32) -> Result<i32, Fail> {
    let id = trial_id(name);
    let request = Request {
        id: id.clone(),
        seconds,
        files: vec![path.display().to_string()],
        restart: vec![name.into()],
        reload: Vec::new(),
    };
    ask(&request.line())?;
    let began = Instant::now();
    if let Err(e) = std::fs::rename(copy, path) {
        let _ = ask(&format!("undo {id}"));
        return Err(util::failed(&path.display(), e));
    }
    println!("{}: trying the change for {seconds} s", path.display());
    if let Err(failed) = command::change("restart", name, &[]) {
        eprintln!("service: {}", failed.message);
        // Spark undoes it once the start fails; this is for one that was
        // refused, which never started.
        let undone = match ask(&format!("undo {id}")) {
            Ok(answer) => said(&answer).to_string(),
            Err(_) => format!("Spark undid the change: {name} runs its file as it was"),
        };
        println!("{undone}");
        return Ok(1);
    }
    let end = began + Duration::from_secs(u64::from(seconds));
    if !util::is_terminal(0) {
        println!(
            "Spark undoes it in {} s unless it is kept: `service keep {name}`",
            end.saturating_duration_since(Instant::now()).as_secs()
        );
        return Ok(0);
    }
    loop {
        let left = end.saturating_duration_since(Instant::now());
        print!(
            "Keep the change to {name}? (y)es keeps it, (n)o undoes it now; Spark undoes it by \
             itself in {} s, unless `service keep {name}` keeps it from another session: ",
            left.as_secs()
        );
        let _ = io::stdout().flush();
        match answer_within(left) {
            Answer::Line(line) if line.trim_start().starts_with(['y', 'Y']) => {
                return match ask(&format!("keep {id}")) {
                    Ok(_) => {
                        println!("kept: {name} runs the change");
                        Ok(0)
                    }
                    Err(failed) => Err(fail(format!("{}; the change was undone", failed.message))),
                };
            }
            Answer::Line(line) if line.trim_start().starts_with(['n', 'N']) => {
                let answer = ask(&format!("undo {id}"))?;
                println!("{}", said(&answer));
                return Ok(1);
            }
            Answer::Line(_) => {}
            Answer::TimeUp => {
                println!();
                let undone = match ask(&format!("undo {id}")) {
                    Ok(answer) => said(&answer).to_string(),
                    Err(_) => format!("Spark undid the change: {name} runs its file as it was"),
                };
                println!("not kept in time: {undone}");
                return Ok(1);
            }
            Answer::Gone => {
                println!();
                return Err(fail_with(
                    130,
                    format!(
                        "Spark undoes the change in {} s unless `service keep {name}` keeps it",
                        end.saturating_duration_since(Instant::now()).as_secs()
                    ),
                ));
            }
        }
    }
}

/// `service keep NAME` and `service undo NAME`: what the trial `service
/// try NAME` began comes to.
pub fn end(verb: &str, operands: &[String]) -> Result<i32, Fail> {
    let [name] = operands else {
        return Err(usage(format!("Usage: service {verb} NAME")));
    };
    service::check_name(name).map_err(fail)?;
    let answer = ask(&format!("{verb} {}", trial_id(name)))?;
    match verb {
        "keep" => println!("kept: {name} runs the change"),
        _ => println!("{}", said(&answer)),
    }
    Ok(0)
}

/// What came of asking at the terminal.
enum Answer {
    /// A line was typed.
    Line(String),
    /// None came in the time.
    TimeUp,
    /// The terminal closed, or SIGINT came.
    Gone,
}

/// A line from standard input within `left`.
fn answer_within(left: Duration) -> Answer {
    let mut line = Vec::new();
    let end = Instant::now() + left;
    loop {
        let wait = end.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            return Answer::TimeUp;
        }
        let mut entry = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        let ms = libc::c_int::try_from(wait.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one pollfd, which lives across the call.
        let ready = unsafe { libc::poll(&raw mut entry, 1, ms) };
        if ready < 0 {
            // Another signal than SIGINT, as a resized terminal's.
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
                && !command::sigint_came()
            {
                continue;
            }
            return Answer::Gone;
        }
        if ready == 0 {
            continue;
        }
        let mut byte = [0u8; 1];
        // SAFETY: reads at most one byte into the array.
        let read = unsafe { libc::read(0, byte.as_mut_ptr().cast(), 1) };
        match read {
            1 if byte[0] == b'\n' => return Answer::Line(String::from_utf8_lossy(&line).into()),
            1 => line.push(byte[0]),
            _ => return Answer::Gone,
        }
    }
}
