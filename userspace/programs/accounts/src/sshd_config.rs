// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sshd_config(5): how people may sign in to sshd(8), and who (ADR
//! 0348), in OpenSSH's format, so that what an administrator knows of
//! Linux's sshd holds:
//!
//! ```text
//! # Passwords sign in too, but only alice's and bob's.
//! PasswordAuthentication yes
//! AllowUsers alice bob
//! ```
//!
//! A line is a keyword, in any case, then its values, after spaces or one
//! `=`; a value in double quotes may hold spaces, and `#` starts a comment
//! at the start of a line or of a value. A keyword given twice keeps its
//! first value, as OpenSSH's does, but the lists of AllowUsers and the
//! others add up. Any keyword sshd does not carry out, or a value it
//! does not take, is a mistake that keeps the whole file from being
//! used, where OpenSSH would carry it out, so that no setting meant to
//! keep people out is passed over.
//!
//! It is here, with what authd(8) is asked, so that its unit tests run on
//! the host; sshd reads it as it starts and when Spark asks it to reload.

use std::fmt::Write as _;

use crate::{Group, Table, User};

/// The file: Linux's `/etc/ssh/sshd_config`.
pub const PATH: &str = "/system/settings/ssh/sshd_config";

/// The settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Whether a password signs a person in (PasswordAuthentication):
    /// not unless the administrator says so.
    pub passwords: bool,
    /// Whether a key in `~/.ssh/authorized_keys` signs a person in
    /// (PubkeyAuthentication).
    pub keys: bool,
    /// The names that may not sign in (DenyUsers): patterns, with `*`
    /// and `?`.
    pub deny_users: Vec<String>,
    /// When there are any, the only names that may (AllowUsers).
    pub allow_users: Vec<String>,
    /// The groups whose people may not sign in (DenyGroups).
    pub deny_groups: Vec<String>,
    /// When there are any, the only groups whose people may
    /// (AllowGroups).
    pub allow_groups: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            passwords: false,
            keys: true,
            deny_users: Vec::new(),
            allow_users: Vec::new(),
            deny_groups: Vec::new(),
            allow_groups: Vec::new(),
        }
    }
}

/// A line of the file sshd does not take, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mistake {
    /// Its number, from 1.
    pub line: usize,
    /// Why, in words.
    pub why: String,
}

/// The longest value taken, and the most values of a list.
const VALUE_MAX: usize = 256;
const LIST_MAX: usize = 256;

/// The words of `line`: the keyword, then its values. `None` for a line
/// with nothing to say.
fn words(line: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let keyword = line[..end].to_string();
    let mut rest = line[end..].trim_start();
    // One `=` may stand between the keyword and its values.
    if let Some(after) = rest.strip_prefix('=') {
        rest = after.trim_start();
    }
    let mut values = Vec::new();
    while !rest.is_empty() {
        if rest.starts_with('#') {
            break;
        }
        let (value, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let Some(close) = quoted.find('"') else {
                return Err("a value's quotes are not closed".into());
            };
            (&quoted[..close], &quoted[close + 1..])
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        if value.len() > VALUE_MAX || value.chars().any(char::is_control) {
            return Err(format!("a value is longer than {VALUE_MAX} or holds a control character"));
        }
        values.push(value.to_string());
        rest = after.trim_start();
    }
    Ok(Some((keyword, values)))
}

/// `yes` or `no`, in any case.
fn flag(keyword: &str, values: &[String]) -> Result<bool, String> {
    match values {
        [value] if value.eq_ignore_ascii_case("yes") => Ok(true),
        [value] if value.eq_ignore_ascii_case("no") => Ok(false),
        _ => Err(format!("{keyword} takes yes or no")),
    }
}

/// The patterns of a list: names with `*` and `?`, which match any
/// characters and any one.
fn patterns(keyword: &str, values: &[String]) -> Result<Vec<String>, String> {
    if values.is_empty() {
        return Err(format!("{keyword} takes one or more names"));
    }
    for value in values {
        if value.contains('@') {
            return Err(format!(
                "{keyword} takes names, not name@address: sshd does not tell clients apart by \
                 address yet"
            ));
        }
        if value.starts_with('!') {
            return Err(format!("{keyword} takes names, not ! before them"));
        }
        if value.is_empty() || value.contains([':', ',', '/']) {
            return Err(format!("{value:?} is not a name for {keyword}"));
        }
    }
    Ok(values.to_vec())
}

/// Reads the settings in `text`, or every mistake in it.
pub fn parse(text: &str) -> Result<Settings, Vec<Mistake>> {
    let mut settings = Settings::default();
    let (mut passwords, mut keys) = (None, None);
    let mut mistakes = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line_number = number + 1;
        let (keyword, values) = match words(line) {
            Ok(Some(words)) => words,
            Ok(None) => continue,
            Err(why) => {
                mistakes.push(Mistake { line: line_number, why });
                continue;
            }
        };
        let taken = match keyword.to_ascii_lowercase().as_str() {
            "passwordauthentication" => flag(&keyword, &values).map(|value| {
                passwords.get_or_insert(value);
            }),
            "pubkeyauthentication" => flag(&keyword, &values).map(|value| {
                keys.get_or_insert(value);
            }),
            "denyusers" => patterns(&keyword, &values).map(|p| settings.deny_users.extend(p)),
            "allowusers" => patterns(&keyword, &values).map(|p| settings.allow_users.extend(p)),
            "denygroups" => patterns(&keyword, &values).map(|p| settings.deny_groups.extend(p)),
            "allowgroups" => patterns(&keyword, &values).map(|p| settings.allow_groups.extend(p)),
            _ => Err(format!(
                "sshd does not take {keyword}: sshd_config(5) lists the keywords it takes"
            )),
        };
        if let Err(why) = taken {
            mistakes.push(Mistake { line: line_number, why });
        }
    }
    let lists = [
        &settings.deny_users,
        &settings.allow_users,
        &settings.deny_groups,
        &settings.allow_groups,
    ];
    if lists.iter().any(|list| list.len() > LIST_MAX) {
        mistakes.push(Mistake {
            line: text.lines().count(),
            why: format!("a list holds more than {LIST_MAX} names"),
        });
    }
    if !mistakes.is_empty() {
        return Err(mistakes);
    }
    settings.passwords = passwords.unwrap_or(settings.passwords);
    settings.keys = keys.unwrap_or(settings.keys);
    Ok(settings)
}

/// The settings in [`PATH`]: the defaults when there is no file; what is
/// wrong otherwise, a line each.
///
/// # Errors
///
/// When the file cannot be read, or has mistakes.
pub fn read() -> Result<Settings, String> {
    read_from(PATH)
}

/// The settings in the file at `path`, as [`read`] reads them.
///
/// # Errors
///
/// When the file cannot be read, or has mistakes.
pub fn read_from(path: &str) -> Result<Settings, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(err) => return Err(format!("cannot read {path}: {err}")),
    };
    parse(&text).map_err(|mistakes| {
        let lines: Vec<String> = mistakes
            .iter()
            .map(|mistake| format!("{path} line {}: {}", mistake.line, mistake.why))
            .collect();
        lines.join("\n")
    })
}

/// Whether `pattern`, with `*` and `?`, matches `text` whole.
pub fn matches(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) =
        (pattern.chars().collect(), text.chars().collect());
    // The last `*` seen, and where in `text` it was tried up to.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((at, tried)) => {
                    p = at + 1;
                    t = tried + 1;
                    star = Some((at, tried + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// The names of the groups `user` is in: its own, then each that lists
/// it.
pub fn group_names(user: &User, groups: &Table<Group>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for group in groups.entries() {
        if (group.gid == user.gid || group.members.contains(&user.name))
            && !names.contains(&group.name)
        {
            names.push(group.name.clone());
        }
    }
    names
}

impl Settings {
    /// Whether `name`, in the groups `groups`, may sign in, as OpenSSH's
    /// sshd checks in turn DenyUsers, AllowUsers, DenyGroups and
    /// AllowGroups; if not, why.
    ///
    /// # Errors
    ///
    /// When the settings keep `name` out.
    pub fn admits(&self, name: &str, groups: &[String]) -> Result<(), String> {
        let any = |patterns: &[String], text: &str| patterns.iter().any(|p| matches(p, text));
        if any(&self.deny_users, name) {
            return Err(format!("DenyUsers names {name}"));
        }
        if !self.allow_users.is_empty() && !any(&self.allow_users, name) {
            return Err(format!("AllowUsers does not name {name}"));
        }
        if let Some(group) = groups.iter().find(|g| any(&self.deny_groups, g)) {
            return Err(format!("DenyGroups names {name}'s group {group}"));
        }
        if !self.allow_groups.is_empty() && !groups.iter().any(|g| any(&self.allow_groups, g)) {
            return Err(format!("AllowGroups names none of {name}'s groups"));
        }
        Ok(())
    }

    /// Whether the groups matter: AllowGroups or DenyGroups name any.
    pub fn checks_groups(&self) -> bool {
        !self.deny_groups.is_empty() || !self.allow_groups.is_empty()
    }

    /// The settings as a file says them, each keyword once: what
    /// `sshd -T` shows, and what [`parse`] reads back the same.
    pub fn text(&self) -> String {
        let yes = |value: bool| if value { "yes" } else { "no" };
        let mut text = format!(
            "PasswordAuthentication {}\nPubkeyAuthentication {}\n",
            yes(self.passwords),
            yes(self.keys)
        );
        for (keyword, list) in [
            ("DenyUsers", &self.deny_users),
            ("AllowUsers", &self.allow_users),
            ("DenyGroups", &self.deny_groups),
            ("AllowGroups", &self.allow_groups),
        ] {
            if !list.is_empty() {
                let quoted: Vec<String> = list
                    .iter()
                    .map(|v| {
                        if v.contains(char::is_whitespace) { format!("\"{v}\"") } else { v.clone() }
                    })
                    .collect();
                let _ = writeln!(text, "{keyword} {}", quoted.join(" "));
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_nothing_said_keys_alone_sign_in() {
        let settings = parse("").unwrap();
        assert_eq!(settings, Settings::default());
        assert!(!settings.passwords && settings.keys);
        assert!(settings.admits("alice", &[]).is_ok());
        // OpenSSH's own file, all comments.
        let text = "# This is the sshd server system-wide configuration file.\n\n\
                    #PasswordAuthentication yes\n   # indented\n";
        assert_eq!(parse(text).unwrap(), Settings::default());
    }

    #[test]
    fn keywords_and_values_are_read_as_openssh_reads_them() {
        let text = "passwordauthentication YES\nPubkeyAuthentication=no\n\
                    AllowUsers alice  bob # the two of them\nAllowUsers \"carol\"\n\
                    DenyUsers = mallory\nAllowGroups\tstaff\nDenyGroups guests\n";
        let settings = parse(text).unwrap();
        assert!(settings.passwords && !settings.keys);
        assert_eq!(settings.allow_users, ["alice", "bob", "carol"]);
        assert_eq!(settings.deny_users, ["mallory"]);
        assert_eq!(settings.allow_groups, ["staff"]);
        assert_eq!(settings.deny_groups, ["guests"]);
    }

    #[test]
    fn a_keyword_given_twice_keeps_its_first_value() {
        let settings = parse("PasswordAuthentication yes\nPasswordAuthentication no\n").unwrap();
        assert!(settings.passwords);
        let settings = parse("PasswordAuthentication no\nPasswordAuthentication yes\n").unwrap();
        assert!(!settings.passwords);
    }

    #[test]
    fn what_sshd_does_not_carry_out_is_a_mistake_with_its_line() {
        let text = "PasswordAuthentication yes\nPort 2222\nPasswordAuthentication maybe\n\
                    AllowUsers alice@192.0.2.7\nMatch User bob\nAllowUsers\nDenyUsers !root\n\
                    AllowUsers \"open\nUsePAM yes\nAllowGroups a:b\n";
        let mistakes = parse(text).unwrap_err();
        let lines: Vec<usize> = mistakes.iter().map(|m| m.line).collect();
        assert_eq!(lines, [2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert!(mistakes[0].why.contains("does not take Port"), "{}", mistakes[0].why);
        assert!(mistakes[1].why.contains("yes or no"), "{}", mistakes[1].why);
        assert!(mistakes[2].why.contains("name@address"), "{}", mistakes[2].why);
        // Nothing of a file with a mistake is used.
        assert!(parse("PasswordAuthentication yes\nX11Forwarding no\n").is_err());
        let long = format!("AllowUsers {}\n", "a".repeat(VALUE_MAX + 1));
        assert!(parse(&long).is_err());
        let many = format!("AllowUsers {}\n", vec!["a"; LIST_MAX + 1].join(" "));
        assert!(parse(&many).is_err());
    }

    #[test]
    fn patterns_match_whole_names() {
        for (pattern, text) in
            [("alice", "alice"), ("a*", "alice"), ("*", ""), ("a?ice", "alice"), ("*e", "alice")]
        {
            assert!(matches(pattern, text), "{pattern} {text}");
        }
        for (pattern, text) in [("alice", "alice2"), ("a?", "a"), ("*x*", "alice"), ("", "a")] {
            assert!(!matches(pattern, text), "{pattern} {text}");
        }
        assert!(matches("a*b*c", "aXbYbZc"));
        assert!(!matches("a*b*c", "aXbYbZ"));
    }

    #[test]
    fn deny_and_allow_lists_keep_people_out_in_openssh_order() {
        let settings = parse("AllowUsers a* bob\nDenyUsers anna\nDenyGroups guests\n").unwrap();
        assert!(settings.admits("alice", &["alice".into()]).is_ok());
        assert!(settings.admits("bob", &[]).is_ok());
        assert!(settings.admits("anna", &[]).unwrap_err().contains("DenyUsers"));
        assert!(settings.admits("carol", &[]).unwrap_err().contains("AllowUsers"));
        let why = settings.admits("alice", &["guests".into()]).unwrap_err();
        assert!(why.contains("DenyGroups") && why.contains("guests"), "{why}");
        let staff = parse("AllowGroups staff admins\n").unwrap();
        assert!(staff.checks_groups());
        assert!(staff.admits("bob", &["bob".into(), "admins".into()]).is_ok());
        assert!(staff.admits("bob", &["bob".into()]).unwrap_err().contains("AllowGroups"));
        assert!(!Settings::default().checks_groups());
    }

    #[test]
    fn the_text_reads_back_the_same() {
        for text in [
            "",
            "PasswordAuthentication yes\nPubkeyAuthentication no\n",
            "AllowUsers alice \"two words\"\nDenyUsers x\nAllowGroups staff\nDenyGroups g?\n",
        ] {
            let settings = parse(text).unwrap();
            assert_eq!(parse(&settings.text()).unwrap(), settings, "{text}");
        }
        assert_eq!(
            Settings::default().text(),
            "PasswordAuthentication no\nPubkeyAuthentication yes\n"
        );
    }

    #[test]
    fn a_persons_groups_are_their_own_and_those_that_list_them() {
        let user =
            User::parse("alice:x:1000:1000:Alice:/users/alice:/system/commands/nukesh").unwrap();
        let groups = crate::groups("alice:x:1000:\nadmins:x:10:bob,alice\nstaff:x:20:bob\n");
        assert_eq!(group_names(&user, &groups), ["alice", "admins"]);
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        assert_eq!(read_from("/nonexistent/sshd_config").unwrap(), Settings::default());
        let dir = std::env::temp_dir().join(format!("sshd-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sshd_config");
        std::fs::write(&path, "PasswordAuthentication yes\nPort 22\n").unwrap();
        let why = read_from(path.to_str().unwrap()).unwrap_err();
        assert!(
            why.ends_with(
                "sshd_config line 2: sshd does not take Port: sshd_config(5) lists \
                               the keywords it takes"
            ),
            "{why}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
