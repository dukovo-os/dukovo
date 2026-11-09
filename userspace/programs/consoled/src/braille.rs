// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a braille display shows of the consoles (ADR 0326).
//!
//! BRLTTY drives the display; its screen driver for Dukovo reads the
//! consoles from consoled, on [`SOCKET`], which only root may open. Each
//! time what the shown console shows changes, consoled sends it whole:
//!
//! ```text
//! screen NUMBER COLUMNS ROWS X Y
//! ```
//!
//! then ROWS lines of COLUMNS characters each, in UTF-8. NUMBER is the
//! console's, 1 for tty1; X and Y are where the cursor is, from 0. The
//! rows are those of the console as the screen shows them, the
//! scrollback while the view is moved back, and one more: consoled's own
//! line, its prompts, its Ctrl+Alt+Delete menu, its notes and the
//! notifications, or blank. No program writes that row, so what is read
//! there is consoled's, as the trusted sound marks it for the screen
//! reader (ADR 0324). While consoled asks for something there, the cursor
//! is on it: after a prompt's words, where the answer goes, or at the
//! start of its menu.
//!
//! The screen driver sends back, a line at a time, what the display's
//! keys type, which consoled takes as if typed on the keyboard
//! ([`crate::keys::named`]):
//!
//! ```text
//! key NAME [shift] [ctrl] [alt]
//! switch NUMBER
//! ```
//!
//! NAME is a character as `U+XXXX`, or `enter`, `tab`, `backspace`,
//! `escape`, `left`, `right`, `up`, `down`, `pageup`, `pagedown`, `home`,
//! `end`, `insert`, `delete` or `f1` to `f12`. `switch` shows console
//! NUMBER.
//!
//! One display is read at a time: a new connection takes the place of
//! the one before, as when BRLTTY starts again. A screen is sent only
//! when it changed, and while the reader is slow only the newest waits,
//! so that a flood of output never keeps the display behind.

use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use crate::term::{Cell, Terminal, WIDE_TAIL};

/// Where consoled listens for BRLTTY.
pub const SOCKET: &str = "/system/live/consoled/braille";

/// The most bytes of a line from the display; a longer one is dropped.
const LINE_MAX: usize = 256;

/// What the display's keys asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A key, by its name, with the modifiers held.
    Key { name: String, shift: bool, ctrl: bool, alt: bool },
    /// Show console `n`, from 1.
    Switch(usize),
}

/// The display's connection.
#[derive(Debug, Default)]
pub struct Braille {
    listener: Option<UnixListener>,
    client: Option<UnixStream>,
    /// The last screen made, so that only a change is sent.
    last: Vec<u8>,
    /// What is being written, and how much of it has gone.
    out: Vec<u8>,
    written: usize,
    /// A newer screen, sent once `out` has all gone.
    next: Option<Vec<u8>>,
    /// What came from the display that is not yet a whole line.
    input: Vec<u8>,
    /// Whether what came is a line too long, dropped until its end.
    dropping: bool,
}

impl Braille {
    /// Listens on `path`, which only root may open. If it cannot, it says
    /// why, and consoled goes on without a display.
    pub fn listen(path: &str) -> Braille {
        match listener(path) {
            Ok(listener) => Braille { listener: Some(listener), ..Braille::default() },
            Err(err) => {
                eprintln!("consoled: braille displays cannot connect: {path}: {err}");
                Braille::default()
            }
        }
    }

    /// The listening socket's descriptor, or -1.
    pub fn listener_fd(&self) -> i32 {
        self.listener.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }

    /// The display's descriptor, or -1, and the events to wait for: what
    /// it sends, and room for what is still to be written.
    pub fn client_fd(&self) -> (i32, i16) {
        match &self.client {
            Some(client) => {
                let writing = self.written < self.out.len();
                let events = libc::POLLIN | if writing { libc::POLLOUT } else { 0 };
                (client.as_raw_fd(), events)
            }
            None => (-1, 0),
        }
    }

    /// Whether a display is connected.
    pub fn connected(&self) -> bool {
        self.client.is_some()
    }

    /// Takes a new connection, in the place of the one before.
    pub fn accept(&mut self) {
        let Some(listener) = &self.listener else { return };
        match listener.accept() {
            Ok((client, _)) => {
                if client.set_nonblocking(true).is_err() {
                    return;
                }
                if self.client.is_some() {
                    eprintln!("consoled: braille display connected again");
                } else {
                    eprintln!("consoled: braille display connected");
                }
                self.connect(client);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) => eprintln!("consoled: braille display could not connect: {err}"),
        }
    }

    /// Starts afresh with `client`: the screen is sent whole at once.
    fn connect(&mut self, client: UnixStream) {
        self.client = Some(client);
        self.last.clear();
        self.out.clear();
        self.written = 0;
        self.next = None;
        self.input.clear();
        self.dropping = false;
    }

    /// Forgets the display.
    fn disconnect(&mut self, why: &str) {
        if self.client.take().is_some() {
            eprintln!("consoled: braille display {why}");
        }
    }

    /// Reads what the display sent: what its keys ask for.
    pub fn read(&mut self) -> Vec<Input> {
        let mut inputs = Vec::new();
        let Some(client) = self.client.as_mut() else { return inputs };
        let mut buffer = [0u8; 512];
        loop {
            match client.read(&mut buffer) {
                Ok(0) => {
                    self.disconnect("went away");
                    break;
                }
                Ok(n) => {
                    for &byte in &buffer[..n] {
                        if byte != b'\n' {
                            if self.input.len() < LINE_MAX {
                                self.input.push(byte);
                            } else {
                                self.dropping = true;
                            }
                            continue;
                        }
                        let line = std::mem::take(&mut self.input);
                        if !std::mem::take(&mut self.dropping) {
                            match parse(&line) {
                                Some(input) => inputs.push(input),
                                None => eprintln!(
                                    "consoled: braille display sent what is not a key: {}",
                                    String::from_utf8_lossy(&line).escape_debug()
                                ),
                            }
                        }
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => {
                    self.disconnect(&format!("could not be read: {err}"));
                    break;
                }
            }
        }
        inputs
    }

    /// Sends `screen` (made by [`screen`]) if it is not what was sent
    /// last, after what is still being written.
    pub fn show(&mut self, screen: Vec<u8>) {
        if self.client.is_none() || screen == self.last {
            return;
        }
        self.last.clone_from(&screen);
        if self.written < self.out.len() {
            self.next = Some(screen);
        } else {
            self.out = screen;
            self.written = 0;
        }
        self.flush();
    }

    /// Writes what it can of what is waiting, without waiting.
    pub fn flush(&mut self) {
        loop {
            if self.written == self.out.len() {
                match self.next.take() {
                    Some(next) => {
                        self.out = next;
                        self.written = 0;
                    }
                    None => return,
                }
            }
            let Some(client) = self.client.as_mut() else { return };
            match client.write(&self.out[self.written..]) {
                Ok(0) => {
                    self.disconnect("went away");
                    return;
                }
                Ok(n) => self.written += n,
                Err(err) if err.kind() == ErrorKind::WouldBlock => return,
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => {
                    self.disconnect(&format!("could not be written: {err}"));
                    return;
                }
            }
        }
    }
}

/// A socket listening on `path`, which only root may open.
fn listener(path: &str) -> io::Result<UnixListener> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// What a line from the display asks for, if it is one of the lines it
/// may send.
fn parse(line: &[u8]) -> Option<Input> {
    let line = std::str::from_utf8(line).ok()?;
    let mut words = line.split(' ');
    match words.next()? {
        "key" => {
            let name = words.next().filter(|name| !name.is_empty())?.to_string();
            let (mut shift, mut ctrl, mut alt) = (false, false, false);
            for word in words {
                match word {
                    "shift" => shift = true,
                    "ctrl" => ctrl = true,
                    "alt" => alt = true,
                    _ => return None,
                }
            }
            Some(Input::Key { name, shift, ctrl, alt })
        }
        "switch" => {
            let number = words.next()?.parse().ok().filter(|&n| n > 0)?;
            words.next().is_none().then_some(Input::Switch(number))
        }
        _ => None,
    }
}

/// Where the cursor goes on consoled's own line while it asks for
/// something there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asking {
    /// A prompt: after its words, where the answer is typed.
    Prompt,
    /// The menu: at the start of its words, so that it reads from there.
    Menu,
}

/// The screen as the display is sent it: console `number`'s `term` as
/// the screen shows it, then consoled's own line, `bottom`, or a blank
/// one. While consoled asks for something there (`asking`), the cursor
/// is on that line.
pub fn screen(
    number: usize,
    term: &Terminal,
    bottom: Option<&[Cell]>,
    asking: Option<Asking>,
) -> Vec<u8> {
    let (cols, rows) = (term.cols(), term.rows());
    let words = |cells: &[Cell]| match asking {
        Some(Asking::Prompt) => cells.iter().rposition(|cell| cell.c != ' ').map_or(0, |at| at + 2),
        _ => cells.iter().position(|cell| cell.c != ' ').unwrap_or(0),
    };
    let (x, y) = match bottom {
        Some(cells) if asking.is_some() => (words(cells).min(cols.saturating_sub(1)), rows),
        _ => term.cursor().unwrap_or_else(|| term.position()),
    };
    let mut text = format!("screen {number} {cols} {} {x} {y}\n", rows + 1).into_bytes();
    let mut row = String::with_capacity(cols * 2 + 1);
    for line in 0..=rows {
        row.clear();
        for col in 0..cols {
            let cell = if line < rows {
                term.shown(col, line)
            } else {
                bottom.and_then(|cells| cells.get(col).copied()).unwrap_or(Cell::BLANK)
            };
            // A wide character's second cell, and anything that is not a
            // character the display can show, is a blank.
            let c = if cell.flags & WIDE_TAIL != 0 || cell.c.is_control() { ' ' } else { cell.c };
            row.push(c);
        }
        row.push('\n');
        text.extend_from_slice(row.as_bytes());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A display connected to a [`Braille`], and its end.
    fn connected() -> (Braille, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        theirs.set_nonblocking(true).unwrap();
        let mut braille = Braille::default();
        braille.connect(ours);
        (braille, theirs)
    }

    fn received(theirs: &mut UnixStream) -> String {
        let mut text = Vec::new();
        let mut buffer = [0u8; 4096];
        while let Ok(n) = theirs.read(&mut buffer) {
            if n == 0 {
                break;
            }
            text.extend_from_slice(&buffer[..n]);
        }
        String::from_utf8(text).unwrap()
    }

    #[test]
    fn the_screen_is_the_consoles_rows_and_consoleds_own_line() {
        let mut term = Terminal::new(6, 2);
        term.feed("ab\r\nc日".as_bytes());
        let shown = |bottom: Option<&[Cell]>, asking| {
            String::from_utf8(screen(3, &term, bottom, asking)).unwrap()
        };
        // The cursor after the wide character; its second cell a blank.
        assert_eq!(shown(None, None), "screen 3 6 3 3 1\nab    \nc日    \n      \n");
        // A prompt on consoled's line has the cursor, after its words.
        let mut bottom = vec![Cell::BLANK; 6];
        for (cell, c) in bottom.iter_mut().zip(" Pw:".chars()) {
            cell.c = c;
        }
        let prompt = Some(Asking::Prompt);
        assert_eq!(shown(Some(&bottom), prompt), "screen 3 6 3 5 2\nab    \nc日    \n Pw:  \n");
        // The menu has it at the start of its words.
        assert!(shown(Some(&bottom), Some(Asking::Menu)).starts_with("screen 3 6 3 1 2\n"));
        // A notification there leaves the cursor where it was.
        assert!(shown(Some(&bottom), None).starts_with("screen 3 6 3 3 1\n"));
    }

    #[test]
    fn a_screen_is_sent_only_when_it_changed_and_only_the_newest_waits() {
        let (mut braille, mut theirs) = connected();
        braille.show(b"screen 1 1 1 0 0\na\n".to_vec());
        braille.show(b"screen 1 1 1 0 0\na\n".to_vec());
        assert_eq!(received(&mut theirs), "screen 1 1 1 0 0\na\n");
        // While a screen waits for the display to read it, two more come:
        // only the newest is sent after it.
        let big = [b"screen 1 1 1 0 0\n".as_slice(), &vec![b'x'; 1 << 20], b"\n"].concat();
        braille.show(big.clone());
        assert!(braille.written < big.len());
        braille.show(b"screen 1 1 1 0 0\nc\n".to_vec());
        braille.show(b"screen 1 1 1 0 0\nd\n".to_vec());
        let last = b"screen 1 1 1 0 0\nd\n";
        let mut all = Vec::new();
        let mut buffer = vec![0u8; 1 << 16];
        while all.len() < big.len() + last.len() {
            braille.flush();
            if let Ok(n) = theirs.read(&mut buffer) {
                all.extend_from_slice(&buffer[..n]);
            }
        }
        assert_eq!(&all[..big.len()], big.as_slice());
        assert_eq!(&all[big.len()..], last);
        assert_eq!(received(&mut theirs), "");
    }

    #[test]
    fn the_displays_keys_and_switches_are_read_and_anything_else_is_not() {
        let (mut braille, mut theirs) = connected();
        theirs
            .write_all(
                b"key U+0061\nkey left shift ctrl alt\nswitch 2\nkey\nswitch 0\nsay hi\nkey enter",
            )
            .unwrap();
        let inputs = braille.read();
        let key = |name: &str, shift, ctrl, alt| Input::Key { name: name.into(), shift, ctrl, alt };
        assert_eq!(
            inputs,
            [key("U+0061", false, false, false), key("left", true, true, true), Input::Switch(2)]
        );
        // The rest of a line comes later.
        theirs.write_all(b" shift\n").unwrap();
        assert_eq!(braille.read(), [key("enter", true, false, false)]);
        // A line too long is dropped whole.
        theirs.write_all(&[b'x'; 300]).unwrap();
        theirs.write_all(b"\nswitch 1\n").unwrap();
        assert_eq!(braille.read(), [Input::Switch(1)]);
        // The display gone, consoled forgets it.
        drop(theirs);
        assert_eq!(braille.read(), []);
        assert!(!braille.connected());
    }

    #[test]
    fn a_new_display_is_sent_the_screen_whole() {
        let (mut braille, mut theirs) = connected();
        braille.show(b"screen 1 1 1 0 0\na\n".to_vec());
        assert_eq!(received(&mut theirs), "screen 1 1 1 0 0\na\n");
        let (ours, mut again) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        again.set_nonblocking(true).unwrap();
        braille.connect(ours);
        braille.show(b"screen 1 1 1 0 0\na\n".to_vec());
        assert_eq!(received(&mut again), "screen 1 1 1 0 0\na\n");
    }
}
