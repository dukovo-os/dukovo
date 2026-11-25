// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Asking at the terminal: a line, a line not shown, as a password is,
//! a yes or no, and one key. Ctrl+C stops a question, not the program,
//! which puts the terminal's echo back first.

use std::io::{self, IsTerminal, Write};
use std::os::fd::AsFd;
use std::time::Duration;

use sysapi::signal::{self, Caught, Saved};
use sysapi::terminal::{self, When};
use sysapi::{fd, poll};

/// Set by a signal that stops the question.
static STOPPED: Caught = Caught::new();

/// Whether the standard input is a terminal.
pub fn interactive() -> bool {
    io::stdin().is_terminal()
}

/// The signals that stop a question.
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Has SIGINT, SIGTERM and SIGHUP stop the question at hand: without
/// SA_RESTART, the read(2) waiting for it ends with EINTR. Gives what
/// they did before, for `restore_signals`.
fn catch_signals() -> Vec<Saved> {
    let mut before = Vec::new();
    for signal in SIGNALS {
        before.extend(signal::save(signal));
        let _ = signal::catch(signal, &STOPPED, 0);
    }
    before
}

/// Has the signals do again what they did before the question: once it
/// is over, Ctrl+C ends the program as it would have.
fn restore_signals(before: &[Saved]) {
    for saved in before {
        let _ = saved.restore();
    }
}

/// Reads descriptor `from` a byte at a time into `line` up to its newline,
/// which it leaves out, or its end; stops early when a signal came.
/// Whether a newline or the end came.
fn read_line(from: impl AsFd, line: &mut Vec<u8>) -> io::Result<bool> {
    let mut byte = [0u8; 1];
    while !STOPPED.is_set() {
        match fd::read(&from, &mut byte) {
            Ok(0) => return Ok(!line.is_empty()),
            Ok(_) if byte[0] == b'\n' => return Ok(true),
            Ok(_) => line.push(byte[0]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(false)
}

/// What a question got.
pub enum Answer {
    /// A line, without its newline.
    Line(Vec<u8>),
    /// Nothing: the input ended, or a signal stopped the question.
    Stopped,
}

/// Shows `prompt` on the standard error and reads a line from the
/// standard input, with the terminal's echo off unless `shown`.
pub fn ask(prompt: &str, shown: bool) -> io::Result<Answer> {
    ask_on(io::stdin(), io::stderr(), prompt, shown)
}

/// Shows `prompt` on descriptor `out` and reads a line from descriptor
/// `from`, with the terminal's echo off unless `shown`: [`ask`] on other
/// descriptors, as elevate(8) asks on its terminal whatever its input
/// and output are.
pub fn ask_on(from: impl AsFd, out: impl AsFd, prompt: &str, shown: bool) -> io::Result<Answer> {
    STOPPED.take();
    // A short write only loses some of the prompt.
    let say = |text: &[u8]| {
        let _ = fd::write(&out, text);
    };
    let _ = io::stderr().flush();
    say(prompt.as_bytes());
    let before = catch_signals();
    let saved = if shown { None } else { terminal::attributes(&from).ok() };
    if let Some(saved) = &saved {
        let mut off = *saved;
        off.c_lflag &= !libc::ECHO;
        let _ = terminal::set_attributes(&from, When::Now, &off);
    }
    let mut line = Vec::new();
    let read = read_line(&from, &mut line);
    if let Some(saved) = &saved {
        let _ = terminal::set_attributes(&from, When::Now, saved);
        say(b"\n");
    }
    restore_signals(&before);
    match read {
        Ok(true) if !STOPPED.is_set() => Ok(Answer::Line(line)),
        Ok(_) => {
            accounts::wipe(&mut line);
            if shown {
                say(b"\n");
            }
            Ok(Answer::Stopped)
        }
        Err(err) => {
            accounts::wipe(&mut line);
            Err(err)
        }
    }
}

/// Asks `question` until it is answered yes or no; no unless it is.
pub fn confirm(question: &str) -> bool {
    loop {
        match ask(&format!("{question} [y/N] "), true) {
            Ok(Answer::Line(line)) => match line.trim_ascii() {
                b"y" | b"Y" | b"yes" | b"Yes" | b"YES" => return true,
                b"" | b"n" | b"N" | b"no" | b"No" | b"NO" => return false,
                _ => {}
            },
            _ => return false,
        }
    }
}

/// What a question for one key got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    /// The key, as the byte it sends: a letter, `\n` for Enter, 0x1b for
    /// Escape and the keys that start with it.
    Key(u8),
    /// No key: the input ended, or a signal stopped the question.
    Stopped,
}

/// The terminal's settings as they were, put back when dropped, on every
/// way out of a question for a key.
struct Restore(terminal::Attributes);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = terminal::set_attributes(io::stdin(), When::Now, &self.0);
    }
}

/// The key a line stands for when the input is no terminal: its first
/// character that is not a space, or Enter for a blank line. A script's
/// answer is a letter and Enter, as a person's is.
fn key_of_line(line: &[u8]) -> u8 {
    line.iter().copied().find(|byte| !byte.is_ascii_whitespace()).unwrap_or(b'\n')
}

/// How long the rest of an Escape sequence is waited for: a
/// terminal sends it in one piece.
const SEQUENCE_WAIT: Duration = Duration::from_millis(30);

/// Reads and drops the rest of the key sequence that began with Escape,
/// so that the arrow keys' `[C` is not a `C` pressed.
fn drop_sequence() {
    let mut byte = [0u8; 1];
    while poll::wait_readable(io::stdin(), SEQUENCE_WAIT).unwrap_or(false) {
        if !matches!(fd::read(io::stdin(), &mut byte), Ok(1)) {
            return;
        }
    }
}

/// Reads one key: at once on a terminal, a line's first letter otherwise.
fn read_key() -> io::Result<Pressed> {
    let Ok(saved) = terminal::attributes(io::stdin()) else {
        let mut line = Vec::new();
        return Ok(if read_line(io::stdin(), &mut line)? {
            Pressed::Key(key_of_line(&line))
        } else {
            Pressed::Stopped
        });
    };
    let _restore = Restore(saved);
    let mut keys = saved;
    keys.c_lflag &= !(libc::ICANON | libc::ECHO);
    keys.c_cc[libc::VMIN] = 1;
    keys.c_cc[libc::VTIME] = 0;
    let _ = terminal::set_attributes(io::stdin(), When::Now, &keys);
    let mut byte = [0u8; 1];
    while !STOPPED.is_set() {
        match fd::read(io::stdin(), &mut byte) {
            Ok(0) => return Ok(Pressed::Stopped),
            Ok(_) => {
                if byte[0] == 0x1b {
                    drop_sequence();
                }
                return Ok(Pressed::Key(byte[0]));
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(Pressed::Stopped)
}

/// Shows `prompt` on the standard error and reads one key from the
/// standard input: without waiting for Enter on a terminal, which is put
/// back as it was; on a script's input, the first letter of a line. Ctrl+C
/// stops the question.
pub fn key(prompt: &str) -> io::Result<Pressed> {
    STOPPED.take();
    let _ = io::stderr().flush();
    // A short write only loses some of the prompt.
    let _ = fd::write(io::stderr(), prompt.as_bytes());
    let before = catch_signals();
    let pressed = read_key();
    restore_signals(&before);
    let _ = fd::write(io::stderr(), b"\n");
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scripts_line_is_its_first_letter_and_a_blank_one_is_enter() {
        assert_eq!(key_of_line(b"s"), b's');
        assert_eq!(key_of_line(b"  Send it"), b'S');
        assert_eq!(key_of_line(b""), b'\n');
        assert_eq!(key_of_line(b"   "), b'\n');
    }
}
