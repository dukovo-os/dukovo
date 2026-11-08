// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A terminal at the far end of the serial port, for the boot tests
//! that read what programs draw on `/devices/ttyS0` (ADR 0318): the
//! installer's screens move the cursor and redraw only the cells that
//! change, so the log's text alone does not say what a person would see.
//!
//! It keeps a grid of characters and the cursor, and knows what
//! crossterm, ratatui, Nukesh and the line editors send: printing with
//! xterm's wrap at the last column, carriage return, line feed,
//! backspace and tab; cursor movement, saving and restoring; erasing in
//! the display and the line; inserting and deleting characters and
//! lines; scrolling and scrolling regions; and the alternate screen. It
//! answers a cursor report, `ESC [ 6 n`, as a terminal does, with where
//! its cursor is: serial-console asks the terminal's size that way.
//! Colours, attributes and window titles are read and passed over.

/// Where the parser is in an escape sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Text.
    Ground,
    /// After ESC.
    Escape,
    /// In a control sequence, after `ESC [`, with its parameters so far.
    Csi(String),
    /// In an operating system command or another string, which BEL or
    /// `ESC \` ends.
    String,
    /// After ESC in a string.
    StringEscape,
    /// After `ESC (` and the like, before the character set's name.
    Charset,
}

/// The terminal.
pub struct Terminal {
    cols: usize,
    rows: usize,
    /// The grid, a row after another.
    cells: Vec<char>,
    row: usize,
    col: usize,
    /// The last character went in the last column: the next one goes to
    /// the start of the next line first, as on xterm.
    wrap_next: bool,
    /// The cursor `ESC 7` saved.
    saved: (usize, usize),
    /// The scrolling region's first and last rows.
    top: usize,
    bottom: usize,
    /// The main screen's grid and cursor while the alternate one shows.
    main: Option<(Vec<char>, usize, usize)>,
    state: State,
    /// The bytes of a character not yet whole.
    partial: Vec<u8>,
    /// What the terminal answers, for the caller to send back.
    replies: Vec<u8>,
}

impl Terminal {
    /// A blank terminal of `cols` by `rows`.
    pub fn new(cols: usize, rows: usize) -> Terminal {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Terminal {
            cols,
            rows,
            cells: vec![' '; cols * rows],
            row: 0,
            col: 0,
            wrap_next: false,
            saved: (0, 0),
            top: 0,
            bottom: rows - 1,
            main: None,
            state: State::Ground,
            partial: Vec::new(),
            replies: Vec::new(),
        }
    }

    /// Takes what the terminal answered since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Each row's text, without the spaces at its end.
    pub fn lines(&self) -> Vec<String> {
        self.cells
            .chunks(self.cols)
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect()
    }

    /// Takes in `bytes` as the serial port carried them: UTF-8 whose
    /// characters may be cut between two calls.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut buffer = std::mem::take(&mut self.partial);
        buffer.extend_from_slice(bytes);
        let mut rest = buffer.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.feed_text(text);
                    return;
                }
                Err(error) => {
                    let (good, bad) = rest.split_at(error.valid_up_to());
                    // from_utf8 found these bytes to be UTF-8.
                    self.feed_text(std::str::from_utf8(good).unwrap_or_default());
                    match error.error_len() {
                        // An invalid sequence shows as one replacement.
                        Some(len) => {
                            self.feed_text("\u{fffd}");
                            rest = &bad[len..];
                        }
                        // A character cut at the end: kept for the next call.
                        None => {
                            self.partial = bad.to_vec();
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Takes in `text`.
    fn feed_text(&mut self, text: &str) {
        for c in text.chars() {
            self.feed_char(c);
        }
    }

    fn feed_char(&mut self, c: char) {
        match std::mem::replace(&mut self.state, State::Ground) {
            State::Ground => self.ground(c),
            State::Escape => self.escape(c),
            State::Csi(mut params) => {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    self.control(&params, c);
                } else if params.len() < 64 {
                    params.push(c);
                    self.state = State::Csi(params);
                } else {
                    // Too long to be one a program meant: passed over.
                }
            }
            State::String => {
                self.state = match c {
                    '\u{7}' => State::Ground,
                    '\u{1b}' => State::StringEscape,
                    _ => State::String,
                };
            }
            // ESC \ ends the string; ESC with anything else ends it too,
            // and starts what follows.
            State::StringEscape if c == '\\' => {}
            State::StringEscape => self.escape(c),
            State::Charset => {}
        }
    }

    /// A character outside any escape sequence.
    fn ground(&mut self, c: char) {
        match c {
            '\u{1b}' => self.state = State::Escape,
            '\r' => self.move_to(self.row, 0),
            '\n' | '\u{b}' | '\u{c}' => self.line_feed(),
            '\u{8}' => self.move_to(self.row, self.col.saturating_sub(1)),
            '\t' => self.move_to(self.row, ((self.col / 8) + 1) * 8),
            c if c.is_control() => {}
            c => self.print(c),
        }
    }

    /// The character after ESC.
    fn escape(&mut self, c: char) {
        match c {
            '[' => self.state = State::Csi(String::new()),
            ']' | 'P' | 'X' | '^' | '_' => self.state = State::String,
            '(' | ')' | '*' | '+' | '#' | '%' => self.state = State::Charset,
            '7' => self.saved = (self.row, self.col),
            '8' => self.move_to(self.saved.0, self.saved.1),
            'D' => self.line_feed(),
            'E' => {
                self.move_to(self.row, 0);
                self.line_feed();
            }
            'M' => self.reverse_line_feed(),
            'c' => *self = Terminal::new(self.cols, self.rows),
            _ => {}
        }
    }

    /// The control sequence `ESC [ params final`.
    fn control(&mut self, params: &str, last: char) {
        let private = params.starts_with(['?', '>', '=', '<']);
        let numbers: Vec<usize> = params
            .trim_start_matches(['?', '>', '=', '<'])
            .split(';')
            .map(|word| word.parse().unwrap_or(0))
            .collect();
        // The nth parameter, `default` when it is missing or 0.
        let arg = |n: usize, default: usize| {
            numbers.get(n).copied().filter(|&value| value != 0).unwrap_or(default)
        };
        let (row, col) = (self.row, self.col);
        match last {
            'A' => self.move_to(row.saturating_sub(arg(0, 1)), col),
            'B' | 'e' => self.move_to(row + arg(0, 1), col),
            'C' | 'a' => self.move_to(row, col + arg(0, 1)),
            'D' => self.move_to(row, col.saturating_sub(arg(0, 1))),
            'E' => self.move_to(row + arg(0, 1), 0),
            'F' => self.move_to(row.saturating_sub(arg(0, 1)), 0),
            'G' | '`' => self.move_to(row, arg(0, 1) - 1),
            'd' => self.move_to(arg(0, 1) - 1, col),
            'H' | 'f' => self.move_to(arg(0, 1) - 1, arg(1, 1) - 1),
            'J' => match numbers.first().copied().unwrap_or(0) {
                0 => self.erase(row * self.cols + col, self.cells.len()),
                1 => self.erase(0, row * self.cols + col + 1),
                _ => self.erase(0, self.cells.len()),
            },
            'K' => {
                let start = row * self.cols;
                match numbers.first().copied().unwrap_or(0) {
                    0 => self.erase(start + col, start + self.cols),
                    1 => self.erase(start, start + col + 1),
                    _ => self.erase(start, start + self.cols),
                }
            }
            'X' => {
                let at = row * self.cols + col;
                self.erase(at, at + arg(0, 1).min(self.cols - col));
            }
            'P' => self.shift_line(arg(0, 1), false),
            '@' => self.shift_line(arg(0, 1), true),
            'L' if (self.top..=self.bottom).contains(&row) => {
                self.scroll_down(row, arg(0, 1));
            }
            'M' if (self.top..=self.bottom).contains(&row) => {
                self.scroll_up(row, arg(0, 1));
            }
            'S' => self.scroll_up(self.top, arg(0, 1)),
            'T' if !private => self.scroll_down(self.top, arg(0, 1)),
            'r' if !private => {
                let (top, bottom) = (arg(0, 1) - 1, arg(1, self.rows) - 1);
                if top < bottom && bottom < self.rows {
                    (self.top, self.bottom) = (top, bottom);
                    self.move_to(0, 0);
                }
            }
            's' if !private => self.saved = (row, col),
            'u' if !private => self.move_to(self.saved.0, self.saved.1),
            'h' | 'l' if private => {
                if numbers.iter().any(|&mode| matches!(mode, 47 | 1047 | 1049)) {
                    self.alternate(last == 'h');
                }
            }
            'n' if !private && numbers.first() == Some(&6) => {
                let report = format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                self.replies.extend_from_slice(report.as_bytes());
            }
            // Colours and attributes, modes, other reports: nothing to show.
            _ => {}
        }
    }

    /// Puts `c` at the cursor and moves it on.
    fn print(&mut self, c: char) {
        if self.wrap_next {
            self.col = 0;
            self.line_feed();
        }
        self.cells[self.row * self.cols + self.col] = c;
        if self.col + 1 == self.cols {
            self.wrap_next = true;
        } else {
            self.col += 1;
        }
    }

    /// Moves the cursor to `row` and `col`, kept on the screen.
    fn move_to(&mut self, row: usize, col: usize) {
        self.row = row.min(self.rows - 1);
        self.col = col.min(self.cols - 1);
        self.wrap_next = false;
    }

    /// Down a row, scrolling the region at its bottom.
    fn line_feed(&mut self) {
        self.wrap_next = false;
        if self.row == self.bottom {
            self.scroll_up(self.top, 1);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    /// Up a row, scrolling the region down at its top.
    fn reverse_line_feed(&mut self) {
        self.wrap_next = false;
        if self.row == self.top {
            self.scroll_down(self.top, 1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    /// Blanks cells `from` to `to`, a range of the grid.
    fn erase(&mut self, from: usize, to: usize) {
        let to = to.min(self.cells.len());
        if from < to {
            self.cells[from..to].fill(' ');
        }
    }

    /// Moves rows `from` to the region's bottom up by `n`, blanking the
    /// rows that come in at the bottom.
    fn scroll_up(&mut self, from: usize, n: usize) {
        let (cols, end) = (self.cols, (self.bottom + 1) * self.cols);
        let n = n.min(self.bottom + 1 - from);
        self.cells.copy_within(from * cols + n * cols..end, from * cols);
        self.erase(end - n * cols, end);
    }

    /// Moves rows `from` to the region's bottom down by `n`, blanking
    /// the rows that come in at `from`.
    fn scroll_down(&mut self, from: usize, n: usize) {
        let (cols, end) = (self.cols, (self.bottom + 1) * self.cols);
        let n = n.min(self.bottom + 1 - from);
        self.cells.copy_within(from * cols..end - n * cols, from * cols + n * cols);
        self.erase(from * cols, from * cols + n * cols);
    }

    /// Inserts `n` blanks at the cursor (`insert`), or deletes `n`
    /// characters there, within the line.
    fn shift_line(&mut self, n: usize, insert: bool) {
        let start = self.row * self.cols;
        let (at, end) = (start + self.col, start + self.cols);
        let n = n.min(end - at);
        if insert {
            self.cells.copy_within(at..end - n, at + n);
            self.erase(at, at + n);
        } else {
            self.cells.copy_within(at + n..end, at);
            self.erase(end - n, end);
        }
        self.wrap_next = false;
    }

    /// Shows the alternate screen, blank, keeping the main one and its
    /// cursor (`on`), or the main one again.
    fn alternate(&mut self, on: bool) {
        match (on, self.main.take()) {
            (true, None) => {
                let blank = vec![' '; self.cells.len()];
                self.main = Some((std::mem::replace(&mut self.cells, blank), self.row, self.col));
            }
            (false, Some((cells, row, col))) => {
                self.cells = cells;
                self.move_to(row, col);
            }
            (_, kept) => self.main = kept,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(cols: usize, rows: usize, text: &str) -> Vec<String> {
        let mut terminal = Terminal::new(cols, rows);
        terminal.feed(text.as_bytes());
        terminal.lines()
    }

    #[test]
    fn text_wraps_and_scrolls_as_on_xterm() {
        assert_eq!(shown(4, 2, "abcd"), ["abcd", ""]);
        assert_eq!(shown(4, 2, "abcde"), ["abcd", "e"]);
        assert_eq!(shown(4, 2, "one\r\ntwo\r\nsix"), ["two", "six"]);
        // A tab goes to the next stop, every eighth column, or the last.
        assert_eq!(shown(10, 1, "ab\tc\x08d"), ["ab      d"]);
        assert_eq!(shown(8, 1, "ab\tc\x08d"), ["ab    dc"]);
    }

    #[test]
    fn cells_are_drawn_where_the_cursor_is_sent() {
        // As ratatui redraws only what changed.
        let lines = shown(10, 3, "\x1b[2J\x1b[1;1Hhello\x1b[3;6Hworld\x1b[1;2HE");
        assert_eq!(lines, ["hEllo", "", "     world"]);
        let lines = shown(10, 2, "abcdefgh\x1b[1;3H\x1b[K\x1b[2;1Hxyz\x1b[1G\x1b[2K");
        assert_eq!(lines, ["ab", ""]);
        assert_eq!(shown(10, 1, "abcdef\x1b[1;2H\x1b[2P"), ["adef"]);
        assert_eq!(shown(10, 1, "abcdef\x1b[1;2H\x1b[2@"), ["a  bcdef"]);
        assert_eq!(shown(10, 1, "abcdef\x1b[1;2H\x1b[2X"), ["a  def"]);
    }

    #[test]
    fn colours_titles_and_modes_show_nothing() {
        let lines = shown(
            20,
            2,
            "\x1b]4;0;rgb:15/18/1c\x07\x1b]104\x1b\\\x1b[?25l\x1b[38;5;3;1mok\x1b[0m\x1b(B",
        );
        assert_eq!(lines, ["ok", ""]);
    }

    #[test]
    fn the_alternate_screen_gives_the_main_one_back() {
        let lines = shown(10, 2, "shell$ \x1b[?1049h\x1b[Hscreens\x1b[?1049lls");
        assert_eq!(lines, ["shell$ ls", ""]);
    }

    #[test]
    fn a_scrolling_region_scrolls_alone() {
        let lines = shown(5, 4, "top\x1b[2;3r\x1b[2;1Ha\r\nb\r\nc\x1b[4;1Hend");
        assert_eq!(lines, ["top", "b", "c", "end"]);
    }

    #[test]
    fn a_cursor_report_says_where_the_cursor_is() {
        let mut terminal = Terminal::new(100, 30);
        terminal.feed(b"\x1b7\x1b[999;999H\x1b[6n\x1b8");
        assert_eq!(terminal.take_replies(), b"\x1b[30;100R");
        terminal.feed(b"\x1b[2;5H\x1b[6n");
        assert_eq!(terminal.take_replies(), b"\x1b[2;5R");
        assert!(terminal.take_replies().is_empty());
    }

    #[test]
    fn characters_cut_between_reads_are_whole() {
        let mut terminal = Terminal::new(10, 1);
        let check = "✓ done".as_bytes();
        terminal.feed(&check[..2]);
        terminal.feed(&check[2..]);
        assert_eq!(terminal.lines(), ["✓ done"]);
    }
}
