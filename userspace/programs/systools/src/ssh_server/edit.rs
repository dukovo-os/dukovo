// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The edit `ssh-server set` makes to sshd_config(5): the file's text and
//! a change in, the new text or a refusal out. It reads and writes no
//! file, so that its cases (`cases.txt`, which the Security page's model
//! reads too) run on the host, and so that a tried change and an applied
//! one make the same text.
//!
//! sshd keeps the first value of a keyword given twice, so a flag is set
//! on its first line; the names of AllowUsers and AllowGroups add up over
//! all their lines, so a list is set on its first line and its later
//! lines go. A keyword the file does not have is added at the end, and
//! one that already says what is asked is left as it is. Every other
//! line, comment and line end stays byte for byte; a line that is
//! changed keeps its indent and the case of its keyword, and loses its
//! trailing comment, since the comment spoke of the old value.
//!
//! When a change does not do what was asked, `ssh-server show` says what
//! the file reads as, and `sshd -T` what sshd carries out.

use accounts::sshd_config::{self, Mistake};

/// The names a list is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Names {
    /// No list: anyone whose account is not denied may sign in.
    Everyone,
    /// Only these.
    Only(Vec<String>),
}

/// What `set` is asked to change: each field is left as it is when none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Change {
    /// Whether a password signs a person in (PasswordAuthentication).
    pub(crate) passwords: Option<bool>,
    /// Whether a key signs a person in (PubkeyAuthentication).
    pub(crate) keys: Option<bool>,
    /// Who may sign in, by account (AllowUsers).
    pub(crate) allow_users: Option<Names>,
    /// Who may sign in, by group (AllowGroups).
    pub(crate) allow_groups: Option<Names>,
}

/// Why a change is not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The file does not read as sshd reads it, so a change in it could
    /// not be told from the mistakes that are there (NET-183).
    Mistakes(Vec<Mistake>),
    /// A name in AllowUsers is no account.
    NoAccount(String),
    /// A name in AllowGroups is no group.
    NoGroup(String),
    /// Neither a password nor a key would sign anyone in.
    Nobody,
}

/// A line of the file: its text, and its end, which is `\n`, `\r\n` or
/// nothing for a last line the file does not end.
#[derive(Debug)]
struct Line {
    body: String,
    end: String,
}

/// The lines of `text`, ends kept.
fn lines_of(text: &str) -> Vec<Line> {
    text.split_inclusive('\n')
        .map(|piece| {
            let end = if piece.ends_with("\r\n") { 2 } else { usize::from(piece.ends_with('\n')) };
            let (body, end) = piece.split_at(piece.len() - end);
            Line { body: body.to_string(), end: end.to_string() }
        })
        .collect()
}

/// The text `lines` make.
fn text_of(lines: &[Line]) -> String {
    lines.iter().flat_map(|line| [line.body.as_str(), line.end.as_str()]).collect()
}

/// The keyword `body` starts with, as the parser takes it: none for a
/// blank line or a comment.
fn keyword_of(body: &str) -> Option<&str> {
    let line = body.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    Some(&line[..end])
}

/// Whether `line` is a line of `keyword`, in any case.
fn is_line_of(line: &Line, keyword: &str) -> bool {
    keyword_of(&line.body).is_some_and(|found| found.eq_ignore_ascii_case(keyword))
}

/// Puts `values` after `keyword` on its first line, or on a line added at
/// the end.
fn put(lines: &mut Vec<Line>, keyword: &str, values: &str) {
    if let Some(at) = lines.iter().position(|line| is_line_of(line, keyword)) {
        let body = &lines[at].body;
        let indent = &body[..body.len() - body.trim_start().len()];
        let spelling = keyword_of(body).unwrap_or(keyword);
        lines[at].body = format!("{indent}{spelling} {values}");
    } else {
        // A last line with no end gets one before another follows it.
        if let Some(last) = lines.last_mut()
            && last.end.is_empty()
        {
            last.end = "\n".into();
        }
        lines.push(Line { body: format!("{keyword} {values}"), end: "\n".into() });
    }
}

/// Sets the flag `keyword` to `wanted`, when the file reads `current`.
fn set_flag(lines: &mut Vec<Line>, keyword: &str, wanted: bool, current: bool) {
    if wanted != current {
        put(lines, keyword, if wanted { "yes" } else { "no" });
    }
}

/// Sets the list `keyword` to `wanted`, when the file reads `current`.
fn set_list(lines: &mut Vec<Line>, keyword: &str, wanted: &Names, current: &[String]) {
    let names: &[String] = match wanted {
        Names::Everyone => &[],
        Names::Only(names) => names,
    };
    if names == current {
        return;
    }
    if !names.is_empty() {
        put(lines, keyword, &names.join(" "));
    }
    // The lists add up, so each line but the first goes, and with no
    // names every one does.
    let mut seen = false;
    lines.retain(|line| {
        let found = is_line_of(line, keyword);
        let keep = !found || (!names.is_empty() && !seen);
        seen |= found;
        keep
    });
}

/// The first of `names` that `known` does not hold.
fn unknown<'a>(names: &'a Option<Names>, known: &[String]) -> Option<&'a String> {
    match names {
        Some(Names::Only(names)) => names.iter().find(|name| !known.contains(name)),
        _ => None,
    }
}

/// `text` with `change` made, where `accounts` and `groups` are the names
/// the machine has.
///
/// # Errors
///
/// A [`Refusal`]: the file has a mistake, a name is none the machine has,
/// or the result lets no one sign in. They come in that order.
pub(crate) fn edited(
    text: &str,
    change: &Change,
    accounts: &[String],
    groups: &[String],
) -> Result<String, Refusal> {
    let before = sshd_config::parse(text).map_err(Refusal::Mistakes)?;
    if let Some(name) = unknown(&change.allow_users, accounts) {
        return Err(Refusal::NoAccount(name.clone()));
    }
    if let Some(name) = unknown(&change.allow_groups, groups) {
        return Err(Refusal::NoGroup(name.clone()));
    }
    let mut lines = lines_of(text);
    if let Some(wanted) = change.passwords {
        set_flag(&mut lines, "PasswordAuthentication", wanted, before.passwords);
    }
    if let Some(wanted) = change.keys {
        set_flag(&mut lines, "PubkeyAuthentication", wanted, before.keys);
    }
    if let Some(wanted) = &change.allow_users {
        set_list(&mut lines, "AllowUsers", wanted, &before.allow_users);
    }
    if let Some(wanted) = &change.allow_groups {
        set_list(&mut lines, "AllowGroups", wanted, &before.allow_groups);
    }
    let new = text_of(&lines);
    // What is written reads, whatever the names were.
    let after = sshd_config::parse(&new).map_err(Refusal::Mistakes)?;
    if !after.passwords && !after.keys {
        return Err(Refusal::Nobody);
    }
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The accounts and groups the cases have, as `cases.txt` says.
    fn names(words: &str) -> Vec<String> {
        words.split_whitespace().map(ToString::to_string).collect()
    }

    /// The words of `change` as `ssh-server set` is given them.
    fn change(words: &str) -> Change {
        let words = names(words);
        crate::ssh_server::set::change(&words).unwrap_or_else(|why| panic!("{words:?}: {why}"))
    }

    /// A refusal as `cases.txt` words it.
    fn refusal_word(refusal: &Refusal) -> String {
        match refusal {
            Refusal::Mistakes(mistakes) => format!("mistake {}", mistakes[0].line),
            Refusal::NoAccount(name) => format!("account {name}"),
            Refusal::NoGroup(name) => format!("group {name}"),
            Refusal::Nobody => "nobody".into(),
        }
    }

    /// The text of the lines a case gives after `mark`, each with its
    /// end, the last without one when `\ no newline` follows it; none
    /// when the case has no such line.
    fn text_after(lines: &[&str], mark: char) -> Option<String> {
        let mut text = String::new();
        let mut found = false;
        let mut last_is_mine = false;
        for line in lines {
            if let Some(body) = line.strip_prefix(mark) {
                found = true;
                last_is_mine = true;
                text += body.strip_prefix(' ').unwrap_or(body);
                text += "\n";
            } else {
                if line.starts_with('\\') && last_is_mine {
                    text.pop();
                }
                last_is_mine = false;
            }
        }
        found.then_some(text)
    }

    #[test]
    fn line_ends_and_spaces_stay_as_they_are() {
        let text = "# a \r\n\tDenyUsers mallory \t\r\n  PasswordAuthentication yes\r\n\r\n";
        let new = edited(text, &change("passwords no"), &[], &[]).unwrap();
        assert_eq!(new, "# a \r\n\tDenyUsers mallory \t\r\n  PasswordAuthentication no\r\n\r\n");
    }

    #[test]
    fn a_line_added_after_lines_that_end_in_return_and_newline_ends_in_a_newline() {
        let new = edited("# a\r\n", &change("passwords yes"), &[], &[]).unwrap();
        assert_eq!(new, "# a\r\nPasswordAuthentication yes\n");
    }

    #[test]
    fn a_name_the_machine_has_is_taken_and_one_it_lacks_is_not() {
        let (accounts, groups) = (names("alice"), names("staff"));
        let new = edited("", &change("allow-users alice allow-groups staff"), &accounts, &groups);
        assert_eq!(new.unwrap(), "AllowUsers alice\nAllowGroups staff\n");
        let refused = edited("", &change("allow-users bob"), &accounts, &groups);
        assert_eq!(refused, Err(Refusal::NoAccount("bob".into())));
        // An account is no group, nor a group an account.
        let refused = edited("", &change("allow-groups alice"), &accounts, &groups);
        assert_eq!(refused, Err(Refusal::NoGroup("alice".into())));
    }

    /// The cases of `cases.txt`: the file before, the change, and the
    /// file after or the refusal.
    #[test]
    fn the_shared_cases_make_the_text_the_page_expects() {
        let mut accounts = Vec::new();
        let mut groups = Vec::new();
        let mut count = 0;
        for block in include_str!("cases.txt").split("\n\n") {
            let lines: Vec<&str> = block.lines().filter(|line| !line.starts_with('#')).collect();
            if let Some(words) = lines.iter().find_map(|line| line.strip_prefix("accounts: ")) {
                accounts = names(words);
            }
            if let Some(words) = lines.iter().find_map(|line| line.strip_prefix("groups: ")) {
                groups = names(words);
            }
            let Some(asked) = lines.iter().find_map(|line| line.strip_prefix("+ ")) else {
                continue;
            };
            let old = text_after(&lines, '<').unwrap_or_default();
            let result = edited(&old, &change(asked), &accounts, &groups);
            let shown = format!("{block}\n");
            if let Some(why) = lines.iter().find_map(|line| line.strip_prefix("! ")) {
                assert_eq!(refusal_word(&result.expect_err(&shown)), why, "{shown}");
            } else {
                let new = result.expect(&shown);
                if lines.contains(&"=") {
                    assert_eq!(new, old, "{shown}");
                } else if lines.contains(&"-") {
                    assert_eq!(new, "", "{shown}");
                } else {
                    assert_eq!(Some(&new), text_after(&lines, '>').as_ref(), "{shown}");
                }
                // What was asked is done: asking again changes nothing.
                assert_eq!(edited(&new, &change(asked), &accounts, &groups), Ok(new), "{shown}");
            }
            count += 1;
        }
        assert!(count >= 25, "{count} cases");
    }
}
