// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! at(1), atq, atrm and batch (ADR 0478): jobs that run once, at a time.
//!
//! `at TIME` reads the commands from standard input, or from the file
//! `-f` names, and sends them to timerd(8) with the environment and the
//! working folder, which POSIX has the job keep (but `TERM`, `DISPLAY`
//! and `_`, which belong to the terminal and the shell that ran `at`).
//! timerd runs them through authd(8) as the person, with `/bin/sh`, at
//! the time, or as soon as it starts if the time passed while the machine
//! was off. The time is read by [`time::parse`].
//!
//! `atq` and `at -l` list the person's jobs (all of them for root), `atrm`
//! and `at -d` remove jobs, and `at -c N` prints job N's script. `batch`
//! is not built: jobs that wait for a low load are a deferred row.
//!
//! The boot test `crontab` covers it.

mod time;

use std::io::{self, Read};

use crate::args::{Arg, Args};
use crate::timerd::protocol::{self, Reply, Request};
use crate::util::{self, Fail, fail, usage};

use self::time::Local;

/// The most bytes of commands `at` reads.
const COMMANDS_MAX: u64 = 128 * 1024;

/// Variables a job does not keep: those of the terminal and the shell
/// that ran `at`.
const NOT_KEPT: &[&str] = &["TERM", "DISPLAY", "_", "OLDPWD", "SHLVL"];

const USAGE: &str = "Usage: at [-f FILE] TIME   (commands from FILE or standard input)\n\
                     \x20      at -l | -c JOB | -d JOB...";

/// What the command line asks for.
enum Do {
    Add,
    List,
    Show(u32),
    Remove,
}

/// `at`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut file = None;
    let mut action = Do::Add;
    let mut operands: Vec<String> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('f') => file = Some(args.text()?),
            Arg::Short('l') => action = Do::List,
            Arg::Short('c') => action = Do::Show(job_number(&args.text()?)?),
            Arg::Short('d') => action = Do::Remove,
            Arg::Short('m' | 'q' | 'M' | 'v' | 't') | Arg::Long(_) => {
                return Err(usage(
                    "at takes -f, -l, -c and -d; mail and queues are not built".into(),
                ));
            }
            Arg::Short(c) => return Err(usage(format!("invalid option -- '{c}'"))),
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
        }
    }
    match action {
        Do::Add => add(&operands, file.as_deref()),
        Do::List => list(),
        Do::Show(id) => show(id),
        Do::Remove => remove(&operands),
    }
}

/// `atq`.
pub fn atq(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    list()
}

/// `atrm JOB...`.
pub fn atrm(args: &mut Args) -> Result<i32, Fail> {
    let mut jobs = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(job) => jobs.push(job.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    remove(&jobs)
}

/// `batch`: not built.
pub fn batch(args: &mut Args) -> Result<i32, Fail> {
    let _ = args;
    Err(fail(
        "batch is not built: jobs that wait for a low load are a deferred row, \"`batch`: jobs \
         that wait for a low load\"; `at now + 1 minute` runs one soon"
            .into(),
    ))
}

fn job_number(text: &str) -> Result<u32, Fail> {
    text.parse().map_err(|_| usage(format!("'{text}' is not a job number")))
}

/// `at TIME`.
fn add(words: &[String], file: Option<&str>) -> Result<i32, Fail> {
    if words.is_empty() {
        return Err(usage(USAGE.into()));
    }
    let now = now();
    let time = time::parse(words, now, &Local)
        .map_err(|why| fail(format!("{why}; at(1) lists the times")))?;
    let command = commands(file)?;
    let folder = std::env::current_dir()
        .map_err(|e| util::failed(&"the working folder", e))?
        .to_string_lossy()
        .into_owned();
    let request = Request::AtAdd { time, folder, environment: environment(), command };
    let Reply::Added { id, time } = protocol::answer(&request).map_err(fail)? else {
        return Err(fail("timerd's answer was not a job".into()));
    };
    eprintln!("warning: commands will be executed using /bin/sh");
    eprintln!("job {id} at {}", util::local_time(time, "%a %b %e %H:%M:%S %Y"));
    Ok(0)
}

/// The time it is, in seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The commands, from `file` or standard input.
fn commands(file: Option<&str>) -> Result<String, Fail> {
    let mut bytes = Vec::new();
    let limit = COMMANDS_MAX + 1;
    match file {
        Some(file) => std::fs::File::open(file)
            .and_then(|f| f.take(limit).read_to_end(&mut bytes))
            .map_err(|e| util::failed(&file, e))?,
        None => io::stdin()
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|e| util::failed(&"standard input", e))?,
    };
    if bytes.len() as u64 > COMMANDS_MAX {
        return Err(fail(format!("the commands are at most {} KiB", COMMANDS_MAX / 1024)));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| fail("the commands are not UTF-8 text".into()))?;
    if text.trim().is_empty() {
        return Err(fail("there are no commands to run".into()));
    }
    Ok(text)
}

/// The environment a job keeps, `NAME=value` each.
fn environment() -> Vec<String> {
    std::env::vars()
        .filter(|(name, _)| !name.is_empty() && !NOT_KEPT.contains(&name.as_str()))
        .map(|(name, value)| format!("{name}={value}"))
        .collect()
}

/// `atq`, `at -l`.
fn list() -> Result<i32, Fail> {
    let Reply::Jobs { jobs } = protocol::answer(&Request::AtList).map_err(fail)? else {
        return Err(fail("timerd's answer was not a list of jobs".into()));
    };
    for job in jobs {
        println!(
            "{}\t{} a {}",
            job.id,
            util::local_time(job.time, "%a %b %e %H:%M:%S %Y"),
            job.owner
        );
    }
    Ok(0)
}

/// `at -c N`.
fn show(id: u32) -> Result<i32, Fail> {
    match protocol::answer(&Request::AtShow { id }).map_err(fail)? {
        Reply::Text { text } => {
            print!("{text}");
            Ok(0)
        }
        _ => Err(fail("timerd's answer was not a script".into())),
    }
}

/// `atrm JOB...`, `at -d JOB...`: each job it can; fails if one could not.
fn remove(jobs: &[String]) -> Result<i32, Fail> {
    if jobs.is_empty() {
        return Err(usage("Usage: atrm JOB...".into()));
    }
    let mut failed = false;
    for job in jobs {
        let id = job_number(job)?;
        if let Err(why) = protocol::answer(&Request::AtRemove { id }) {
            eprintln!("atrm: {why}");
            failed = true;
        }
    }
    Ok(i32::from(failed))
}

#[cfg(test)]
mod tests {
    use super::time::Zone;
    use super::*;

    #[test]
    fn a_job_keeps_the_environment_but_the_terminals() {
        let kept = environment();
        assert!(kept.iter().all(|v| !v.starts_with("TERM=")
            && !v.starts_with("DISPLAY=")
            && !v.starts_with("_=")));
        assert!(kept.iter().all(|v| v.contains('=')));
    }

    #[test]
    fn a_job_number_is_a_number() {
        assert_eq!(job_number("12").unwrap(), 12);
        assert!(job_number("x").is_err() && job_number("-1").is_err() && job_number("").is_err());
    }

    #[test]
    fn batch_says_what_is_deferred() {
        let mut args = Args::new("batch", Vec::new());
        let failure = batch(&mut args).unwrap_err();
        assert!(failure.message.contains("jobs that wait for a low load"));
    }

    #[test]
    fn the_local_zone_reads_a_time_it_wrote() {
        let now = 1_794_659_696;
        let civil = Local.civil(now);
        assert_eq!(Local.epoch(&civil), now - now % 60);
    }
}
