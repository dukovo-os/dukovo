// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The jobs authd runs for timerd (ADR 0478): a crontab line's or an
//! `at` job's command, started as its owner with `/bin/sh -c`, as a
//! session's first program is, with the one descriptor timerd sent as
//! its standard output and error.
//!
//! A person's job runs while they are signed in, at a console or over
//! SSH. While they are not, it runs only when they keep their jobs
//! running (`crontab --keep-running on`), or chose nothing and the
//! system's default, [`SETTINGS`], says so, and no administrator barred
//! it (`user set --jobs-signed-out never`). One that does not run is
//! counted, and their next sign-in says how many. Root's jobs always run.
//!
//! [`STATE`] keeps, for each person's number, what they chose, the bar
//! and the count: numbers are never given again (ADR 0220), so a rename
//! keeps it and nothing else can take it.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;

use accounts::User;
use accounts::request::{self, Answer, KeepRunning};
use serde::Deserialize;

use super::{Authd, Files, Peer, SHARE_TERMINAL, Session, WHEN, read_text, replace, shown};
use crate::{signin, util};

/// What authd keeps of each person's jobs, one line for each number:
/// `NUMBER CHOSEN BAR MISSED FIRST`, as `1000 on allowed 0 0`, where
/// CHOSEN is `on`, `off` or `default`, BAR `allowed` or `barred`, and
/// MISSED how many jobs did not run since they last signed in, the first
/// at FIRST, in seconds since 1970.
pub(super) const STATE: &str = "/system/state/authd/jobs";

/// The system's default for whether people's jobs run while they are
/// signed out: `keep_running = true` turns it on, as the `server` preset
/// does. Without the file it is off.
pub(super) const SETTINGS: &str = "/system/settings/jobs.toml";

/// What the terminal of a job's session is called, in the log.
pub(super) const JOB_TERMINAL: &str = "job";

/// What is kept of one person's jobs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Kept {
    /// What they chose, or `None` for the system's default.
    pub chosen: Option<bool>,
    /// Whether an administrator barred their jobs while they are signed
    /// out.
    pub barred: bool,
    /// How many of their jobs did not run since they last signed in.
    pub missed: u32,
    /// When the first of those was, in seconds since 1970.
    pub first_missed: i64,
}

/// What is kept of everyone's jobs, by their numbers.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Jobs(BTreeMap<u32, Kept>);

impl Jobs {
    /// What [`STATE`] holds: nothing when it is not there.
    pub fn read() -> Jobs {
        match read_text(STATE) {
            Ok(text) => Jobs::parse(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Jobs::default(),
            Err(err) => {
                eprintln!("authd: cannot read {STATE}: {}", util::strip_os_error(&err));
                Jobs::default()
            }
        }
    }

    /// The lines of `text`; one that is not right is left out.
    pub fn parse(text: &str) -> Jobs {
        let on = |word: &str| match word {
            "on" => Some(Some(true)),
            "off" => Some(Some(false)),
            "default" => Some(None),
            _ => None,
        };
        let line = |line: &str| -> Option<(u32, Kept)> {
            let [number, chosen, bar, missed, first] = line.split(' ').collect::<Vec<_>>()[..]
            else {
                return None;
            };
            let barred = match bar {
                "barred" => true,
                "allowed" => false,
                _ => return None,
            };
            let kept = Kept {
                chosen: on(chosen)?,
                barred,
                missed: missed.parse().ok()?,
                first_missed: first.parse().ok()?,
            };
            Some((number.parse().ok()?, kept))
        };
        Jobs(text.lines().filter_map(line).collect())
    }

    /// Its text, without the people of whom nothing is kept.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for (number, kept) in self.0.iter().filter(|(_, kept)| **kept != Kept::default()) {
            let chosen = match kept.chosen {
                Some(true) => "on",
                Some(false) => "off",
                None => "default",
            };
            let bar = if kept.barred { "barred" } else { "allowed" };
            text.push_str(&format!(
                "{number} {chosen} {bar} {} {}\n",
                kept.missed, kept.first_missed
            ));
        }
        text
    }

    /// Writes it to [`STATE`], which only root reads.
    pub fn write(&self) -> Result<(), String> {
        replace(STATE, &self.text(), 0o600)
    }

    /// What is kept of the person numbered `number`.
    pub fn of(&self, number: u32) -> Kept {
        self.0.get(&number).copied().unwrap_or_default()
    }

    /// Changes what is kept of `number` with `change`.
    pub fn change(&mut self, number: u32, change: impl FnOnce(&mut Kept)) {
        change(self.0.entry(number).or_default());
    }

    /// Without the person numbered `number`, whose account went.
    pub fn without(mut self, number: u32) -> Jobs {
        self.0.remove(&number);
        self
    }
}

impl Kept {
    /// Counts a job that did not run at `time`.
    pub fn count_missed(&mut self, time: i64) {
        if self.missed == 0 {
            self.first_missed = time;
        }
        self.missed = self.missed.saturating_add(1);
    }
}

/// The settings file's keys.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// Whether people's jobs run while they are signed out.
    keep_running: bool,
}

/// Whether `text`, [`SETTINGS`]' text, turns people's jobs on while they
/// are signed out, or why it says nothing that can be read.
fn default_in(text: &str) -> Result<bool, String> {
    toml::from_str::<Settings>(text).map(|s| s.keep_running).map_err(|err| err.message().into())
}

/// The system's default: off unless [`SETTINGS`] says otherwise.
pub(super) fn system_default() -> bool {
    match read_text(SETTINGS) {
        Ok(text) => default_in(&text).unwrap_or_else(|why| {
            eprintln!("authd: {SETTINGS}: {why}; jobs keep to the default, off");
            false
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => {
            eprintln!(
                "authd: cannot read {SETTINGS}: {}; jobs keep to the default, off",
                util::strip_os_error(&err)
            );
            false
        }
    }
}

/// The setting of a person, from what is kept of them and the system's
/// default.
fn setting(kept: Kept, default: bool) -> KeepRunning {
    KeepRunning { chosen: kept.chosen, default, barred: kept.barred }
}

/// What a sign-in tells a person whose jobs did not run while they were
/// signed out, as `kept` counts them, with `when` saying a time.
pub(super) fn missed_line(kept: Kept, when: impl Fn(i64) -> String) -> Option<String> {
    let first = when(kept.first_missed);
    let what = match kept.missed {
        0 => return None,
        1 => format!("A scheduled job of yours did not run while you were signed out, on {first}."),
        n => format!(
            "{n} scheduled jobs of yours did not run while you were signed out, the first on \
             {first}."
        ),
    };
    let why = if kept.barred {
        "An administrator barred your jobs from running then."
    } else {
        "crontab --keep-running on lets them run then."
    };
    Some(format!("{what} {why}"))
}

/// A job's environment: a session's, without a terminal, and `asked`
/// over it, each `NAME=value` in place of one of that name.
fn environment(user: &User, asked: &[String]) -> Vec<String> {
    let name = |setting: &str| setting.split_once('=').map_or("", |(name, _)| name).to_string();
    let mut env: Vec<String> =
        signin::environment(user, "").into_iter().filter(|v| !v.starts_with("TERM=")).collect();
    for setting in asked {
        let named = name(setting);
        env.retain(|v| name(v) != named);
        env.push(setting.clone());
    }
    env
}

/// What a `job` request asks.
pub(super) struct JobAsked<'a> {
    pub owner: &'a str,
    pub label: &'a str,
    pub folder: &'a str,
    pub command: &'a str,
    pub environment: &'a [String],
}

impl Session {
    /// Whether it is a person's own: a sign-in at a console or over SSH,
    /// and not a share's, an elevated command or a job.
    pub(super) fn is_signed_in(&self) -> bool {
        !self.elevated && self.job.is_none() && self.terminal != SHARE_TERMINAL
    }
}

impl Authd {
    /// Whether the person numbered `number` is signed in.
    pub(super) fn signed_in(&self, number: u32) -> bool {
        self.sessions.values().any(|s| s.number == number && s.is_signed_in())
    }

    /// Runs a job of a person's for timerd, client `i`, with the
    /// descriptor it sent for the job's output.
    pub(super) fn job(&mut self, i: usize, files: &Files, asked: &JobAsked, peer: Peer) -> Answer {
        let refused = |why: &str| Answer::Refused(why.into());
        let descriptors = std::mem::take(&mut self.clients[i].descriptors);
        if !peer.is_root() {
            return refused("only timerd, which runs as root, runs people's jobs");
        }
        let Ok([output]) = <[OwnedFd; 1]>::try_from(descriptors) else {
            return refused("a job comes with one descriptor, where its output goes");
        };
        let Some(names) = self.names else {
            return refused(
                "authd holds no names capability to run jobs with: its service file needs `names \
                 = true`",
            );
        };
        let running = self.sessions.values().filter(|s| s.job.is_some()).count();
        if running >= request::JOBS_MAX {
            return Answer::Refused(format!(
                "there are {} jobs running at once already",
                request::JOBS_MAX
            ));
        }
        let owner = asked.owner;
        let Some(user) = files.user(owner).filter(|u| u.uid == 0 || u.is_person()) else {
            return refused(request::JOB_NO_USER);
        };
        if files.disabled(owner) {
            return refused(request::JOB_DISABLED);
        }
        let signed_in = self.signed_in(user.uid);
        if user.uid != 0 && !signed_in {
            let mut jobs = Jobs::read();
            let kept = jobs.of(user.uid);
            if !setting(kept, system_default()).on() {
                let now = util::now_ns() as i64 / 1_000_000_000;
                jobs.change(user.uid, |kept| kept.count_missed(now));
                // timerd logs that it did not run, and why.
                if let Err(why) = jobs.write() {
                    eprintln!("authd: cannot count {}'s job: {why}", shown(owner));
                }
                return refused(if kept.barred {
                    request::JOB_BARRED
                } else {
                    request::JOB_SIGNED_OUT
                });
            }
        }
        let groups =
            accounts::other_groups(user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        let env = environment(user, asked.environment);
        let job = signin::Job { folder: asked.folder, command: asked.command, output: &output };
        let process = match signin::start_job(names, user, &groups, &job, env) {
            Ok(process) => process as i32,
            Err(why) => return Answer::Refused(why),
        };
        let when = match (user.uid, signed_in) {
            (0, _) => "as root",
            (_, true) => "while they are signed in",
            (_, false) => "while they are signed out",
        };
        audit!(
            "{owner}'s job {}, for process {}: process {process}, {when}",
            shown(asked.label),
            peer.process
        );
        let session = Session {
            name: user.name.clone(),
            terminal: JOB_TERMINAL.into(),
            from: String::new(),
            number: user.uid,
            group: user.gid,
            elevated: false,
            remote: false,
            recorded: false,
            job: Some(asked.label.to_string()),
        };
        self.sessions.insert(process, session);
        self.clients[i].session = Some(process);
        Answer::Done(process.to_string())
    }
}

/// The asker's setting, or with `chosen`, sets it first: their own,
/// for anyone with a person's account, or root's.
pub(super) fn keep_running(files: &Files, chosen: Option<Option<bool>>, peer: Peer) -> Answer {
    let Some(user) = files.users.entries().find(|u| u.uid == peer.user) else {
        return Answer::Refused("only a person's account has jobs".into());
    };
    if !(user.uid == 0 || user.is_person()) {
        return Answer::Refused("only a person's account has jobs".into());
    }
    let mut jobs = Jobs::read();
    if let Some(chosen) = chosen {
        jobs.change(user.uid, |kept| kept.chosen = chosen);
        if let Err(why) = jobs.write() {
            return Answer::Refused(why);
        }
        let word = match chosen {
            Some(true) => "on",
            Some(false) => "off",
            None => "the system's default",
        };
        eprintln!("authd: {} keeps their jobs running while signed out: {word}", user.name);
    }
    Answer::Done(setting(jobs.of(user.uid), system_default()).words())
}

/// The line a sign-in of `user` shows for their jobs that did not run,
/// and what is kept once it has shown, if anything did not.
pub(super) fn missed_at_sign_in(user: &User) -> Option<(String, Jobs)> {
    let jobs = Jobs::read();
    let kept = jobs.of(user.uid);
    let line = missed_line(kept, |time| util::local_time(time, WHEN))?;
    let mut jobs = jobs;
    jobs.change(user.uid, |kept| {
        kept.missed = 0;
        kept.first_missed = 0;
    });
    Some((line, jobs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_kept_reads_back_and_leaves_out_what_is_not() {
        let mut jobs = Jobs::default();
        jobs.change(1000, |kept| kept.chosen = Some(true));
        jobs.change(1001, |kept| {
            kept.barred = true;
            kept.count_missed(50);
            kept.count_missed(70);
        });
        // Nothing kept of 1002: no line.
        jobs.change(1002, |kept| kept.chosen = None);
        let text = jobs.text();
        assert_eq!(text, "1000 on allowed 0 0\n1001 default barred 2 50\n");
        assert_eq!(Jobs::parse(&text).text(), text);
        assert_eq!(Jobs::parse(&text).of(1001).first_missed, 50);
        assert_eq!(Jobs::parse(&text).of(1002), Kept::default());
        assert_eq!(Jobs::parse(&text).without(1000).text(), "1001 default barred 2 50\n");
        // Lines that are not right are left out.
        let parsed = Jobs::parse(
            "x on allowed 0 0\n1 maybe allowed 0 0\n2 on no 0 0\n3 on\n4 off allowed 1 9\n",
        );
        assert_eq!(parsed.text(), "4 off allowed 1 9\n");
    }

    #[test]
    fn the_system_default_is_off_unless_the_settings_say_so() {
        assert_eq!(default_in("keep_running = true\n"), Ok(true));
        assert_eq!(default_in("# the server preset\nkeep_running = false\n"), Ok(false));
        assert!(default_in("keep_running = \"yes\"\n").is_err());
        assert!(default_in("keep_running = true\nother = 1\n").is_err());
        assert!(default_in("").is_err());
    }

    #[test]
    fn a_job_keeps_a_session_s_environment_with_its_own_over_it() {
        let user = User {
            name: "alice".into(),
            uid: 1000,
            gid: 1000,
            full_name: String::new(),
            home: "/users/alice".into(),
            shell: "/system/commands/nukesh".into(),
        };
        let env = environment(&user, &["SHELL=/bin/sh".into(), "MAILTO=".into(), "A=b=c".into()]);
        assert!(env.contains(&"SHELL=/bin/sh".to_string()));
        assert!(!env.iter().any(|v| v == "SHELL=/system/commands/nukesh"));
        assert!(env.contains(&"HOME=/users/alice".to_string()));
        assert!(env.contains(&"MAILTO=".to_string()));
        assert!(env.contains(&"A=b=c".to_string()));
        assert!(!env.iter().any(|v| v.starts_with("TERM=")));
        assert_eq!(env.iter().filter(|v| v.starts_with("SHELL=")).count(), 1);
    }

    #[test]
    fn a_sign_in_says_how_many_jobs_did_not_run() {
        let when = |time: i64| format!("t{time}");
        assert_eq!(missed_line(Kept::default(), when), None);
        let mut kept = Kept::default();
        kept.count_missed(5);
        assert_eq!(
            missed_line(kept, when).unwrap(),
            "A scheduled job of yours did not run while you were signed out, on t5. crontab \
             --keep-running on lets them run then."
        );
        kept.count_missed(9);
        kept.barred = true;
        assert_eq!(
            missed_line(kept, when).unwrap(),
            "2 scheduled jobs of yours did not run while you were signed out, the first on t5. An \
             administrator barred your jobs from running then."
        );
    }
}
