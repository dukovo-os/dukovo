// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How authd(8) starts a session (ADR 0250): the first process of
//! someone who signed in, acting for them, on the terminal sign-in sent.
//!
//! authd holds the names capability (radix-names(2)), which Spark gives
//! only a service whose file asks for it. With it authd stamps a copy of
//! its root for the user, derives one without the bypass right or the
//! control right from it,
//! makes a process, names it for the user and their groups, and gives
//! it that root, the random source, the log and the terminal as its
//! descriptors 0 to 2. The process runs [`FIRST_PROGRAM`], `login
//! --session`, as the user: it makes the terminal its session's and
//! runs their shell. Nothing of authd's own passes to it: not its root,
//! not the names capability, not the bypass right.
//!
//! A person's session has a view of its own for its root (ADRs 0215 and
//! 0255): every name of the root, each at its place, but `/tmp`, which
//! is the person's own, [`TMP`]`/NUMBER`, shared by their sessions and by
//! no one else's. Root's sessions keep the whole tree, as pam_namespace
//! leaves root out.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;

use radix_abi::capability::{self, ObjectKind, Rights};
use radix_abi::directory::{self, OpenFlags};
use radix_abi::{Status, names, network, process, startup};

use accounts::User;

use crate::sys::{self, SYS};

/// What a session runs first: login(1), which makes the terminal the
/// session's and runs the user's shell.
pub const FIRST_PROGRAM: &str = "/system/commands/login";

/// The slots a session's process has, as a POSIX program Spark starts.
pub const SLOTS: u32 = 128;

/// Where each person's own `/tmp` is (ADR 0255): a folder for each
/// account, named by its number, which only root lists.
pub const TMP: &str = "/system/live/authd/tmp";

/// The rights of what a session's view holds: to read and change, and to
/// copy and pass on, as `fork` and `exec` need; never the bypass right,
/// and never control, so nothing is attached to it or mounted in it.
const GIVEN: Rights = Rights(
    Rights::READ.bits() | Rights::WRITE.bits() | Rights::COPY.bits() | Rights::TRANSFER.bits(),
);

/// `user`'s own `/tmp`: theirs, mode 1777 as `/tmp` is, made if it is not
/// there, in [`TMP`], which is root's, mode 0700.
pub fn own_tmp(user: &User) -> io::Result<String> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(TMP)?;
    fs::set_permissions(TMP, fs::Permissions::from_mode(0o700))?;
    let path = format!("{TMP}/{}", user.uid);
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Err(err) if err.kind() != io::ErrorKind::AlreadyExists => return Err(err),
        _ => {}
    }
    std::os::unix::fs::chown(&path, Some(user.uid), Some(user.gid))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o1777))?;
    Ok(path)
}

/// Builds a session's view in `view`, acting for `root`'s name without
/// the bypass right: each name of the root at its place, but `tmp`, which
/// is `tmp` of the root, the person's own. Each place is opened through
/// `stamped`, with the bypass right, into `scratch`, and given without
/// it, so the owners and modes of its files decide, as without the view.
fn build_view(root: u32, stamped: u32, view: u32, scratch: u32, tmp: &str) -> Result<(), String> {
    let failed =
        |what: String| move |status: Status| format!("{what}: {}", sys::status_name(status.0));
    directory::make_view(&SYS, root, view).map_err(failed("making its view".into()))?;
    // Opens `path` of the root and attaches it at `at` in the view.
    let attach = |path: &str, at: &str, flags: OpenFlags| -> Result<(), String> {
        directory::open(&SYS, stamped, path.as_bytes(), flags, GIVEN, scratch, 0)
            .map_err(failed(format!("opening /{path}")))?;
        let attached = directory::attach(&SYS, view, at.as_bytes(), scratch);
        let _ = capability::delete(&SYS, scratch);
        attached.map_err(failed(format!("attaching /{at}")))
    };
    let entries = fs::read_dir("/").map_err(|err| format!("listing /: {err}"))?;
    for entry in entries.flatten() {
        // A name is no path: one that is not UTF-8 is passed over.
        let Ok(name) = entry.file_name().into_string() else { continue };
        if name == "tmp" {
            continue;
        }
        let path = Path::new("/").join(&name);
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(|err| format!("reading /{name}: {err}"))?;
            directory::attach_link(&SYS, view, name.as_bytes(), target.as_os_str().as_bytes())
                .map_err(failed(format!("attaching /{name}")))?;
        } else if meta.is_dir() {
            attach(&name, &name, OpenFlags::DIRECTORY)?;
        } else if meta.is_file() {
            attach(&name, &name, OpenFlags::NONE)?;
        }
    }
    attach(tmp.trim_start_matches('/'), "tmp", OpenFlags::DIRECTORY)
}

/// Builds an NFS worker's view in `view`, acting for `root`'s name
/// without the bypass right: each of the shares' `folders` at `.nfs-`
/// and its key, opened through `stamped`, with the bypass right, into
/// `scratch`, and given without it. A folder that cannot be opened is
/// left out, and said so.
fn build_share_view(
    root: u32,
    stamped: u32,
    view: u32,
    scratch: u32,
    folders: &[(u64, String)],
) -> Result<(), String> {
    let failed =
        |what: String| move |status: Status| format!("{what}: {}", sys::status_name(status.0));
    directory::make_view(&SYS, root, view).map_err(failed("making its view".into()))?;
    for (key, folder) in folders {
        let path = folder.trim_start_matches('/');
        let opened = directory::open(
            &SYS,
            stamped,
            path.as_bytes(),
            OpenFlags::DIRECTORY,
            GIVEN,
            scratch,
            0,
        );
        if let Err(status) = opened {
            eprintln!("authd: an NFS worker goes without {folder}: {}", sys::status_name(status.0));
            continue;
        }
        let at = format!(".nfs-{key:016x}");
        let attached = directory::attach(&SYS, view, at.as_bytes(), scratch);
        let _ = capability::delete(&SYS, scratch);
        attached.map_err(failed(format!("attaching {folder}")))?;
    }
    Ok(())
}

/// The environment variable that says which descriptor holds the names
/// capability Spark gave authd (service.toml(5), `names`).
pub const NAMES_DESCRIPTOR: &str = "NAMES_DESCRIPTOR";

/// The slot of the names capability Spark gave authd, if it did.
pub fn names_slot() -> Option<u32> {
    let fd: i32 = std::env::var(NAMES_DESCRIPTOR).ok()?.parse().ok()?;
    let slot = sys::slot_of(fd);
    let info = capability::info(&SYS, slot).ok()?;
    (info.kind == ObjectKind::Names).then_some(slot)
}

/// The most descriptors a request comes with: an `elevate` request's
/// three for the command and its pipe.
pub const DESCRIPTORS_MAX: usize = 4;

/// Reads from `stream` into `buffer`, as read(2) does, and keeps the
/// descriptors sent with the bytes (SCM_RIGHTS) in `fds`, up to
/// [`DESCRIPTORS_MAX`] in all; any more are closed.
pub fn receive(
    stream: &UnixStream,
    buffer: &mut [u8],
    fds: &mut Vec<OwnedFd>,
) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
    // Room for more descriptors than are kept, aligned for the headers.
    let mut control = [0u64; 8];
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = size_of_val(&control) as _;
    // SAFETY: the message points at our buffer and control space.
    let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &raw mut message, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the control messages are those recvmsg wrote into
    // `control`, walked with the C library's macros; each descriptor in
    // one is ours to own.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&raw const message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header);
                let bytes = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for k in 0..bytes / size_of::<libc::c_int>() {
                    let at = data.add(k * size_of::<libc::c_int>());
                    let received = OwnedFd::from_raw_fd(at.cast::<libc::c_int>().read_unaligned());
                    if fds.len() < DESCRIPTORS_MAX {
                        fds.push(received);
                    }
                }
            }
            header = libc::CMSG_NXTHDR(&raw const message, header);
        }
    }
    Ok(n as usize)
}

/// Sends `bytes` on `stream` with the descriptors `fds` (SCM_RIGHTS) in
/// the first message, as [`receive`] reads them.
pub fn send_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    let mut iov = libc::iovec { iov_base: bytes.as_ptr().cast_mut().cast(), iov_len: bytes.len() };
    let mut control = [0u64; 8];
    let room = size_of_val(&fds[..fds.len().min(DESCRIPTORS_MAX)]) as u32;
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: CMSG_SPACE only computes a size, which fits `control` for
    // DESCRIPTORS_MAX descriptors.
    message.msg_controllen = unsafe { libc::CMSG_SPACE(room) } as _;
    // SAFETY: the control space holds one header and the descriptors, as
    // CMSG_SPACE said; the macros point into it.
    let sent = unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(room) as _;
        let data = libc::CMSG_DATA(header);
        for (k, fd) in fds.iter().take(DESCRIPTORS_MAX).enumerate() {
            data.add(k * size_of::<libc::c_int>()).cast::<libc::c_int>().write_unaligned(*fd);
        }
        libc::sendmsg(stream.as_raw_fd(), &raw const message, 0)
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    // What the first message did not take goes after it, as a write.
    let rest = &bytes[sent as usize..];
    if rest.is_empty() { Ok(()) } else { (&mut &*stream).write_all(rest) }
}

/// Whether `fd` is a terminal.
pub fn is_terminal(fd: &OwnedFd) -> bool {
    // SAFETY: isatty only looks at the descriptor.
    unsafe { libc::isatty(fd.as_raw_fd()) == 1 }
}

/// The name of the terminal in `fd`, as `pts/3`.
pub fn terminal_name(fd: &OwnedFd) -> String {
    // SAFETY: ttyname returns a C string the C library keeps, or null;
    // authd has one thread.
    let name = unsafe { libc::ttyname(fd.as_raw_fd()) };
    if name.is_null() {
        return "a terminal".into();
    }
    // SAFETY: a non-null result is a C string.
    let path = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
    crate::util::terminal_name(&path).to_string()
}

/// `term` if it is a terminal type a session may be given, `dumb` if not:
/// letters, digits, `-`, `_`, `.` and `+`, at most 64.
pub fn terminal_type(term: &str) -> &str {
    let fine = |c: char| c.is_ascii_alphanumeric() || "-_.+".contains(c);
    if !term.is_empty() && term.len() <= 64 && term.chars().all(fine) { term } else { "dumb" }
}

/// The environment a session starts with: what login(1) sets on Linux,
/// over what Spark gives every POSIX program. `PATH` names the system's
/// commands, then the folders where bundled(8) links the commands of the
/// bundles for everyone and of the person's own (ADR 0225), so that
/// installing never edits it.
pub fn environment(user: &User, term: &str) -> Vec<String> {
    let home = user.home.trim_end_matches('/');
    vec![
        format!("HOME={}", user.home),
        format!("SHELL={}", user.shell),
        format!("USER={}", user.name),
        format!("LOGNAME={}", user.name),
        format!("PATH=/system/commands:/applications/commands:{home}/applications/commands"),
        format!("TERM={}", terminal_type(term)),
        "LANG=C.UTF-8".into(),
        "EDITOR=nano".into(),
    ]
}

/// The block `exec` takes (radix-process(2)): `args` with `env`, in `/`.
pub fn exec_block(path: &[u8], args: &[&[u8]], env: &[String]) -> Vec<u8> {
    exec_block_in(b"/", path, args, env)
}

/// The block `exec` takes: `args` with `env`, in the folder `cwd`.
pub fn exec_block_in(cwd: &[u8], path: &[u8], args: &[&[u8]], env: &[String]) -> Vec<u8> {
    let mut block = Vec::new();
    let header = [process::EXEC_VERSION, args.len() as u32, env.len() as u32, 2];
    for word in header {
        block.extend_from_slice(&word.to_le_bytes());
    }
    let strings = args.iter().copied().chain(env.iter().map(String::as_bytes)).chain([cwd, path]);
    for string in strings {
        block.extend_from_slice(string);
        block.push(0);
    }
    block
}

/// Slots of our own to put capabilities in, for as long as they live:
/// those of descriptors of /devices/null, emptied once all are open. An
/// empty slot is a free descriptor to the C library, which the next open
/// would take, so emptying each as it opened would give the same slot
/// every time. Closing the descriptors deletes what is in them.
pub struct Spares(Vec<File>);

impl Spares {
    pub fn new(count: usize) -> io::Result<Spares> {
        let files =
            (0..count).map(|_| File::open("/devices/null")).collect::<io::Result<Vec<_>>>()?;
        for file in &files {
            let _ = capability::delete(&SYS, sys::slot_of(file.as_raw_fd()));
        }
        Ok(Spares(files))
    }

    pub fn slot(&self, index: usize) -> u32 {
        sys::slot_of(self.0[index].as_raw_fd())
    }
}

/// Starts `user`'s session on `terminal`, acting for them and the
/// groups `groups`, with the names capability in `names`. Returns the
/// number of its first process, a child of authd's.
pub fn start(
    names: u32,
    user: &User,
    groups: &[u32],
    terminal: &OwnedFd,
    term: &str,
) -> Result<u64, String> {
    let tty = terminal.as_raw_fd();
    let program = Program {
        what: "the session",
        path: FIRST_PROGRAM,
        args: &[b"login", b"--session"],
        env: environment(user, term),
        descriptors: &[(tty, 0), (tty, 1), (tty, 2)],
        network: true,
        shares: None,
    };
    launch(names, user, groups, &program)
}

/// What an SSH session runs on: the terminal sshd made for it, or three
/// pipes for its standard input, output and error.
pub enum SshIo<'a> {
    /// A terminal, as the session's 0, 1 and 2.
    Terminal(&'a OwnedFd),
    /// The session's 0, 1 and 2.
    Pipes([&'a OwnedFd; 3]),
}

/// The environment an SSH session starts with (ADR 0347): a console
/// session's, but with `TERM` only on a terminal, the client's
/// `language` settings over `LANG`, and what Linux's sshd adds:
/// `SSH_CLIENT`, `SSH_CONNECTION`, and `SSH_TTY` on a terminal.
/// `connection` is the client's address and port, then this machine's.
pub fn ssh_environment(
    user: &User,
    term: Option<&str>,
    tty: Option<&str>,
    connection: &str,
    language: &[String],
) -> Vec<String> {
    let mut env: Vec<String> = environment(user, term.unwrap_or(""))
        .into_iter()
        .filter(|v| term.is_some() || !v.starts_with("TERM="))
        .filter(|v| !(v.starts_with("LANG=") && language.iter().any(|l| l.starts_with("LANG="))))
        .collect();
    env.extend(language.iter().cloned());
    let words: Vec<&str> = connection.split(' ').collect();
    if let [address, port, _, local_port] = words[..] {
        env.push(format!("SSH_CLIENT={address} {port} {local_port}"));
    }
    env.push(format!("SSH_CONNECTION={connection}"));
    if let Some(tty) = tty {
        env.push(format!("SSH_TTY={tty}"));
    }
    env
}

/// Starts `user`'s session over SSH (ADR 0347), acting for them and the
/// groups `groups`, with the names capability in `names`: `login
/// --session`, with `command` if the client asked for one, on `io`, in
/// the environment `env`. Returns the number of its first process, a
/// child of authd's.
pub fn start_ssh(
    names: u32,
    user: &User,
    groups: &[u32],
    io: &SshIo,
    env: Vec<String>,
    command: Option<&[u8]>,
) -> Result<u64, String> {
    let descriptors = match io {
        SshIo::Terminal(tty) => {
            let tty = tty.as_raw_fd();
            [(tty, 0), (tty, 1), (tty, 2)]
        }
        SshIo::Pipes([input, output, error]) => {
            [(input.as_raw_fd(), 0), (output.as_raw_fd(), 1), (error.as_raw_fd(), 2)]
        }
    };
    let mut args: Vec<&[u8]> = vec![b"login", b"--session"];
    if let Some(command) = command {
        args.extend([b"--command".as_slice(), command]);
    }
    let program = Program {
        what: "the SSH session",
        path: FIRST_PROGRAM,
        args: &args,
        env,
        descriptors: &descriptors,
        network: true,
        shares: None,
    };
    launch(names, user, groups, &program)
}

/// sshd(8), whose `--forward` makes an SSH connection's forwarded
/// connections, and listens on the ports its client forwards (ADR 0349).
pub const FORWARDER: &str = "/system/commands/sshd";

/// Starts the forwarder of `user`'s SSH connection, `sshd --forward`,
/// acting for them and the groups `groups`, with the network a session
/// gets (ADR 0257) and nothing else of a session's: no shell runs it, as
/// what a shell's startup writes would break its frames. Its 0, 1 and 2
/// are `pipes`, and its environment `env`. Returns the number of its
/// process, a child of authd's.
pub fn start_forwarder(
    names: u32,
    user: &User,
    groups: &[u32],
    [input, output, error]: [&OwnedFd; 3],
    env: Vec<String>,
) -> Result<u64, String> {
    let program = Program {
        what: "the SSH connection's forwarder",
        path: FORWARDER,
        args: &[b"sshd", b"--forward"],
        env,
        descriptors: &[(input.as_raw_fd(), 0), (output.as_raw_fd(), 1), (error.as_raw_fd(), 2)],
        network: true,
        shares: None,
    };
    launch(names, user, groups, &program)
}

/// smbd(8)'s worker, which serves a Windows PC's share session as the
/// person who signed in (ADR 0228).
pub const SHARE_PROGRAM: &str = "/system/commands/smbd";

/// Starts the worker of a share session of `user`'s, acting for them and
/// the groups `groups`, with the descriptors smbd sent: `session`, the
/// pipe it reads the session from, as its 0; `connection`, the PC's, as
/// its 3; and `log`, smbd's standard error, as its 1 and 2. It gets no
/// network of its own: only the connection it serves. Returns the number
/// of its process, a child of authd's.
pub fn start_share(
    names: u32,
    user: &User,
    groups: &[u32],
    session: &OwnedFd,
    connection: &OwnedFd,
    log: &OwnedFd,
) -> Result<u64, String> {
    let home = user.home.trim_end_matches('/');
    let program = Program {
        what: "the share session",
        path: SHARE_PROGRAM,
        args: &[b"smbd", b"--worker"],
        env: vec![
            format!("HOME={}", user.home),
            format!("USER={}", user.name),
            format!("LOGNAME={}", user.name),
            format!("PATH=/system/commands:/applications/commands:{home}/applications/commands"),
            "LANG=C.UTF-8".into(),
        ],
        descriptors: &[
            (session.as_raw_fd(), 0),
            (log.as_raw_fd(), 1),
            (log.as_raw_fd(), 2),
            (connection.as_raw_fd(), 3),
        ],
        network: false,
        shares: None,
    };
    launch(names, user, groups, &program)
}

/// nfsd(8)'s worker, which serves NFS's calls as a person (ADR 0343).
pub const NFS_PROGRAM: &str = "/system/commands/nfsd";

/// The number of the user no one is, whom NFS serves root as, and a
/// number with no person's account: as Linux's `nobody`, it owns nothing
/// and no account has it.
pub const NOBODY: u32 = 65_534;

/// Who an NFS worker for root, or for a number with no person's
/// account, runs as.
pub fn nobody() -> User {
    User {
        name: "nobody".into(),
        uid: NOBODY,
        gid: NOBODY,
        full_name: String::new(),
        home: "/".into(),
        shell: String::new(),
    }
}

/// Starts an NFS worker as `user`, acting for them and the groups
/// `groups`, with the descriptors nfsd sent: the read end of nfsd's pipe
/// of requests as its 3, the write end of the pipe of its answers as its
/// 4, and nfsd's standard error as its 1 and 2; its 0 is /dev/null. Its
/// view holds the shares' `folders` alone, each at `/.nfs-` and its key
/// in hexadecimal, opened past the folders above them as Linux's server
/// does: below each, the owners and modes of the files decide. It gets no
/// network. Returns the number of its process, a child of authd's.
pub fn start_nfs(
    names: u32,
    user: &User,
    groups: &[u32],
    [requests, answers, log]: [&OwnedFd; 3],
    folders: &[(u64, String)],
) -> Result<u64, String> {
    // Rust's runtime opens /dev/null for a standard descriptor that is
    // closed, and aborts if it cannot, as it cannot in the worker's view.
    let null = File::open("/dev/null")
        .map_err(|err| format!("cannot start the NFS worker: /dev/null: {err}"))?;
    let program = Program {
        what: "the NFS worker",
        path: NFS_PROGRAM,
        args: &[b"nfsd", b"--worker"],
        env: vec![format!("USER={}", user.name), "HOME=/".into(), "LANG=C.UTF-8".into()],
        descriptors: &[
            (null.as_raw_fd(), 0),
            (log.as_raw_fd(), 1),
            (log.as_raw_fd(), 2),
            (requests.as_raw_fd(), 3),
            (answers.as_raw_fd(), 4),
        ],
        network: false,
        shares: Some(folders),
    };
    launch(names, user, groups, &program)
}

/// A program to start as a person.
struct Program<'a> {
    /// What it is, for errors: "the session".
    what: &'static str,
    /// Its path, which is opened through the person's root.
    path: &'a str,
    /// Its arguments, its name first.
    args: &'a [&'a [u8]],
    /// Its environment.
    env: Vec<String>,
    /// Descriptors of authd's, each with the number it has in the
    /// program.
    descriptors: &'a [(RawFd, u32)],
    /// Whether it gets the network, as a session does (ADR 0257).
    network: bool,
    /// For an NFS worker, the shares' folders, by their keys, which its
    /// view holds alone.
    shares: Option<&'a [(u64, String)]>,
}

/// Starts `program` as `user`, acting for them and the groups `groups`,
/// with the names capability in `names`, in a view of its own unless it
/// is root's. Returns the number of its process, a child of authd's.
fn launch(names: u32, user: &User, groups: &[u32], program: &Program) -> Result<u64, String> {
    let what = program.what;
    let failed = |step: &'static str| {
        move |status: Status| format!("cannot start {what}: {step}: {}", sys::status_name(status.0))
    };
    let spares = Spares::new(8).map_err(|err| format!("cannot start {what}: no slots: {err}"))?;
    let [stamped, root, opened, child, view, given, scratch, network] =
        [0, 1, 2, 3, 4, 5, 6, 7].map(|i| spares.slot(i));
    let others: Vec<u8> = groups.iter().flat_map(|g| g.to_le_bytes()).collect();
    names::stamp(&SYS, names, startup::SLOT_ROOT, stamped, user.uid, user.gid, &others)
        .map_err(failed("stamping its root"))?;
    let rights = capability::info(&SYS, stamped).map_err(failed("its root"))?.rights;
    // Without the bypass right, and without control, which would let
    // them mount, unmount and attach on any folder they may walk to, for
    // everyone: that is root's.
    let without = Rights(rights.bits() & !(Rights::BYPASS.bits() | Rights::CONTROL.bits()));
    capability::derive(&SYS, stamped, root, without, 0).map_err(failed("its root"))?;
    // The program is opened through the user's root, so that it is one
    // they may run.
    let path = program.path.trim_start_matches('/');
    directory::open(&SYS, root, path.as_bytes(), OpenFlags::NONE, Rights::READ, opened, 0)
        .map_err(failed("opening its program"))?;
    process::create(&SYS, startup::SLOT_PROCESS, ObjectKind::Process, SLOTS, child)
        .map_err(failed("making its process"))?;
    names::name_process(&SYS, names, child, user.uid, user.gid, &others)
        .map_err(failed("naming its process"))?;
    // A person's root is a view with their own /tmp; root's, the tree;
    // an NFS worker's, the shares.
    let own_root = if let Some(folders) = program.shares {
        build_share_view(root, stamped, view, scratch, folders)
            .map_err(|why| format!("cannot start {what}: {why}"))?;
        capability::derive(&SYS, view, given, GIVEN, 0).map_err(failed("its view"))?;
        given
    } else if user.uid == 0 {
        root
    } else {
        let tmp = own_tmp(user).map_err(|err| format!("cannot start {what}: its /tmp: {err}"))?;
        build_view(root, stamped, view, scratch, &tmp)
            .map_err(|why| format!("cannot start {what}: {why}"))?;
        capability::derive(&SYS, view, given, GIVEN, 0).map_err(failed("its view"))?;
        given
    };
    let base = [
        (own_root, startup::SLOT_ROOT),
        (startup::SLOT_RANDOM, startup::SLOT_RANDOM),
        (startup::SLOT_LOG, startup::SLOT_LOG),
    ];
    let descriptors = program
        .descriptors
        .iter()
        .map(|&(fd, number)| (sys::slot_of(fd), startup::SLOT_FD_BASE + number));
    for (from, to) in base.into_iter().chain(descriptors) {
        process::share(&SYS, child, from, to, Rights::ALL, 0)
            .map_err(failed("giving it its capabilities"))?;
    }
    if program.network {
        give_network(child, network, user.uid == 0).map_err(failed("giving it the network"))?;
    }
    let block = exec_block(program.path.as_bytes(), program.args, &program.env);
    process::exec(&SYS, child, opened, &block).map_err(failed("starting its program"))?;
    // The child stays in the tree after its capability goes, for wait.
    let number = process::info(&SYS, child).map_err(failed("its number"))?.number;
    Ok(number)
}

/// The first port everyone may listen on, as Linux's
/// `net.ipv4.ip_unprivileged_port_start`: below it, only root's session
/// and elevated commands listen (ADR 0257).
pub const FIRST_OPEN_PORT: u32 = 1024;

/// Every right but inbound: a program a person runs, even as root,
/// listens for this machine alone until the firewall allows it from
/// others (ADR 0334).
const ALL_BUT_INBOUND: Rights =
    match Rights::from_bits(Rights::ALL.bits() & !Rights::INBOUND.bits()) {
        Some(rights) => rights,
        None => Rights::ALL,
    };

/// Gives `child` the network, from authd's network capabilities in the
/// NETWORK_SLOTS slots from SLOT_NETWORK into its own from SLOT_NETWORK
/// (ADR 0257). With `everything`, for root's session and elevated
/// commands, each goes as authd holds it, setting up the interfaces
/// included. Otherwise the child connects out, to the machines on this
/// one's networks too (the local right, ADR 0337), and listens on the
/// ports from [`FIRST_OPEN_PORT`], but sets up nothing, as anyone may on
/// Linux.
/// Neither has the inbound right, which authd's has: what the child
/// listens on, other machines reach only as the firewall's rules allow
/// (ADR 0334). `scratch` is an empty slot to restrict through.
pub fn give_network(child: u32, scratch: u32, everything: bool) -> Result<(), Status> {
    // Kept through fork and exec, as Spark gives them (ADR 0184).
    let kept = Rights::COPY.union(Rights::TRANSFER);
    let end = startup::SLOT_NETWORK + startup::NETWORK_SLOTS;
    let mut to = startup::SLOT_NETWORK;
    for from in startup::SLOT_NETWORK..end {
        let Ok(info) = capability::info(&SYS, from) else { continue };
        if info.kind != ObjectKind::Network {
            continue;
        }
        if everything {
            process::share(&SYS, child, from, to, ALL_BUT_INBOUND, 0)?;
            to += 1;
            continue;
        }
        let ports = network::ports(&SYS, from)?;
        let past = ports.first.saturating_add(ports.count);
        for (right, lowest) in [(Rights::SEND, 0), (Rights::RECEIVE, FIRST_OPEN_PORT)] {
            let first = ports.first.max(lowest);
            if info.rights.bits() & right.bits() == 0 || first >= past || to >= end {
                continue;
            }
            // With the local right, if it is held, beside the send right.
            let local = Rights::LOCAL.bits() & info.rights.bits();
            let right = if right == Rights::SEND {
                Rights::from_bits(right.bits() | local).unwrap_or(right)
            } else {
                right
            };
            network::restrict(&SYS, from, scratch, first, past - first, right.union(kept))?;
            let shared = process::share(&SYS, child, scratch, to, Rights::ALL, 0);
            let _ = capability::delete(&SYS, scratch);
            shared?;
            to += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> User {
        accounts::users("alice:x:1000:1000::/users/alice:/system/commands/nukesh\n")
            .entries()
            .next()
            .cloned()
            .unwrap()
    }

    #[test]
    fn a_session_starts_with_a_login_environment() {
        let env = environment(&alice(), "linux");
        let path =
            "PATH=/system/commands:/applications/commands:/users/alice/applications/commands";
        for set in ["HOME=/users/alice", "USER=alice", "LOGNAME=alice", "TERM=linux", path] {
            assert!(env.iter().any(|e| e == set), "{set}");
        }
        assert!(environment(&alice(), "x\ny").iter().any(|e| e == "TERM=dumb"));
        assert!(environment(&alice(), "").iter().any(|e| e == "TERM=dumb"));
    }

    #[test]
    fn an_ssh_session_says_where_it_came_from() {
        let connection = "192.0.2.7 51234 192.0.2.1 22";
        let language = ["LANG=de_DE.UTF-8".to_string(), "LC_TIME=en_GB.UTF-8".to_string()];
        let env =
            ssh_environment(&alice(), Some("xterm"), Some("/devices/pts/3"), connection, &language);
        for set in [
            "TERM=xterm",
            "LANG=de_DE.UTF-8",
            "LC_TIME=en_GB.UTF-8",
            "SSH_CLIENT=192.0.2.7 51234 22",
            "SSH_CONNECTION=192.0.2.7 51234 192.0.2.1 22",
            "SSH_TTY=/devices/pts/3",
            "HOME=/users/alice",
        ] {
            assert!(env.iter().any(|e| e == set), "{set}: {env:?}");
        }
        assert_eq!(env.iter().filter(|e| e.starts_with("LANG=")).count(), 1);
        // Without a terminal: no TERM or SSH_TTY, and the system's LANG.
        let env = ssh_environment(&alice(), None, None, connection, &[]);
        assert!(!env.iter().any(|e| e.starts_with("TERM=") || e.starts_with("SSH_TTY=")));
        assert!(env.iter().any(|e| e == "LANG=C.UTF-8"));
    }

    #[test]
    fn the_block_holds_the_arguments_then_the_environment() {
        let block = exec_block(b"/p", &[b"p", b"-s"], &["A=1".into()]);
        let words: Vec<u32> =
            block[..16].chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        assert_eq!(words, [process::EXEC_VERSION, 2, 1, 2]);
        assert_eq!(&block[16..], b"p\0-s\0A=1\0/\0/p\0");
    }
}
