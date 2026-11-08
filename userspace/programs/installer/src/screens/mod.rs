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
//! - [`draw`] draws them, and the startup screen before the first page.
//! - [`look`] has the design's colours, which the screens set as the
//!   terminal's palette while they show.
//! - [`rdisk`] is the manual layout's screen, where each partition is
//!   chosen (ADR 0311).
//! - [`stick`] reads and saves setup files on the stick; a setup file
//!   from the web is [`crate::download`]'s.

pub mod app;
pub mod draw;
pub mod look;
pub mod rdisk;
pub mod stick;
#[cfg(test)]
mod tests;

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as Input, KeyCode, KeyEventKind};
use ratatui::{DefaultTerminal, Frame};

use self::app::{App, Outcome};
use crate::answers::{self, Answers};
use crate::download;
use crate::features;
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
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(err) => {
            eprintln!("dukovo-install: the screens need a terminal: {err}");
            return 1;
        }
    };
    // The design's colours, for as long as the screens show.
    send(&look::set_palette());
    // On the serial port, the screens are drawn in plain ASCII.
    let plain = crate::machine::on_serial_console();
    // The consoles keep no second screen for full-screen programs, so
    // the screens clear what was there before them, and what they drew
    // once they end, and give the colours back, before the shell's
    // prompt comes.
    let ended = terminal.clear().map_err(|err| format!("the terminal: {err}")).and_then(|()| {
        let (mut app, log, source) = get_ready(&mut terminal, release, plain)
            .inspect_err(|why| cannot_start(&mut terminal, why, plain))?;
        show(&mut terminal, &mut app, log, source).map_err(|err| format!("the terminal: {err}"))
    });
    // What is printed next, a problem or the shell's prompt, starts at
    // the top left.
    let _ = terminal.clear().and_then(|()| terminal.set_cursor_position((0, 0)));
    send(look::RESET_PALETTE);
    ratatui::restore();
    match ended {
        Ok(status) => status,
        Err(why) => {
            eprintln!("dukovo-install: {why}");
            1
        }
    }
}

/// Draws a screen with `draw`, in plain ASCII if `plain`, as on a serial
/// console ([`draw::plain`]).
fn draw_screen(
    terminal: &mut DefaultTerminal,
    plain: bool,
    draw: impl FnOnce(&mut Frame<'_>),
) -> io::Result<()> {
    terminal
        .draw(|frame| {
            draw(frame);
            if plain {
                draw::plain(frame.buffer_mut());
            }
        })
        .map(|_| ())
}

/// Sends `codes` to the terminal, past the screens' drawing.
fn send(codes: &str) {
    let mut out = io::stdout();
    let _ = out.write_all(codes.as_bytes()).and_then(|()| out.flush());
}

/// What the startup screen tells, in order, while the installer gets
/// ready for the first page: its bar fills a part for each.
const STARTING: [&str; 3] = ["starting-release", "starting-machine", "starting-setup"];

/// Gets the screens ready, the startup screen showing what is being done:
/// the release on the stick found and its signatures checked, the
/// machine's disks and network cards read, and the setup files on the
/// stick listed. Gives the screens' answers, the install log and the
/// release, or why the installer cannot go on. `plain` draws in ASCII.
fn get_ready(
    terminal: &mut DefaultTerminal,
    release: Option<&Path>,
    plain: bool,
) -> Result<(App, Log, Source), String> {
    let mut step = |done: usize| {
        draw_screen(terminal, plain, |frame| {
            draw::startup(frame, done, STARTING.len(), &messages::text(STARTING[done]));
        })
        .map_err(|err| format!("the terminal: {err}"))
    };
    step(0)?;
    let mut log = Log::create(Path::new(log::LIVE_PATH), false)?;
    let source = install::find_release(release, &mut log).inspect_err(|why| log.problem(why))?;
    step(1)?;
    let machine = Machine::probe();
    step(2)?;
    let mut app = App::new(machine, &source.release.version, source.release.bytes());
    app.setup_files = stick::setup_files(source.stick.as_deref());
    // The live system is the release the install copies.
    app.features = features::offered(Path::new("/system"));
    // The release's signatures were checked before the first page.
    app.told(Event::Step(Step::Release));
    Ok((app, log, source))
}

/// Tells on the startup screen why the installer cannot start, `why`,
/// and waits for Enter or Esc. Printed once the screens end, it would not
/// be read on the stick: the shell consoled starts on tty1 then clears
/// the screen. `plain` draws in ASCII.
fn cannot_start(terminal: &mut DefaultTerminal, why: &str, plain: bool) {
    loop {
        if draw_screen(terminal, plain, |frame| draw::cannot_start(frame, why)).is_err() {
            return;
        }
        match event::read() {
            Ok(Input::Key(key))
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Enter | KeyCode::Esc) =>
            {
                return;
            }
            // Any other key, or the screen's size changed: drawn again.
            Ok(_) => {}
            Err(_) => return,
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
        // The machine says whether the screens run on the serial port.
        draw_screen(terminal, app.machine.serial_console, |frame| draw::draw(frame, app))?;
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
                Outcome::Download(url, sha256) => {
                    // Drawn first, so that the page says it is
                    // downloading for as long as it is.
                    draw_screen(terminal, app.machine.serial_console, |frame| {
                        draw::draw(frame, app);
                    })?;
                    let fetched = download::fetch(&url, sha256);
                    if let Some((log, _)) = ready.as_mut() {
                        let address = app.setup_url.trim();
                        log.note(&match &fetched {
                            Ok(file) => format!("setup file from {}", file.source(address)),
                            Err(why) => format!("setup file from {address}: {why}"),
                        });
                    }
                    app.downloaded(fetched);
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
                    ("disk", Arg::Text(&answers.disks())),
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
