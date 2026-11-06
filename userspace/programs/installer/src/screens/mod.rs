// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The installer's screens (ADR 0310), as the approved design draws them:
//! Welcome, Network, Disk, You and Review, then the install's progress
//! and its end. `dukovo-install` with no arguments shows them on its
//! terminal; on the install stick, consoled starts it on tty1 when the
//! menu's *Install Dukovo* starts the system.
//!
//! The screens only ask: they build the same answers an answers file
//! holds and hand them to the same engine ([`crate::install`]), which
//! runs in a thread of its own while the screens draw its progress from
//! the log's events. Nothing is written to a disk before the review's
//! Install.
//!
//! - [`app`] keeps the answers and acts on each key.
//! - [`draw`] draws them.
//! - [`rdisk`] is the manual layout's screen, where each partition is
//!   chosen (ADR 0311).
//! - [`stick`] reads and saves setup files on the stick.

pub mod app;
pub mod draw;
pub mod rdisk;
pub mod stick;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as Input, KeyEventKind};

use self::app::{App, Outcome};
use crate::answers::{self, Answers};
use crate::install::{self, Source};
use crate::log::{self, Event, Log, Step};
use crate::machine::Machine;
use crate::messages::{self, Arg};

/// How long the screens wait for a key before they look at the install's
/// progress again.
const TICK: Duration = Duration::from_millis(200);

/// Shows the screens on the terminal and installs as they are answered,
/// from the release in `release` or on the stick. Gives the program's
/// exit status: 0 when the screens were left, as for the live console,
/// or the install finished.
#[must_use]
pub fn run(release: Option<&Path>) -> u8 {
    println!("{}", messages::text("starting"));
    let mut log = match Log::create(Path::new(log::LIVE_PATH), false) {
        Ok(log) => log,
        Err(why) => {
            eprintln!("dukovo-install: {why}");
            return 1;
        }
    };
    let source = match install::find_release(release, &mut log) {
        Ok(source) => source,
        Err(why) => {
            log.problem(&why);
            eprintln!("dukovo-install: {why}");
            return 1;
        }
    };
    let mut app = App::new(Machine::probe(), &source.release.version, source.release.bytes());
    app.setup_files = stick::setup_files(source.stick.as_deref());
    // The release's signatures were checked before the first page.
    app.told(Event::Step(Step::Release));
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(err) => {
            eprintln!("dukovo-install: the screens need a terminal: {err}");
            return 1;
        }
    };
    // The consoles keep no second screen for full-screen programs, so
    // the screens clear what was there before them, and what they drew
    // once they end, before the shell's prompt comes.
    let ended = terminal.clear().and_then(|()| show(&mut terminal, &mut app, log, source));
    let _ = terminal.clear();
    ratatui::restore();
    match ended {
        Ok(status) => status,
        Err(err) => {
            eprintln!("dukovo-install: the terminal: {err}");
            1
        }
    }
}

/// The install running in its thread: the log's events, and its end.
struct Running {
    events: Receiver<Event>,
    finished: Receiver<Result<String, String>>,
}

/// Draws `app` and acts on each key until the screens are left.
fn show(terminal: &mut DefaultTerminal, app: &mut App, log: Log, source: Source) -> io::Result<u8> {
    let stick = source.stick.clone();
    let mut ready = Some((log, source));
    let mut running: Option<Running> = None;
    loop {
        terminal.draw(|frame| draw::draw(frame, app))?;
        if event::poll(TICK)?
            && let Input::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match app.key(key) {
                Outcome::Stay => {}
                Outcome::Quit => return Ok(0),
                Outcome::Restart => {
                    let _ = Command::new("/system/commands/reboot").status();
                    return Ok(0);
                }
                Outcome::SaveSetup => {
                    app.said = Some(match app.answers() {
                        Ok(answers) => stick::save(stick.as_deref(), &answers),
                        Err(why) => why,
                    });
                }
                Outcome::Install => {
                    if let Some((log, source)) = ready.take() {
                        running = Some(start(app, log, source));
                    }
                }
            }
        }
        if let Some(run) = &running {
            while let Ok(event) = run.events.try_recv() {
                app.told(event);
            }
            if let Ok(result) = run.finished.try_recv() {
                while let Ok(event) = run.events.try_recv() {
                    app.told(event);
                }
                app.finished(result);
                running = None;
            }
        }
    }
}

/// Starts the install of `app`'s answers in a thread of its own.
fn start(app: &mut App, log: Log, source: Source) -> Running {
    let (tell, events) = mpsc::channel();
    let (end, finished) = mpsc::channel();
    let answers = app.answers();
    // A setup file's answers come with their hashes; typed ones do not.
    let passwords = if app.loaded.is_some() { Vec::new() } else { app.passwords() };
    app.forget_passwords();
    std::thread::spawn(move || {
        let mut log = log.watched(tell);
        let result = answers.and_then(|mut answers| {
            hash_passwords(&mut answers, passwords, &mut log)?;
            install::install(&answers, &source, &mut log)?;
            Ok(messages::with(
                "installed-line",
                &[
                    ("version", Arg::Text(&source.release.version)),
                    ("disk", Arg::Text(answers.target())),
                ],
            ))
        });
        match &result {
            Ok(line) => log.finish(line),
            Err(why) => log.problem(why),
        }
        let _ = end.send(result);
    });
    Running { events, finished }
}

/// Hashes each password typed into its account's answers: the
/// administrator's first, then each other person's. An empty password is
/// none, as an answers file's `""`.
fn hash_passwords(
    answers: &mut Answers,
    passwords: Vec<String>,
    log: &mut Log,
) -> Result<(), String> {
    if passwords.is_empty() {
        return Ok(());
    }
    log.note("hashing the passwords typed");
    let accounts = std::iter::once(&mut answers.admin).chain(answers.users.iter_mut());
    for (account, password) in accounts.zip(passwords) {
        let mut bytes = password.into_bytes();
        let hashed = if bytes.is_empty() {
            Ok(String::new())
        } else {
            answers::hash_password(&bytes, accounts::hash::Cost::DEFAULT)
        };
        accounts::wipe(&mut bytes);
        account.password_hash = Some(hashed?);
    }
    Ok(())
}

/// Makes the keys type in the layout `name` from now on, as keymap(1)
/// does, so that the passwords are typed as they will be at the sign-in.
/// Gives what went wrong, if anything.
#[must_use]
pub fn set_keyboard(name: &str) -> Option<String> {
    let keymap = PathBuf::from("/system/commands/keymap");
    match Command::new(&keymap).arg(name).output() {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        Err(err) => Some(format!("keymap: {err}")),
    }
}
