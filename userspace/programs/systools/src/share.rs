// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! share(1): folders shared with Windows PCs, Macs and Linux machines,
//! which smbd(8) serves over SMB 3, signed and encrypted (ADR 0228).
//!
//! - `share` and `share list` list the shares, and say whether sharing
//!   is on.
//! - `share add NAME FOLDER [--read-only] [--comment TEXT]` and `share
//!   remove NAME` change shares.toml(5), as root.
//! - `share on` and `share off`, as root, start smbd and have it start
//!   at each boot, or stop it, which ends every share session.
//! - `share password [--clear] [NAME]` sets your sharing password, the
//!   one a PC signs in with, after your login password; root sets
//!   anyone's. Only its NT hash is kept, by authd, apart from the login
//!   password's, which stays Argon2id alone.
//!
//! Every person with a sharing password may sign in, and sees in each
//! share what their own permissions let them, as at their own shell.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use accounts::request::{Answer, Request};
use manifest::service;
use manifest::shares::{self, Share, Shares};

use crate::account::{self, Input};
use crate::args::{Arg, Args};
use crate::power;
use crate::servicefiles;
use crate::util::{self, Fail, fail, usage};

/// smbd's service file.
const SERVICE: &str = "smbd";
/// Where the system's own service files are kept, as Spark copies them.
const DEFAULTS: &str = "/system/resources/default-settings/spark/services";

const HELP: &str = "Usage: share [list]
       share add [--read-only] [--comment TEXT] NAME FOLDER
       share remove NAME
       share on | off
       share password [--clear] [NAME]

Shares folders with Windows PCs and other computers over SMB 3, signed
and encrypted, by smbd(8). A PC signs in with a person's name and their
sharing password, and sees in each share what that person may see.

  list      the shares, and whether sharing is on
  add       shares FOLDER as NAME; --read-only lets no one change it
  remove    stops sharing NAME
  on, off   starts smbd, now and at each boot, or stops it and ends
            every session
  password  sets your sharing password, asked after your login password;
            --clear removes it. Root sets anyone's.";

/// share(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(l)) if l == "help" => {
                println!("{HELP}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => list(args),
        };
    };
    match subcommand.to_str() {
        Some("list") => list(args),
        Some("add") => add(args),
        Some("remove") => remove(args),
        Some("on") => turn(args, true),
        Some("off") => turn(args, false),
        Some("password") => password(args),
        _ => Err(usage(format!("no such command: {}\n{HELP}", subcommand.to_string_lossy()))),
    }
}

/// Fails unless the program runs as root.
fn root_only(what: &str) -> Result<(), Fail> {
    if util::own_user() == 0 {
        Ok(())
    } else {
        Err(fail(format!("only root {what}: elevate first, as `sudo share ...`")))
    }
}

/// Fails if anything is left in `args`.
fn nothing_more(args: &mut Args) -> Result<(), Fail> {
    match args.next()? {
        None => Ok(()),
        Some(other) => Err(args.unknown(&other)),
    }
}

/// The shares in shares.toml; none if it is not there.
fn read() -> Result<Shares, Fail> {
    match fs::read_to_string(shares::PATH) {
        Ok(text) => shares::from_toml(&text).map_err(|e| fail(format!("{}: {e}", shares::PATH))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Shares::default()),
        Err(e) => Err(util::failed(&shares::PATH, e)),
    }
}

/// Writes `text` to `path` whole, through a new file renamed over it,
/// with `mode`.
fn write(path: &str, text: &str, mode: u32) -> Result<(), Fail> {
    let path = Path::new(path);
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let new = path.with_file_name(format!(".{name}.new"));
    let _ = fs::remove_file(&new);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&new)
        .and_then(|mut file| file.write_all(text.as_bytes()).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&new, path));
    if let Err(e) = written {
        let _ = fs::remove_file(&new);
        return Err(util::failed(&path.display(), e));
    }
    Ok(())
}

/// The path of smbd's service file.
fn service_path() -> String {
    format!("{}/{SERVICE}.toml", servicefiles::DIR)
}

/// smbd's service file, from the system's own if it is not there.
fn service_text() -> Result<String, Fail> {
    let path = service_path();
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let default = format!("{DEFAULTS}/{SERVICE}.toml");
            fs::read_to_string(&default).map_err(|e| util::failed(&default, e))
        }
        Err(e) => Err(util::failed(&path, e)),
    }
}

/// Whether sharing is on: smbd starts at boot.
fn is_on() -> bool {
    service_text().ok().and_then(|text| service::from_toml(&text).ok()).is_some_and(|s| s.enabled)
}

/// `text`, a service file, with its `enabled` line saying `on`, or one
/// added after its `program` line.
fn with_enabled(text: &str, on: bool) -> String {
    let line = format!("enabled = {on}");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    // The top-level keys: those before the first table.
    let top = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let key_of = |l: &str| l.split_once('=').map(|(key, _)| key.trim().to_string());
    if let Some(at) = lines[..top].iter().position(|l| key_of(l).as_deref() == Some("enabled")) {
        lines[at] = line;
    } else {
        let at = lines[..top]
            .iter()
            .position(|l| key_of(l).as_deref() == Some("program"))
            .map_or(0, |at| at + 1);
        lines.insert(at, line);
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// `share` and `share list`.
fn list(args: &mut Args) -> Result<i32, Fail> {
    nothing_more(args)?;
    let shares = read()?;
    if is_on() {
        println!("Sharing is on: smbd(8) serves these shares on port 445.");
    } else {
        println!("Sharing is off; `share on` turns it on.");
    }
    if shares.shares.is_empty() {
        println!("No folder is shared; `share add NAME FOLDER` shares one.");
        return Ok(0);
    }
    let width = shares.shares.iter().map(|s| s.name.chars().count()).max().unwrap_or(4).max(4);
    let folders = shares.shares.iter().map(|s| s.folder.chars().count()).max().unwrap_or(6).max(6);
    println!("{:width$}  {:folders$}  {:10}  COMMENT", "NAME", "FOLDER", "ACCESS");
    for share in &shares.shares {
        let access = if share.read_only { "read-only" } else { "read-write" };
        println!(
            "{:width$}  {:folders$}  {access:10}  {}",
            share.name, share.folder, share.comment
        );
    }
    Ok(0)
}

/// `share add`.
fn add(args: &mut Args) -> Result<i32, Fail> {
    let (mut read_only, mut comment, mut operands) = (false, String::new(), Vec::new());
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "read-only" => read_only = true,
            Arg::Long(l) if l == "comment" => {
                comment = args.value()?.to_string_lossy().into_owned();
            }
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let [name, folder] = <[String; 2]>::try_from(operands)
        .map_err(|_| usage("share add takes a name and a folder: share add NAME FOLDER".into()))?;
    root_only("shares folders")?;
    let folder = fs::canonicalize(&folder).map_err(|e| util::failed(&folder, e))?;
    if !folder.is_dir() {
        return Err(fail(format!("{} is not a folder", folder.display())));
    }
    let share = Share { name, folder: folder.to_string_lossy().into_owned(), comment, read_only };
    let mut all = read()?;
    all.add(share.clone()).map_err(fail)?;
    write(shares::PATH, &all.to_toml(), 0o644)?;
    let how = if share.read_only { "read-only" } else { "read-write" };
    println!("Shared {} as {}, {how}.", share.folder, share.name);
    if !is_on() {
        println!("Sharing is off; `share on` turns it on.");
    }
    Ok(0)
}

/// `share remove`.
fn remove(args: &mut Args) -> Result<i32, Fail> {
    let mut names = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) => names.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let [name] = <[String; 1]>::try_from(names)
        .map_err(|_| usage("share remove takes a share's name: share remove NAME".into()))?;
    root_only("stops sharing folders")?;
    let mut all = read()?;
    // By its own name, which may differ from `name` in case.
    let Some(shared) = all.find(&name).map(|share| share.name.clone()) else {
        return Err(fail(format!("no share is called {name}")));
    };
    all.remove(&name);
    write(shares::PATH, &all.to_toml(), 0o644)?;
    println!("Stopped sharing {shared}; PCs connected to it are told so.");
    Ok(0)
}

/// `share on` and `share off`.
fn turn(args: &mut Args, on: bool) -> Result<i32, Fail> {
    nothing_more(args)?;
    root_only(if on { "turns sharing on" } else { "turns sharing off" })?;
    let text = with_enabled(&service_text()?, on);
    service::from_toml(&text).map_err(|e| fail(format!("{}: {e}", service_path())))?;
    write(&service_path(), &text, 0o644)?;
    let verb = if on { "start" } else { "stop" };
    let answer = power::request(&format!("{verb} {SERVICE}"))?;
    if let Some(why) = answer.strip_prefix("error: ") {
        return Err(fail(format!("cannot {verb} smbd: {}", why.trim_end())));
    }
    if on {
        println!("Sharing is on: smbd(8) serves the shares on port 445, and starts at each boot.");
        if read()?.shares.is_empty() {
            println!("No folder is shared yet; `share add NAME FOLDER` shares one.");
        }
        println!("Each person signs in with their sharing password: `share password`.");
        return Ok(0);
    }
    // smbd ends them as it stops; this ends any it could not.
    let ended = match account::ask(&Request::EndShares)? {
        Answer::Done(count) => count,
        Answer::Refused(why) => return Err(fail(why)),
    };
    println!(
        "Sharing is off; {} share sessions ended.",
        if ended.is_empty() { "0" } else { &ended }
    );
    Ok(0)
}

/// `share password`.
fn password(args: &mut Args) -> Result<i32, Fail> {
    let (mut clear, mut name) = (false, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "clear" => clear = true,
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let name = match name {
        Some(name) => name,
        None => account::own_name()?,
    };
    let root = util::own_user() == 0;
    let mut input = Input::new();
    if let Input::Terminal = input {
        if clear {
            eprintln!(
                "Clearing the sharing password of {name}: PCs can no longer sign in as {name}."
            );
        } else {
            eprintln!(
                "Setting the sharing password of {name}, which PCs sign in to the shares with."
            );
        }
    }
    // Root sets anyone's without theirs.
    let current = if root { Vec::new() } else { input.password("Your login password: ")? };
    let new = if clear { None } else { Some(input.new_password(&name)?) };
    match account::ask(&Request::SharePassword { name: name.clone(), current, new })? {
        Answer::Done(_) if clear => eprintln!("The sharing password of {name} is cleared."),
        Answer::Done(_) => eprintln!("The sharing password of {name} is set."),
        Answer::Refused(why) => return Err(fail(why)),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_is_set_in_place_or_after_program() {
        let file = "# smbd\nservice_version = 1\nprogram = \"/system/commands/smbd\"\nenabled = false\n\n[permissions]\nenabled = 1\n";
        let on = with_enabled(file, true);
        assert!(on.contains("\nenabled = true\n\n[permissions]\nenabled = 1\n"), "{on}");
        assert_eq!(with_enabled(&on, false), file);
        let without = "service_version = 1\nprogram = \"/x\"\norder = 40\n";
        assert_eq!(
            with_enabled(without, true),
            "service_version = 1\nprogram = \"/x\"\nenabled = true\norder = 40\n"
        );
    }
}
