// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! timerd's socket (ADR 0478): a connection sends one request, a line of
//! JSON ([`super::protocol`]), and reads one answer, after which timerd
//! closes it. Each connection has a thread of its own, at most
//! [`super::CONNECTIONS_MAX`] at a time, and a request has ten seconds to
//! arrive and be answered.
//!
//! Who asks is the user the kernel gives for the connection
//! (`socket_state`, which is POSIX's SO_PEERCRED), never what the request
//! says. Everyone may ask about their own crontab and their own `at`
//! jobs; only root may name another user, read every `at` job, or ask
//! for the service timers.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::protocol::{JobRow, LineRow, REQUEST_MAX, Reply, Request, TIME, TimerRow};
use super::state::{AT_JOBS_MAX, AtJob, plain_name};
use super::{Daemon, Entry, Key, clocks, crontab, log};
use crate::deadline::Deadline;
use crate::{sys, util};

/// The most `at` jobs one user keeps.
const AT_PER_USER: usize = 200;

/// The most bytes an `at` job's commands and environment are.
const AT_TEXT_MAX: usize = 128 * 1024;

/// The most bytes of a job's output `crontab --status` shows.
const OUTPUT_SHOWN: usize = 4096;

/// Who asks.
pub struct Asker {
    uid: u32,
    name: String,
}

/// Serves `stream` on a thread of its own, or says timerd is busy.
pub fn accept(daemon: &Arc<Daemon>, serving: &Arc<AtomicUsize>, max: usize, stream: UnixStream) {
    if serving.fetch_add(1, Ordering::AcqRel) >= max {
        serving.fetch_sub(1, Ordering::AcqRel);
        refuse(&stream, "timerd is busy with other requests; try again in a moment");
        return;
    }
    let (daemon, serving) = (Arc::clone(daemon), Arc::clone(serving));
    thread::spawn(move || {
        serve(&daemon, &stream);
        serving.fetch_sub(1, Ordering::AcqRel);
    });
}

fn refuse(stream: &UnixStream, message: &str) {
    if let Ok(mut out) = Deadline::new(stream, TIME) {
        let _ = write_reply(&mut out, &Reply::Error { message: message.into() }); // the asker may be gone
    }
}

fn write_reply(out: &mut impl Write, reply: &Reply) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(reply).map_err(std::io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// Who is at the other end of `stream`.
fn who_is(stream: &UnixStream) -> Result<Asker, String> {
    let slot = sys::slot_of(stream.as_raw_fd());
    let state = radix_abi::file::socket_state(&sys::SYS, slot)
        .map_err(|_| "timerd cannot tell who is asking".to_string())?;
    Ok(Asker { uid: state.user, name: util::user_name(state.user) })
}

fn serve(daemon: &Daemon, stream: &UnixStream) {
    let who = who_is(stream);
    let (Ok(input), Ok(mut out)) = (Deadline::new(stream, TIME), Deadline::new(stream, TIME))
    else {
        return;
    };
    let mut text = Vec::new();
    let limit = (REQUEST_MAX + 1) as u64;
    let read = BufReader::new(input).take(limit).read_until(b'\n', &mut text);
    let reply = match (read, who) {
        (Err(_) | Ok(0), _) => Reply::Error { message: "the request is empty".into() },
        (Ok(_), _) if text.len() > REQUEST_MAX => {
            Reply::Error { message: format!("a request is at most {} KiB", REQUEST_MAX / 1024) }
        }
        (_, Err(message)) => Reply::Error { message },
        (_, Ok(who)) => match serde_json::from_slice::<Request>(&text) {
            Ok(request) => daemon.answer(&who, request),
            Err(err) => Reply::Error { message: format!("the request does not read: {err}") },
        },
    };
    let _ = write_reply(&mut out, &reply); // the asker may be gone
}

fn error(message: impl Into<String>) -> Reply {
    Reply::Error { message: message.into() }
}

impl Daemon {
    /// The answer to `request` from `who`.
    fn answer(&self, who: &Asker, request: Request) -> Reply {
        match request {
            Request::Timers if who.uid == 0 => self.timers(),
            Request::Timers => error("only root sees the timers of the services; try elevate"),
            Request::CrontabCheck { text } => match crontab::parse(&text) {
                Ok(_) => Reply::Done,
                Err(mistake) => error(mistake.to_string()),
            },
            Request::CrontabInstall { user, text } => {
                Self::with_user(who, user, |user| self.install_crontab(user, &text))
            }
            Request::CrontabShow { user } => Self::with_user(who, user, |user| {
                self.store.crontab(user).map_or_else(
                    || error(format!("no crontab for {user}")),
                    |text| Reply::Text { text },
                )
            }),
            Request::CrontabRemove { user } => {
                Self::with_user(who, user, |user| self.remove_crontab(user))
            }
            Request::CrontabStatus { user } => {
                Self::with_user(who, user, |user| self.crontab_status(user))
            }
            Request::AtAdd { time, folder, environment, command } => {
                self.add_at(who, time, folder, environment, command)
            }
            Request::AtList => self.list_at(who),
            Request::AtRemove { id } => self.with_at(
                who,
                id,
                |_| Ok(()),
                |daemon, id| {
                    daemon.remove_at(id);
                    Reply::Done
                },
            ),
            Request::AtShow { id } => self.with_at(
                who,
                id,
                |_| Ok(()),
                |daemon, id| {
                    daemon.lock().at_jobs.get(&id).map_or_else(
                        || error(format!("no at job {id}")),
                        |job| Reply::Text { text: script(job) },
                    )
                },
            ),
        }
    }

    /// Runs `then` for the user a crontab request is about: the asker,
    /// or the user root names.
    fn with_user(who: &Asker, named: Option<String>, then: impl FnOnce(&str) -> Reply) -> Reply {
        let user = match named {
            Some(user) if user != who.name => {
                if who.uid != 0 {
                    return error("only root may use another user's crontab");
                }
                if util::user_id(&user).is_none() {
                    return error(format!("no user is called {user}"));
                }
                user
            }
            _ => who.name.clone(),
        };
        if !plain_name(&user) {
            return error(format!("{user} cannot have a crontab"));
        }
        then(&user)
    }

    fn timers(&self) -> Reply {
        let inner = self.lock();
        let timers = inner
            .entries
            .iter()
            .filter_map(|e| match &e.key {
                Key::Service(name) => {
                    Some(TimerRow { name: name.clone(), last: e.last_run, next: e.next })
                }
                _ => None,
            })
            .collect();
        Reply::Timers { timers }
    }

    fn install_crontab(&self, user: &str, text: &str) -> Reply {
        let crontab = match crontab::parse(text) {
            Ok(crontab) => crontab,
            Err(mistake) => return error(mistake.to_string()),
        };
        if let Err(err) = self.store.save_crontab(user, text) {
            log(3, &format!("cannot keep {user}'s crontab: {err}"));
            return error(format!("the crontab could not be kept: {}", util::strip_os_error(&err)));
        }
        self.store.clear_runs(user);
        let (now, _) = clocks();
        let mut inner = self.lock();
        self.put_crontab(&mut inner, user, crontab, now, false);
        log(6, &format!("crontab {user}: kept"));
        Reply::Done
    }

    fn remove_crontab(&self, user: &str) -> Reply {
        if !self.store.remove_crontab(user) {
            return error(format!("no crontab for {user}"));
        }
        let mut inner = self.lock();
        inner.crontabs.remove(user);
        inner.entries.retain(|e| !matches!(&e.key, Key::Line { user: u, .. } if u == user));
        log(6, &format!("crontab {user}: removed"));
        Reply::Done
    }

    fn crontab_status(&self, user: &str) -> Reply {
        let inner = self.lock();
        let Some(tab) = inner.crontabs.get(user) else {
            return error(format!("no crontab for {user}"));
        };
        let lines = tab
            .entries
            .iter()
            .map(|line| {
                let key = Key::Line { user: user.to_string(), line: line.line };
                let next = inner.entries.iter().find(|e| e.key == key).and_then(|e| e.next);
                let run = key.record().and_then(|r| self.store.run(&r)).unwrap_or_default();
                LineRow {
                    line: line.line,
                    time: line.time.clone(),
                    command: line.command.clone(),
                    next,
                    last_start: run.start,
                    last_end: run.end,
                    status: run.status,
                    not_run: run.not_run,
                    output: last_bytes(&run.output, OUTPUT_SHOWN),
                }
            })
            .collect();
        Reply::Status { lines }
    }

    fn add_at(
        &self,
        who: &Asker,
        time: i64,
        folder: String,
        environment: Vec<String>,
        command: String,
    ) -> Reply {
        if !plain_name(&who.name) {
            return error(format!("{} cannot have at jobs", who.name));
        }
        if !folder.starts_with('/') || folder.contains('\0') {
            return error("the folder is an absolute path");
        }
        if command.trim().is_empty() || command.contains('\0') {
            return error("there are no commands to run");
        }
        let size = command.len() + environment.iter().map(String::len).sum::<usize>();
        if size > AT_TEXT_MAX {
            return error(format!(
                "the commands and environment are at most {} KiB",
                AT_TEXT_MAX / 1024
            ));
        }
        if let Some(bad) =
            environment.iter().find(|e| !e.contains('=') || e.starts_with('=') || e.contains('\0'))
        {
            return error(format!("`{}` is not NAME=value", bad.escape_debug()));
        }
        let mut inner = self.lock();
        let mine = inner.at_jobs.values().filter(|j| j.owner == who.name).count();
        if inner.at_jobs.len() >= AT_JOBS_MAX || mine >= AT_PER_USER {
            return error(format!(
                "at keeps at most {AT_PER_USER} jobs for a user and {AT_JOBS_MAX} in all"
            ));
        }
        let job = AtJob { id: 0, owner: who.name.clone(), time, folder, environment, command };
        let taken = inner.at_jobs.keys().next_back().copied().unwrap_or(0);
        let job = match self.store.add_at_job(job, taken) {
            Ok(job) => job,
            Err(err) => {
                return error(format!("the job could not be kept: {}", util::strip_os_error(&err)));
            }
        };
        log(6, &format!("at {}: kept for {}", job.id, job.owner));
        inner.entries.push(Entry {
            key: Key::At(job.id),
            timing: None,
            last_run: None,
            next: Some(time),
            running: false,
        });
        let reply = Reply::Added { id: job.id, time };
        inner.at_jobs.insert(job.id, job);
        reply
    }

    fn list_at(&self, who: &Asker) -> Reply {
        let inner = self.lock();
        let jobs = inner
            .at_jobs
            .values()
            .filter(|job| who.uid == 0 || job.owner == who.name)
            .map(|job| JobRow {
                id: job.id,
                owner: job.owner.clone(),
                time: job.time,
                command: job.command.lines().next().unwrap_or("").to_string(),
            })
            .collect();
        Reply::Jobs { jobs }
    }

    /// Runs `then` for `at` job `id` when `who` may use it: its owner, or
    /// root; and when it is not running.
    fn with_at(
        &self,
        who: &Asker,
        id: u32,
        check: impl FnOnce(&AtJob) -> Result<(), String>,
        then: impl FnOnce(&Daemon, u32) -> Reply,
    ) -> Reply {
        {
            let inner = self.lock();
            let Some(job) = inner.at_jobs.get(&id) else { return error(format!("no at job {id}")) };
            if job.owner != who.name && who.uid != 0 {
                // The same words as for a job that is not there: another
                // user's jobs are not told of.
                return error(format!("no at job {id}"));
            }
            if let Err(why) = check(job) {
                return error(why);
            }
            if inner.entries.iter().any(|e| e.key == Key::At(id) && e.running) {
                return error(format!("at job {id} is running"));
            }
        }
        then(self, id)
    }
}

/// The last `max` bytes of `text`, from a character.
fn last_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut from = text.len() - max;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    text[from..].to_string()
}

/// `text` as a shell word, in single quotes.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The script of `at` job `job`, as `at -c` prints it: its environment,
/// its folder and its commands, as a shell reads them.
fn script(job: &AtJob) -> String {
    let mut text = format!("#!/bin/sh\n# at job {}, for {}\n", job.id, job.owner);
    for variable in &job.environment {
        if let Some((name, value)) = variable.split_once('=') {
            text.push_str(&format!("{name}={}; export {name}\n", quoted(value)));
        }
    }
    text.push_str(&format!(
        "cd {} || {{\n\techo 'Execution directory inaccessible' >&2\n\texit 1\n}}\n",
        quoted(&job.folder)
    ));
    text.push_str(&job.command);
    if !job.command.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> AtJob {
        AtJob {
            id: 7,
            owner: "alice".into(),
            time: 0,
            folder: "/home/it's".into(),
            environment: vec!["A=b c".into(), "Q=it's".into()],
            command: "echo hi".into(),
        }
    }

    #[test]
    fn a_script_has_the_environment_the_folder_and_the_commands() {
        let text = script(&job());
        assert!(text.starts_with("#!/bin/sh\n# at job 7, for alice\n"));
        assert!(text.contains("A='b c'; export A\n"));
        assert!(text.contains("Q='it'\\''s'; export Q\n"));
        assert!(text.contains("cd '/home/it'\\''s' || {"));
        assert!(text.ends_with("echo hi\n"));
    }

    #[test]
    fn the_tail_of_an_output_starts_on_a_character() {
        assert_eq!(last_bytes("abc", 10), "abc");
        assert_eq!(last_bytes("abcdef", 3), "def");
        assert_eq!(last_bytes("a\u{e9}b", 2), "b");
    }
}
