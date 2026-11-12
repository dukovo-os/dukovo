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
//! - [`speak`] tells the screen reader what the screens show (ADR 0422).
//! - [`stick`] reads and saves setup files on the stick; a setup file
//!   from the web is [`crate::download`]'s.

pub mod app;
pub mod draw;
pub mod look;
pub mod rdisk;
pub mod speak;
pub mod stick;
#[cfg(test)]
mod tests;

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as Input, KeyCode, KeyEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::{DefaultTerminal, Frame};

use self::app::{Access, App, Outcome};
use self::speak::Heard;
use crate::answers::{self, Answers, TextSize};
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
    let ended = clear(&mut terminal).map_err(|err| format!("the terminal: {err}")).and_then(|()| {
        let (mut app, log, source) = get_ready(&mut terminal, release, plain)
            .inspect_err(|why| cannot_start(&mut terminal, why, plain))?;
        show(&mut terminal, &mut app, log, source).map_err(|err| format!("the terminal: {err}"))
    });
    // What is printed next, a problem or the shell's prompt, starts at
    // the top left.
    let _ = clear(&mut terminal).and_then(|()| terminal.set_cursor_position((0, 0)));
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

/// Clears the whole screen. Not with the terminal's own clear, which first
/// asks the terminal where its cursor is and gives up if no answer comes
/// within 2 seconds: on a serial line the answer comes from the terminal
/// program at the far end, which can be late or, on a plain logger, never
/// come. A resize to the size it has clears the screen without asking,
/// and the screens then draw every cell anew, as at their start.
fn clear(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let size = terminal.size()?;
    terminal.resize(Rect::from((Position::ORIGIN, size)))
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
        let now = messages::text(STARTING[done]);
        draw_screen(terminal, plain, |frame| draw::startup(frame, done, STARTING.len(), &now))
            .map_err(|err| format!("the terminal: {err}"))?;
        // Said, not read from the screen: the screens name all they show
        // from here on (ADR 0422).
        if !plain {
            send(&speak::osc(&now));
        }
        Ok::<(), String>(())
    };
    step(0)?;
    let mut log = Log::create(Path::new(log::LIVE_PATH), false)?;
    let source = install::find_release(release, &mut log).inspect_err(|why| log.problem(why))?;
    step(1)?;
    let machine = Machine::probe();
    step(2)?;
    let mut app = App::new(machine, &source.release.version, source.release.bytes());
    // The screen reader and the consoles' settings as the live system has
    // them: with speech, or turned on with `reader on`.
    app.access.reader |= reader_on();
    live_access(&mut app.access);
    // A disc holds no setup file: it has no EFI system partition to put
    // one on.
    if !source.disc {
        app.setup_files = stick::setup_files(source.stick.as_deref());
    }
    app.from_disc = source.disc;
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
    if !plain {
        let words =
            [messages::text("cannot-start"), why.to_string(), messages::text("cannot-start-leave")];
        send(&speak::osc(&words.join(". ")));
    }
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
    // What the screen reader was last told; the serial console has none.
    let speaks = !app.machine.serial_console;
    let mut heard: Option<Heard> = None;
    // The accessibility the live system has, which the Welcome page
    // changes.
    let mut applied = app.access;
    loop {
        // The machine says whether the screens run on the serial port.
        draw_screen(terminal, app.machine.serial_console, |frame| draw::draw(frame, app))?;
        if speaks {
            let now = Heard::of(app);
            if let Some(words) = speak::speech(heard.as_ref(), &now) {
                send(&speak::osc(&words));
            }
            heard = Some(now);
        }
        let input = if event::poll(TICK)? { Some(event::read()?) } else { None };
        if matches!(input, Some(Input::Key(_) | Input::Resize(..))) {
            // consoled's own keys change the text size and sticky keys
            // too, and write its settings file: Ctrl+Alt+Plus resizes the
            // screen, and Shift pressed five times comes before a key.
            live_access(&mut app.access);
            applied = app.access;
        }
        if let Some(Input::Key(key)) = input
            && key.kind == KeyEventKind::Press
        {
            match app.key(key) {
                Outcome::Stay => {}
                Outcome::Access => {
                    app.said = set_access(app.access, applied);
                    // The screen reader just started hears the page whole,
                    // as consoled says it once readerd listens.
                    if app.access.reader && !applied.reader {
                        heard = None;
                    }
                    applied = app.access;
                }
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
                    ("medium", Arg::Text(source.medium())),
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
    command("keymap", &[name])
}

/// Runs `/system/commands/<name>` with `args`, its output kept off the
/// screens. Gives what went wrong, if anything.
fn command(name: &str, args: &[&str]) -> Option<String> {
    let program = PathBuf::from("/system/commands").join(name);
    match Command::new(&program).args(args).output() {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        Err(err) => Some(format!("{name}: {err}")),
    }
}

/// The live system's consoles' settings, accessibility.toml(5), which
/// consoled reads on `service consoled reload` (ADR 0325).
const CONSOLE_SETTINGS: &str = "/system/settings/console/accessibility.toml";

/// reader.toml(5), which says whether the screen reader is on.
const READER_SETTINGS: &str = "/system/state/reader/reader.toml";

/// The consoles' settings file's text, read as TOML, if it is there and
/// is.
fn console_settings() -> Option<toml::Table> {
    std::fs::read_to_string(CONSOLE_SETTINGS).ok()?.parse().ok()
}

/// Whether reader.toml(5) says the screen reader is on, as `reader on`
/// sets it.
fn reader_on() -> bool {
    std::fs::read_to_string(READER_SETTINGS)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| table.get("on").and_then(toml::Value::as_bool))
        .unwrap_or(false)
}

/// Takes the consoles' text size and keys into `access` from the live
/// system's settings file, where it gives them: consoled writes it as
/// its own keys change them.
fn live_access(access: &mut Access) {
    let Some(table) = console_settings() else { return };
    let size = table.get("text-size").and_then(toml::Value::as_str).and_then(TextSize::from_id);
    if let Some(size) = size {
        access.text_size = size;
    }
    if let Some(on) = table.get("sticky-keys").and_then(toml::Value::as_bool) {
        access.sticky_keys = on;
    }
    if let Some(on) = table.get("slow-keys").and_then(toml::Value::as_bool) {
        access.slow_keys = on;
    }
}

/// Makes the live system as `access` says where it differs from `was`
/// (ADR 0422): the consoles' settings file written and consoled told to
/// read it, and the screen reader turned on or off, as reader(1) does.
/// Gives what went wrong, if anything.
fn set_access(access: Access, was: Access) -> Option<String> {
    let mut problems = Vec::new();
    let consoles = |a: Access| (a.text_size, a.sticky_keys, a.slow_keys);
    if consoles(access) != consoles(was) {
        // Slow keys' delay is kept, which the Welcome page does not set.
        let delay = console_settings()
            .and_then(|table| table.get("slow-keys-delay").and_then(toml::Value::as_integer))
            .and_then(|ms| u64::try_from(ms).ok());
        let settings = answers::Accessibility {
            text_size: access.text_size,
            sticky_keys: access.sticky_keys,
            slow_keys: access.slow_keys,
            ..answers::Accessibility::default()
        };
        let path = Path::new(CONSOLE_SETTINGS);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, settings.console_file(delay)));
        match written {
            Ok(()) => problems.extend(command("service", &["consoled", "reload"])),
            Err(err) => problems.push(format!("{CONSOLE_SETTINGS}: {err}")),
        }
    }
    if access.reader != was.reader {
        problems.extend(command("reader", &[if access.reader { "on" } else { "off" }]));
    }
    (!problems.is_empty()).then(|| problems.join("; "))
}
