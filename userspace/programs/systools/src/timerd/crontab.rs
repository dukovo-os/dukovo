// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A person's crontab, as crontab(5) writes it (ADR 0478), read the way
//! cronie reads it: comment lines, `NAME=value` lines, and lines of five
//! time fields (or an `@` word) and a command. timerd(8) checks a text
//! with [`parse`] before it keeps it, and crontab(1) uses the same
//! reading to check what an editor saved.
//!
//! A mistake is given with its line and its column, both counted from 1,
//! the column in bytes of the line as it is written.

use manifest::schedule::{self, Calendar};

/// The most bytes a crontab holds.
pub const TEXT_MAX: usize = 64 * 1024;

/// The most lines of commands a crontab holds.
pub const ENTRIES_MAX: usize = 500;

/// The most `NAME=value` lines a crontab holds.
pub const VARIABLES_MAX: usize = 100;

/// A crontab, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Crontab {
    /// Its `NAME=value` lines, in order: each applies to every command,
    /// and a name set twice has its last value.
    pub variables: Vec<(String, String)>,
    /// Its lines of commands, in order.
    pub entries: Vec<Entry>,
}

/// A line of a crontab that runs a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its line in the text, from 1.
    pub line: usize,
    /// When it runs.
    pub when: When,
    /// The time as the line writes it: five fields, or an `@` word.
    pub time: String,
    /// The command `/bin/sh -c` runs: the text up to the first `%` that
    /// is not escaped.
    pub command: String,
    /// What the command reads as its standard input: the text after that
    /// `%`, each further `%` a new line, and a new line at its end.
    pub input: Option<String>,
}

/// When an [`Entry`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// Once as the machine starts.
    Reboot,
    /// At the times a calendar matches.
    At(Calendar),
}

/// What is wrong with a crontab, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mistake {
    /// The line, from 1.
    pub line: usize,
    /// The column, from 1; at the end of the text for a line cut short.
    pub column: usize,
    /// What is wrong, in plain words.
    pub message: String,
}

impl std::fmt::Display for Mistake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)
    }
}

impl Crontab {
    /// The value of the variable `name`, if the crontab sets it.
    #[must_use]
    pub fn variable(&self, name: &str) -> Option<&str> {
        self.variables.iter().rev().find(|(n, _)| n == name).map(|(_, value)| value.as_str())
    }
}

/// Reads the crontab `text`.
///
/// # Errors
///
/// The first mistake in it: a time that does not read, a line with no
/// command, a variable that may not be set, or a text too long.
pub fn parse(text: &str) -> Result<Crontab, Mistake> {
    if text.len() > TEXT_MAX {
        return Err(Mistake {
            line: 1,
            column: 1,
            message: format!("a crontab is at most {} KiB", TEXT_MAX / 1024),
        });
    }
    let mut crontab = Crontab::default();
    for (at, raw) in text.lines().enumerate() {
        let line = at + 1;
        if let Some(column) = raw.find(|c: char| c.is_control() && c != '\t') {
            return Err(Mistake {
                line,
                column: column + 1,
                message: "a control character".into(),
            });
        }
        let body = raw.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let lead = raw.len() - body.len();
        let here =
            |column: usize, message: String| Mistake { line, column: lead + column + 1, message };
        if let Some((name, value, offset)) = variable(body) {
            check_variable(name, value).map_err(|why| here(offset, why))?;
            if crontab.variables.len() >= VARIABLES_MAX {
                return Err(here(0, format!("a crontab sets at most {VARIABLES_MAX} variables")));
            }
            crontab.variables.push((name.to_string(), value.to_string()));
            continue;
        }
        if crontab.entries.len() >= ENTRIES_MAX {
            return Err(here(0, format!("a crontab has at most {ENTRIES_MAX} lines of commands")));
        }
        let (when, rest) = time(body).map_err(|(column, why)| here(column, why))?;
        let command = rest.trim_start();
        if command.is_empty() {
            return Err(here(body.len(), "this line has a time and no command".into()));
        }
        let (command, input) = split_percent(command);
        let time = body[..body.len() - rest.len()].trim().to_string();
        crontab.entries.push(Entry { line, when, time, command, input });
    }
    Ok(crontab)
}

/// The name, the value and the value's offset when `body` is a
/// `NAME=value` line: a name of letters, digits and `_` that does not
/// start with a digit, spaces allowed around the `=`, and a value that is
/// the rest of the line, in quotes or not.
fn variable(body: &str) -> Option<(&str, &str, usize)> {
    let end = body.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(body.len());
    let name = &body[..end];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let after = body[end..].trim_start();
    let value = after.strip_prefix('=')?;
    let at = body.len() - value.len();
    let trimmed = value.trim();
    let at = at + (value.len() - value.trim_start().len());
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|q| trimmed.strip_prefix(*q).and_then(|t| t.strip_suffix(*q)))
        .filter(|_| trimmed.len() >= 2);
    match unquoted {
        Some(inner) => Some((name, inner, at + 1)),
        None => Some((name, trimmed, at)),
    }
}

/// Whether the crontab may set `name` to `value`.
fn check_variable(name: &str, value: &str) -> Result<(), String> {
    match name {
        "SHELL" if value != "/bin/sh" => {
            Err(format!("SHELL is /bin/sh: timerd runs every line with it, and not with `{value}`"))
        }
        "CRON_TZ" => manifest::service::check_time_zone(value),
        _ => Ok(()),
    }
}

/// The time at the start of `body` and what follows it; or the column of
/// the mistake in `body` and what it is.
fn time(body: &str) -> Result<(When, &str), (usize, String)> {
    if let Some(rest) = body.strip_prefix('@') {
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        let word = &rest[..end];
        let when = if word == "reboot" {
            When::Reboot
        } else {
            let calendar = schedule::word(word).ok_or_else(|| {
                (
                    0,
                    format!(
                        "`@{word}` is none of @reboot, @hourly, @daily, @midnight, @weekly, \
                         @monthly, @yearly and @annually"
                    ),
                )
            })?;
            When::At(calendar)
        };
        return Ok((when, &rest[end..]));
    }
    let mut end = 0;
    let mut seen = 0;
    let bytes = body.as_bytes();
    while seen < 5 {
        while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
            end += 1;
        }
        if end == bytes.len() {
            break;
        }
        while end < bytes.len() && bytes[end] != b' ' && bytes[end] != b'\t' {
            end += 1;
        }
        seen += 1;
    }
    let calendar = schedule::parse(&body[..end])?;
    Ok((When::At(calendar), &body[end..]))
}

/// Splits a command at its first `%` that is not escaped by a backslash:
/// the command is the text before it, and the standard input what
/// follows, each further `%` a new line, with a new line at the end. A
/// `\%` is a `%` in either, as cron has it.
fn split_percent(text: &str) -> (String, Option<String>) {
    let mut command = String::new();
    let mut input: Option<String> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let target = input.as_mut().unwrap_or(&mut command);
        match c {
            '\\' if chars.peek() == Some(&'%') => {
                chars.next();
                target.push('%');
            }
            '%' => match &mut input {
                None => input = Some(String::new()),
                Some(input) => input.push('\n'),
            },
            c => target.push(c),
        }
    }
    if let Some(input) = &mut input {
        input.push('\n');
    }
    (command.trim_end().to_string(), input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> Entry {
        parse(text).unwrap().entries.remove(0)
    }

    fn mistake(text: &str) -> (usize, usize) {
        let mistake = parse(text).unwrap_err();
        (mistake.line, mistake.column)
    }

    #[test]
    fn a_line_has_a_time_and_a_command() {
        let line = entry("30 2 * * 1-5 /usr/bin/backup --all\n");
        assert_eq!(line.line, 1);
        assert_eq!(line.when, When::At(schedule::parse("30 2 * * 1-5").unwrap()));
        assert_eq!(line.time, "30 2 * * 1-5");
        assert_eq!(line.command, "/usr/bin/backup --all");
        assert_eq!(line.input, None);
    }

    #[test]
    fn the_words_and_reboot_are_times() {
        assert_eq!(entry("@daily x").when, When::At(schedule::word("daily").unwrap()));
        assert_eq!(entry("@midnight x").when, When::At(schedule::word("daily").unwrap()));
        assert_eq!(entry("@annually x").when, When::At(schedule::word("yearly").unwrap()));
        assert_eq!(entry("@reboot  echo up").when, When::Reboot);
        assert_eq!(entry("@reboot  echo up").command, "echo up");
    }

    #[test]
    fn comments_blank_lines_and_indents_are_skipped_and_lines_counted() {
        let crontab =
            parse("# a comment\n\n   # indented\n  * * * * * first\n\t@hourly second\n").unwrap();
        let lines: Vec<(usize, &str)> =
            crontab.entries.iter().map(|e| (e.line, e.command.as_str())).collect();
        assert_eq!(lines, [(4, "first"), (5, "second")]);
        assert_eq!(parse("").unwrap(), Crontab::default());
        assert!(parse("# only\n").unwrap().entries.is_empty());
    }

    #[test]
    fn a_comment_after_a_command_is_part_of_it() {
        assert_eq!(entry("* * * * * echo hi # not a comment").command, "echo hi # not a comment");
    }

    #[test]
    fn variables_are_read_with_or_without_spaces_and_quotes() {
        let crontab = parse(
            "MAILTO=alice@example.org\nPATH = /bin:/usr/bin\nHOME=\"/home/a b\"\nX='y'\nEMPTY=\n\
             * * * * * true\n",
        )
        .unwrap();
        assert_eq!(crontab.variable("MAILTO"), Some("alice@example.org"));
        assert_eq!(crontab.variable("PATH"), Some("/bin:/usr/bin"));
        assert_eq!(crontab.variable("HOME"), Some("/home/a b"));
        assert_eq!(crontab.variable("X"), Some("y"));
        assert_eq!(crontab.variable("EMPTY"), Some(""));
        assert_eq!(crontab.variable("NOPE"), None);
        assert_eq!(parse("A=1\nA=2\n").unwrap().variable("A"), Some("2"));
        assert_eq!(parse("SHELL=/bin/sh\n").unwrap().variable("SHELL"), Some("/bin/sh"));
    }

    #[test]
    fn a_time_line_is_not_a_variable() {
        assert_eq!(entry("* * * * * A=1 run").command, "A=1 run");
        assert_eq!(entry("0 0 * * * x=y").command, "x=y");
    }

    #[test]
    fn a_percent_ends_the_command_and_starts_its_input() {
        let line = entry("0 0 * * * mail -s hi bob%Hello%%Bye%");
        assert_eq!(line.command, "mail -s hi bob");
        assert_eq!(line.input.as_deref(), Some("Hello\n\nBye\n\n"));
        let line = entry("0 0 * * * cat%one");
        assert_eq!(line.input.as_deref(), Some("one\n"));
        let line = entry("0 0 * * * date +\\%Y\\%m\\%d");
        assert_eq!(line.command, "date +%Y%m%d");
        assert_eq!(line.input, None);
        let line = entry("0 0 * * * cat%a\\%b");
        assert_eq!(line.input.as_deref(), Some("a%b\n"));
        // Other backslashes stay.
        assert_eq!(entry("0 0 * * * echo \\n").command, "echo \\n");
    }

    #[test]
    fn a_mistake_has_its_line_and_column() {
        assert_eq!(mistake("* * * * * ok\n61 * * * * bad\n"), (2, 1));
        assert_eq!(mistake("* * * * * ok\n\n * 24 * * * bad\n"), (3, 4));
        assert_eq!(mistake("* * * * *\n"), (1, 10));
        assert_eq!(mistake("* * * *\n"), (1, 8));
        assert_eq!(mistake("@sometimes x\n"), (1, 1));
        assert_eq!(mistake("  @weekly\n"), (1, 10));
        assert_eq!(mistake("* * * * * x\n0 0 * Foo * y\n"), (2, 7));
        assert_eq!(mistake("SHELL=/bin/bash\n"), (1, 7));
        assert_eq!(mistake("CRON_TZ=../x\n"), (1, 9));
        assert_eq!(mistake("* * * * * a\u{7}b\n"), (1, 12));
        assert!(
            parse("* * * * * x\n61 * * * * y\n")
                .unwrap_err()
                .to_string()
                .starts_with("line 2, column 1:")
        );
    }

    #[test]
    fn a_crontab_has_limits() {
        assert!(parse(&"# x\n".repeat(TEXT_MAX)).is_err());
        let many = "* * * * * x\n".repeat(ENTRIES_MAX + 1);
        assert_eq!(mistake(&many), (ENTRIES_MAX + 1, 1));
        assert!(parse(&"* * * * * x\n".repeat(ENTRIES_MAX)).is_ok());
        let many = "A=1\n".repeat(VARIABLES_MAX + 1);
        assert_eq!(mistake(&many), (VARIABLES_MAX + 1, 1));
    }
}
