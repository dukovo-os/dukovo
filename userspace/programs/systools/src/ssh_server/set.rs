// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ssh-server set`: the words of the change, and the steps that make it,
//! each a function of its own: [`read_config`] and [`known`] read what
//! the edit needs, [`super::edit::edited`] makes the text, [`write`]
//! puts it in place, and [`reload`] tells sshd. A tried change takes the
//! same steps in its own order, with Spark's trial between them.
//!
//! When a change is refused, [`refused`] has the words; when the file
//! is written and sshd does not take it up, `service status sshd` says
//! why.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use accounts::sshd_config::PATH;

use super::SERVICE;
use super::edit::{Change, Names, Refusal, edited};
use crate::util::{self, Fail, fail};
use crate::{net_errors, service, trial};

/// The mode of a sshd_config that is not there yet: the default file's.
pub(super) const MODE: u32 = 0o644;

/// The words that begin a part of the change.
const KEYWORDS: [&str; 4] = ["passwords", "keys", "allow-users", "allow-groups"];

/// The change the words after `set` ask for, or what is wrong with them.
pub(crate) fn change(words: &[String]) -> Result<Change, String> {
    let mut change = Change::default();
    let mut rest = words;
    while let Some((keyword, after)) = rest.split_first() {
        // A keyword takes the words up to the next one.
        let end = after.iter().position(|word| KEYWORDS.contains(&word.as_str()));
        let (values, next) = after.split_at(end.unwrap_or(after.len()));
        rest = next;
        match keyword.as_str() {
            "passwords" => once(&mut change.passwords, keyword, flag(keyword, values)?)?,
            "keys" => once(&mut change.keys, keyword, flag(keyword, values)?)?,
            "allow-users" => once(&mut change.allow_users, keyword, names(keyword, values)?)?,
            "allow-groups" => once(&mut change.allow_groups, keyword, names(keyword, values)?)?,
            other => return Err(format!("`{other}` is not something to set")),
        }
    }
    if change == Change::default() {
        return Err("set needs something to change".into());
    }
    Ok(change)
}

/// Puts `value` in `slot`, which `keyword` has not filled before.
fn once<T>(slot: &mut Option<T>, keyword: &str, value: T) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{keyword} is given twice"));
    }
    Ok(())
}

/// The `yes` or `no` that `keyword` takes.
fn flag(keyword: &str, values: &[String]) -> Result<bool, String> {
    match values {
        [value] if value == "yes" => Ok(true),
        [value] if value == "no" => Ok(false),
        _ => Err(format!("{keyword} takes yes or no")),
    }
}

/// The names that `keyword` takes: one or more, or `everyone` alone.
fn names(keyword: &str, values: &[String]) -> Result<Names, String> {
    match values {
        [] => Err(format!("{keyword} takes names, or everyone")),
        [only] if only == "everyone" => Ok(Names::Everyone),
        _ if values.iter().any(|value| value == "everyone") => {
            Err("everyone stands alone: it takes no names beside it".into())
        }
        _ => {
            let twice = values.iter().enumerate().find(|(at, value)| values[..*at].contains(value));
            match twice {
                Some((_, value)) => Err(format!("{value} is named twice")),
                None => Ok(Names::Only(values.to_vec())),
            }
        }
    }
}

/// The text of the file at `path`, which is empty when there is none, as
/// sshd takes it.
///
/// # Errors
///
/// When the file is there and cannot be read.
pub(super) fn read_config(path: &Path) -> Result<String, Fail> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(util::failed(&path.display(), error)),
    }
}

/// The names of the accounts and of the groups the machine has.
///
/// # Errors
///
/// When either file cannot be read.
pub(super) fn known() -> Result<(Vec<String>, Vec<String>), Fail> {
    let read = |path: &str| fs::read_to_string(path).map_err(|error| util::failed(&path, error));
    let accounts = accounts::users(&read(accounts::PASSWD)?);
    let groups = accounts::groups(&read(accounts::GROUP)?);
    Ok((
        accounts.entries().map(|user| user.name.clone()).collect(),
        groups.entries().map(|group| group.name.clone()).collect(),
    ))
}

/// What a refusal of the edit comes to as a failure of the command, with
/// NET-183's ID for a file that does not read.
pub(super) fn refused(refusal: Refusal) -> Fail {
    match refusal {
        Refusal::Mistakes(mistakes) => net_errors::ssh_file_has_a_mistake(PATH, &mistakes),
        Refusal::NoAccount(name) => fail(format!(
            "{name} is not an account: AllowUsers takes the names of accounts, which `user list` \
             shows"
        )),
        Refusal::NoGroup(name) => fail(format!(
            "{name} is not a group: AllowGroups takes the names of groups, which \
             /system/settings/group lists"
        )),
        Refusal::Nobody => fail(
            "nobody could sign in over SSH: leave passwords or keys on, or turn sshd off with \
             `ssh-server off`"
                .into(),
        ),
    }
}

/// Puts `text` in place of the file at `path`, whole or not at all, with
/// the mode the file has, or [`MODE`] for one that is not there yet.
///
/// # Errors
///
/// When the new file cannot be written or renamed; the old one is then as
/// it was.
pub(super) fn write(path: &Path, text: &str) -> Result<(), Fail> {
    trial::replace(path, text.as_bytes(), MODE)
}

/// What asking sshd to read its file again came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reloaded {
    /// Spark sent sshd its signal; connections that are open stay as
    /// they were.
    Asked,
    /// sshd is not running, so it reads the file when it starts.
    NotRunning,
}

/// Asks Spark to have sshd read its file again.
///
/// # Errors
///
/// When Spark cannot be reached or refuses.
pub(super) fn reload() -> Result<Reloaded, Fail> {
    match service::ask(&format!("reload {SERVICE}")) {
        Ok(_) => Ok(Reloaded::Asked),
        Err(refused) if refused.message == format!("{SERVICE} is not running") => {
            Ok(Reloaded::NotRunning)
        }
        Err(refused) => Err(refused),
    }
}

/// Makes `change` in the file at `path`, given the names the machine has:
/// nothing is written when the file says so already. Gives whether it
/// wrote.
///
/// # Errors
///
/// The refusal of the edit, or a file that cannot be read or written.
pub(super) fn apply(
    path: &Path,
    change: &Change,
    accounts: &[String],
    groups: &[String],
) -> Result<bool, Fail> {
    let old = read_config(path)?;
    let new = edited(&old, change, accounts, groups).map_err(refused)?;
    if new == old {
        return Ok(false);
    }
    write(path, &new)?;
    Ok(true)
}

/// Fails unless this is root, whom sshd_config and Spark's control socket
/// are for.
///
/// # Errors
///
/// When this is another account.
pub(super) fn require_root() -> Result<(), Fail> {
    if util::own_user() == 0 {
        return Ok(());
    }
    Err(fail(
        "only root may change SSH's settings: run it with elevate, as `elevate ssh-server set \
         ...`"
            .into(),
    ))
}

/// `ssh-server set`, as root: makes the change in sshd_config and has sshd
/// read it.
pub(super) fn run(change: &Change) -> Result<i32, Fail> {
    let (accounts, groups) = known()?;
    if !apply(Path::new(PATH), change, &accounts, &groups)? {
        println!("ssh-server: {PATH} says so already; nothing is changed");
        return Ok(0);
    }
    match reload() {
        Ok(Reloaded::Asked) => println!(
            "ssh-server: {PATH} is changed, and sshd reads it for the connections that come \
             next; the connections that are open stay as they are"
        ),
        Ok(Reloaded::NotRunning) => {
            println!(
                "ssh-server: {PATH} is changed; sshd is not running, and reads it when it starts"
            );
        }
        Err(refused) => {
            return Err(fail(format!(
                "{PATH} is changed, but sshd was not asked to read it: {}",
                refused.message
            )));
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(ToString::to_string).collect()
    }

    #[test]
    fn the_words_make_the_change_they_name() {
        let got = change(&words("passwords yes keys no allow-users alice bob allow-groups staff"));
        assert_eq!(
            got,
            Ok(Change {
                passwords: Some(true),
                keys: Some(false),
                allow_users: Some(Names::Only(words("alice bob"))),
                allow_groups: Some(Names::Only(words("staff"))),
            })
        );
        let got = change(&words("allow-groups everyone allow-users everyone")).unwrap();
        assert_eq!(got.allow_users, Some(Names::Everyone));
        assert_eq!(got.allow_groups, Some(Names::Everyone));
        assert_eq!(change(&words("keys yes")).unwrap().passwords, None);
    }

    #[test]
    fn a_change_that_is_not_made_of_the_keywords_is_a_mistake() {
        for bad in [
            "",
            "alice",
            "passwords",
            "passwords maybe",
            "passwords yes no",
            "passwords yes passwords no",
            "keys YES",
            "allow-users",
            "allow-users keys yes",
            "allow-users alice everyone",
            "allow-users everyone alice",
            "allow-users alice bob alice",
            "allow-groups everyone everyone",
            "deny-users alice",
        ] {
            assert!(change(&words(bad)).is_err(), "{bad:?}");
        }
    }

    /// A folder of its own for each call, as the tests run at once.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new() -> Folder {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("ssh-server-{}-{call}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Folder(dir)
        }

        fn file(&self) -> std::path::PathBuf {
            self.0.join("sshd_config")
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    fn passwords(on: bool) -> Change {
        Change { passwords: Some(on), ..Change::default() }
    }

    #[test]
    fn a_change_is_written_whole_with_the_mode_the_file_has() {
        use std::os::unix::fs::PermissionsExt;
        let folder = Folder::new();
        fs::write(folder.file(), "# kept\nPasswordAuthentication no\n").unwrap();
        fs::set_permissions(folder.file(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(apply(&folder.file(), &passwords(true), &[], &[]).unwrap());
        assert_eq!(
            fs::read_to_string(folder.file()).unwrap(),
            "# kept\nPasswordAuthentication yes\n"
        );
        assert_eq!(mode_of(&folder.file()), 0o640);
        // Nothing is left beside it.
        assert_eq!(folder.names(), ["sshd_config"]);
    }

    #[test]
    fn a_file_that_is_not_there_is_made_with_the_default_file_s_mode() {
        let folder = Folder::new();
        assert!(apply(&folder.file(), &passwords(true), &[], &[]).unwrap());
        assert_eq!(fs::read_to_string(folder.file()).unwrap(), "PasswordAuthentication yes\n");
        assert_eq!(mode_of(&folder.file()), MODE);
    }

    #[test]
    fn a_file_that_says_so_already_is_not_written() {
        let folder = Folder::new();
        assert!(!apply(&folder.file(), &passwords(false), &[], &[]).unwrap());
        assert!(!folder.file().exists());
        fs::write(folder.file(), "PasswordAuthentication yes\n").unwrap();
        assert!(!apply(&folder.file(), &passwords(true), &[], &[]).unwrap());
        assert_eq!(folder.names(), ["sshd_config"]);
    }

    #[test]
    fn a_refused_change_leaves_the_file_as_it_was() {
        let folder = Folder::new();
        let text = "Port 22\nPasswordAuthentication yes\n";
        fs::write(folder.file(), text).unwrap();
        let refused = apply(&folder.file(), &passwords(false), &[], &[]).unwrap_err();
        assert!(refused.message.contains("line 1"), "{}", refused.message);
        assert!(refused.message.ends_with("(NET-183)"), "{}", refused.message);
        let both = Change { passwords: Some(false), keys: Some(false), ..Change::default() };
        let good = "PasswordAuthentication yes\n";
        fs::write(folder.file(), good).unwrap();
        let refused = apply(&folder.file(), &both, &[], &[]).unwrap_err();
        assert!(
            refused.message.starts_with("nobody could sign in over SSH"),
            "{}",
            refused.message
        );
        let unknown =
            Change { allow_users: Some(Names::Only(words("mallory"))), ..Change::default() };
        let refused = apply(&folder.file(), &unknown, &[], &[]).unwrap_err();
        assert!(refused.message.starts_with("mallory is not an account"), "{}", refused.message);
        assert_eq!(fs::read_to_string(folder.file()).unwrap(), good);
        assert_eq!(folder.names(), ["sshd_config"]);
        assert_eq!((refused.code, refused.usage), (1, false));
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_error_and_a_missing_one_is_empty() {
        let folder = Folder::new();
        assert_eq!(read_config(&folder.file()).unwrap(), "");
        // A folder where the file should be.
        fs::create_dir(folder.file()).unwrap();
        assert!(read_config(&folder.file()).is_err());
    }

    #[test]
    fn a_name_that_is_a_keyword_ends_the_list() {
        // The words cannot tell an account called `keys` from the keyword.
        assert!(change(&words("allow-users alice keys")).is_err());
    }
}
