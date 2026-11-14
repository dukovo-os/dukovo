// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Asking at the terminal: a line, a line not shown, as a password is,
//! a yes or no, and one key. Ctrl+C stops a question, not the program,
//! which puts the terminal's echo back first.

use std::io::{self, Write};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::util;

/// Set by a signal that stops the question.
static STOPPED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOPPED.store(true, Ordering::Relaxed);
}

/// Whether the standard input is a terminal.
pub fn interactive() -> bool {
    // SAFETY: isatty only looks at the descriptor.
    unsafe { libc::isatty(0) == 1 }
}

/// The signals that stop a question.
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Has SIGINT, SIGTERM and SIGHUP stop the question at hand: without
/// SA_RESTART, the read(2) waiting for it ends with EINTR. Gives what
/// they did before, for `restore_signals`.
fn catch_signals() -> [libc::sigaction; 3] {
    // SAFETY: all zeros is a valid sigaction: numbers, a signal set and
    // a function address.
    let mut before: [libc::sigaction; 3] = unsafe { util::zeroed() };
    for (signal, before) in SIGNALS.iter().zip(&mut before) {
        // SAFETY: as above.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as *const () as usize;
        // SAFETY: the handler only stores to an atomic; `before` is a
        // sigaction to fill.
        unsafe { libc::sigaction(*signal, &raw const action, before) };
    }
    before
}

/// Has the signals do again what they did before the question: once it
/// is over, Ctrl+C ends the program as it would have.
fn restore_signals(before: &[libc::sigaction; 3]) {
    for (signal, before) in SIGNALS.iter().zip(before) {
        // SAFETY: `before` is what sigaction gave for this signal.
        unsafe { libc::sigaction(*signal, before, std::ptr::null_mut()) };
    }
}

/// Reads descriptor `fd` a byte at a time into `line` up to its newline,
/// which it leaves out, or its end; stops early when a signal came.
/// Whether a newline or the end came.
fn read_line(fd: RawFd, line: &mut Vec<u8>) -> io::Result<bool> {
    let mut byte = 0u8;
    while !STOPPED.load(Ordering::Relaxed) {
        // SAFETY: `byte` is one writable byte.
        match unsafe { libc::read(fd, (&raw mut byte).cast(), 1) } {
            0 => return Ok(!line.is_empty()),
            1 if byte == b'\n' => return Ok(true),
            1 => line.push(byte),
            _ => match io::Error::last_os_error() {
                err if err.kind() == io::ErrorKind::Interrupted => {}
                err => return Err(err),
            },
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
    ask_on(0, 2, prompt, shown)
}

/// Shows `prompt` on descriptor `out` and reads a line from descriptor
/// `fd`, with the terminal's echo off unless `shown`: [`ask`] on other
/// descriptors, as elevate(8) asks on its terminal whatever its input
/// and output are.
pub fn ask_on(fd: RawFd, out: RawFd, prompt: &str, shown: bool) -> io::Result<Answer> {
    STOPPED.store(false, Ordering::Relaxed);
    let say = |text: &[u8]| {
        // SAFETY: writes from a valid buffer; a short write only loses
        // some of the prompt.
        unsafe { libc::write(out, text.as_ptr().cast(), text.len()) };
    };
    let _ = io::stderr().flush();
    say(prompt.as_bytes());
    let before = catch_signals();
    // SAFETY: all zeros is a valid termios: numbers.
    let mut saved: libc::termios = unsafe { util::zeroed() };
    // SAFETY: tcgetattr fills `saved`.
    let quiet = !shown && unsafe { libc::tcgetattr(fd, &raw mut saved) } == 0;
    if quiet {
        let mut off = saved;
        off.c_lflag &= !libc::ECHO;
        // SAFETY: `off` is a valid termios.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw const off) };
    }
    let mut line = Vec::new();
    let read = read_line(fd, &mut line);
    if quiet {
        // SAFETY: `saved` is the termios read above.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw const saved) };
        say(b"\n");
    }
    restore_signals(&before);
    match read {
        Ok(true) if !STOPPED.load(Ordering::Relaxed) => Ok(Answer::Line(line)),
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
struct Restore(libc::termios);

impl Drop for Restore {
    fn drop(&mut self) {
        // SAFETY: the termios was read from the terminal at the start.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw const self.0) };
    }
}

/// The key a line stands for when the input is no terminal: its first
/// character that is not a space, or Enter for a blank line. A script's
/// answer is a letter and Enter, as a person's is.
fn key_of_line(line: &[u8]) -> u8 {
    line.iter().copied().find(|byte| !byte.is_ascii_whitespace()).unwrap_or(b'\n')
}

/// How long the rest of an Escape sequence is waited for, in milliseconds:
/// a terminal sends it in one piece.
const SEQUENCE_WAIT: libc::c_int = 30;

/// Reads and drops the rest of the key sequence that began with Escape,
/// so that the arrow keys' `[C` is not a `C` pressed.
fn drop_sequence() {
    loop {
        let mut fd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, for descriptor 0.
        if unsafe { libc::poll(&raw mut fd, 1, SEQUENCE_WAIT) } <= 0 {
            return;
        }
        let mut byte = 0u8;
        // SAFETY: `byte` is one writable byte.
        if unsafe { libc::read(0, (&raw mut byte).cast(), 1) } != 1 {
            return;
        }
    }
}

/// Reads one key: at once on a terminal, a line's first letter otherwise.
fn read_key() -> io::Result<Pressed> {
    // SAFETY: all zeros is a valid termios: numbers.
    let mut saved: libc::termios = unsafe { util::zeroed() };
    // SAFETY: tcgetattr fills `saved`.
    if unsafe { libc::tcgetattr(0, &raw mut saved) } != 0 {
        let mut line = Vec::new();
        return Ok(if read_line(0, &mut line)? {
            Pressed::Key(key_of_line(&line))
        } else {
            Pressed::Stopped
        });
    }
    let _restore = Restore(saved);
    let mut keys = saved;
    keys.c_lflag &= !(libc::ICANON | libc::ECHO);
    keys.c_cc[libc::VMIN] = 1;
    keys.c_cc[libc::VTIME] = 0;
    // SAFETY: `keys` is a valid termios.
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw const keys) };
    while !STOPPED.load(Ordering::Relaxed) {
        let mut byte = 0u8;
        // SAFETY: `byte` is one writable byte.
        match unsafe { libc::read(0, (&raw mut byte).cast(), 1) } {
            0 => return Ok(Pressed::Stopped),
            1 => {
                if byte == 0x1b {
                    drop_sequence();
                }
                return Ok(Pressed::Key(byte));
            }
            _ => match io::Error::last_os_error() {
                err if err.kind() == io::ErrorKind::Interrupted => {}
                err => return Err(err),
            },
        }
    }
    Ok(Pressed::Stopped)
}

/// Shows `prompt` on the standard error and reads one key from the
/// standard input: without waiting for Enter on a terminal, which is put
/// back as it was; on a script's input, the first letter of a line. Ctrl+C
/// stops the question.
pub fn key(prompt: &str) -> io::Result<Pressed> {
    STOPPED.store(false, Ordering::Relaxed);
    let _ = io::stderr().flush();
    // SAFETY: writes from a valid buffer; a short write only loses some of
    // the prompt.
    unsafe { libc::write(2, prompt.as_ptr().cast(), prompt.len()) };
    let before = catch_signals();
    let pressed = read_key();
    restore_signals(&before);
    // SAFETY: as above.
    unsafe { libc::write(2, b"\n".as_ptr().cast(), 1) };
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
