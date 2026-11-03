// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfs(1): a folder a Linux or other Unix machine shares over NFS 4.1 or
//! 4.2, as a folder here (ADR 0344).
//!
//! - `nfs mount [-f] [-o OPTIONS] SERVER:/PATH DIR` mounts the server's
//!   folder at DIR through /devices/fuse (ADR 0226), then serves it, in
//!   the background unless `-f`, until it is unmounted. mount(8) runs it
//!   for `mount -t nfs` and `-t nfs4`, and for a source `SERVER:/PATH`.
//! - `nfs ls [-o OPTIONS] SERVER:/PATH` lists a folder.
//!
//! Calls carry AUTH_SYS's credential: the caller's user and groups, by
//! number, as Linux's client sends them (ADR 0344). The options are
//! mount.nfs(8)'s (options.rs).

mod attrs;
mod client;
mod fs;
mod ids;
mod options;
mod rpc;
mod sys;
#[cfg(test)]
mod tests;

use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use fuser::{FileType, Session, SessionACL};
use nfsd::nfs::{kind, op};
use radix_abi::directory::{self, MountFlags};
use radix_abi::startup;

use crate::attrs::Attr;
use crate::client::{Client, Fail, Ops, Timing};
use crate::fs::{NfsFs, ROOT};
use crate::ids::Ids;
use crate::options::{Options, PORT, Target};
use crate::rpc::Cred;
use crate::sys::Sys;

const USAGE: &str = "usage: nfs mount [-f] [-o OPTIONS] SERVER:/PATH DIR
       nfs ls [-o OPTIONS] SERVER:/PATH";

const HELP: &str = "Mounts a folder a Linux or other Unix machine shares over NFS 4.1 or 4.2,
and lists folders.

  mount   mounts the server's folder at DIR and serves it until it is
          unmounted (umount DIR); -f stays in the foreground
  ls      lists a server's folder

OPTIONS, comma-separated, as mount.nfs takes them:
  vers=4.2, vers=4.1   the version spoken (4.2, else 4.1)
  port=PORT            the server's port (2049)
  addr=ADDRESS         the server's address, not looked up by its name
  hard, soft           calls wait for a server that is away (hard), or
                       fail after retrans tries (soft)
  timeo=N, retrans=N   tenths of a second a reply is waited for (600),
                       and tries before a soft mount fails (2)
  rsize=N, wsize=N     the most bytes each read and write carries
  ro, rw               mounts to read only, or to write too (rw)
  sec=sys              the caller's numbers (AUTH_SYS), the only one yet
Other options of mount.nfs are taken and change nothing.

See nfs(1).";

/// Why nfs stopped: a mistake in how it was run (exit 2), or a failure
/// (exit 1).
enum Stop {
    Usage(String),
    Failed(String),
}

fn usage(what: impl Into<String>) -> Stop {
    Stop::Usage(what.into())
}

fn failed(what: impl Into<String>) -> Stop {
    Stop::Failed(what.into())
}

fn main() {
    let mut args = Vec::new();
    for arg in env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => {
                eprintln!("nfs: {}: not text", arg.to_string_lossy());
                exit(2);
            }
        }
    }
    match run(&args) {
        Ok(()) => exit(0),
        Err(Stop::Usage(what)) => {
            eprintln!("nfs: {what}\n{USAGE}");
            exit(2);
        }
        Err(Stop::Failed(what)) => {
            eprintln!("nfs: {what}");
            exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), Stop> {
    let Some((command, args)) = args.split_first() else { return Err(usage("no command given")) };
    match command.as_str() {
        "mount" => mount(args),
        "ls" => list(args),
        "-h" | "--help" | "help" => {
            println!("{USAGE}\n\n{HELP}");
            Ok(())
        }
        "--version" => {
            println!("nfs {}", env!("CARGO_PKG_VERSION"));
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

fn parse(args: &[String], flags: &str) -> Result<Parsed, Stop> {
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

/// The server's address: `addr=`'s, the target's own, or its name's.
fn address(target: &Target, options: &Options) -> Result<SocketAddr, String> {
    let port = options.port.or(target.port).unwrap_or(PORT);
    if let Some(ip) = options.addr {
        return Ok(SocketAddr::new(ip, port));
    }
    if let Ok(ip) = target.host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    (target.host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("{}: {e}", target.host))?
        .next()
        .ok_or_else(|| format!("{}: no address", target.host))
}

/// This machine's name, which calls carry.
fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return "dukovo".into();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// Eight random bytes from the kernel.
fn random() -> Result<[u8; 8], String> {
    let mut out = [0u8; 8];
    let mut got = 0;
    while got < out.len() {
        // SAFETY: the rest of `out` is valid for its length.
        let n = unsafe { libc::getrandom(out[got..].as_mut_ptr().cast(), out.len() - got, 0) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("no random bytes: {err}"));
        }
        got += n as usize;
    }
    Ok(out)
}

/// The caller's credential.
fn caller(ids: &mut Ids) -> Cred {
    // SAFETY: getuid and getgid only read the process's IDs.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    ids.cred(uid, gid)
}

/// How many LOOKUPs one COMPOUND carries, well within the operations a
/// session takes.
const LOOKUPS: usize = 8;

/// Walks from the server's root through `names`, as `cred`: the folder's
/// filehandle and attributes.
fn walk(client: &mut Client, cred: &Cred, names: &[String]) -> Result<(Vec<u8>, Attr), Fail> {
    let mut fh: Option<Vec<u8>> = None;
    let mut chunks: Vec<&[String]> = names.chunks(LOOKUPS).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.into_iter().enumerate() {
        let mut ops = Ops::new();
        match &fh {
            None => {
                ops.op(op::PUTROOTFH);
            }
            Some(fh) => {
                ops.putfh(fh);
            }
        }
        for name in chunk {
            ops.op(op::LOOKUP).opaque(name.as_bytes());
        }
        ops.op(op::GETFH);
        if i == last {
            attrs::ask(ops.op(op::GETATTR), attrs::SERVER);
        }
        let results = client.run(cred, &ops)?;
        let mut walk = results.walk();
        walk.next(if fh.is_none() { op::PUTROOTFH } else { op::PUTFH })?;
        for _ in chunk {
            walk.next(op::LOOKUP)?;
        }
        fh = Some(walk.next(op::GETFH)?.opaque(128)?.to_vec());
        if i == last {
            let a = attrs::decode(walk.next(op::GETATTR)?)?;
            return Ok((fh.unwrap_or_default(), a));
        }
    }
    Err(Fail::Status(nfsd::nfs::err::SERVERFAULT))
}

/// Connects to the server and finds the folder: the client, and the
/// folder's filehandle. The calls of a mount wait as `options` say once
/// it is made; until then they fail after `retrans` tries.
fn connect(target: &Target, options: &Options, cred: &Cred) -> Result<(Client, Vec<u8>), String> {
    let addr = address(target, options)?;
    let source = target.source();
    let timing = Timing { timeout: options.timeout(), retrans: options.retrans, hard: false };
    let machine = hostname();
    let verifier = random()?;
    let mut owner = format!("dukovo-nfs {machine} ").into_bytes();
    owner.extend_from_slice(&verifier);
    let mut client = Client::new(addr, timing, options.minor, machine.as_bytes(), &owner, verifier);
    let (root, a) = walk(&mut client, cred, &target.names).map_err(|f| format!("{source}: {f}"))?;
    if a.kind != kind::DIR {
        return Err(format!("{source}: not a folder"));
    }
    if a.lease != 0 {
        client.lease = Duration::from_secs(u64::from(a.lease));
    }
    let read = options.rsize.map_or(a.max_read, |most| u64::from(most).min(a.max_read.max(1)));
    let write = options.wsize.map_or(a.max_write, |most| u64::from(most).min(a.max_write.max(1)));
    client.limit_io(read, write);
    client.timing.hard = options.hard;
    Ok((client, root))
}

fn mount(args: &[String]) -> Result<(), Stop> {
    let Parsed { options, flags, operands } = parse(args, "f")?;
    let [source, dir] = operands.as_slice() else {
        return Err(usage("mount takes a server's folder and a folder"));
    };
    let target = Target::parse(source).map_err(usage)?;
    let dir = std::fs::canonicalize(dir).map_err(|e| failed(format!("{dir}: {e}")))?;
    if !dir.is_dir() {
        return Err(failed(format!("{}: not a folder", dir.display())));
    }
    let how = Mount { target, options, dir };
    if flags.contains(&'f') {
        let session = how.mounted().map_err(failed)?;
        return serve(session);
    }
    in_background(&how)
}

/// What to mount, where and how.
struct Mount {
    target: Target,
    options: Options,
    dir: PathBuf,
}

impl Mount {
    /// Connects to the server and mounts its folder, for its filesystem
    /// to be served.
    fn mounted(&self) -> Result<Session<NfsFs>, String> {
        let mut ids = Ids::new();
        let (client, root) = connect(&self.target, &self.options, &caller(&mut ids))?;
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/devices/fuse")
            .map_err(|e| format!("/devices/fuse: {e}"))?;
        let slot = startup::SLOT_FD_BASE + device.as_raw_fd() as u32;
        let path = self.dir.to_str().ok_or_else(|| format!("{}: not text", self.dir.display()))?;
        let flags = if self.options.read_only { MountFlags::READ_ONLY } else { MountFlags::NONE };
        directory::mount_fuse(
            &Sys,
            startup::SLOT_ROOT,
            path.trim_start_matches('/').as_bytes(),
            slot,
            self.target.source().as_bytes(),
            flags,
        )
        .map_err(|status| format!("cannot mount at {path}: {}", sys::said(status)))?;
        let client = Arc::new(Mutex::new(client));
        renewing(Arc::clone(&client));
        let fs = NfsFs::new(client, &root, self.options.read_only);
        Ok(Session::from_fd(fs, OwnedFd::from(device), SessionACL::All))
    }
}

/// Renews the lease while no call does, from a thread of its own.
fn renewing(client: Arc<Mutex<Client>>) {
    let _ = thread::Builder::new().name("lease".into()).spawn(move || {
        loop {
            let lease = client.lock().unwrap_or_else(PoisonError::into_inner).lease;
            let period = (lease / 3).max(Duration::from_secs(5));
            thread::sleep(period);
            let mut client = client.lock().unwrap_or_else(PoisonError::into_inner);
            if client.last.elapsed() >= period {
                let _ = client.renew();
            }
        }
    });
}

/// Serves the folder until it is unmounted.
fn serve(mut session: Session<NfsFs>) -> Result<(), Stop> {
    let served = session.run();
    drop(session);
    served.map_err(|e| failed(format!("the mount ended: {e}")))
}

/// Mounts in a process of its own, which serves the folder in the
/// background, in a session of its own: this one returns once the folder
/// is mounted, or says why it was not.
fn in_background(how: &Mount) -> Result<(), Stop> {
    let mut ends = [0; 2];
    // SAFETY: pipe writes two descriptors to `ends`.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(failed(format!("no pipe: {}", io::Error::last_os_error())));
    }
    // SAFETY: the descriptors are the pipe's, owned from here on.
    let (mut from, to) = unsafe { (File::from_raw_fd(ends[0]), File::from_raw_fd(ends[1])) };
    // SAFETY: nfs has one thread here (the lease's is started after), so
    // the child is a whole copy of it.
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
                return Err(failed("the mount's process ended before it mounted the folder"));
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

/// `nfs ls`: a folder's names, folders with a `/` after.
fn list(args: &[String]) -> Result<(), Stop> {
    let Parsed { options, operands, .. } = parse(args, "")?;
    let [folder] = operands.as_slice() else { return Err(usage("ls takes a server's folder")) };
    let target = Target::parse(folder).map_err(usage)?;
    let mut ids = Ids::new();
    let cred = caller(&mut ids);
    let (client, root) = connect(&target, &options, &cred).map_err(failed)?;
    let mut fs = NfsFs::new(Arc::new(Mutex::new(client)), &root, true);
    let listed = fs.list(&cred, ROOT).and_then(|handle| fs.names(handle));
    fs.close_all();
    let mut names = listed.map_err(|errno| {
        failed(format!("{}: {}", target.source(), io::Error::from_raw_os_error(errno)))
    })?;
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = io::stdout().lock();
    for (name, kind) in &names {
        let slash = if *kind == FileType::Directory { "/" } else { "" };
        let _ = out.write_all(name.as_bytes());
        let _ = writeln!(out, "{slash}");
    }
    Ok(())
}
