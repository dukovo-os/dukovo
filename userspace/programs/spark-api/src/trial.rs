// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Settings tried with an undo (ADR 0472): the `try` request that asks
//! Spark to keep a copy of some settings files and to put them back, and
//! restart what reads them, unless the change is kept in time; and the
//! record of a trial that Spark keeps on the disk, so that a start that
//! follows one never kept undoes it before any service starts.
//!
//! A trial names files under [`SETTINGS`] alone, so that what Spark
//! writes back is only ever settings, and each by a plain path, so that
//! the path Spark checks is the path it writes.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use core::ops::RangeInclusive;

/// Where every file a trial names is.
pub const SETTINGS: &str = "/system/settings/";

/// The trials that may run at once.
pub const TRIALS_MAX: usize = 4;

/// The files one trial may name.
pub const FILES_MAX: usize = 16;

/// The services one trial may restart or reload, together.
pub const SERVICES_MAX: usize = 8;

/// The largest file a trial keeps a copy of, in bytes.
pub const FILE_MAX: usize = 1 << 20;

/// How long a trial may run before it is undone, in seconds: long enough
/// to sign in again over SSH, short enough that a machine cut off by a
/// change comes back within the hour.
pub const SECONDS: RangeInclusive<u32> = 10..=3600;

/// How long a trial runs unless the request says otherwise, in seconds.
pub const SECONDS_DEFAULT: u32 = 60;

/// The longest name of a trial.
pub const ID_MAX: usize = 64;

/// The longest path a trial names.
pub const PATH_MAX: usize = 255;

/// A `try` request: `try ID SECONDS file=PATH... restart=NAME...
/// reload=NAME...`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// Its name, which `keep` and `undo` give.
    pub id: String,
    /// How long it runs before it is undone.
    pub seconds: u32,
    /// The files whose copies are kept, in the order given.
    pub files: Vec<String>,
    /// The services restarted when it is undone, if they ran when it
    /// began, and stopped if they did not.
    pub restart: Vec<String>,
    /// The services asked to reload when it is undone.
    pub reload: Vec<String>,
}

impl Request {
    /// The request whose words follow `try`, or why it is not one. A
    /// service's name is checked by Spark, which knows the services.
    pub fn parse(words: &[&str]) -> Result<Request, String> {
        let (id, seconds, rest) = match words {
            [id, seconds, rest @ ..] => (*id, *seconds, rest),
            _ => return Err("try takes a name, seconds, and file=PATH for each file".into()),
        };
        check_id(id)?;
        let seconds =
            seconds.parse::<u32>().ok().filter(|s| SECONDS.contains(s)).ok_or_else(|| {
                format!(
                    "a trial runs {} to {} seconds, not {seconds}",
                    SECONDS.start(),
                    SECONDS.end()
                )
            })?;
        let mut request = Request { id: id.into(), seconds, ..Request::default() };
        for word in rest {
            let (key, value) = word.split_once('=').unwrap_or((word, ""));
            let list = match key {
                "file" => &mut request.files,
                "restart" => &mut request.restart,
                "reload" => &mut request.reload,
                _ => return Err(format!("`{word}` is not file=, restart= or reload=")),
            };
            if value.is_empty() {
                return Err(format!("{key}= names nothing"));
            }
            if list.iter().any(|v| v == value) {
                return Err(format!("{key}={value} is given twice"));
            }
            list.push(value.into());
        }
        if request.files.is_empty() {
            return Err("a trial names at least one file=".into());
        }
        if request.files.len() > FILES_MAX {
            return Err(format!("a trial names at most {FILES_MAX} files"));
        }
        if request.restart.len() + request.reload.len() > SERVICES_MAX {
            return Err(format!("a trial restarts or reloads at most {SERVICES_MAX} services"));
        }
        if let Some(name) = request.restart.iter().find(|n| request.reload.contains(n)) {
            return Err(format!("{name} is both restarted and reloaded"));
        }
        for path in &request.files {
            check_path(path)?;
        }
        Ok(request)
    }

    /// The request as a line to send, with its newline.
    pub fn line(&self) -> String {
        let mut line = format!("try {} {}", self.id, self.seconds);
        for (key, list) in
            [("file", &self.files), ("restart", &self.restart), ("reload", &self.reload)]
        {
            for value in list {
                let _ = write!(line, " {key}={value}");
            }
        }
        line.push('\n');
        line
    }
}

/// Whether `id` may name a trial: 1 to [`ID_MAX`] letters, digits, `-`,
/// `_` and `.`, starting with a letter or a digit, as a service's name
/// is, so that it is also a folder's name.
pub fn check_id(id: &str) -> Result<(), String> {
    let fits = (1..=ID_MAX).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && id.starts_with(|c: char| c.is_ascii_alphanumeric());
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{id}` is not a trial's name: it has 1 to {ID_MAX} letters, digits, `-`, `_` and \
             `.`, and starts with a letter or a digit"
        ))
    }
}

/// Whether a trial may name the file at `path`: beneath [`SETTINGS`],
/// at most [`PATH_MAX`] bytes, each name in it letters, digits, `-`, `_`,
/// `.` and `@`, none starting with `.`, so that no name is `..`, a hidden
/// file such as an edit's copy, or a space that would split the request.
pub fn check_path(path: &str) -> Result<(), String> {
    let Some(rest) = path.strip_prefix(SETTINGS) else {
        return Err(format!("{path} is not beneath {SETTINGS}: a trial changes settings alone"));
    };
    if path.len() > PATH_MAX {
        return Err(format!("{path} is longer than {PATH_MAX} bytes"));
    }
    let plain = |name: &str| {
        !name.is_empty()
            && !name.starts_with('.')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
    };
    if rest.split('/').all(plain) {
        Ok(())
    } else {
        Err(format!(
            "{path} is not a plain path: each name in it has letters, digits, `-`, `_`, `.` and \
             `@`, and none starts with `.`"
        ))
    }
}

/// A file a trial keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    /// Its path.
    pub path: String,
    /// Its copy, or None when there was no file, so that an undo removes
    /// the one the change made.
    pub copy: Option<Saved>,
}

/// The copy of a file a trial keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// Its name in the trial's folder: its number.
    pub name: String,
    /// The file's permission bits, which the file put back has again, so
    /// that an undo never opens a file that was private.
    pub mode: u16,
    /// The number of the user the file belonged to.
    pub owner: u32,
    /// The number of the file's group.
    pub group: u32,
}

/// A service a trial restarts or reloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    /// Its name.
    pub name: String,
    /// Whether it ran, or was starting, when the trial began: an undo
    /// restarts one that did and stops one that did not.
    pub ran: bool,
}

/// The record of a trial that Spark keeps on the disk beside the copies,
/// as lines of words:
///
/// ```text
/// seconds 60
/// file /system/settings/spark/services/web.toml 0 644 0 0
/// absent /system/settings/spark/services/settings.d/web.toml
/// restart web ran
/// reload netd
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// How long it was to run.
    pub seconds: u32,
    /// The files, in the order the request gave them.
    pub files: Vec<Kept>,
    /// The services restarted or stopped by an undo.
    pub restart: Vec<Named>,
    /// The services asked to reload by an undo.
    pub reload: Vec<String>,
}

impl Record {
    /// The record as its file holds it.
    pub fn text(&self) -> String {
        let mut text = format!("seconds {}\n", self.seconds);
        for kept in &self.files {
            let _ = match &kept.copy {
                Some(copy) => writeln!(
                    text,
                    "file {} {} {:o} {} {}",
                    kept.path, copy.name, copy.mode, copy.owner, copy.group
                ),
                None => writeln!(text, "absent {}", kept.path),
            };
        }
        for named in &self.restart {
            let ran = if named.ran { "ran" } else { "stopped" };
            let _ = writeln!(text, "restart {} {ran}", named.name);
        }
        for name in &self.reload {
            let _ = writeln!(text, "reload {name}");
        }
        text
    }

    /// The record its file's `text` holds, or why it is not one: a
    /// record Spark cannot read whole is never half undone.
    pub fn parse(text: &str) -> Result<Record, String> {
        let mut record = Record::default();
        for (number, line) in text.lines().enumerate() {
            let words: Vec<&str> = line.split_whitespace().collect();
            let bad = || format!("line {}: `{line}` is not part of a trial's record", number + 1);
            match words.as_slice() {
                [] => {}
                ["seconds", seconds] => record.seconds = seconds.parse().map_err(|_| bad())?,
                ["file", path, name, mode, owner, group] => {
                    check_path(path).map_err(|_| bad())?;
                    // A copy is named by its number in the folder.
                    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(bad());
                    }
                    let mode = u16::from_str_radix(mode, 8)
                        .ok()
                        .filter(|&m| m <= 0o7777)
                        .ok_or_else(bad)?;
                    let copy = Saved {
                        name: name.to_string(),
                        mode,
                        owner: owner.parse().map_err(|_| bad())?,
                        group: group.parse().map_err(|_| bad())?,
                    };
                    record.files.push(Kept { path: path.to_string(), copy: Some(copy) });
                }
                ["absent", path] => {
                    check_path(path).map_err(|_| bad())?;
                    record.files.push(Kept { path: path.to_string(), copy: None });
                }
                ["restart", name, ran @ ("ran" | "stopped")] => {
                    record.restart.push(Named { name: name.to_string(), ran: *ran == "ran" });
                }
                ["reload", name] => record.reload.push(name.to_string()),
                _ => return Err(bad()),
            }
        }
        if record.files.is_empty() {
            return Err("it names no file".into());
        }
        Ok(record)
    }
}

/// Why a trial was undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// Its time ran out before it was kept.
    TimeUp,
    /// This service, which it restarts or reloads, failed to start or
    /// failed its health check with the change.
    Failed(String),
    /// A request asked for it.
    Asked,
    /// The machine started again before it was kept.
    Restarted,
}

impl Why {
    /// Why, as Spark's line says it in brackets: "undid service.web (it
    /// was not kept in time)".
    pub fn words(&self) -> String {
        match self {
            Why::TimeUp => "it was not kept in time".into(),
            Why::Failed(name) => format!("{name} failed with the change"),
            Why::Asked => "asked".into(),
            Why::Restarted => "the machine started again before it was kept".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_request_is_read_and_written_back_the_same() {
        let line = "try service.web 60 file=/system/settings/spark/services/web.toml \
                    file=/system/settings/spark/services/settings.d/web.toml restart=web reload=netd";
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let request = Request::parse(&words).unwrap();
        assert_eq!(request.id, "service.web");
        assert_eq!(request.seconds, 60);
        assert_eq!(request.files.len(), 2);
        assert_eq!(request.restart, vec!["web"]);
        assert_eq!(request.reload, vec!["netd"]);
        assert_eq!(request.line(), format!("{line}\n"));
    }

    #[test]
    fn a_request_that_is_wrong_says_how() {
        let wrong = |line: &str| {
            let words: Vec<&str> = line.split_whitespace().collect();
            Request::parse(&words).unwrap_err()
        };
        assert_eq!(wrong("web"), "try takes a name, seconds, and file=PATH for each file");
        assert!(wrong("../x 60 file=/system/settings/a").contains("is not a trial's name"));
        assert_eq!(
            wrong("web 9 file=/system/settings/a"),
            "a trial runs 10 to 3600 seconds, not 9"
        );
        assert_eq!(
            wrong("web 3601 file=/system/settings/a"),
            "a trial runs 10 to 3600 seconds, not 3601"
        );
        assert_eq!(wrong("web 60"), "a trial names at least one file=");
        assert_eq!(wrong("web 60 path=/x"), "`path=/x` is not file=, restart= or reload=");
        assert_eq!(wrong("web 60 file="), "file= names nothing");
        assert_eq!(
            wrong("web 60 file=/system/settings/a file=/system/settings/a"),
            "file=/system/settings/a is given twice"
        );
        assert_eq!(
            wrong("web 60 file=/system/settings/a restart=web reload=web"),
            "web is both restarted and reloaded"
        );
        assert!(wrong("web 60 file=/etc/passwd").contains("is not beneath /system/settings/"));
        let files: Vec<String> =
            (0..=FILES_MAX).map(|i| format!("file=/system/settings/{i}")).collect();
        let mut words = vec!["web", "60"];
        words.extend(files.iter().map(String::as_str));
        assert_eq!(Request::parse(&words).unwrap_err(), "a trial names at most 16 files");
        let names: Vec<String> = (0..=SERVICES_MAX).map(|i| format!("restart=s{i}")).collect();
        let mut words = vec!["web", "60", "file=/system/settings/a"];
        words.extend(names.iter().map(String::as_str));
        assert_eq!(
            Request::parse(&words).unwrap_err(),
            "a trial restarts or reloads at most 8 services"
        );
    }

    #[test]
    fn only_plain_paths_beneath_the_settings_are_tried() {
        assert!(check_path("/system/settings/spark/services/web.toml").is_ok());
        assert!(check_path("/system/settings/network/eth0@home.toml").is_ok());
        for path in [
            "/system/settings",
            "/system/settings/",
            "/system/settings//a",
            "/system/settings/a/",
            "/system/settings/../state/x",
            "/system/settings/a/../../x",
            "/system/settings/spark/services/.web.toml.edit",
            "/system/settings/a b",
            "/system/settings/a\nb",
            "/system/state/spark/good/web.toml",
            "system/settings/a",
        ] {
            assert!(check_path(path).is_err(), "{path:?}");
        }
        let long = format!("/system/settings/{}", "a".repeat(PATH_MAX));
        assert!(check_path(&long).unwrap_err().contains("longer than 255 bytes"));
    }

    #[test]
    fn a_trials_name_is_a_folders_name() {
        assert!(check_id("service.web").is_ok());
        assert!(check_id("network").is_ok());
        for id in ["", ".", "..", ".web", "-web", "a/b", "a b", &"a".repeat(ID_MAX + 1)] {
            assert!(check_id(id).is_err(), "{id:?}");
        }
    }

    #[test]
    fn a_record_is_written_and_read_back_the_same() {
        let record = Record {
            seconds: 60,
            files: vec![
                Kept {
                    path: "/system/settings/spark/services/web.toml".into(),
                    copy: Some(Saved { name: "0".into(), mode: 0o600, owner: 0, group: 5 }),
                },
                Kept {
                    path: "/system/settings/spark/services/settings.d/web.toml".into(),
                    copy: None,
                },
            ],
            restart: vec![
                Named { name: "web".into(), ran: true },
                Named { name: "db".into(), ran: false },
            ],
            reload: vec!["netd".into()],
        };
        let text = record.text();
        assert_eq!(
            text,
            "seconds 60\n\
             file /system/settings/spark/services/web.toml 0 600 0 5\n\
             absent /system/settings/spark/services/settings.d/web.toml\n\
             restart web ran\n\
             restart db stopped\n\
             reload netd\n"
        );
        assert_eq!(Record::parse(&text).unwrap(), record);
    }

    #[test]
    fn why_a_trial_was_undone_is_said() {
        assert_eq!(Why::TimeUp.words(), "it was not kept in time");
        assert_eq!(Why::Failed("web".into()).words(), "web failed with the change");
        assert_eq!(Why::Asked.words(), "asked");
        assert_eq!(Why::Restarted.words(), "the machine started again before it was kept");
    }

    #[test]
    fn a_record_that_is_not_whole_is_refused() {
        assert_eq!(Record::parse("seconds 60\n").unwrap_err(), "it names no file");
        for text in [
            "seconds sixty\nfile /system/settings/a 0 644 0 0\n",
            "file /etc/passwd 0 644 0 0\n",
            "file /system/settings/a ../x 644 0 0\n",
            "file /system/settings/a 0 999 0 0\n",
            "file /system/settings/a 0 17777 0 0\n",
            "file /system/settings/a 0 644 root 0\n",
            "file /system/settings/a 0\n",
            "absent /system/state/x\n",
            "restart web maybe\nfile /system/settings/a 0 644 0 0\n",
            "file /system/settings/a 0 644 0 0\nsomething else\n",
        ] {
            assert!(Record::parse(text).is_err(), "{text:?}");
        }
    }
}
