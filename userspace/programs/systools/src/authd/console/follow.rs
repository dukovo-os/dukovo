// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's follows of the log (ADR 0518). admind opens two pipes
//! and keeps their read ends; authd starts `log --output json --follow
//! --lines 0` with their write ends as its output and its errors, so that
//! following the log is one command for as long as the page follows, not
//! a read every few seconds.
//!
//! authd writes the command itself from the options it is given, which it
//! checks: admind sends no command, and authd starts nothing but log(1)
//! this way.
//!
//! - **A person's own follow** runs as them, and shows the records of
//!   their own programs, as log(1) at their terminal does (ADR 0252). It
//!   ends when the console session ends.
//! - **A follow of the whole log** runs as root, as `elevate log` does,
//!   for an administrator whose console session is unlocked now, with an
//!   audit line for its start. authd ends it, on its own clock, when the
//!   session locks, its unlock's time runs out, or it ends: SIGHUP, then
//!   SIGKILL 5 seconds later, as for an administrator's shell.
//!
//! admind ends a follow itself when its page goes, at an hour, when the
//! session starts another, and when admind stops, by closing the pipes:
//! log ends at its next write. authd answers with the process and does
//! not hold the request.

use std::fs::File;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use accounts::console::Filter;
use accounts::request::{Answer, Mode};

use super::signal_command;
use super::{Authd, Files, NO_SESSION, Session, TERMINAL, may_sign_in, no, short, shown};
use crate::{elevate, log, signin};

/// How long a follow of the whole log, hung up, has to end before it is
/// killed.
const KILL_AFTER: Duration = Duration::from_secs(5);

/// The most follows authd keeps at once. admind allows eight, and one for
/// each session; this is authd's own bound, with room for follows that
/// are ending while their sessions start others.
const FOLLOWS_MAX: usize = 16;

/// log(1).
const LOG: &str = "/system/commands/log";

/// A follow authd started.
pub(in crate::authd) struct Follow {
    /// Whose.
    name: String,
    /// The console session that asked for it.
    session: String,
    /// Whether it reads the whole log, as root.
    all: bool,
    /// When it was hung up.
    hung_up: Option<Instant>,
    /// Whether it was killed.
    killed: bool,
    /// Whether its process ended.
    reaped: bool,
}

impl Follow {
    /// Whether authd looks at it each second: a follow of the whole log,
    /// which ends within a second of its session's lock, or one that was
    /// hung up, until it is killed or gone.
    pub(super) fn watched(&self) -> bool {
        (self.all || self.hung_up.is_some()) && !self.killed
    }
}

/// Why `name`'s follow is refused, if it is: how many there are first,
/// then the whole log's rules.
pub(super) fn refusal(
    name: &str,
    is_admin: bool,
    all: bool,
    unlocked: bool,
    running: usize,
) -> Option<String> {
    if running >= FOLLOWS_MAX {
        return Some(format!("ADM-134: the console follows {running} logs, as many as it may"));
    }
    if all && !is_admin {
        return Some(format!(
            "ADM-122: {} is not an administrator, and reads the records of their own programs",
            shown(name)
        ));
    }
    if all && !unlocked {
        return Some("ADM-106: the session is locked: unlock it to follow the whole log".into());
    }
    None
}

/// Whether `fd` is a pipe.
fn is_pipe(fd: &OwnedFd) -> bool {
    sysapi::dir::stat(fd).is_ok_and(|stat| stat.st_mode & libc::S_IFMT == libc::S_IFIFO)
}

/// Opens /dev/null, for a follow's standard input: log(1) reads none, and
/// Rust's runtime aborts a program whose descriptor 0 is closed and that
/// cannot open it.
fn null() -> Result<OwnedFd, String> {
    File::open("/dev/null")
        .map(OwnedFd::from)
        .map_err(|err| format!("cannot start a follow: /dev/null: {err}"))
}

impl Authd {
    /// `console-follow`: starts the follow of the log that console session
    /// `session` asks for, on the two pipes client `i` sent: the person's
    /// own, or the whole log as root with `all`. See the module's comment.
    pub(super) fn console_follow(
        &mut self,
        i: usize,
        files: &Files,
        session: &str,
        all: bool,
        filter: &Filter,
        now: Instant,
    ) -> (Answer, Instant) {
        let descriptors = std::mem::take(&mut self.clients[i].descriptors);
        let [output, errors]: [OwnedFd; 2] = match <[OwnedFd; 2]>::try_from(descriptors) {
            Ok([output, errors]) if is_pipe(&output) && is_pipe(&errors) => [output, errors],
            _ => return no("a follow comes with two pipes, for its output and its errors", now),
        };
        let (name, from, unlocked) = match self.usable(session, now) {
            Ok(live) => {
                (live.name.clone(), live.from.clone(), live.unlocked.is_some_and(|u| u > now))
            }
            Err(why) => return no(why, now),
        };
        let Some(user) = files.user(&name).cloned() else {
            return no(NO_SESSION, now);
        };
        if let Err(why) = may_sign_in(files, &user) {
            return no(why, now);
        }
        let is_admin = accounts::is_admin(&user, &files.groups);
        let running = self.console.follows.values().filter(|follow| !follow.reaped).count();
        if let Some(why) = refusal(&name, is_admin, all, unlocked, running) {
            if all {
                audit!("refused {name} a follow of the whole log from {}: {why}", shown(&from));
            }
            return no(why, now);
        }
        let command: Vec<Vec<u8>> = match log::console_follow(filter) {
            Ok(words) => words.into_iter().map(String::into_bytes).collect(),
            Err(why) => return no(format!("ADM-125: {why}"), now),
        };
        let null = match null() {
            Ok(null) => null,
            Err(why) => return no(why, now),
        };
        let started = if all {
            let root = elevate::root_account(files.users.entries().cloned());
            elevate::plan(Mode::Command, &command, b"/", &root).and_then(|plan| {
                let mut env = elevate::environment(&root, &user, "", &[], &plan.shown);
                let tag = session.chars().take(8).collect::<String>();
                env.push(format!("DUKOVO_CONSOLE_SESSION={tag}"));
                let process = elevate::start(&[null, output, errors], &plan, &env, None)?;
                Ok((process, plan.shown))
            })
        } else {
            self.start_own_follow(&user, files, &command, [&null, &output, &errors])
                .map(|process| (process, String::new()))
        };
        let process = match started {
            Ok((process, shown_command)) => {
                let process = process as i32;
                if all {
                    audit!(
                        "{name} follows the whole log in the web console session {}: process \
                         {process} runs {shown_command}",
                        short(session)
                    );
                    let elevated = Session {
                        name: name.clone(),
                        terminal: TERMINAL.into(),
                        from: String::new(),
                        number: user.uid,
                        group: user.gid,
                        elevated: true,
                        remote: false,
                        recorded: false,
                        job: None,
                    };
                    self.sessions.insert(process, elevated);
                }
                process
            }
            Err(why) => return no(why, now),
        };
        let follow = Follow {
            name,
            session: session.to_string(),
            all,
            hung_up: None,
            killed: false,
            reaped: false,
        };
        self.console.follows.insert(process, follow);
        (Answer::Done(process.to_string()), now)
    }

    /// `console-follow-end`: ends the follow `process` of console session
    /// `session`, which admind no longer reads: SIGHUP at once, and SIGKILL
    /// [`KILL_AFTER`] later if it has not ended. authd, which reaps it,
    /// signals it, so that the number is never another process's.
    pub(super) fn console_follow_end(
        &mut self,
        session: &str,
        process: i32,
        now: Instant,
    ) -> (Answer, Instant) {
        match self.console.follows.get_mut(&process) {
            Some(follow) if follow.session == session && !follow.reaped => {
                if follow.hung_up.is_none() {
                    signal_command(process, libc::SIGHUP);
                    follow.hung_up = Some(now);
                }
                (Answer::Done(String::new()), now)
            }
            _ => no("that follow has ended", now),
        }
    }

    /// Starts `command`, log(1)'s, as `user`, with `fds` as its 0, 1 and
    /// 2.
    fn start_own_follow(
        &self,
        user: &accounts::User,
        files: &Files,
        command: &[Vec<u8>],
        fds: [&OwnedFd; 3],
    ) -> Result<u64, String> {
        let names = self.names.ok_or(
            "authd holds no names capability to start a follow with: its service file needs \
             `names = true`",
        )?;
        let groups =
            accounts::other_groups(user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        signin::start_console_follow(names, user, &groups, LOG, command, fds)
    }

    /// Ends the follows whose console session ended, and the follows of the
    /// whole log whose session is no longer unlocked: SIGHUP at once, and
    /// SIGKILL [`KILL_AFTER`] later if it has not ended.
    pub(in crate::authd) fn end_follows(&mut self, now: Instant) {
        let state = &mut self.console;
        let live = &state.live;
        let mut gone = Vec::new();
        for (&process, follow) in &mut state.follows {
            match follow.hung_up {
                None => {
                    let why = match live.get(&follow.session) {
                        None => "the console session ended",
                        Some(_) if !follow.all => continue,
                        Some(session) => match session.unlocked {
                            Some(until) if until > now => continue,
                            Some(_) => "the console's unlock ran out",
                            None => "the console locked",
                        },
                    };
                    signal_command(process, libc::SIGHUP);
                    if follow.all {
                        audit!(
                            "{}'s follow of the whole log, process {process}, ended: {why}",
                            follow.name
                        );
                    }
                    follow.hung_up = Some(now);
                }
                Some(at) if now.saturating_duration_since(at) >= KILL_AFTER => {
                    if !follow.killed && !follow.reaped {
                        signal_command(process, libc::SIGKILL);
                        follow.killed = true;
                    }
                    if follow.reaped {
                        gone.push(process);
                    }
                }
                Some(_) => {}
            }
        }
        for process in gone {
            state.follows.remove(&process);
        }
    }
}

impl super::State {
    /// Takes in that the follow `process` ended, if it was one. A follow
    /// that was hung up stays until it is killed or [`KILL_AFTER`] has
    /// passed, so that its number is not signalled once it is another
    /// process's.
    pub(in crate::authd) fn follow_ended(&mut self, process: i32) {
        match self.follows.get_mut(&process) {
            Some(follow) if follow.hung_up.is_some() && !follow.killed => follow.reaped = true,
            Some(_) => {
                self.follows.remove(&process);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_log_is_an_unlocked_administrator_s_and_there_are_at_most_sixteen() {
        assert_eq!(refusal("dave", false, false, false, 0), None);
        assert_eq!(refusal("chase", true, true, true, 0), None);
        let reader = refusal("dave", false, true, true, 0).unwrap();
        assert_eq!(
            reader,
            "ADM-122: dave is not an administrator, and reads the records of their own programs"
        );
        let locked = refusal("chase", true, true, false, 0).unwrap();
        assert!(locked.starts_with("ADM-106: the session is locked"), "{locked}");
        let many = refusal("dave", false, false, false, FOLLOWS_MAX).unwrap();
        assert!(many.starts_with("ADM-134: "), "{many}");
        assert_eq!(refusal("dave", false, false, false, FOLLOWS_MAX - 1), None);
    }

    #[test]
    fn a_follow_that_ended_is_forgotten_and_a_hung_up_one_kept_until_it_is_killed() {
        let mut state = super::super::State::default();
        let follow = |all, hung_up: Option<Instant>| Follow {
            name: "chase".into(),
            session: "s".into(),
            all,
            hung_up,
            killed: false,
            reaped: false,
        };
        state.follows.insert(10, follow(false, None));
        state.follows.insert(11, follow(true, Some(Instant::now())));
        assert!(!state.follows[&10].watched());
        assert!(state.follows[&11].watched());
        state.follow_ended(10);
        state.follow_ended(11);
        assert!(!state.follows.contains_key(&10));
        assert!(state.follows[&11].reaped);
        state.follow_ended(12);
    }

    #[test]
    fn a_follow_is_given_pipes() {
        let (read, write) = sysapi::fd::pipe().unwrap();
        assert!(is_pipe(&read) && is_pipe(&write));
        let file = OwnedFd::from(File::open("/dev/null").unwrap());
        assert!(!is_pipe(&file));
    }
}
