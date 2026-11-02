// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! smb(1): a share of a Windows network, or of any server of SMB 2 or 3
//! (Samba, a NAS), as a folder (ADR 0227).
//!
//! - `smb mount [-f] [-o OPTIONS] //SERVER/SHARE[/FOLDER] DIR` signs in,
//!   connects to the share and mounts it at DIR through /devices/fuse
//!   (ADR 0226), then serves it, in the background unless `-f`, until it
//!   is unmounted. mount(8) runs it for `mount -t smb` and `-t cifs`.
//! - `smb shares [-o OPTIONS] //SERVER` lists the server's shares of
//!   files.
//! - `smb ls [-o OPTIONS] //SERVER/SHARE[/FOLDER]` lists a folder.
//!
//! The options are mount.cifs(8)'s (options.rs). The password is the
//! option's, the credentials file's or `$PASSWD`'s, or else is asked for
//! on the terminal; the account is the option's or `$USER`.

mod fs;
mod options;
mod share;
mod sys;

use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::exit;

use fuser::{Session, SessionACL};
use radix_abi::directory::{self, MountFlags};
use radix_abi::startup;

use crate::fs::{Shown, SmbFs};
use crate::options::{Options, Target};
use crate::share::{Account, Share};
use crate::sys::Sys;

const USAGE: &str = "usage: smb mount [-f] [-o OPTIONS] //SERVER/SHARE[/FOLDER] DIR
       smb shares [-o OPTIONS] //SERVER
       smb ls [-o OPTIONS] //SERVER/SHARE[/FOLDER]";

const HELP: &str = "Mounts a share of a Windows network (SMB 2 and 3) as a folder, and lists
shares and folders.

  mount   signs in, mounts the share at DIR and serves it until it is
          unmounted (umount DIR); -f stays in the foreground
  shares  lists the server's shares of files
  ls      lists a folder of a share

OPTIONS, comma-separated, as mount.cifs takes them:
  user=NAME            the account (DOMAIN/NAME and NAME%PASSWORD too);
                       $USER if not given
  password=PASSWORD    else credentials=, $PASSWD, or asked for
  domain=DOMAIN        the account's domain or workgroup
  credentials=FILE     username=, password= and domain= lines
  guest                signs in as a guest, without an account
  port=PORT            the server's port (445)
  uid=UID, gid=GID     whose the files are shown as (the caller's)
  file_mode=MODE       the files' modes (0755)
  dir_mode=MODE        the folders' modes (0755)
  ro, rw               mounts to read only, or to write too (rw)

See smb(1).";

/// Why smb stopped: a mistake in how it was run (exit 2), or a failure
/// (exit 1).
enum Fail {
    Usage(String),
    Failed(String),
}

fn usage(what: impl Into<String>) -> Fail {
    Fail::Usage(what.into())
}

fn failed(what: impl Into<String>) -> Fail {
    Fail::Failed(what.into())
}

fn main() {
    let mut args = Vec::new();
    for arg in env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => {
                eprintln!("smb: {}: not text", arg.to_string_lossy());
                exit(2);
            }
        }
    }
    match run(&args) {
        Ok(()) => exit(0),
        Err(Fail::Usage(what)) => {
            eprintln!("smb: {what}\n{USAGE}");
            exit(2);
        }
        Err(Fail::Failed(what)) => {
            eprintln!("smb: {what}");
            exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), Fail> {
    let Some((command, args)) = args.split_first() else { return Err(usage("no command given")) };
    match command.as_str() {
        "mount" => mount(args),
        "shares" => shares(args),
        "ls" => list(args),
        "-h" | "--help" | "help" => {
            println!("{USAGE}\n\n{HELP}");
            Ok(())
        }
        "--version" => {
            println!("smb {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(usage(format!("{other}: not a command"))),
    }
}

/// A command's arguments: its options, its flags of `flags`, and its
/// operands.
struct Parsed {
    options: Options,
    flags: Vec<char>,
    operands: Vec<String>,
}

fn parse(args: &[String], flags: &str) -> Result<Parsed, Fail> {
    let mut parsed =
        Parsed { options: Options::default(), flags: Vec::new(), operands: Vec::new() };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            parsed.operands.extend(args.by_ref().cloned());
            break;
        }
        let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) else {
            parsed.operands.push(arg.clone());
            continue;
        };
        for (i, letter) in letters.char_indices() {
            if letter == 'o' {
                let rest = &letters[i + 1..];
                let text = if rest.is_empty() {
                    args.next().ok_or_else(|| usage("-o needs options"))?
                } else {
                    rest
                };
                parsed.options.add(text).map_err(usage)?;
                break;
            }
            if !flags.contains(letter) {
                return Err(usage(format!("-{letter}: not an option of this command")));
            }
            parsed.flags.push(letter);
        }
    }
    Ok(parsed)
}

fn mount(args: &[String]) -> Result<(), Fail> {
    let Parsed { mut options, flags, operands } = parse(args, "f")?;
    let [source, dir] = operands.as_slice() else {
        return Err(usage("mount takes a share and a folder"));
    };
    let target = Target::parse(source).map_err(usage)?;
    if target.share.is_empty() {
        return Err(usage(format!("{source}: no share named")));
    }
    let dir = std::fs::canonicalize(dir).map_err(|e| failed(format!("{dir}: {e}")))?;
    if !dir.is_dir() {
        return Err(failed(format!("{}: not a folder", dir.display())));
    }
    let account = account(&target, &mut options)?;
    // SAFETY: getuid and getgid only read the process's IDs.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let shown = Shown {
        uid: options.uid.unwrap_or(uid),
        gid: options.gid.unwrap_or(gid),
        file_mode: options.file_mode.unwrap_or(0o755),
        dir_mode: options.dir_mode.unwrap_or(0o755),
    };
    let how = Mount { target, account, dir, shown, read_only: options.read_only };
    if flags.contains(&'f') {
        let session = how.mounted().map_err(failed)?;
        return serve(session);
    }
    in_background(&how)
}

/// What to mount, where and how.
struct Mount {
    target: Target,
    account: Account,
    dir: PathBuf,
    shown: Shown,
    read_only: bool,
}

impl Mount {
    /// Connects to the share and mounts it, for its filesystem to be
    /// served.
    fn mounted(&self) -> Result<Session<SmbFs>, String> {
        let mut share = Share::connect(&self.account, &self.target.share)?;
        let source = self.target.source();
        if !self.target.path.is_empty() {
            let info = share.stat(&self.target.path).map_err(|e| format!("{source}: {e}"))?;
            if !info.is_directory {
                return Err(format!("{source}: not a folder"));
            }
        }
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/devices/fuse")
            .map_err(|e| format!("/devices/fuse: {e}"))?;
        let slot = startup::SLOT_FD_BASE + device.as_raw_fd() as u32;
        let path = self.dir.to_str().ok_or_else(|| format!("{}: not text", self.dir.display()))?;
        let flags = if self.read_only { MountFlags::READ_ONLY } else { MountFlags::NONE };
        directory::mount_fuse(
            &Sys,
            startup::SLOT_ROOT,
            path.trim_start_matches('/').as_bytes(),
            slot,
            source.as_bytes(),
            flags,
        )
        .map_err(|status| format!("cannot mount at {path}: {}", sys::said(status)))?;
        let fs = SmbFs::new(share, &self.target.path, self.shown, self.read_only);
        Ok(Session::from_fd(fs, OwnedFd::from(device), SessionACL::All))
    }
}

/// Serves the share until it is unmounted, then signs out.
fn serve(mut session: Session<SmbFs>) -> Result<(), Fail> {
    let served = session.run();
    drop(session);
    served.map_err(|e| failed(format!("the mount ended: {e}")))
}

/// Mounts in a process of its own, which serves the share in the
/// background, in a session of its own: this one returns once the share
/// is mounted, or says why it was not.
fn in_background(how: &Mount) -> Result<(), Fail> {
    let mut ends = [0; 2];
    // SAFETY: pipe writes two descriptors to `ends`.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(failed(format!("no pipe: {}", io::Error::last_os_error())));
    }
    // SAFETY: the descriptors are the pipe's, owned from here on.
    let (mut from, to) = unsafe { (File::from_raw_fd(ends[0]), File::from_raw_fd(ends[1])) };
    // SAFETY: smb has one thread here (its runtime is made after), so the
    // child is a whole copy of it.
    match unsafe { libc::fork() } {
        -1 => Err(failed(format!("cannot start: {}", io::Error::last_os_error()))),
        0 => {
            drop(from);
            exit(background(how, to))
        }
        child => {
            drop(to);
            let mut said = Vec::new();
            let _ = from.read_to_end(&mut said);
            if said == [0] {
                return Ok(());
            }
            let mut status = 0;
            // SAFETY: waits for the child, which has ended or is ending.
            unsafe { libc::waitpid(child, &raw mut status, 0) };
            if said.is_empty() {
                return Err(failed("the mount's process ended before it mounted the share"));
            }
            Err(failed(String::from_utf8_lossy(&said).into_owned()))
        }
    }
}

/// The background process: mounts, says so through `report` (a 0, or
/// why not), and serves. Returns its exit status.
fn background(how: &Mount, mut report: File) -> i32 {
    // SAFETY: setsid takes no arguments; the child leads no group, so it
    // makes a session of its own, away from the terminal.
    unsafe { libc::setsid() };
    match how.mounted() {
        Ok(session) => {
            quiet();
            let _ = report.write_all(&[0]);
            drop(report);
            i32::from(serve(session).is_err())
        }
        Err(why) => {
            let _ = report.write_all(why.as_bytes());
            1
        }
    }
}

/// Points the standard input, output and error at /devices/null.
fn quiet() {
    let Ok(null) = OpenOptions::new().read(true).write(true).open("/devices/null") else { return };
    for fd in 0..3 {
        // SAFETY: dup2 replaces a standard descriptor with /devices/null.
        unsafe { libc::dup2(null.as_raw_fd(), fd) };
    }
}

/// Who signs in, and where: the options' account, `$USER`, or a guest;
/// the password as the module says.
fn account(target: &Target, options: &mut Options) -> Result<Account, Fail> {
    if let Some(path) = options.credentials.clone() {
        let text = std::fs::read_to_string(&path).map_err(|e| failed(format!("{path}: {e}")))?;
        options.add_credentials(&text).map_err(|e| failed(format!("{path}: {e}")))?;
    }
    let addr = target.addr(options.port);
    let domain = options.domain.clone().unwrap_or_default();
    if options.guest {
        return Ok(Account { addr, user: String::new(), password: String::new(), domain });
    }
    let user = options
        .user
        .clone()
        .or_else(|| env::var("USER").ok().filter(|user| !user.is_empty()))
        .ok_or_else(|| usage("no account: give user=NAME, or guest"))?;
    let password = match options.password.clone().or_else(|| env::var("PASSWD").ok()) {
        Some(password) => password,
        None => ask(&format!("Password for {user}@{}: ", target.source()))?,
    };
    Ok(Account { addr, user, password, domain })
}

/// Asks for a password on the terminal, without showing it.
fn ask(prompt: &str) -> Result<String, Fail> {
    // SAFETY: isatty only looks at the descriptor.
    if unsafe { libc::isatty(0) } != 1 {
        return Err(failed("no password: give password=, credentials= or $PASSWD"));
    }
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    // SAFETY: all zeros is a valid termios: numbers.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr fills `saved`.
    let quiet = unsafe { libc::tcgetattr(0, &raw mut saved) } == 0;
    if quiet {
        let mut off = saved;
        off.c_lflag &= !libc::ECHO;
        // SAFETY: `off` is a valid termios.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw const off) };
    }
    let mut line = String::new();
    let read = io::stdin().read_line(&mut line);
    if quiet {
        // SAFETY: `saved` is the termios read above.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw const saved) };
        eprintln!();
    }
    match read {
        Ok(0) => Err(failed("no password given")),
        Ok(_) => Ok(line.strip_suffix('\n').unwrap_or(&line).to_string()),
        Err(e) => Err(failed(format!("cannot read the password: {e}"))),
    }
}

/// `smb shares`: the server's shares of files, without its
/// administrative ones (`C$`, `ADMIN$`), which smb2 leaves out.
fn shares(args: &[String]) -> Result<(), Fail> {
    let Parsed { mut options, operands, .. } = parse(args, "")?;
    let [server] = operands.as_slice() else { return Err(usage("shares takes a server")) };
    let target = Target::parse(server).map_err(usage)?;
    if !target.share.is_empty() {
        return Err(usage(format!("{server}: give the server alone (//{})", target.host)));
    }
    let account = account(&target, &mut options)?;
    let rt = share::runtime().map_err(|e| failed(format!("no runtime: {e}")))?;
    let mut client = share::sign_in(&rt, &account)
        .map_err(|e| failed(format!("cannot sign in to {}: {e}", account.addr)))?;
    let mut shares = rt
        .block_on(client.list_shares())
        .map_err(|e| failed(format!("cannot list the shares of {}: {e}", account.addr)))?;
    shares.sort_by_key(|share| share.name.to_lowercase());
    let width = shares.iter().map(|share| share.name.chars().count()).max().unwrap_or(0);
    let mut out = io::stdout().lock();
    for share in &shares {
        let line = format!("{:<width$}  {}", share.name, share.comment);
        let _ = writeln!(out, "{}", line.trim_end());
    }
    Ok(())
}

/// `smb ls`: a folder's names, folders with a `/` after.
fn list(args: &[String]) -> Result<(), Fail> {
    let Parsed { mut options, operands, .. } = parse(args, "")?;
    let [folder] = operands.as_slice() else { return Err(usage("ls takes a share's folder")) };
    let target = Target::parse(folder).map_err(usage)?;
    if target.share.is_empty() {
        return Err(usage(format!("{folder}: no share named")));
    }
    let account = account(&target, &mut options)?;
    let mut share = Share::connect(&account, &target.share).map_err(failed)?;
    let listed = share.list(&target.path);
    share.disconnect();
    let mut entries = listed.map_err(|e| failed(format!("{}: {e}", target.source())))?;
    entries.retain(|entry| entry.name != "." && entry.name != "..");
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = io::stdout().lock();
    for entry in &entries {
        let slash = if entry.is_directory { "/" } else { "" };
        let _ = writeln!(out, "{}{slash}", entry.name);
    }
    Ok(())
}
