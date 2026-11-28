// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! windowd: Dukovo's window server, which the `desktop` command starts
//! (desktop(1), ADR 0712).
//!
//! It borrows the screens from consoled, asks inputd for the keyboards
//! and pointers, and shows the windows of the programs that connect to
//! its socket, which speak Wayland. A window's pixels are drawn in a
//! picture of the display layer's (radix-display(2)) by pixman, on the
//! processor.

#![forbid(unsafe_code)]

mod borrowed;
mod clients;
mod curve;
mod desktop;
mod devices;
mod keys;
mod link;
mod manager;
mod panel;
mod pointer;
mod scene;
mod screens;
mod session;
mod settings;
mod shell;
mod socket;
mod stamps;
mod titlebar;
mod windows;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::process::{Child, Command, ExitCode};

use display::Holder;
use sysapi::{fd, process, signal};

use borrowed::Borrowed;
use desktop::Desktop;
use devices::Devices;
use link::Card;
use session::{CHILD, END, RELOAD, Session};
use socket::Listening;

const USAGE: &str = "\
usage: desktop [COMMAND [ARGUMENT]...]
Shows the desktop on the screens, and in it the windows of COMMAND and
of the programs it starts. The desktop ends when COMMAND does, and with
Ctrl+Alt+Backspace. Ctrl+Alt+F1 to F12 show a console; the desktop's own
shows the desktop again.";

/// The command the desktop was started with, if any: every argument,
/// once the options are read.
fn command() -> Result<Vec<OsString>, String> {
    let mut arguments = std::env::args_os().skip(1).peekable();
    match arguments.peek().and_then(|first| first.to_str()) {
        Some("--help" | "-h") => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Some("--version") => {
            println!("desktop (Dukovo) {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        // A command may be named after `--`, as one that starts with a
        // dash is.
        Some("--") => drop(arguments.next()),
        Some(option) if option.starts_with('-') => {
            return Err(format!("unknown option `{option}`\n{USAGE}"));
        }
        _ => {}
    }
    Ok(arguments.collect())
}

/// Has the signals that end the desktop, the one that has it read its
/// settings again, and a child's end, wake the wait. Returns the read end of the pipe they wake it through.
fn catch_signals() -> io::Result<File> {
    let (woken, wakes) = fd::pipe()?;
    fd::set_nonblocking(&woken, true)?;
    fd::set_nonblocking(&wakes, true)?;
    END.wake_through(wakes.try_clone()?)?;
    RELOAD.wake_through(wakes.try_clone()?)?;
    CHILD.wake_through(wakes)?;
    for ends in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        signal::catch(ends, &END, libc::SA_RESTART)?;
    }
    signal::catch(libc::SIGUSR1, &RELOAD, libc::SA_RESTART)?;
    signal::catch(libc::SIGCHLD, &CHILD, libc::SA_RESTART | libc::SA_NOCLDSTOP)?;
    Ok(File::from(woken))
}

/// Starts `command`, whose windows show in the desktop that listens as
/// `listening` says.
fn start(command: &[OsString], listening: &Listening) -> Result<Option<Child>, String> {
    let Some((program, arguments)) = command.split_first() else { return Ok(None) };
    let mut command = Command::new(program);
    command.args(arguments);
    command.env("XDG_RUNTIME_DIR", &listening.folder).env("WAYLAND_DISPLAY", &listening.name);
    let child = command.spawn().map_err(|err| format!("{}: {err}", program.to_string_lossy()))?;
    Ok(Some(child))
}

/// Runs the desktop to its end.
fn run() -> Result<ExitCode, String> {
    let command = command()?;
    let layout = keymap::chosen().unwrap_or_else(|why| {
        eprintln!("desktop: {why}; the keys type in English (US)");
        keymap::DEFAULT.to_owned()
    });
    let named = std::env::var_os("XDG_RUNTIME_DIR").map(Into::into);
    let listening = Listening::start(named, process::euid())
        .map_err(|err| format!("the socket for the programs: {err}"))?;
    let woken = catch_signals().map_err(|err| format!("the signals could not be caught: {err}"))?;
    let (borrowed, lent) = Borrowed::ask()?;
    let card = Card::new(lent).map_err(|err| format!("the display layer: {err}"))?;
    let devices = Devices::ask(card.as_fd()).map_err(|err| format!("inputd: {err}"))?;
    let desktop = Desktop::new(Holder::new(card), &layout).map_err(|err| err.to_string())?;
    let mut session = Session {
        desktop,
        devices,
        borrowed,
        listening,
        command: None,
        woken,
        inputd_there: true,
        personal: settings::personal(),
    };
    let ended = start(&command, &session.listening).and_then(|started| {
        session.command = started;
        session.run()
    });
    session.end();
    ended
}

fn main() -> ExitCode {
    run().unwrap_or_else(|why| {
        eprintln!("desktop: {why}");
        ExitCode::FAILURE
    })
}
