// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! share(1): folders shared with Windows PCs, Macs and Linux machines,
//! which smbd(8) serves over SMB 3, signed and encrypted (ADR 0228), and
//! nfsd(8) over NFS 4.2 to the networks a share names (ADR 0343).
//!
//! - `share` and `share list` list the shares, and say whether sharing
//!   is on.
//! - `share add NAME FOLDER [--read-only] [--comment TEXT] [--nfs
//!   NETWORK]...` and `share remove NAME` change shares.toml(5), as root.
//! - `share on` and `share off`, as root, start smbd and have it start
//!   at each boot, or stop it, which ends every share session.
//! - `share nfs on` and `share nfs off` do the same for nfsd.
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
/// nfsd's.
const NFS_SERVICE: &str = "nfsd";
/// Where the system's own service files are kept, as Spark copies them.
const DEFAULTS: &str = "/system/resources/default-settings/spark/services";

const HELP: &str = "Usage: share [list]
       share add [--read-only] [--comment TEXT] [--nfs NETWORK]... NAME FOLDER
       share remove NAME
       share on | off
       share nfs on | off
       share password [--clear] [NAME]

Shares folders with Windows PCs and other computers over SMB 3, signed
and encrypted, by smbd(8). A PC signs in with a person's name and their
sharing password, and sees in each share what that person may see.

  list      the shares, and whether sharing is on
  add       shares FOLDER as NAME; --read-only lets no one change it;
            --nfs lets machines on NETWORK (192.168.1.0/24, or one
            address) mount it over NFS, which takes them at their word
            for who they are
  remove    stops sharing NAME
  on, off   starts smbd, now and at each boot, or stops it and ends
            every session
  nfs       the same for nfsd(8), NFS 4.2's server, off at first
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
        Some("on") => turn(args, SERVICE, true),
        Some("off") => turn(args, SERVICE, false),
        Some("nfs") => nfs(args),
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

/// The path of `service`'s service file.
fn service_path(service: &str) -> String {
    format!("{}/{service}.toml", servicefiles::DIR)
}

/// `service`'s service file, from the system's own if it is not there.
fn service_text(service: &str) -> Result<String, Fail> {
    let path = service_path(service);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let default = format!("{DEFAULTS}/{service}.toml");
            fs::read_to_string(&default).map_err(|e| util::failed(&default, e))
        }
        Err(e) => Err(util::failed(&path, e)),
    }
}

/// Whether `service` starts at boot: smbd for sharing, nfsd for NFS.
fn is_on(service: &str) -> bool {
    service_text(service)
        .ok()
        .and_then(|text| service::from_toml(&text).ok())
        .is_some_and(|s| s.enabled)
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
    if is_on(SERVICE) {
        println!("Sharing is on: smbd(8) serves these shares on port 445.");
    } else {
        println!("Sharing is off; `share on` turns it on.");
    }
    let nfs = shares.shares.iter().any(|s| !s.nfs.is_empty());
    if is_on(NFS_SERVICE) {
        println!("NFS is on: nfsd(8) serves the shares that name networks on port 2049.");
    } else if nfs {
        println!("NFS is off; `share nfs on` turns it on.");
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
        if !share.nfs.is_empty() {
            println!("{:width$}  NFS: {}", "", share.nfs.join(", "));
        }
    }
    Ok(0)
}

/// `share add`.
fn add(args: &mut Args) -> Result<i32, Fail> {
    let (mut read_only, mut comment, mut operands) = (false, String::new(), Vec::new());
    let mut nfs = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "read-only" => read_only = true,
            Arg::Long(l) if l == "comment" => {
                comment = args.value()?.to_string_lossy().into_owned();
            }
            Arg::Long(l) if l == "nfs" => {
                let network = args.value()?.to_string_lossy().into_owned();
                shares::Network::parse(&network).map_err(usage)?;
                nfs.push(network);
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
    let folder = folder.to_string_lossy().into_owned();
    let share = Share { name, folder, comment, read_only, nfs };
    let mut all = read()?;
    all.add(share.clone()).map_err(fail)?;
    write(shares::PATH, &all.to_toml(), 0o644)?;
    let how = if share.read_only { "read-only" } else { "read-write" };
    println!("Shared {} as {}, {how}.", share.folder, share.name);
    if !is_on(SERVICE) {
        println!("Sharing is off; `share on` turns it on.");
    }
    if !share.nfs.is_empty() {
        println!("NFS clients on {} mount it as :/{}.", share.nfs.join(", "), share.name);
        if !is_on(NFS_SERVICE) {
            println!("NFS is off; `share nfs on` turns it on.");
        }
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

/// `share nfs on` and `share nfs off`.
fn nfs(args: &mut Args) -> Result<i32, Fail> {
    let Some(Arg::Operand(which)) = args.next()? else {
        return Err(usage("share nfs takes on or off: share nfs on".into()));
    };
    match which.to_str() {
        Some("on") => turn(args, NFS_SERVICE, true),
        Some("off") => turn(args, NFS_SERVICE, false),
        _ => Err(usage("share nfs takes on or off: share nfs on".into())),
    }
}

/// `share on` and `share off`, and `share nfs on` and `off`: `service`
/// starts or stops, now and at each boot.
fn turn(args: &mut Args, service: &str, on: bool) -> Result<i32, Fail> {
    nothing_more(args)?;
    let what = if service == NFS_SERVICE { "NFS" } else { "sharing" };
    root_only(&format!("turns {what} {}", if on { "on" } else { "off" }))?;
    let text = with_enabled(&service_text(service)?, on);
    service::from_toml(&text).map_err(|e| fail(format!("{}: {e}", service_path(service))))?;
    write(&service_path(service), &text, 0o644)?;
    let verb = if on { "start" } else { "stop" };
    let answer = power::request(&format!("{verb} {service}"))?;
    if let Some(why) = answer.strip_prefix("error: ") {
        return Err(fail(format!("cannot {verb} {service}: {}", why.trim_end())));
    }
    if service == NFS_SERVICE {
        if !on {
            println!("NFS is off; its clients' mounts stop answering.");
            return Ok(0);
        }
        println!("NFS is on: nfsd(8) serves on port 2049, and starts at each boot.");
        if !read()?.shares.iter().any(|s| !s.nfs.is_empty()) {
            println!(
                "No share names a network yet; `share add --nfs NETWORK NAME FOLDER` adds one."
            );
        }
        return Ok(0);
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
