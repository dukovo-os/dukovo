// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dukovo's files that describe programs, read the same way by everything
//! that reads them (ADR 0216): service files (service.toml(5)), which
//! Spark and the service command read, and the manifests of bundles
//! (manifest.json(5), feature `bundle`), whose services take the same
//! settings; the shares file (shares.toml(5)), which share(1) writes
//! and smbd(8) reads; and features (feature.toml(5)), which feature(8)
//! adds and the installer offers.
//!
//! People write TOML, with comments; tools write JSON. Both come through
//! the same types, so the two can never drift apart. An error says which
//! line is wrong and why, in plain words.

#![no_std]

extern crate alloc;

#[cfg(feature = "bundle")]
pub mod bundle;
pub mod feature;
pub mod legacy;
pub mod service;
pub mod shares;
#[cfg(test)]
mod tests;

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

/// What is wrong with a file, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The line, from 1, when the error is on one.
    pub line: Option<usize>,
    /// The column, from 1, in characters, when the error is at one.
    pub column: Option<usize>,
    /// How many characters the error covers from `column` on its line, at
    /// least 1 when `column` is some.
    pub width: usize,
    /// Whether `line`, `column` and `width` are in the file's override
    /// (ADR 0386), which sets the setting that is wrong, and not in the
    /// file itself.
    pub in_override: bool,
    /// The setting it is about, as a dotted path (`health.every`), when it
    /// is about one.
    pub key: Option<String>,
    /// What is wrong, in plain words.
    pub message: String,
    /// What to do about it, when there is one thing to do: for a setting
    /// written wrongly, what it was probably meant to be, from the text
    /// alone.
    pub fix: Option<String>,
    /// Whether the file reads, but its settings do not fit together, and
    /// not a setting is written wrongly. Display, the line that Spark
    /// logs, says what is wrong and the fix of the first, with no line,
    /// and the line and what is wrong of the second, with no fix.
    pub checked: bool,
}

/// A file's path and text, for [`Error::report`].
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    /// Where the file is, as the report names it.
    pub path: &'a str,
    /// What the file says.
    pub text: &'a str,
}

impl Error {
    /// An error on no line in particular.
    pub fn new(message: impl Into<String>) -> Error {
        Error {
            line: None,
            column: None,
            width: 0,
            in_override: false,
            key: None,
            message: message.into(),
            fix: None,
            checked: false,
        }
    }

    /// An error on line `line`.
    pub fn at(line: usize, message: impl Into<String>) -> Error {
        Error { line: Some(line), ..Error::new(message) }
    }

    /// The same error with `fix` as the one thing to do about it.
    #[must_use]
    pub fn with_fix(self, fix: impl Into<String>) -> Error {
        Error { fix: Some(fix.into()), ..self }
    }

    /// The same error about the setting `key`, a dotted path.
    #[must_use]
    pub fn with_key(self, key: impl Into<String>) -> Error {
        Error { key: Some(key.into()), ..self }
    }

    /// The same error at `span` of `text`.
    #[must_use]
    pub(crate) fn on(self, text: &str, span: Range<usize>) -> Error {
        let start = span.start.min(text.len());
        let line_start =
            text.as_bytes()[..start].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let line_end = text.as_bytes()[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(text.len(), |i| start + i);
        let end = span.end.clamp(start, line_end);
        Error {
            line: Some(line_of(text, start)),
            column: Some(chars(text, line_start, start) + 1),
            width: chars(text, start, end).max(1),
            ..self
        }
    }

    /// The error `toml` found in `text`, with its place, and its message in
    /// the words service.toml(5) uses.
    fn from_toml(text: &str, err: &toml::de::Error) -> Error {
        Error::from_message(text, err.message(), err.span())
    }

    /// The error that `message` says, found at `span` of `text`.
    fn from_message(text: &str, message: &str, span: Option<Range<usize>>) -> Error {
        let error = Error::new(plain(message));
        // A whole table's span is where the table starts: for the file
        // itself, line 1, which says nothing.
        let error = match span.filter(|span| span.start > 0 || span.end < text.len()) {
            Some(span) => error.on(text, span),
            None => error,
        };
        Error { fix: did_you_mean(message), ..error }
    }

    /// The file the error's place is in.
    fn found_in<'a>(&self, file: Source<'a>, over: Option<Source<'a>>) -> Source<'a> {
        match over {
            Some(over) if self.in_override => over,
            _ => file,
        }
    }

    /// The first line of the report: where it is and what is wrong.
    fn head(&self, path: &str) -> String {
        let at = match (self.line, self.column) {
            (Some(line), Some(column)) => format!("{path}:{line}:{column}"),
            (Some(line), None) => format!("{path}:{line}"),
            (None, _) => path.to_owned(),
        };
        format!("{at}: {}", self.message)
    }

    /// The report as one line: where, what is wrong, and the fix.
    /// `over` is the file's override, when it has one and this error may
    /// be in it.
    #[must_use]
    pub fn brief(&self, file: Source<'_>, over: Option<Source<'_>>) -> String {
        let mut said = self.head(self.found_in(file, over).path);
        if let Some(fix) = &self.fix {
            said.push_str(": ");
            said.push_str(fix);
        }
        said
    }

    /// The report, as a compiler gives one: where, what is wrong, the line
    /// itself with carets under the place, and the fix. `file` is the file
    /// read and `over` its override, when it has one: an error in the
    /// override is shown there and says which file it is set over.
    #[must_use]
    pub fn report(&self, file: Source<'_>, over: Option<Source<'_>>) -> String {
        let at = self.found_in(file, over);
        let mut said = self.head(at.path);
        let gutter = " ".repeat(self.line.map_or(1, |line| line.to_string().len()) + 1);
        if let Some(line) = self.line {
            let shown = at.text.lines().nth(line.saturating_sub(1)).unwrap_or("");
            said.push_str(&format!("\n{gutter}|\n{line} | {shown}"));
            if let Some(column) = self.column {
                // Tabs stay tabs, so that the carets line up under a line
                // that is indented with them.
                let indent: String = shown
                    .chars()
                    .take(column - 1)
                    .map(|c| if c == '\t' { '\t' } else { ' ' })
                    .collect();
                said.push_str(&format!("\n{gutter}| {indent}{}", "^".repeat(self.width.max(1))));
            }
        }
        if let Some(fix) = &self.fix {
            said.push_str(&format!("\n{gutter}= fix: {fix}"));
        }
        if self.in_override {
            said.push_str(&format!("\n{gutter}= note: set over {}", file.path));
        }
        said
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.checked, self.in_override) {
            (Some(line), false, false) => write!(f, "line {line}: ")?,
            (Some(line), false, true) => write!(f, "line {line} of its override: ")?,
            _ => {}
        }
        f.write_str(&self.message)?;
        match &self.fix {
            Some(fix) if self.checked => write!(f, ": {fix}"),
            _ => Ok(()),
        }
    }
}

/// How many characters of `text` lie from byte `from` to byte `to`.
fn chars(text: &str, from: usize, to: usize) -> usize {
    text.get(from..to).map_or(to - from, |part| part.chars().count())
}

/// The line, from 1, that byte `at` of `text` is on.
fn line_of(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.as_bytes()[..at].iter().filter(|&&b| b == b'\n').count() + 1
}

/// `message` from serde or toml, in plain words: on one line, with Rust's
/// names of types as service.toml(5) says them.
pub(crate) fn plain(message: &str) -> String {
    if let Some(rest) = message.strip_prefix("unknown field `")
        && let Some((name, _)) = rest.split_once('`')
    {
        return format!("`{name}` is not a setting here; service.toml(5) lists them");
    }
    let mut text = message.trim().replace('\n', "; ");
    for (rust, words) in [
        ("expected u32", "expected a whole number"),
        ("expected u16", "expected a whole number"),
        ("expected a boolean", "expected true or false"),
        ("expected a sequence", "expected a list such as [\"a\", \"b\"]"),
        ("expected a string", "expected text in double quotes"),
        ("expected a map", "expected a table"),
    ] {
        text = text.replace(rust, words);
    }
    text
}

/// "did you mean `x`?" for what serde says of a key it does not know,
/// when one of the keys it expected is near enough.
fn did_you_mean(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown field `")?;
    let (name, expected) = rest.split_once('`')?;
    // serde lists the keys it expected between backticks, after the one
    // it did not know.
    let near = nearest(name, expected.split('`').skip(1).step_by(2))?;
    Some(format!("did you mean `{near}`?"))
}

/// The name of `expected` nearest to `name`, if it is at most two edits
/// away: the first of those that tie.
pub(crate) fn nearest<'a>(name: &str, expected: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in expected {
        let distance = edits(name, candidate);
        if distance <= 2 && best.is_none_or(|(least, _)| distance < least) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// How many characters must be added, removed or changed to make `a`
/// into `b`.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    // Row `i` holds the distances from the first `i` characters of `a`
    // to each prefix of `b`.
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if x == *y { diagonal } else { 1 + diagonal.min(above).min(row[j]) };
            diagonal = above;
        }
    }
    row[b.len()]
}
