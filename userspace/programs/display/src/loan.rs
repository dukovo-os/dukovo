// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lines of consoled's loan of the screens (consoled(8), ADR 0713),
//! as both ends write and read them.
//!
//! A program that draws the screens itself connects to [`SOCKET`], and
//! connecting is the request. consoled answers with one line, an
//! [`Answer`]. With [`Answer::Lent`] comes a descriptor (SCM_RIGHTS): a
//! holder of _/devices/display/control_ of the borrower's own, which is
//! in front, and the number of the console the borrower stands in, which
//! is the one that showed. The connection then stays open for as long as
//! the borrower wants that console: it writes an [`Ask`] before it gives
//! the screens back for another console, and consoled lends them to it
//! again when the person chooses its console. A borrower that closes the
//! connection has the screens until it gives them back or ends, and is
//! not lent them again.
//!
//! Every line ends with a newline and is no longer than [`LINE_MAX`].

/// Where consoled listens for a program that asks for the screens.
pub const SOCKET: &str = "/system/live/consoled/display";

/// The bytes of the longest line, with its newline.
pub const LINE_MAX: usize = 32;

/// The consoles a line may name: 1 to this.
pub const CONSOLES_MAX: u32 = 12;

/// A console's number, if `word` is one.
fn console(word: &str) -> Option<u32> {
    // No sign, no space and no leading zero: one way to write each.
    let plain = !word.starts_with('0') && word.bytes().all(|byte| byte.is_ascii_digit());
    word.parse().ok().filter(|number| plain && (1..=CONSOLES_MAX).contains(number))
}

/// What consoled answers a program that asked for the screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The screens are the asker's, which stands in the console of this
    /// number. Its holder's descriptor comes with the line.
    Lent(u32),
    /// consoled has not the screens to lend: another program has
    /// borrowed them, or no screen shows the consoles.
    Busy,
    /// The loan could not be made; consoled's log says why.
    Failed,
    /// The asker is not root, and was not started at the console that
    /// shows.
    Refused,
}

impl Answer {
    /// The line that says it.
    pub fn line(self) -> String {
        match self {
            Answer::Lent(console) => format!("lent {console}\n"),
            Answer::Busy => "busy\n".to_owned(),
            Answer::Failed => "failed\n".to_owned(),
            Answer::Refused => "refused\n".to_owned(),
        }
    }

    /// The answer `line` is, with or without its newline, if it is one.
    pub fn parse(line: &str) -> Option<Answer> {
        match line.strip_suffix('\n').unwrap_or(line) {
            "busy" => Some(Answer::Busy),
            "failed" => Some(Answer::Failed),
            "refused" => Some(Answer::Refused),
            lent => lent.strip_prefix("lent ").and_then(console).map(Answer::Lent),
        }
    }
}

/// What a borrower asks of consoled on the connection it kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// The console of this number is to show when the borrower gives the
    /// screens back, which it does next.
    Console(u32),
}

impl Ask {
    /// The line that says it.
    pub fn line(self) -> String {
        match self {
            Ask::Console(console) => format!("console {console}\n"),
        }
    }

    /// What `line` asks, with or without its newline, if it is a line a
    /// borrower may send.
    pub fn parse(line: &str) -> Option<Ask> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        line.strip_prefix("console ").and_then(console).map(Ask::Console)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_is_read_as_what_it_was_written_for() {
        let answers = [Answer::Lent(1), Answer::Lent(12), Answer::Busy, Answer::Failed];
        for answer in answers.into_iter().chain([Answer::Refused]) {
            let line = answer.line();
            assert!(line.ends_with('\n') && line.len() <= LINE_MAX, "{line:?}");
            assert_eq!(Answer::parse(&line), Some(answer));
            assert_eq!(Answer::parse(line.trim_end()), Some(answer));
        }
        for console in 1..=CONSOLES_MAX {
            let line = Ask::Console(console).line();
            assert!(line.len() <= LINE_MAX);
            assert_eq!(Ask::parse(&line), Some(Ask::Console(console)));
        }
    }

    #[test]
    fn a_line_that_is_none_of_them_is_nothing() {
        for line in ["", "lent", "lent ", "lent 0", "lent 13", "lent 01", "lent +1", "lent 1 2"] {
            assert_eq!(Answer::parse(line), None, "{line:?}");
        }
        assert_eq!(Answer::parse("lent 4294967297"), None);
        assert_eq!(Answer::parse("Busy"), None);
        assert_eq!(Answer::parse("busy\n\n"), None);
        for line in ["console", "console 0", "console 13", "console -1", "console 2 ", "lent 2"] {
            assert_eq!(Ask::parse(line), None, "{line:?}");
        }
    }
}
