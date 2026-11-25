// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The shells of the web console's terminals (ADR 0517). admind opens
//! each pane's terminal and keeps its master side; authd starts the shell
//! on the other side, so that the sign-in rules, the records, the audit
//! and the limits are the ones an SSH session on a terminal has.
//!
//! - **A person's own shell** is their login shell, started as an SSH
//!   session's is (`login --session` on the terminal), with a sign-in
//!   record of the terminal and the browser's address, so who(1) and
//!   last(1) show it. It outlives the console session that asked for it,
//!   as an SSH shell outlives nothing but its terminal: it ends when
//!   admind closes the master, or when it exits.
//! - **An administrator's shell** is root's login shell, started as
//!   elevate's `-i` is, for an administrator whose console session is
//!   unlocked now. authd ends it, on its own clock, when the session
//!   locks, its unlock's time runs out, or it ends: SIGHUP to its process
//!   group, then SIGKILL 5 seconds later to what is left of the group. A
//!   program moved out of the group (setsid) goes on, as one started with
//!   elevate does; the audit has its start.
//!
//! authd answers with the shell's first process and does not hold the
//! request: admind holds the terminal. A refusal that the console shows
//! with an error ID of its own starts with the ID (`ADM-130: ...`); the
//! rest are authd's words, which admind reads as it reads any refusal.

use std::io::IsTerminal;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::{Duration, Instant};

use accounts::request::{self, Answer, Mode};
use accounts::signins::{self, Event, Seat};

use super::{Authd, Files, NO_SESSION, Session, may_sign_in, no, record, short, shown};
use super::{signal_command, signin};
use crate::admin_settings::{Allowed, Settings};
use crate::authd::{Notices, greet};
use crate::elevate;

/// How often authd looks at the administrators' shells while one runs.
pub(super) const CHECK_EVERY: Duration = Duration::from_secs(1);

/// How long an administrator's shell, hung up, has to end before what is
/// left of its process group is killed.
const KILL_AFTER: Duration = Duration::from_secs(5);

/// How many shells an account may have on the console's terminals: as
/// many panes as its terminals hold, four each. admind counts terminals
/// and panes itself; this is authd's own bound.
const PANES_PER_TERMINAL: u32 = 4;

/// Where the machine's terminal descriptions are (ncurses, ADR 0086).
const TERMINFO: &str = "/system/resources/terminfo";

/// The most language settings a request may bring.
const LANGUAGE_MAX: usize = 16;

/// A shell authd started on a console's terminal.
pub(in crate::authd) struct Shell {
    /// Whose.
    pub(in crate::authd) name: String,
    /// The console session that asked for it.
    session: String,
    /// Whether it is root's.
    pub(in crate::authd) admin: bool,
    /// When it was hung up, for an administrator's shell whose session
    /// locked.
    pub(in crate::authd) hung_up: Option<Instant>,
    /// Whether what was left of its process group was killed.
    pub(in crate::authd) killed: bool,
    /// Whether its first process ended, while the rest of its group may
    /// not have.
    pub(in crate::authd) reaped: bool,
}

/// What a request for a shell asks, and what authd knows of the asker,
/// for [`refusal`].
#[derive(Clone, Debug)]
pub(super) struct Asked<'a> {
    /// The account.
    pub name: &'a str,
    /// Its shell, from the accounts' file.
    pub shell: &'a str,
    /// Whether the account is an administrator.
    pub is_admin: bool,
    /// Whether root's shell is asked for.
    pub admin: bool,
    /// Whether the console session is unlocked now.
    pub unlocked: bool,
    /// Who admin.toml lets open a terminal.
    pub allowed: Allowed,
    /// admin.toml's `sessions-per-account`.
    pub sessions_per_account: u32,
    /// How many shells the account has on the console's terminals.
    pub running: usize,
}

/// Why `asked` is refused, if it is: the account's right to a terminal
/// first, then the administrator's shell's rules.
pub(super) fn refusal(asked: &Asked) -> Option<String> {
    let name = shown(asked.name);
    if asked.shell.is_empty() || asked.shell == accounts::NO_SHELL {
        return Some(format!("ADM-132: {name} has no shell to start"));
    }
    if !asked.allowed.lets(asked.is_admin) {
        let whom = match asked.allowed {
            Allowed::Administrators => "administrators alone",
            Allowed::Everyone | Allowed::Nobody => "no one",
        };
        return Some(format!(
            "ADM-132: the console's settings let {whom} open a terminal (allowed = \"{}\" in \
             admin.toml)",
            asked.allowed.word()
        ));
    }
    let most = asked.sessions_per_account.saturating_mul(PANES_PER_TERMINAL) as usize;
    if asked.running >= most {
        return Some(format!(
            "ADM-130: {name} has {} shells in the console's terminals, as many as {} terminals \
             hold",
            asked.running, asked.sessions_per_account
        ));
    }
    if asked.admin && !asked.is_admin {
        return Some(format!("{name} is not an administrator, and makes no changes"));
    }
    if asked.admin && !asked.unlocked {
        return Some("the session is locked: unlock it for an administrator's terminal".into());
    }
    None
}

/// Whether `term` names one of the machine's terminal descriptions, in
/// the folder `terminfo`: by its first letter, or that letter's number in
/// hexadecimal, as ncurses keeps them.
pub(super) fn known_terminal(terminfo: &Path, term: &str) -> bool {
    let Some(first) = term.chars().next() else { return false };
    if signin::terminal_type(term) != term {
        return false;
    }
    [first.to_string(), format!("{:02x}", u32::from(first))]
        .iter()
        .any(|folder| terminfo.join(folder).join(term).is_file())
}

/// The language settings of `language` that a session takes, as an SSH
/// client's are: `LANG`, `LANGUAGE` and `LC_*`, at most
/// [`LANGUAGE_MAX`].
pub(super) fn languages(language: &[String]) -> Vec<String> {
    language
        .iter()
        .filter(|setting| request::ssh_language(setting))
        .take(LANGUAGE_MAX)
        .cloned()
        .collect()
}

/// Starts root's login shell on `tty` for the administrator `user`, as
/// `elevate -i` does, with root's account from `files`.
fn start_admin_shell(
    user: &accounts::User,
    files: &Files,
    tty: &OwnedFd,
    term: &str,
    language: &[String],
) -> Result<u64, String> {
    let root = elevate::root_account(files.users.entries().cloned());
    let plan = elevate::plan(Mode::Login, &[], b"/", &root)?;
    let mut env = elevate::environment(&root, user, term, language, &plan.shown);
    env.extend(signin::WEB_TERMINAL_SETTINGS.map(String::from));
    let copies = [tty.try_clone(), tty.try_clone(), tty.try_clone()];
    let fds: Vec<OwnedFd> = copies
        .into_iter()
        .collect::<std::io::Result<_>>()
        .map_err(|err| format!("cannot give the shell its terminal: {err}"))?;
    elevate::start(&fds, &plan, &env, None)
}

impl Authd {
    /// `console-terminal`: starts the shell of a pane of the console's
    /// terminal on the terminal client `i` sent, for console session
    /// `session`: the person's own, or root's with `admin`. See the
    /// module's comment.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn console_terminal(
        &mut self,
        i: usize,
        files: &Files,
        session: &str,
        admin: bool,
        term: &str,
        language: &[String],
        now: Instant,
    ) -> (Answer, Instant) {
        let mut descriptors = std::mem::take(&mut self.clients[i].descriptors);
        let tty = match descriptors.pop() {
            Some(tty) if descriptors.is_empty() && tty.is_terminal() => tty,
            _ => return no("a console terminal comes with one descriptor, its terminal", now),
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
        let settings = match Settings::read() {
            Ok(settings) => settings,
            Err(why) => return no(why, now),
        };
        let is_admin = accounts::is_admin(&user, &files.groups);
        let running = self.console.shells.values().filter(|s| s.name == name && !s.reaped).count();
        let asked = Asked {
            name: &name,
            shell: &user.shell,
            is_admin,
            admin,
            unlocked,
            allowed: settings.terminal.allowed,
            sessions_per_account: settings.terminal.sessions_per_account,
            running,
        };
        if let Some(why) = refusal(&asked) {
            audit!(
                "refused {name} a console terminal from {from}{}: {why}",
                if admin { " as an administrator" } else { "" }
            );
            return no(why, now);
        }
        if !known_terminal(Path::new(TERMINFO), term) {
            return no(format!("{} is not a terminal type this machine knows", shown(term)), now);
        }
        let language = languages(language);
        let tty_name = signin::terminal_name(&tty);
        let started = if admin {
            start_admin_shell(&user, files, &tty, term, &language)
        } else {
            self.start_own_shell(&user, files, &tty, term, &language)
        };
        let process = match started {
            Ok(process) => process as i32,
            Err(why) => return no(why, now),
        };
        let shown_from = shown(&from);
        if admin {
            audit!(
                "console administrator shell started for {name} from {shown_from} on {tty_name}, \
                 in the web console session {}: process {process}",
                short(session)
            );
        } else {
            audit!(
                "console terminal started for {name} from {shown_from} on {tty_name}: session \
                 process {process}"
            );
            let seat = Seat {
                name: name.clone(),
                terminal: tty_name.clone(),
                from: shown_from.clone(),
                session: process as u32,
                via: "console".into(),
            };
            record(&signins::of(user.uid), user.gid, 0o640, Event::In(seat));
        }
        let recorded = Session {
            name: name.clone(),
            terminal: tty_name,
            from: shown_from,
            number: user.uid,
            group: user.gid,
            elevated: admin,
            remote: false,
            recorded: !admin,
            job: None,
        };
        self.sessions.insert(process, recorded);
        let shell = Shell {
            name,
            session: session.to_string(),
            admin,
            hung_up: None,
            killed: false,
            reaped: false,
        };
        self.console.shells.insert(process, shell);
        (Answer::Done(process.to_string()), now)
    }

    /// Starts `user`'s own login shell on `tty`, as an SSH session's on a
    /// terminal, after showing them their last sign-in and their notices.
    fn start_own_shell(
        &self,
        user: &accounts::User,
        files: &Files,
        tty: &OwnedFd,
        term: &str,
        language: &[String],
    ) -> Result<u64, String> {
        let names = self.names.ok_or(
            "authd holds no names capability to start shells with: its service file needs \
             `names = true`",
        )?;
        let groups =
            accounts::other_groups(user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        let notices = Notices::read();
        let theirs = notices.of(&user.name);
        greet(tty, user, &theirs);
        let env = signin::web_terminal_environment(user, term, language);
        let process = signin::start_web_terminal(names, user, &groups, tty, env)?;
        if !theirs.is_empty()
            && let Err(why) = notices.without(&user.name).write()
        {
            eprintln!("authd: {why}");
        }
        Ok(process)
    }

    /// Ends the administrators' shells whose console session is no longer
    /// unlocked: SIGHUP to each one's process group at once, and SIGKILL
    /// [`KILL_AFTER`] later to what is left of it.
    pub(in crate::authd) fn end_admin_shells(&mut self, now: Instant) {
        let state = &mut self.console;
        let live = &state.live;
        let mut gone = Vec::new();
        for (&process, shell) in state.shells.iter_mut().filter(|(_, shell)| shell.admin) {
            match shell.hung_up {
                None => {
                    let why = match live.get(&shell.session) {
                        None => "the console session ended",
                        Some(session) => match session.unlocked {
                            Some(until) if until > now => continue,
                            Some(_) => "the console's unlock ran out",
                            None => "the console locked",
                        },
                    };
                    signal_command(process, libc::SIGHUP);
                    audit!(
                        "{}'s console administrator shell, process {process}, ended: {why}",
                        shell.name
                    );
                    shell.hung_up = Some(now);
                }
                Some(at) if now.saturating_duration_since(at) >= KILL_AFTER => {
                    if !shell.killed {
                        signal_command(process, libc::SIGKILL);
                        shell.killed = true;
                    }
                    if shell.reaped {
                        gone.push(process);
                    }
                }
                Some(_) => {}
            }
        }
        for process in gone {
            state.shells.remove(&process);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked() -> Asked<'static> {
        Asked {
            name: "alice",
            shell: "/system/commands/nukesh",
            is_admin: true,
            admin: false,
            unlocked: false,
            allowed: Allowed::Everyone,
            sessions_per_account: 32,
            running: 0,
        }
    }

    #[test]
    fn a_person_with_a_shell_gets_one_and_an_administrator_root_s_once_unlocked() {
        assert_eq!(refusal(&asked()), None);
        assert_eq!(refusal(&Asked { is_admin: false, ..asked() }), None);
        let admin = Asked { admin: true, ..asked() };
        let locked = refusal(&admin).unwrap();
        assert!(locked.contains("is locked"), "{locked}");
        assert_eq!(refusal(&Asked { unlocked: true, ..admin.clone() }), None);
        let not_admin = refusal(&Asked { is_admin: false, unlocked: true, ..admin }).unwrap();
        assert!(
            not_admin.ends_with("is not an administrator, and makes no changes"),
            "{not_admin}"
        );
    }

    #[test]
    fn the_settings_and_the_accounts_shell_say_who_may_open_one() {
        let no_shell = refusal(&Asked { shell: accounts::NO_SHELL, ..asked() }).unwrap();
        assert!(no_shell.starts_with("ADM-132: "), "{no_shell}");
        assert!(refusal(&Asked { shell: "", ..asked() }).unwrap().starts_with("ADM-132: "));
        let admins = Asked { allowed: Allowed::Administrators, ..asked() };
        assert_eq!(refusal(&admins), None);
        let refused = refusal(&Asked { is_admin: false, ..admins }).unwrap();
        assert!(refused.starts_with("ADM-132: ") && refused.contains("administrators alone"));
        let nobody = refusal(&Asked { allowed: Allowed::Nobody, ..asked() }).unwrap();
        assert!(nobody.contains("let no one") && nobody.contains("\"nobody\""), "{nobody}");
    }

    #[test]
    fn an_account_has_as_many_shells_as_its_terminals_hold() {
        let full = Asked { sessions_per_account: 2, running: 7, ..asked() };
        assert_eq!(refusal(&full), None);
        let over = refusal(&Asked { running: 8, ..full }).unwrap();
        assert!(over.starts_with("ADM-130: alice has 8 shells"), "{over}");
        // Who may comes before how many.
        let both = Asked { allowed: Allowed::Nobody, running: 500, ..asked() };
        assert!(refusal(&both).unwrap().starts_with("ADM-132: "));
    }

    #[test]
    fn a_terminal_type_is_one_the_machine_describes() {
        let dir = std::env::temp_dir().join(format!("authd-terminfo-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("x")).unwrap();
        std::fs::create_dir_all(dir.join("6c")).unwrap();
        std::fs::write(dir.join("x/xterm-256color"), b"").unwrap();
        std::fs::write(dir.join("6c/linux"), b"").unwrap();
        assert!(known_terminal(&dir, "xterm-256color"));
        assert!(known_terminal(&dir, "linux"));
        for term in ["", "xterm", "../x/xterm-256color", "x/xterm-256color", "xterm-256color\n"] {
            assert!(!known_terminal(&dir, term), "{term:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shell_takes_the_language_settings_an_ssh_client_may_give() {
        let given: Vec<String> =
            ["LANG=de_DE.UTF-8", "PATH=/tmp", "LC_TIME=en_GB.UTF-8", "LD_PRELOAD=x.so", "LANG"]
                .map(String::from)
                .to_vec();
        assert_eq!(languages(&given), ["LANG=de_DE.UTF-8", "LC_TIME=en_GB.UTF-8"]);
        let many: Vec<String> = (0..40).map(|_| "LC_ALL=C.UTF-8".to_string()).collect();
        assert_eq!(languages(&many).len(), LANGUAGE_MAX);
    }

    #[test]
    fn a_shell_that_ended_is_forgotten_and_a_hung_up_one_kept_until_its_group_is_killed() {
        let mut state = super::super::State::default();
        let shell = |admin, hung_up: Option<Instant>| Shell {
            name: "alice".into(),
            session: "s".into(),
            admin,
            hung_up,
            killed: false,
            reaped: false,
        };
        state.shells.insert(10, shell(false, None));
        state.shells.insert(11, shell(true, Some(Instant::now())));
        assert!(state.next_check(Instant::now()).is_some());
        state.shell_ended(10);
        state.shell_ended(11);
        assert!(!state.shells.contains_key(&10));
        assert!(state.shells[&11].reaped);
        state.shells.get_mut(&11).unwrap().killed = true;
        assert_eq!(state.next_check(Instant::now()), None);
        state.shell_ended(12);
    }
}
