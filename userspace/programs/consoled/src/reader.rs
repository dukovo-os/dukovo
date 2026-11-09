// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screen reader (ADR 0324): what consoled gives readerd(8) to say.
//!
//! consoled holds every console's text and draws the prompts no program
//! draws, so it alone decides what the reader says, and readerd only
//! speaks: what is written to the shown console, a line at a time; the
//! character, word or line the cursor moves onto when a key moves it
//! and nothing is written; "Console 2" and the cursor's line on a switch;
//! notifications; and consoled's own prompts and menu, as `trusted`, after
//! a sound that nothing else makes. Nothing goes the other way: readerd
//! has no way to type into a console or to answer a prompt, and what is
//! typed at a password prompt is never said.
//!
//! The reader key is Caps Lock or Insert, as in NVDA. Held, it makes the
//! keys pressed with it the reader's, which no program receives:
//!
//! - Up and Down: the line above or below the review cursor, which starts
//!   at the cursor each time the screen changes;
//! - Left and Right: the word before or after it;
//! - Comma and Period: the character before or after it;
//! - L: the review line again; A: every line from it down; T: which
//!   console this is; H: these keys;
//! - Page Up and Page Down: faster and slower.
//!
//! Ctrl alone stops the speech, and any key pressed for a program stops
//! what was being said. Pressed twice quickly, Caps Lock or Insert is
//! itself, for the program. While readerd is not running, consoled tries
//! again every [`RETRY`], and the keys are the programs' as always.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use radix_abi::input::*;

use crate::notices::connect_now;
use crate::term::Terminal;

/// readerd's socket.
pub const SOCKET: &str = "/system/live/readerd/socket";

/// How long consoled waits to try readerd again.
const RETRY: Duration = Duration::from_secs(2);

/// How soon a second press of the reader key passes it to the program.
const DOUBLE: Duration = Duration::from_millis(400);

/// How long the shown console must be quiet before what was written
/// last, a line not yet ended, is said: a program's answer to a key, or
/// its output, comes in pieces.
const SETTLE: Duration = Duration::from_millis(80);

/// The most lines of output said at once: more are summed up, so that a
/// long listing does not keep the reader talking for minutes.
const LINES_MAX: usize = 40;

/// The most bytes waiting to go to readerd; more are dropped.
const WAITING_MAX: usize = 64 * 1024;

/// What a key moved onto, to be said once the program has answered it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moved {
    Character,
    Line,
}

/// The screen reader's side of consoled.
pub struct Reader {
    /// The connection to readerd, while it runs.
    stream: Option<UnixStream>,
    /// When to try readerd again.
    retry_at: Instant,
    /// Lines written while the socket was full.
    waiting: Vec<u8>,
    /// The review cursor: a column and a row of the shown console.
    review: (usize, usize),
    /// The reader key is held.
    held: bool,
    /// When the reader key was last pressed alone, for a double press.
    last_alone: Option<Instant>,
    /// What the last key may move onto, if the program writes nothing.
    pending: Option<Moved>,
    /// What was written to the shown console and not said yet: the end
    /// of a line, or more.
    partial: String,
    /// The lines said since the console was last quiet.
    burst: usize,
    /// When the console has been quiet long enough: what is not said yet
    /// is said then.
    settle_at: Option<Instant>,
    /// The prompt last said, so that each is said once.
    said_prompt: Option<u64>,
    /// The bottom line consoled last said: its menu or a note.
    said_bar: Option<String>,
    /// The notification last said.
    said_notice: Option<String>,
}

/// What [`Reader::key`] did with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// It was the reader's: nothing else sees it.
    Taken,
    /// It goes on to the console, as usual.
    Passed,
    /// The reader key pressed twice: it goes on as itself.
    Itself,
}

/// `line` with the blanks at its ends taken off, or None if nothing is
/// left to say.
fn sayable(line: &str) -> Option<&str> {
    let line = line.trim();
    (!line.is_empty() && line.chars().any(|c| !c.is_whitespace())).then_some(line)
}

/// The word of `line` at character `x`, and where it starts: the run of
/// non-blanks around it, or None on a blank.
fn word_at(line: &[char], x: usize) -> Option<(usize, String)> {
    if line.get(x).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let start = (0..=x).rev().take_while(|&i| !line[i].is_whitespace()).last().unwrap_or(x);
    let end = (x..line.len()).take_while(|&i| !line[i].is_whitespace()).last().unwrap_or(x);
    Some((start, line[start..=end].iter().collect()))
}

/// How a character is said on its own: its name, for those eSpeak NG
/// would say nothing for.
fn character(c: char) -> String {
    match c {
        ' ' => "space".to_string(),
        '.' => "dot".to_string(),
        ',' => "comma".to_string(),
        '-' => "dash".to_string(),
        '/' => "slash".to_string(),
        '\\' => "backslash".to_string(),
        '|' => "bar".to_string(),
        '_' => "underscore".to_string(),
        '*' => "star".to_string(),
        '#' => "hash".to_string(),
        '~' => "tilde".to_string(),
        '$' => "dollar".to_string(),
        '"' => "quote".to_string(),
        '\'' => "apostrophe".to_string(),
        c if c.is_uppercase() => format!("capital {c}"),
        c => c.to_string(),
    }
}

impl Reader {
    pub fn new(now: Instant) -> Reader {
        Reader {
            stream: None,
            retry_at: now,
            waiting: Vec::new(),
            review: (0, 0),
            held: false,
            last_alone: None,
            pending: None,
            partial: String::new(),
            burst: 0,
            settle_at: None,
            said_prompt: None,
            said_bar: None,
            said_notice: None,
        }
    }

    /// Whether readerd is listening.
    pub fn connected(&self) -> bool {
        self.stream.is_some()
    }

    /// The socket, to poll for readerd going away.
    pub fn fd(&self) -> i32 {
        self.stream.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }

    /// When consoled should try readerd again, or say what was written
    /// last.
    pub fn deadline(&self) -> Option<Instant> {
        match self.stream {
            None => Some(self.retry_at),
            Some(_) => self.settle_at,
        }
    }

    /// Tries readerd, if it is time: true if it is now connected anew.
    pub fn connect(&mut self, now: Instant) -> bool {
        if self.stream.is_some() || now < self.retry_at {
            return false;
        }
        self.retry_at = now + RETRY;
        match connect_now(SOCKET) {
            Ok(stream) => {
                self.stream = Some(stream);
                self.waiting.clear();
                self.forget();
                self.said_prompt = None;
                self.said_bar = None;
                self.said_notice = None;
                eprintln!("consoled: the screen reader is listening");
                true
            }
            Err(_) => false,
        }
    }

    /// Reads what readerd wrote, which is nothing: at its end, it has
    /// gone.
    pub fn read(&mut self, now: Instant) {
        let Some(stream) = self.stream.as_mut() else { return };
        let mut buffer = [0u8; 256];
        match stream.read(&mut buffer) {
            Ok(0) => {}
            Ok(_) => return,
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                return;
            }
            Err(_) => {}
        }
        self.gone(now);
    }

    fn gone(&mut self, now: Instant) {
        self.stream = None;
        self.waiting.clear();
        self.held = false;
        self.forget();
        self.retry_at = now + RETRY;
        eprintln!("consoled: the screen reader went away");
    }

    /// Sends readerd the line `word text`, or keeps it for when the
    /// socket has room.
    fn send(&mut self, word: &str, text: &str) {
        if self.stream.is_none() {
            return;
        }
        // One line: the text's own line breaks would start others.
        let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let line = if text.is_empty() { format!("{word}\n") } else { format!("{word} {text}\n") };
        if self.waiting.len() + line.len() > WAITING_MAX {
            return;
        }
        self.waiting.extend_from_slice(line.as_bytes());
        self.flush();
    }

    /// Writes what waits, as much as the socket takes.
    pub fn flush(&mut self) {
        let Some(stream) = self.stream.as_mut() else { return };
        while !self.waiting.is_empty() {
            match stream.write(&self.waiting) {
                Ok(0) => break,
                Ok(n) => {
                    self.waiting.drain(..n);
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.gone(Instant::now());
                    break;
                }
            }
        }
    }

    pub fn say(&mut self, text: &str) {
        if let Some(text) = sayable(text) {
            self.send("say", text);
        }
    }

    pub fn now(&mut self, text: &str) {
        if let Some(text) = sayable(text) {
            self.send("now", text);
        }
    }

    pub fn trusted(&mut self, text: &str) {
        if let Some(text) = sayable(text) {
            self.send("trusted", text);
        }
    }

    pub fn stop(&mut self) {
        self.send("stop", "");
    }

    /// Forgets what was written and not said, and what the last key
    /// moved: for another console, or a key consoled itself took.
    pub fn forget(&mut self) {
        self.partial.clear();
        self.pending = None;
        self.burst = 0;
        self.settle_at = None;
    }

    /// Says what was written to the shown console, `term`, since the last
    /// time: each line as it ends, and the rest once the console has been
    /// quiet for [`SETTLE`]; or, if nothing was written, what the last key
    /// moved the cursor onto.
    pub fn written(&mut self, term: &mut Terminal, now: Instant) {
        let text = term.take_spoken();
        let named = term.take_named();
        if !self.connected() {
            return;
        }
        // What the program named has the focus now (ADR 0325): it is said
        // at once, over what was being said, and the review cursor starts
        // at the terminal's cursor again. What it drew with it is its
        // own, and not read.
        if let Some(named) = named {
            self.forget();
            self.review = term.position();
            self.now(&named);
            return;
        }
        if !text.is_empty() {
            // The review cursor starts at the cursor again.
            self.review = term.position();
            self.partial.push_str(&text);
            self.settle_at = Some(now + SETTLE);
            while let Some(end) = self.partial.find('\n') {
                let line: String = self.partial.drain(..=end).collect();
                self.line(&line);
            }
        }
        if self.settle_at.is_none_or(|at| at > now) {
            return;
        }
        self.settle_at = None;
        let rest = std::mem::take(&mut self.partial);
        let moved = self.pending.take();
        if sayable(&rest).is_some() {
            self.line(&rest);
        } else if let Some(moved) = moved
            && self.burst == 0
        {
            self.review = term.position();
            self.moved(term, moved);
        }
        if self.burst > LINES_MAX {
            self.say(&format!("and {} more lines", self.burst - LINES_MAX));
        }
        self.burst = 0;
    }

    /// Says a line written to the console, unless too many were said
    /// since it was last quiet.
    fn line(&mut self, line: &str) {
        let Some(line) = sayable(line) else { return };
        self.burst += 1;
        if self.burst <= LINES_MAX {
            let line = line.to_string();
            self.say(&line);
        }
    }

    /// Says what the cursor of `term` is on: the character, word or line.
    fn moved(&mut self, term: &Terminal, moved: Moved) {
        let (x, y) = term.position();
        let line: Vec<char> = term.row_text(y).chars().collect();
        match moved {
            Moved::Character => {
                let c = line.get(x).copied().unwrap_or(' ');
                self.now(&character(c));
            }
            Moved::Line => self.say_line(&line.iter().collect::<String>()),
        }
    }

    /// Says a line, or "blank".
    fn say_line(&mut self, line: &str) {
        match sayable(line) {
            Some(line) => self.now(line),
            None => self.now("blank"),
        }
    }

    /// Says which console is shown, and its cursor's line.
    pub fn switched(&mut self, number: usize, term: &mut Terminal) {
        // What was written while it was hidden is not said.
        term.take_spoken();
        self.forget();
        self.review = term.position();
        self.now(&format!("Console {number}"));
        let line = term.row_text(term.position().1);
        if let Some(line) = sayable(&line) {
            self.say(line);
        }
    }

    /// Says the whole shown screen: when readerd has just started
    /// listening.
    pub fn screen(&mut self, number: usize, term: &mut Terminal) {
        term.take_spoken();
        self.forget();
        self.review = term.position();
        self.now(&format!("Console {number}"));
        for y in 0..term.rows() {
            let line = term.row_text(y);
            if let Some(line) = sayable(&line) {
                self.say(line);
            }
        }
    }

    /// Says the prompt of consoled's on the shown console once, as
    /// trusted: `id` tells one from the next. None when there is none, so
    /// that a prompt is said again when its console is shown again.
    pub fn prompt(&mut self, prompt: Option<(u64, String)>) {
        let id = prompt.as_ref().map(|(id, _)| *id);
        if self.said_prompt != id {
            self.said_prompt = id;
            if let Some((_, text)) = prompt {
                self.trusted(&text);
            }
        }
    }

    /// Says consoled's own bottom line once, as trusted: its menu, or a
    /// note. None when it is gone.
    pub fn bar(&mut self, text: Option<&str>) {
        // The menu's keys are set apart by runs of spaces: said, by pauses.
        let text = text.map(|text| {
            let parts: Vec<&str> = text
                .split("  ")
                .map(|part| part.trim().trim_end_matches('.'))
                .filter(|s| !s.is_empty())
                .collect();
            parts.join(". ")
        });
        if self.said_bar != text {
            if let Some(text) = &text {
                self.trusted(text);
            }
            self.said_bar = text;
        }
    }

    /// Says a notification once, as it shows: not trusted, as any program
    /// may send one.
    pub fn notice(&mut self, text: Option<&str>) {
        let text = text.map(str::to_string);
        if self.said_notice != text {
            if let Some(text) = &text {
                self.say(text);
            }
            self.said_notice = text;
        }
    }

    /// Takes the key `code` going `value` (input(2)) first, for the
    /// shown console `term`, number `number`: the reader key and the keys
    /// pressed with it are the reader's.
    pub fn key(&mut self, code: u16, value: u16, number: usize, term: &Terminal) -> Key {
        if !self.connected() {
            return Key::Passed;
        }
        let now = Instant::now();
        let pressed = value == PRESSED;
        if matches!(code, KEY_CAPSLOCK | KEY_INSERT) {
            if value == RELEASED {
                self.held = false;
                return Key::Taken;
            }
            if !pressed {
                return Key::Taken;
            }
            if self.last_alone.is_some_and(|at| now.duration_since(at) < DOUBLE) {
                self.last_alone = None;
                self.held = false;
                return Key::Itself;
            }
            self.held = true;
            self.last_alone = Some(now);
            return Key::Taken;
        }
        if !self.held {
            if pressed {
                self.last_alone = None;
                match code {
                    // Ctrl alone stops the speech; it is still Ctrl.
                    KEY_LEFTCTRL | KEY_RIGHTCTRL => self.stop(),
                    KEY_LEFTSHIFT | KEY_RIGHTSHIFT | KEY_LEFTALT | KEY_RIGHTALT => {}
                    _ => {
                        self.stop();
                        // What the program writes in answer is said once
                        // it is quiet; if it writes nothing, what the
                        // cursor moved onto.
                        self.pending = match code {
                            KEY_LEFT | KEY_RIGHT | KEY_BACKSPACE | KEY_DELETE => {
                                Some(Moved::Character)
                            }
                            KEY_UP | KEY_DOWN | KEY_HOME | KEY_END => Some(Moved::Line),
                            _ => None,
                        };
                        self.settle_at = Some(now + SETTLE);
                    }
                }
            }
            return Key::Passed;
        }
        if pressed || value == REPEATED {
            self.last_alone = None;
            self.command(code, number, term);
        }
        Key::Taken
    }

    /// Does what the key `code`, pressed with the reader key, asks.
    fn command(&mut self, code: u16, number: usize, term: &Terminal) {
        let (cols, rows) = (term.cols(), term.rows());
        let (x, y) = self.review;
        let line: Vec<char> = term.row_text(y.min(rows - 1)).chars().collect();
        match code {
            KEY_UP | KEY_DOWN => {
                let y = if code == KEY_UP {
                    if y == 0 {
                        self.now("top");
                        return;
                    }
                    y - 1
                } else {
                    if y + 1 >= rows {
                        self.now("bottom");
                        return;
                    }
                    y + 1
                };
                self.review = (0, y);
                self.say_line(&term.row_text(y));
            }
            KEY_LEFT | KEY_RIGHT => {
                // The next or the last word's start on the line.
                let starts: Vec<usize> = (0..line.len())
                    .filter(|&i| {
                        !line[i].is_whitespace() && (i == 0 || line[i - 1].is_whitespace())
                    })
                    .collect();
                let found = if code == KEY_LEFT {
                    starts.iter().rev().find(|&&s| s < x).copied()
                } else {
                    starts.iter().find(|&&s| s > x).copied()
                };
                match found.and_then(|s| word_at(&line, s)) {
                    Some((start, word)) => {
                        self.review = (start, y);
                        self.now(&word);
                    }
                    None => {
                        self.now(if code == KEY_LEFT { "start of line" } else { "end of line" });
                    }
                }
            }
            KEY_COMMA | KEY_DOT => {
                let x = if code == KEY_COMMA { x.checked_sub(1) } else { Some(x + 1) };
                match x.filter(|&x| x < cols) {
                    Some(x) => {
                        self.review = (x, y);
                        self.now(&character(line.get(x).copied().unwrap_or(' ')));
                    }
                    None => {
                        self.now(if code == KEY_COMMA { "start of line" } else { "end of line" });
                    }
                }
            }
            KEY_L => self.say_line(&line.iter().collect::<String>()),
            KEY_A => {
                self.stop();
                for row in y..rows {
                    let text = term.row_text(row);
                    if let Some(text) = sayable(&text) {
                        self.say(text);
                    }
                }
            }
            KEY_T => self.now(&format!("Console {number}")),
            KEY_H => self.now(
                "Reader keys, with Caps Lock or Insert held: Up and Down, lines. Left and Right, \
                 words. Comma and Period, characters. L, the line again. A, read on from it. T, \
                 which console. Page Up and Page Down, faster and slower. Ctrl stops the speech. \
                 Caps Lock twice is Caps Lock.",
            ),
            KEY_PAGEUP => self.send("faster", ""),
            KEY_PAGEDOWN => self.send("slower", ""),
            _ => {}
        }
    }

    /// Whether the reader key is held now.
    #[cfg(test)]
    fn holding(&self) -> bool {
        self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_found_around_a_column() {
        let line: Vec<char> = "ls -l /system".chars().collect();
        assert_eq!(word_at(&line, 0), Some((0, "ls".to_string())));
        assert_eq!(word_at(&line, 1), Some((0, "ls".to_string())));
        assert_eq!(word_at(&line, 4), Some((3, "-l".to_string())));
        assert_eq!(word_at(&line, 12), Some((6, "/system".to_string())));
        assert_eq!(word_at(&line, 2), None);
        assert_eq!(word_at(&line, 40), None);
    }

    #[test]
    fn characters_are_named_where_speech_would_skip_them() {
        assert_eq!(character(' '), "space");
        assert_eq!(character('A'), "capital A");
        assert_eq!(character('a'), "a");
        assert_eq!(character('/'), "slash");
    }

    #[test]
    fn nothing_is_said_of_blanks() {
        assert_eq!(sayable("   "), None);
        assert_eq!(sayable("  # ls "), Some("# ls"));
    }

    /// A reader connected to one end of a socket pair, and the other end,
    /// where readerd would be.
    fn listening() -> (Reader, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        theirs.set_nonblocking(true).unwrap();
        let mut reader = Reader::new(Instant::now());
        reader.stream = Some(ours);
        (reader, theirs)
    }

    /// What readerd was sent so far.
    fn heard(theirs: &mut UnixStream) -> String {
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
    fn output_is_said_a_line_at_a_time_and_the_rest_once_quiet() {
        let (mut reader, mut theirs) = listening();
        let mut term = Terminal::new(40, 5);
        term.set_speaking(true);
        let now = Instant::now();
        term.feed(b"total 2\r\nboot  sys");
        reader.written(&mut term, now);
        assert_eq!(heard(&mut theirs), "say total 2\n");
        term.feed(b"tem\r\n# ");
        reader.written(&mut term, now);
        assert_eq!(heard(&mut theirs), "say boot  system\n");
        // The prompt, not ended by a line break, once the console is quiet.
        reader.written(&mut term, now + SETTLE / 2);
        assert_eq!(heard(&mut theirs), "");
        reader.written(&mut term, now + SETTLE);
        assert_eq!(heard(&mut theirs), "say #\n");
    }

    #[test]
    fn what_a_program_names_is_said_at_once_and_its_drawing_is_not() {
        let (mut reader, mut theirs) = listening();
        let mut term = Terminal::new(40, 5);
        term.set_speaking(true);
        let now = Instant::now();
        term.feed(b"\x1b[?1049h\x1b[2J  Network  > Disk\x1b]7700;Disk, step 3 of 6\x1b\\");
        reader.written(&mut term, now);
        assert_eq!(heard(&mut theirs), "now Disk, step 3 of 6\n");
        reader.written(&mut term, now + SETTLE);
        assert_eq!(heard(&mut theirs), "");
    }

    #[test]
    fn a_flood_of_output_is_summed_up() {
        let (mut reader, mut theirs) = listening();
        let mut term = Terminal::new(40, 5);
        term.set_speaking(true);
        let now = Instant::now();
        for n in 0..LINES_MAX + 10 {
            term.feed(format!("line {n}\r\n").as_bytes());
        }
        reader.written(&mut term, now);
        reader.written(&mut term, now + SETTLE);
        let heard = heard(&mut theirs);
        assert_eq!(heard.lines().count(), LINES_MAX + 1);
        assert!(heard.ends_with("say and 10 more lines\n"), "{heard}");
    }

    #[test]
    fn a_key_the_program_answers_by_moving_the_cursor_says_where_it_went() {
        let (mut reader, mut theirs) = listening();
        let mut term = Terminal::new(40, 5);
        term.feed(b"# Ls");
        term.set_speaking(true);
        assert_eq!(reader.key(KEY_LEFT, PRESSED, 1, &term), Key::Passed);
        assert_eq!(heard(&mut theirs), "stop\n");
        // The shell moves the cursor back over the s and writes nothing.
        term.feed(b"\x1b[D");
        reader.written(&mut term, Instant::now() + SETTLE);
        assert_eq!(heard(&mut theirs), "now s\n");
        assert_eq!(reader.key(KEY_LEFT, PRESSED, 1, &term), Key::Passed);
        term.feed(b"\x1b[D");
        reader.written(&mut term, Instant::now() + SETTLE);
        assert_eq!(heard(&mut theirs), "stop\nnow capital L\n");
    }

    #[test]
    fn the_reader_key_reviews_the_screen_and_no_program_sees_it() {
        let (mut reader, mut theirs) = listening();
        let mut term = Terminal::new(40, 5);
        term.set_speaking(true);
        term.feed(b"first line\r\nsecond line\r\n# ");
        reader.written(&mut term, Instant::now() + SETTLE);
        heard(&mut theirs);
        assert_eq!(reader.key(KEY_CAPSLOCK, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_UP, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_UP, RELEASED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_UP, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_RIGHT, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_DOT, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_T, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_CAPSLOCK, RELEASED, 1, &term), Key::Taken);
        assert_eq!(
            heard(&mut theirs),
            "now second line\nnow first line\nnow line\nnow i\nnow Console 1\n"
        );
        // Let go, the keys are the programs' again.
        assert_eq!(reader.key(KEY_UP, PRESSED, 1, &term), Key::Passed);
    }

    #[test]
    fn the_reader_key_pressed_twice_is_itself() {
        let (mut reader, _theirs) = listening();
        let term = Terminal::new(40, 5);
        assert_eq!(reader.key(KEY_CAPSLOCK, PRESSED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_CAPSLOCK, RELEASED, 1, &term), Key::Taken);
        assert_eq!(reader.key(KEY_CAPSLOCK, PRESSED, 1, &term), Key::Itself);
        assert!(!reader.holding());
    }

    #[test]
    fn prompts_are_trusted_and_notifications_are_not() {
        let (mut reader, mut theirs) = listening();
        reader.prompt(Some((7, "Elevate. Password for ada, to run ls.".to_string())));
        reader.prompt(Some((7, "Elevate. Password for ada, to run ls.".to_string())));
        reader.notice(Some("backup: Elevate. Password for ada"));
        reader.bar(Some("Ctrl+Alt+Delete    R: restart    Esc: back"));
        assert_eq!(
            heard(&mut theirs),
            "trusted Elevate. Password for ada, to run ls.\n\
             say backup: Elevate. Password for ada\n\
             trusted Ctrl+Alt+Delete. R: restart. Esc: back\n"
        );
        // Shown again after another console, a prompt is said again.
        reader.prompt(None);
        reader.prompt(Some((7, "Elevate.".to_string())));
        assert_eq!(heard(&mut theirs), "trusted Elevate.\n");
    }

    #[test]
    fn without_readerd_every_key_is_the_programs() {
        let term = Terminal::new(20, 5);
        let mut reader = Reader::new(Instant::now());
        assert_eq!(reader.key(KEY_CAPSLOCK, PRESSED, 1, &term), Key::Passed);
        assert!(!reader.holding());
    }
}
