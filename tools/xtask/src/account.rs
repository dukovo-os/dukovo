// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob image --user` and `ob run --user`: an administrator's account in
//! the image's root, made before its first boot as authd(8) makes one
//! (ADR 0218), so that the machine starts with someone to sign in as.
//!
//! The password is asked at the terminal, unseen and twice, or read from
//! the first line of a file. No rules are forced on it: an empty or short
//! one gets the same warning `user add` gives, and at a terminal, a
//! question. Only its Argon2id hash goes into the image; the home is made
//! by authd at the first boot, since every file of an image is root's.

use std::io::{BufRead as _, IsTerminal as _, Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

use accounts::hash::{self, Cost};

use crate::{Args, Context as _, Ctx, Error, Result, bail, userspace};

/// An account to make in an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Its name.
    pub name: String,
    /// Its full name, which may be empty.
    pub full_name: String,
    /// Its password's hash, a PHC string; empty for an empty password.
    pub hash: String,
}

/// What `--user`, `--full-name`, `--password-file` and `--test-cost`
/// said.
#[derive(Debug, Default)]
pub struct Options {
    name: Option<String>,
    full_name: Option<String>,
    file: Option<String>,
    test_cost: bool,
}

impl Options {
    /// Reads them from `args`.
    pub fn from_args(args: &mut Args) -> Result<Options> {
        let options = Options {
            name: args.value("--user")?,
            full_name: args.value("--full-name")?,
            file: args.value("--password-file")?,
            test_cost: args.flag("--test-cost"),
        };
        let rest = options.full_name.is_some() || options.file.is_some() || options.test_cost;
        if options.name.is_none() && rest {
            bail!("--full-name, --password-file and --test-cost go with --user <name>");
        }
        Ok(options)
    }

    /// Whether they make an account.
    pub fn any(&self) -> bool {
        self.name.is_some()
    }

    /// The account they make, if they make one, its password asked for
    /// unless it is in a file: before the build, not after it. What the
    /// defaults hold already, the names of root and the administrators,
    /// is refused.
    pub fn account(self, ctx: &Ctx) -> Result<Option<Account>> {
        let Options { name: Some(name), full_name, file, test_cost } = self else {
            return Ok(None);
        };
        let full_name = full_name.unwrap_or_default();
        let (mut users, mut groups) = read_tables(&ctx.root.join(userspace::DEFAULTS))?;
        accounts::add(&mut users, &mut groups, &name, &full_name, true, &[]).map_err(Error::new)?;
        let mut password = match file {
            Some(path) => read_password_file(Path::new(&path))?,
            None => ask_password(&name)?,
        };
        let cost = if test_cost { Cost::TEST } else { Cost::DEFAULT };
        let hashed = if password.is_empty() {
            Ok(String::new())
        } else {
            salt().and_then(|salt| hash::hash(&password, &salt, cost).map_err(Error::new))
        };
        accounts::wipe(&mut password);
        Ok(Some(Account { name, full_name, hash: hashed? }))
    }
}

/// The passwd and group files in `dir`.
fn read_tables(
    dir: &Path,
) -> Result<(accounts::Table<accounts::User>, accounts::Table<accounts::Group>)> {
    let read = |name: &str| {
        let path = dir.join(name);
        std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))
    };
    Ok((accounts::users(&read("passwd")?), accounts::groups(&read("group")?)))
}

/// The first line of the file at `path`, without its newline.
fn read_password_file(path: &Path) -> Result<Vec<u8>> {
    let mut text = std::fs::read(path).context(|| format!("reading {}", path.display()))?;
    let end = text.iter().position(|&b| b == b'\n').unwrap_or(text.len());
    let mut line = text[..end].strip_suffix(b"\r").unwrap_or(&text[..end]).to_vec();
    accounts::wipe(&mut text);
    if line.len() > accounts::PASSWORD_MAX {
        accounts::wipe(&mut line);
        bail!("a password has at most {} bytes", accounts::PASSWORD_MAX);
    }
    Ok(line)
}

/// A new password for `name`, asked at the terminal twice, unseen, then
/// warned about if it is empty or short and kept only if the person says
/// so; or a line of the standard input when it is no terminal.
fn ask_password(name: &str) -> Result<Vec<u8>> {
    let terminal = std::io::stdin().is_terminal();
    if !terminal {
        let mut line = read_line(false)?;
        warn(name, &line, false).inspect_err(|_| accounts::wipe(&mut line))?;
        return Ok(line);
    }
    eprintln!("Making an administrator's account for {name} in the image.");
    let mut password = read_line(true)?;
    let mut again =
        read_line_after("The same again: ", true).inspect_err(|_| accounts::wipe(&mut password))?;
    let same = again == password;
    accounts::wipe(&mut again);
    if !same {
        accounts::wipe(&mut password);
        bail!("the two were not the same; no image was made");
    }
    warn(name, &password, true).inspect_err(|_| accounts::wipe(&mut password))?;
    Ok(password)
}

/// Prints the warning `password` gets, if it gets one, and at a
/// `terminal`, asks whether to keep it.
fn warn(name: &str, password: &[u8], terminal: bool) -> Result<()> {
    let Some(warning) = accounts::password_warning(name, &String::from_utf8_lossy(password)) else {
        return Ok(());
    };
    eprintln!("{warning}");
    if !terminal {
        return Ok(());
    }
    loop {
        let answer = read_line_after("Use it anyway? [y/N] ", false)?;
        match answer.trim_ascii() {
            b"y" | b"Y" | b"yes" | b"Yes" | b"YES" => return Ok(()),
            b"" | b"n" | b"N" | b"no" | b"No" | b"NO" => bail!("no image was made"),
            _ => {}
        }
    }
}

/// "New password: " and a line read, unseen if `unseen`.
fn read_line(unseen: bool) -> Result<Vec<u8>> {
    read_line_after(if unseen { "New password: " } else { "" }, unseen)
}

/// Shows `prompt` and reads a line of the standard input, with the
/// terminal's echo off if `unseen`: through stty(1), as the build tool
/// has no unsafe code to call termios with. Were ob stopped while it is
/// off, the shell puts the terminal back as it was.
fn read_line_after(prompt: &str, unseen: bool) -> Result<Vec<u8>> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let stty = |setting: &str| {
        Command::new("stty")
            .arg(setting)
            .stdin(Stdio::inherit())
            .status()
            .is_ok_and(|status| status.success())
    };
    let quiet = unseen && stty("-echo");
    let mut line = Vec::new();
    let read = std::io::stdin().lock().read_until(b'\n', &mut line);
    if quiet {
        stty("echo");
        eprintln!();
    }
    match read {
        Ok(0) => bail!("the standard input ended before a password; no image was made"),
        Ok(_) => {
            if line.ends_with(b"\n") {
                line.pop();
            }
            if line.len() > accounts::PASSWORD_MAX {
                accounts::wipe(&mut line);
                bail!("a password has at most {} bytes", accounts::PASSWORD_MAX);
            }
            Ok(line)
        }
        Err(err) => {
            accounts::wipe(&mut line);
            Err(Error::new(format!("reading the standard input: {err}")))
        }
    }
}

/// A salt from the host's random numbers.
fn salt() -> Result<[u8; hash::SALT]> {
    let mut salt = [0u8; hash::SALT];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut salt))
        .context(|| "reading /dev/urandom".to_string())?;
    Ok(salt)
}

/// Makes `account` in a root being staged, as an administrator: its user
/// and group, and the administrators' group, in the `settings`
/// subvolume's passwd and group, which start as the defaults the `system`
/// subvolume has, so that Spark adds none over them at the first boot;
/// and its hash in the `state` subvolume's `authd/passwords`, which only
/// authd reads. A second account (`--user` with `--lab`) is added to what
/// the first wrote. Gives the account's user and group numbers.
pub fn add(system: &Path, settings: &Path, state: &Path, account: &Account) -> Result<(u32, u32)> {
    let defaults = userspace::DEFAULTS_DIR.strip_prefix("system/").expect("in the system");
    let tables = if settings.join("passwd").exists() {
        settings.to_path_buf()
    } else {
        system.join(defaults)
    };
    let (mut users, mut groups) = read_tables(&tables)?;
    let Account { name, full_name, hash } = account;
    let user =
        accounts::add(&mut users, &mut groups, name, full_name, true, &[]).map_err(Error::new)?;
    let authd = state.join("authd");
    let passwords_file = authd.join("passwords");
    let written = std::fs::read_to_string(&passwords_file).unwrap_or_default();
    let mut passwords = accounts::passwords(&written);
    passwords.push(accounts::Password { name: name.clone(), hash: hash.clone() });
    write(&settings.join("passwd"), &users.text(), 0o644)?;
    write(&settings.join("group"), &groups.text(), 0o644)?;
    if !authd.exists() {
        crate::process::create_dir(&authd)?;
    }
    set_mode(&authd, 0o700)?;
    write(&passwords_file, &passwords.text(), 0o600)?;
    Ok((user.uid, user.gid))
}

/// The hash of `password`, as authd keeps it, with a new salt.
pub fn hash_password(password: &[u8]) -> Result<String> {
    salt().and_then(|salt| hash::hash(password, &salt, Cost::DEFAULT).map_err(Error::new))
}

/// The hash of a password no one knows: 32 random bytes, thrown away
/// once hashed. An account with it signs in only with a key.
pub fn unknown_password_hash() -> Result<String> {
    let mut password = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut password))
        .context(|| "reading /dev/urandom".to_string())?;
    let hashed =
        salt().and_then(|salt| hash::hash(&password, &salt, Cost::DEFAULT).map_err(Error::new));
    password.fill(0);
    hashed
}

/// Writes `text` to the file at `path` with `mode`.
fn write(path: &Path, text: &str, mode: u32) -> Result<()> {
    std::fs::write(path, text).context(|| format!("writing {}", path.display()))?;
    set_mode(path, mode)
}

/// Sets the mode of `path`, whatever the umask.
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .context(|| format!("setting the mode of {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_an_administrator_over_the_defaults() {
        let staging = std::env::temp_dir().join(format!("ob-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let defaults = staging.join("@system.a/resources/default-settings");
        for dir in [&defaults, &staging.join("@settings"), &staging.join("@state")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for name in ["passwd", "group"] {
            std::fs::copy(root.join("userspace/defaults").join(name), defaults.join(name)).unwrap();
        }
        let account = Account {
            name: "chase".into(),
            full_name: "Chase".into(),
            hash: "$argon2id$v=19$m=8192,t=1,p=1$c2FsdA$aGFzaA".into(),
        };
        let add = |account: &Account| {
            let at = |name: &str| staging.join(name);
            super::add(&at("@system.a"), &at("@settings"), &at("@state"), account)
        };
        assert_eq!(add(&account).unwrap(), (1000, 1000));
        let read = |path: &str| std::fs::read_to_string(staging.join(path)).unwrap();
        assert_eq!(
            read("@settings/passwd"),
            "root:x:0:0:root:/:/system/commands/nukesh\n\
             chase:x:1000:1000:Chase:/users/chase:/system/commands/nukesh\n"
        );
        assert_eq!(read("@settings/group"), "root:x:0:\nadmins:x:10:chase\nchase:x:1000:\n");
        assert_eq!(read("@state/authd/passwords"), format!("chase:{}:\n", account.hash));
        let mode = |path: &str| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(staging.join(path)).unwrap().permissions().mode() & 0o7777
        };
        assert_eq!(
            [mode("@settings/passwd"), mode("@state/authd"), mode("@state/authd/passwords")],
            [0o644, 0o700, 0o600]
        );
        // A second account goes beside the first.
        let lab = Account { name: "lab".into(), full_name: "Dukovo Lab".into(), hash: "x".into() };
        assert_eq!(add(&lab).unwrap(), (1001, 1001));
        assert!(
            read("@settings/passwd")
                .ends_with("lab:x:1001:1001:Dukovo Lab:/users/lab:/system/commands/nukesh\n")
        );
        assert_eq!(read("@state/authd/passwords"), format!("chase:{}:\nlab:x:\n", account.hash));
        // The defaults' names are taken.
        let root_account = Account { name: "root".into(), ..account };
        assert!(add(&root_account).is_err());
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[test]
    fn password_files_give_their_first_line() {
        let path = std::env::temp_dir().join(format!("ob-password-{}", std::process::id()));
        for (text, password) in [
            (&b"correct horse\nnext\n"[..], &b"correct horse"[..]),
            (b"windows\r\n", b"windows"),
            (b"", b""),
            (b"\n", b""),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(read_password_file(&path).unwrap(), password);
        }
        std::fs::remove_file(&path).unwrap();
    }
}
