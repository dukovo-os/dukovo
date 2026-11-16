// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The sessions an account has at the machine: what `user show` lists and
//! `user end NAME SESSION` ends. A session is what who(1) finds
//! (`sessions.rs`): a process that leads its session and has a terminal.
//!
//! A session's ID is its leader's number and the second it began, as
//! `4123-1763110000`. The number alone would name another process once the
//! leader had ended and the number been given again, so `user end` ends
//! what the ID names only if the process with that number began then, and
//! acts for the account.
//!
//! authd sends the signals (ADR 0562). It started the session, so its
//! processes are below it, and the kernel lets a process signal only its
//! own session's and those below it: root's `user end` could not.

use std::thread;
use std::time::{Duration, Instant};

use accounts::request::{Answer, Request};
use accounts::signins::{Event, Record};

use super::{Accounts, ask, operands};
use crate::account_errors;
use crate::args::Args;
use crate::proc::{self, Process};
use crate::sessions;
use crate::util::{self, Fail, fail, usage};

/// How long what was hung up has to end before it is killed.
const KILL_AFTER: Duration = Duration::from_secs(5);

/// How often `user end` looks whether what it hung up has ended.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// How far a session's start may differ from the one in its ID, in
/// seconds. The machine's start, which every start is counted from, is
/// read from the kernel or worked out from the clock, and the second it
/// falls in can change between two reads.
const START_SKEW: i64 = 2;

/// A sign-in is recorded after its session began, never before, but by
/// the same clock's reading of the second.
const RECORD_SKEW: i64 = 2;

/// A session at the machine.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Found {
    /// Its leader's number.
    pub leader: u64,
    /// Its terminal, as `pts/3`.
    pub terminal: String,
    /// When it began, in seconds since the epoch.
    pub start: i64,
    /// The address it came from, if the sign-in record names one.
    pub from: Option<String>,
    /// The processes it holds that have not ended, each with when it was
    /// made, so that a number given again is known for another process.
    pub members: Vec<(u64, u64)>,
}

impl Found {
    /// Its ID.
    pub fn id(&self) -> String {
        format!("{}-{}", self.leader, self.start)
    }

    /// What it is at: `ssh` for a terminal that someone came to from an
    /// address, `serial` for a serial line, `screen` for the machine's own
    /// screen, `other` where the terminal says no more. The console's
    /// terminals are `ssh` too, as /processes does not tell them apart.
    pub fn kind(&self) -> &'static str {
        let terminal = self.terminal.as_str();
        let numbered = |prefix: &str| {
            terminal
                .strip_prefix(prefix)
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty())
        };
        if ["ttyS", "ttyUSB", "ttyACM", "hvc"].iter().any(|prefix| numbered(prefix)) {
            "serial"
        } else if numbered("tty") {
            "screen"
        } else if terminal.starts_with("pts/") && self.from.is_some() {
            "ssh"
        } else {
            "other"
        }
    }
}

/// The sessions of user `uid` among `processes`, the oldest first. `boot`
/// is when the machine started, in seconds since the epoch, and `records`
/// the account's sign-in records, which say where each came from.
pub(super) fn of(processes: &[Process], uid: u32, boot: i64, records: &[Record]) -> Vec<Found> {
    let mut found: Vec<Found> = sessions::sessions(processes)
        .into_iter()
        .filter(|session| session.leader.user == uid)
        .map(|session| {
            let leader = session.leader.number;
            let start = sessions::login_seconds(&session, boot);
            let members = processes
                .iter()
                .filter(|p| p.session == leader && !p.zombie())
                .map(|p| (p.number, p.started_ns))
                .collect();
            Found {
                leader,
                terminal: session.terminal,
                start,
                from: came_from(records, leader, start),
                members,
            }
        })
        .collect();
    found.sort_by_key(|f| (f.start, f.leader));
    found
}

/// Where the session led by `leader`, begun at `start`, came from: the
/// newest sign-in record for that number from then on. An older record
/// with the number is an earlier session's. None if there is no record, or
/// the sign-in was at the machine itself.
fn came_from(records: &[Record], leader: u64, start: i64) -> Option<String> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            Event::In(seat)
                if u64::from(seat.session) == leader && record.time >= start - RECORD_SKEW =>
            {
                Some((record.time, &seat.from))
            }
            _ => None,
        })
        .max_by_key(|(time, _)| *time)
        .map(|(_, from)| from.clone())
        .filter(|from| !from.is_empty())
}

/// The session in `found` that the ID `leader` and `start` names: its
/// leader's number, and a start within [`START_SKEW`] of it.
pub(super) fn find(found: Vec<Found>, leader: u64, start: i64) -> Option<Found> {
    found.into_iter().find(|f| f.leader == leader && (f.start - start).abs() <= START_SKEW)
}

/// A session's leader and start from its ID, `NUMBER-SECONDS`: two
/// numbers of digits, with a dash between.
pub(crate) fn parse_id(id: &str) -> Option<(u64, i64)> {
    let (leader, start) = id.split_once('-')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(leader) || !digits(start) {
        return None;
    }
    Some((leader.parse().ok()?, start.parse().ok()?))
}

/// Whether the process `member` is, by number and when it was made, is
/// there still, in the session led by `leader`.
fn there(leader: u64, (number, started_ns): (u64, u64)) -> bool {
    proc::process(number)
        .is_ok_and(|p| p.started_ns == started_ns && p.session == leader && !p.zombie())
}

/// Asks authd to hang up, or with `kill` to kill, the processes of
/// `name`'s session that `leader` leads. How many it sent the signal to.
fn signal(name: &str, leader: u64, kill: bool) -> Result<usize, Fail> {
    match ask(&Request::EndSession { name: name.to_string(), session: leader, kill })? {
        Answer::Done(sent) => Ok(sent.trim().parse().unwrap_or(0)),
        Answer::Refused(why) => Err(fail(why)),
    }
}

/// What `user end` says of how the session ended.
fn ended(name: &str, id: &str, hung_up: usize, killed: usize) -> String {
    let processes =
        |n: usize| if n == 1 { "1 process".to_string() } else { format!("{n} processes") };
    if killed == 0 {
        format!("Ended the session {id} of {name}: {} hung up.", processes(hung_up))
    } else {
        format!(
            "Ended the session {id} of {name}: {} hung up, and {} did not end, so it was killed.",
            processes(hung_up),
            processes(killed)
        )
    }
}

/// `user end NAME SESSION`: has authd hang up the processes of NAME's
/// session at the machine, and kill those that are there five seconds
/// later.
pub(super) fn end(args: &mut Args) -> Result<i32, Fail> {
    let (names, _) =
        operands(args, "user end needs an account and a session", 2, |_, _| Ok(false))?;
    let (name, id) = (&names[0], &names[1]);
    let Some((leader, start)) = parse_id(id) else {
        return Err(usage(format!(
            "`{id}` is not a session: it is two numbers with a dash between, as `user show {name}` \
             lists them"
        )));
    };
    if util::own_user() != 0 {
        return Err(fail("only an administrator, inside elevate, ends a session".into()));
    }
    let user = Accounts::read()?.user(name)?.clone();
    let processes = proc::processes().map_err(|e| util::failed(&proc::PROC, e))?;
    let found = find(of(&processes, user.uid, sessions::boot_seconds(), &[]), leader, start)
        .ok_or_else(|| account_errors::no_such_session(name, id))?;
    // Ended under the command that ends it, it would not get to kill what
    // is left.
    if found.members.iter().any(|&(number, _)| number == proc::own_number()) {
        return Err(fail(format!("{id} is the session this runs in: end it from another session")));
    }
    let hung_up = signal(name, found.leader, false)?;
    let until = Instant::now() + KILL_AFTER;
    let left = loop {
        let left: Vec<(u64, u64)> =
            found.members.iter().copied().filter(|&m| there(found.leader, m)).collect();
        if left.is_empty() || Instant::now() >= until {
            break left;
        }
        thread::sleep(LOOK_EVERY);
    };
    let killed = if left.is_empty() { 0 } else { signal(name, found.leader, true)? };
    println!("{}", ended(name, &found.id(), hung_up, killed));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use accounts::signins::Seat;

    /// A process of user 1000 in session 41.
    fn process(number: u64, session: u64, terminal: Option<&str>, started_s: u64) -> Process {
        Process {
            number,
            session,
            user: 1000,
            terminal: terminal.map(Into::into),
            started_ns: started_s * 1_000_000_000,
            run: "sleeping".into(),
            ..Process::default()
        }
    }

    fn signed_in(time: i64, terminal: &str, from: &str, session: u32) -> Record {
        let seat = Seat {
            name: "ann".into(),
            terminal: terminal.into(),
            from: from.into(),
            session,
            via: String::new(),
        };
        Record { time, event: Event::In(seat) }
    }

    /// The machine began at 1000; the leader 41 at 1100 (the ID `41-2100`)
    /// with a shell and a job, and another account's session is there.
    fn machine() -> Vec<Process> {
        let mut zombie = process(44, 41, Some("pts/3"), 101);
        zombie.run = "zombie".into();
        let mut theirs = process(60, 60, Some("pts/4"), 50);
        theirs.user = 1001;
        vec![
            process(41, 41, Some("pts/3"), 100),
            process(42, 41, Some("pts/3"), 101),
            process(43, 43, Some("pts/3"), 102),
            zombie,
            process(50, 50, None, 10),
            theirs,
        ]
    }

    #[test]
    fn a_session_is_a_leader_with_a_terminal_and_its_processes() {
        let records = [signed_in(2100, "pts/3", "10.8.0.2", 41)];
        let found = of(&machine(), 1000, 2000, &records);
        // The process that leads its own session is one too, as who(1)
        // has it; the leaderless 50 and the other account's 60 are not.
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id(), "41-2100");
        assert_eq!(found[0].terminal, "pts/3");
        assert_eq!(found[0].from.as_deref(), Some("10.8.0.2"));
        assert_eq!(found[0].kind(), "ssh");
        // Its processes that have not ended, the zombie not among them.
        assert_eq!(found[0].members, [(41, 100_000_000_000), (42, 101_000_000_000)]);
        assert_eq!(found[1].id(), "43-2102");
        assert_eq!(found[1].from, None);
        assert_eq!(found[1].kind(), "other");
    }

    #[test]
    fn where_a_session_came_from_is_its_own_newest_record() {
        let records = [
            signed_in(1500, "pts/3", "10.0.0.9", 41),
            signed_in(2100, "pts/3", "10.8.0.2", 41),
            signed_in(2100, "pts/5", "10.8.0.3", 42),
        ];
        assert_eq!(came_from(&records, 41, 2100).as_deref(), Some("10.8.0.2"));
        // The record is a second behind the start, or ahead of it.
        assert_eq!(came_from(&records, 41, 2101).as_deref(), Some("10.8.0.2"));
        // Only an earlier session of the number was recorded.
        assert_eq!(came_from(&records, 41, 3000), None);
        // At the machine itself, which a record says with no address.
        assert_eq!(came_from(&[signed_in(10, "tty1", "", 7)], 7, 10), None);
    }

    #[test]
    fn the_kind_is_told_by_the_terminal_and_where_it_came_from() {
        let session = |terminal: &str, from: Option<&str>| Found {
            leader: 1,
            terminal: terminal.into(),
            start: 0,
            from: from.map(Into::into),
            members: Vec::new(),
        };
        assert_eq!(session("pts/0", Some("192.0.2.4")).kind(), "ssh");
        assert_eq!(session("pts/0", None).kind(), "other");
        assert_eq!(session("tty1", None).kind(), "screen");
        for serial in ["ttyS0", "ttyUSB1", "ttyACM0", "hvc0"] {
            assert_eq!(session(serial, None).kind(), "serial", "{serial}");
        }
        assert_eq!(session("tty", None).kind(), "other");
        assert_eq!(session("ttyS", None).kind(), "other");
        assert_eq!(session("console", None).kind(), "other");
    }

    #[test]
    fn an_id_names_the_leader_and_the_second_it_began() {
        let found = || of(&machine(), 1000, 2000, &[]);
        assert_eq!(find(found(), 41, 2100).map(|f| f.leader), Some(41));
        // The machine's start, read again, differs by a second.
        assert_eq!(find(found(), 41, 2102).map(|f| f.leader), Some(41));
        assert_eq!(find(found(), 41, 2098).map(|f| f.leader), Some(41));
        // The number given again to a process that began later is not it.
        assert_eq!(find(found(), 41, 2200), None);
        assert_eq!(find(found(), 41, 100), None);
        // A number that leads no session, or another account's.
        assert_eq!(find(found(), 42, 2101), None);
        assert_eq!(find(found(), 60, 2050), None);
        assert_eq!(parse_id("4123-1763110000"), Some((4123, 1_763_110_000)));
        for bad in
            ["", "x", "41", "41-", "-41", "41-x", "x-41", "41-2100-1", "41 2100", "41--5", "+41-5"]
        {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_words_say_what_ended_and_what_was_killed() {
        assert_eq!(
            ended("ann", "41-2100", 3, 0),
            "Ended the session 41-2100 of ann: 3 processes hung up."
        );
        assert_eq!(
            ended("ann", "41-2100", 1, 1),
            "Ended the session 41-2100 of ann: 1 process hung up, and 1 process did not end, so \
             it was killed."
        );
    }
}
