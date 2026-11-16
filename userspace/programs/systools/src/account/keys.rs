// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `user key add [NAME] LINE` and `user key remove [NAME] FINGERPRINT`: an
//! account's keys in `~/.ssh/authorized_keys`, which sshd(8) and authd(8)
//! read for the keys that sign it in over SSH.
//!
//! Root edits the file for any account, so nothing in the person's home may
//! lead it elsewhere: every name is looked up beneath a descriptor, never
//! through a link, and the file is written whole into a new one beside it
//! and renamed over it. The folder is 0700 and the file 0600, theirs, as
//! sshd's strict mode needs them. A line is checked by the parser authd
//! reads keys with (`sshkeys.rs`) and is taken only if authd would take the
//! key from it.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

use accounts::User;

use super::{Accounts, own_name};
use crate::account_errors;
use crate::args::{Arg, Args};
use crate::bundles::folder::{Dir, check, fd_stat};
use crate::sshkeys::{self, Entry};
use crate::util::{self, Fail, fail, usage};

/// The mode of the folder `.ssh`, and of the file in it.
const FOLDER_MODE: libc::mode_t = 0o700;
const FILE_MODE: libc::mode_t = 0o600;

/// The file's name in the folder.
const FILE: &str = "authorized_keys";

/// Whether `line` is a public key that Dukovo takes: one line as ssh-keygen
/// writes it, of a kind authd signs in with, and with no options before
/// the key. The key, or what is wrong with the line, said so as to follow
/// "...:" (ACC-103).
pub(crate) fn check_line(line: &str) -> Result<Entry, String> {
    let line = line.trim();
    if line.chars().any(char::is_control) {
        return Err("that is not one line".into());
    }
    let Some(entry) = sshkeys::entries(line).into_iter().next() else {
        return Err("that is not a public key".into());
    };
    if entry.options {
        Err("that line has options before its key".into())
    } else if !sshkeys::ALGORITHMS.contains(&entry.algorithm.as_str()) {
        Err(format!("keys of the kind {} are not taken", entry.algorithm))
    } else if !sshkeys::whole(&entry.key) {
        Err("that key is not whole: copy the line again".into())
    } else {
        Ok(entry)
    }
}

/// Whether `text` is a fingerprint as `ssh-keygen -l` and `user show` give
/// it: `SHA256:` and the 43 characters of a digest's base64 without padding.
pub(crate) fn fingerprint_ok(text: &str) -> bool {
    text.strip_prefix("SHA256:").is_some_and(|digest| {
        digest.len() == 43
            && digest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    })
}

/// `user key`: adds or removes a key.
pub(super) fn key(args: &mut Args) -> Result<i32, Fail> {
    let help = "Usage: user key add [name] line\n       user key remove [name] fingerprint";
    let adding = match args.subcommand().as_deref().and_then(|word| word.to_str()) {
        Some("add") => true,
        Some("remove") => false,
        _ => return Err(usage(format!("which? add or remove\n{help}"))),
    };
    let mut given = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if given.len() < 2 => given
                .push(operand.into_string().map_err(|_| fail("a key and a name are text".into()))?),
            other => return Err(args.unknown(&other)),
        }
    }
    // One operand is the line or the fingerprint, and the account is the
    // caller's own; two are the account and it.
    let (name, what) = match <[String; 2]>::try_from(given) {
        Ok([name, what]) => (Some(name), what),
        Err(mut one) if one.len() == 1 => (None, one.remove(0)),
        Err(_) => return Err(usage(help.to_string())),
    };
    let name = match name {
        Some(name) => name,
        None => own_name()?,
    };
    let user = Accounts::read()?.user(&name)?.clone();
    let own = util::own_user();
    if own != 0 && user.uid != own {
        return Err(fail(format!(
            "only {name}, or an administrator inside elevate, changes {name}'s keys"
        )));
    }
    if !user.is_person() {
        return Err(fail(format!("{name} is not a person's account")));
    }
    if adding {
        let entry = check_line(&what).map_err(|why| account_errors::key_refused(&why))?;
        let print = sshkeys::fingerprint(&entry.key);
        match edit(&user, &Change::Add(entry, what.trim()))? {
            Outcome::Already => println!("{name} has that key already: nothing changed."),
            _ => println!("Added {print} to {name}'s keys."),
        }
    } else {
        if !fingerprint_ok(&what) {
            return Err(usage(format!(
                "`{what}` is not a fingerprint: it is SHA256: and 43 characters, as `user show \
                 {name}` gives it"
            )));
        }
        match edit(&user, &Change::Remove(&what))? {
            Outcome::Removed(0) => return Err(account_errors::no_such_key(&name, &what)),
            Outcome::Removed(1) => println!("Removed {what} from {name}'s keys."),
            Outcome::Removed(lines) => {
                println!("Removed {what}, {lines} lines of it, from {name}'s keys.");
            }
            _ => {}
        }
    }
    Ok(0)
}

/// What to do to the file.
enum Change<'a> {
    /// Add this key, as this line.
    Add(Entry, &'a str),
    /// Remove the keys with this fingerprint.
    Remove(&'a str),
}

/// What came of it.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The line was added.
    Added,
    /// Its key was there already, with no options.
    Already,
    /// This many lines went.
    Removed(usize),
}

/// What `text` is with the key of `entry` added as `line`, or `None` if
/// the key is there already: a line with options does not count, since
/// authd passes it over.
fn added(text: &str, entry: &Entry, line: &str) -> Option<String> {
    if sshkeys::entries(text).iter().any(|e| !e.options && e.key == entry.key) {
        return None;
    }
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    Some(text)
}

/// `text` with the lines of the keys with `fingerprint` gone, and how many.
/// The other lines, comments and blank ones too, stay as they are.
fn removed(text: &str, fingerprint: &str) -> (String, usize) {
    let gone: Vec<usize> = sshkeys::entries(text)
        .iter()
        .filter(|e| sshkeys::fingerprint(&e.key) == fingerprint)
        .map(|e| e.line)
        .collect();
    let mut kept = String::new();
    for (at, line) in text.lines().enumerate() {
        if !gone.contains(&(at + 1)) {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    (kept, gone.len())
}

/// Makes `change` to `user`'s `authorized_keys`.
fn edit(user: &User, change: &Change) -> Result<Outcome, Fail> {
    let adding = matches!(change, Change::Add(..));
    let shown = format!("{}/.ssh", user.home.trim_end_matches('/'));
    let Some(folder) = folder(user, &shown, adding)? else {
        return Ok(Outcome::Removed(0));
    };
    let file = format!("{shown}/{FILE}");
    let text = read(&folder, user, &file)?;
    let (text, outcome) = match change {
        Change::Add(entry, line) => match added(&text, entry, line) {
            Some(text) => (text, Outcome::Added),
            None => return Ok(Outcome::Already),
        },
        Change::Remove(fingerprint) => match removed(&text, fingerprint) {
            (_, 0) => return Ok(Outcome::Removed(0)),
            (text, lines) => (text, Outcome::Removed(lines)),
        },
    };
    if text.len() as u64 > sshkeys::FILE_MAX {
        return Err(fail(format!("{file} would be over {} bytes", sshkeys::FILE_MAX)));
    }
    replace(&folder, user, &text).map_err(|e| util::failed(&file, e))?;
    Ok(outcome)
}

/// `user`'s `.ssh` folder, reached from their home without a link and
/// checked as sshd's strict mode checks it. If it is not there, it is made
/// when `make`, and otherwise `None`.
fn folder(user: &User, shown: &str, make: bool) -> Result<Option<Dir>, Fail> {
    // In Rescue, the home in the installed system's root.
    let home = accounts::rescue::installed(&user.home);
    let home = home.to_str().ok_or_else(|| fail(format!("{}: not a name", user.home)))?;
    let home = Dir::open(home).map_err(|e| util::failed(&user.home, e))?;
    sshkeys::safe(&home.0, user, &user.home).map_err(fail)?;
    let made = match home.stat_at(".ssh").map_err(|e| util::failed(&shown, e))? {
        Some(_) => false,
        None if make => {
            home.make(".ssh", FOLDER_MODE).map_err(|e| util::failed(&shown, e))?;
            true
        }
        None => return Ok(None),
    };
    let folder = home.dir_at(".ssh").map_err(|e| util::failed(&shown, e))?;
    if made {
        own(&folder.0, user, FOLDER_MODE).map_err(|e| util::failed(&shown, e))?;
    }
    sshkeys::safe(&folder.0, user, shown).map_err(fail)?;
    Ok(Some(folder))
}

/// The text of the file in `folder`, empty if there is none, if it is a
/// file that is `user`'s or root's, that no one else may change, and that
/// has no other name. Root writes what it read into the person's new file,
/// so a second name, a hard link the person made to a file of root's
/// elsewhere, would copy that file's text to them.
fn read(folder: &Dir, user: &User, shown: &str) -> Result<String, Fail> {
    let fd = match folder.open_at(FILE, false) {
        Ok(fd) => fd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(util::failed(&shown, e)),
    };
    let meta = sshkeys::safe(&fd, user, shown).map_err(fail)?;
    if !meta.is_file() {
        return Err(fail(format!("{shown} is not a file")));
    }
    if meta.nlink() != 1 {
        return Err(fail(format!(
            "{shown} has another name, a hard link: remove the file, and add the keys again"
        )));
    }
    let mut text = String::new();
    File::from(fd)
        .take(sshkeys::FILE_MAX + 1)
        .read_to_string(&mut text)
        .map_err(|e| util::failed(&shown, e))?;
    if text.len() as u64 > sshkeys::FILE_MAX {
        return Err(fail(format!("{shown} is over {} bytes", sshkeys::FILE_MAX)));
    }
    Ok(text)
}

/// Makes `fd` `user`'s, with `mode`, changing the owner only if it is not
/// theirs already: a person changes nothing of what is theirs.
fn own(fd: &OwnedFd, user: &User, mode: libc::mode_t) -> io::Result<()> {
    let st = fd_stat(fd)?;
    if st.st_uid != user.uid || st.st_gid != user.gid {
        // SAFETY: fchown changes what the open descriptor names.
        check(unsafe { libc::fchown(fd.as_raw_fd(), user.uid, user.gid) })?;
    }
    // SAFETY: fchmod changes what the open descriptor names.
    check(unsafe { libc::fchmod(fd.as_raw_fd(), mode) })
}

/// Puts `text` in the file in `folder`: written whole to a new file, made
/// `user`'s with the mode sshd needs, and renamed over the old, so that the
/// file is never half written and no link in its place is followed.
fn replace(folder: &Dir, user: &User, text: &str) -> io::Result<()> {
    let temporary = format!(".{FILE}.{}", std::process::id());
    let fd = match folder.create_new(&temporary, FILE_MODE) {
        // What a command that was stopped left, if it was this number.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            folder.unlink(&temporary)?;
            folder.create_new(&temporary, FILE_MODE)?
        }
        made => made?,
    };
    let made = own(&fd, user, FILE_MODE).and_then(|()| {
        let mut file = File::from(fd);
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    made.and_then(|()| folder.rename(&temporary, FILE)).inspect_err(|_| {
        let _ = folder.unlink(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    /// Two Ed25519 keys, which differ in their last byte.
    const FIRST: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f laptop";
    const SECOND: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4g phone";
    const FIRST_PRINT: &str = "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA";

    fn entry(line: &str) -> Entry {
        check_line(line).unwrap()
    }

    #[test]
    fn a_line_is_taken_if_authd_would_take_its_key() {
        assert_eq!(entry(FIRST).comment, "laptop");
        assert_eq!(entry(&format!("  {FIRST}  \n")).comment, "laptop");
        // A key with no comment.
        let bare = FIRST.trim_end_matches(" laptop");
        assert_eq!(entry(bare).comment, "");
        for (line, why) in [
            ("", "that is not a public key"),
            ("hello", "that is not a public key"),
            ("ssh-dss AAAA…", "that is not a public key"),
            ("ssh-ed25519 AAAA", "that is not a public key"),
            // The kind's name and no key, and a key one byte short.
            ("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5", "that key is not whole: copy the line again"),
            (
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAHwABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4=",
                "that key is not whole: copy the line again",
            ),
            (&format!("{FIRST}\n{SECOND}"), "that is not one line"),
            (&format!("{FIRST}\0"), "that is not one line"),
            (&format!("command=\"x\" {FIRST}"), "that line has options before its key"),
            (&format!("ecdsa-sha2-nistp256 {}", &FIRST[12..]), "that is not a public key"),
        ] {
            assert_eq!(check_line(line).unwrap_err(), why, "{line:?}");
        }
        // A key of a kind that authd does not sign in with.
        let sk = "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fAAAABHNzaDo=";
        assert_eq!(
            check_line(sk).unwrap_err(),
            "keys of the kind sk-ssh-ed25519@openssh.com are not taken"
        );
    }

    #[test]
    fn a_fingerprint_is_sha256_and_43_characters() {
        assert!(fingerprint_ok(FIRST_PRINT));
        for bad in [
            "",
            "SHA256:",
            "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncC",
            "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCAA",
            "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSnc=A",
            "MD5:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA",
            "ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA",
        ] {
            assert!(!fingerprint_ok(bad), "{bad}");
        }
    }

    #[test]
    fn a_key_is_added_once_and_the_rest_of_the_file_stays() {
        let first = entry(FIRST);
        assert_eq!(added("", &first, FIRST).as_deref(), Some(format!("{FIRST}\n").as_str()));
        let file = format!("# mine\n\n{SECOND}");
        assert_eq!(
            added(&file, &first, FIRST).as_deref(),
            Some(format!("# mine\n\n{SECOND}\n{FIRST}\n").as_str())
        );
        assert_eq!(added(&format!("{FIRST}\n"), &first, FIRST), None);
        // The same key under another comment is the key.
        assert_eq!(added(&format!("{}\n", FIRST.replace("laptop", "old")), &first, FIRST), None);
        // With options, authd passes the line over: the key is not there.
        let restricted = format!("command=\"x\" {FIRST}\n");
        assert_eq!(
            added(&restricted, &first, FIRST).as_deref(),
            Some(format!("{restricted}{FIRST}\n").as_str())
        );
    }

    #[test]
    fn a_key_is_removed_wherever_it_is_and_the_rest_stays() {
        let file = format!("# mine\n{FIRST}\n\n{SECOND}\ncommand=\"x\" {FIRST}\n");
        let (text, lines) = removed(&file, FIRST_PRINT);
        assert_eq!((text.as_str(), lines), (format!("# mine\n\n{SECOND}\n").as_str(), 2));
        let (same, none) = removed(&file, "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCB");
        assert_eq!((same, none), (file, 0));
    }

    /// A home of the user running the tests, in a new folder.
    fn home(name: &str) -> (PathBuf, User) {
        let dir = std::env::temp_dir().join(format!("keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: getuid and getgid read this process's ids and cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let user = User {
            name: "me".into(),
            uid,
            gid,
            full_name: String::new(),
            home: dir.display().to_string(),
            shell: String::new(),
        };
        (dir, user)
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().mode() & 0o7777
    }

    fn key_of(line: &str) -> Change<'_> {
        Change::Add(entry(line), line)
    }

    #[test]
    fn the_file_is_made_with_the_modes_sshd_needs_and_changed_in_place() {
        let (dir, user) = home("edit");
        let file = dir.join(".ssh/authorized_keys");
        assert_eq!(edit(&user, &key_of(FIRST)).unwrap(), Outcome::Added);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{FIRST}\n"));
        assert_eq!((mode(&dir.join(".ssh")), mode(&file)), (0o700, 0o600));
        let owner = std::fs::metadata(&file).unwrap();
        assert_eq!((owner.uid(), owner.gid()), (user.uid, user.gid));
        assert_eq!(edit(&user, &key_of(FIRST)).unwrap(), Outcome::Already);
        assert_eq!(edit(&user, &key_of(SECOND)).unwrap(), Outcome::Added);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{FIRST}\n{SECOND}\n"));
        assert_eq!(edit(&user, &Change::Remove(FIRST_PRINT)).unwrap(), Outcome::Removed(1));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{SECOND}\n"));
        assert_eq!(edit(&user, &Change::Remove(FIRST_PRINT)).unwrap(), Outcome::Removed(0));
        // No temporary file is left.
        let names: Vec<_> =
            std::fs::read_dir(dir.join(".ssh")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["authorized_keys"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removing_from_an_account_with_no_folder_makes_none() {
        let (dir, user) = home("none");
        assert_eq!(edit(&user, &Change::Remove(FIRST_PRINT)).unwrap(), Outcome::Removed(0));
        assert!(!dir.join(".ssh").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_or_a_folder_others_may_change_is_not_written_through() {
        let (dir, user) = home("links");
        let ssh = dir.join(".ssh");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        // The file is a link: its target is left as it is.
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(elsewhere.join("target"), "x\n").unwrap();
        std::os::unix::fs::symlink(elsewhere.join("target"), ssh.join("authorized_keys")).unwrap();
        assert!(edit(&user, &key_of(FIRST)).is_err());
        assert_eq!(std::fs::read_to_string(elsewhere.join("target")).unwrap(), "x\n");
        // A temporary file's name that is a link is not followed either.
        std::fs::remove_file(ssh.join("authorized_keys")).unwrap();
        let temporary = ssh.join(format!(".authorized_keys.{}", std::process::id()));
        std::os::unix::fs::symlink(elsewhere.join("made"), &temporary).unwrap();
        assert_eq!(edit(&user, &key_of(FIRST)).unwrap(), Outcome::Added);
        assert!(!elsewhere.join("made").exists());
        // The folder is a link, or others may change it.
        std::fs::remove_dir_all(&ssh).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &ssh).unwrap();
        assert!(edit(&user, &key_of(FIRST)).is_err());
        std::fs::remove_file(&ssh).unwrap();
        std::fs::create_dir(&ssh).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o777)).unwrap();
        let why = edit(&user, &key_of(FIRST)).map(|_| ()).unwrap_err().message;
        assert!(why.contains("changed by others"), "{why}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_with_another_name_is_not_read_into_the_new_one() {
        let (dir, user) = home("hard");
        let ssh = dir.join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        // As a person's hard link to a file of root's would be.
        let other = dir.join("other");
        std::fs::write(&other, "not a key file\n").unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&other, ssh.join("authorized_keys")).unwrap();
        let why = edit(&user, &key_of(FIRST)).map(|_| ()).unwrap_err().message;
        assert!(why.contains("hard link"), "{why}");
        let why = edit(&user, &Change::Remove(FIRST_PRINT)).map(|_| ()).unwrap_err().message;
        assert!(why.contains("hard link"), "{why}");
        assert_eq!(
            std::fs::read_to_string(ssh.join("authorized_keys")).unwrap(),
            "not a key file\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_too_large_is_refused() {
        let (dir, user) = home("large");
        let ssh = dir.join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        let big = format!("# {}\n", "x".repeat(usize::try_from(sshkeys::FILE_MAX).unwrap()));
        std::fs::write(ssh.join("authorized_keys"), big).unwrap();
        let why = edit(&user, &key_of(FIRST)).map(|_| ()).unwrap_err().message;
        assert!(why.contains("is over"), "{why}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
