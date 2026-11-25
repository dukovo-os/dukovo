// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! script(1): runs a shell, or a command (`-c`), on a new pseudo-terminal
//! and saves all it writes to a file (`typescript` by default), as
//! util-linux's script does, while passing it on to the terminal.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;

use sysapi::fd;
use sysapi::poll::{self, PollFd};
use sysapi::process::{self, Fork};
use sysapi::signal::{self, Caught};
use sysapi::terminal::{self, When};

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail};

/// Set when the terminal's size changes.
static RESIZED: Caught = Caught::new();

/// script(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut append = false;
    let mut quiet = false;
    let mut command: Option<String> = None;
    let mut return_code = false;
    let mut flush = false;
    let mut file: Option<OsString> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('a') => append = true,
            Arg::Long(l) if l == "append" => append = true,
            Arg::Short('q') => quiet = true,
            Arg::Long(l) if l == "quiet" => quiet = true,
            Arg::Short('c') => command = Some(args.text()?),
            Arg::Long(l) if l == "command" => command = Some(args.text()?),
            Arg::Short('e') => return_code = true,
            Arg::Long(l) if l == "return" => return_code = true,
            Arg::Short('f') => flush = true,
            Arg::Long(l) if l == "flush" => flush = true,
            Arg::Short('E') => {
                let _ = args.text()?;
            }
            Arg::Long(l) if l == "echo" => {
                let _ = args.text()?;
            }
            Arg::Operand(name) if file.is_none() => file = Some(name),
            other => return Err(args.unknown(&other)),
        }
    }
    let path = file.unwrap_or_else(|| "typescript".into());
    let mut log = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&path)
        .map_err(|e| util::failed(&std::path::Path::new(&path).display(), e))?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let now = (util::now_ns() / 1_000_000_000) as i64;
    let tty = util::is_terminal(std::io::stdin());
    let (columns, lines) = util::terminal_size(std::io::stdin()).unwrap_or((80, 24));
    let term = std::env::var("TERM").unwrap_or_else(|_| "dumb".into());
    let mut header =
        format!("Script started on {} [", util::local_time(now, "%Y-%m-%d %H:%M:%S%z"));
    if let Some(command) = &command {
        header.push_str(&format!("COMMAND=\"{command}\" "));
    }
    header.push_str(&format!("TERM=\"{term}\" COLUMNS=\"{columns}\" LINES=\"{lines}\"]\n"));
    log.write_all(header.as_bytes())?;
    if !quiet {
        println!("Script started, output log file is '{}'.", std::path::Path::new(&path).display());
    }

    // The child's terminal: the size of ours.
    let size = if tty { terminal::window_size(io::stdin()).ok() } else { None };
    let pty = terminal::open_pty(size)
        .map_err(|e| fail(format!("openpty failed: {}", util::strip_os_error(&e))))?;
    let terminal::Pty { master, terminal: slave, .. } = pty;
    let mut child = Command::new(&shell);
    match &command {
        Some(command) => child.arg("-c").arg(command),
        None => child.arg("-i"),
    };
    let pid = match process::fork()? {
        Fork::Child => {
            drop(master);
            if process::login_terminal(slave).is_ok() {
                let _ = child.exec();
            }
            process::exit_now(127);
        }
        Fork::Parent(pid) => pid,
    };
    drop(slave);
    // A closed output is an error to stop at with the terminal put back
    // (util-linux's script dies of the SIGPIPE and leaves it raw). Only
    // here, after the fork, so the child keeps the default.
    let _ = signal::ignore(libc::SIGPIPE);
    let log_name = std::path::Path::new(&path).display();
    let mut saved = Restore(None);
    if tty {
        if let Ok(attributes) = terminal::attributes(io::stdin()) {
            saved.0 = Some(attributes);
            let mut raw = attributes;
            terminal::make_raw(&mut raw);
            let _ = terminal::set_attributes(io::stdin(), When::Now, &raw);
        }
        let _ = signal::catch(libc::SIGWINCH, &RESIZED, 0);
    }
    let mut stdin_open = true;
    let mut buffer = [0u8; 4096];
    let mut stdout = io::stdout().lock();
    let stdin = io::stdin();
    loop {
        if RESIZED.take()
            && let Ok(size) = terminal::window_size(&stdin)
        {
            let _ = terminal::set_window_size(&master, size);
        }
        let mut fds = [
            PollFd::new(master.as_fd(), libc::POLLIN),
            PollFd::optional(stdin_open.then(|| stdin.as_fd()), libc::POLLIN),
        ];
        match poll::poll(&mut fds, None) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        let (from_child, from_user) = (fds[0].revents() != 0, fds[1].revents() != 0);
        if from_user {
            match fd::read(&stdin, &mut buffer) {
                Ok(n) if n > 0 => write_all_fd(&master, &buffer[..n]),
                _ => {
                    stdin_open = false;
                    // The end of what was piped in is the end of the
                    // terminal's input: an end-of-file character (^D).
                    if !tty {
                        write_all_fd(&master, &[4]);
                    }
                }
            }
        }
        if from_child {
            let n = match fd::read(&master, &mut buffer) {
                Ok(n) if n > 0 => n,
                // The child's side has closed: it has ended.
                _ => break,
            };
            let data = &buffer[..n];
            stdout
                .write_all(data)
                .and_then(|()| stdout.flush())
                .map_err(|e| util::failed(&"write error", e))?;
            log.write_all(data)
                .and_then(|()| if flush { log.flush() } else { Ok(()) })
                .map_err(|e| util::failed(&log_name, e))?;
        }
    }
    drop(saved);
    let status = process::wait(pid);
    drop(master);
    let code = status
        .map_or(128, |status| status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)));
    let now = (util::now_ns() / 1_000_000_000) as i64;
    log.write_all(
        format!(
            "\nScript done on {} [COMMAND_EXIT_CODE=\"{code}\"]\n",
            util::local_time(now, "%Y-%m-%d %H:%M:%S%z")
        )
        .as_bytes(),
    )?;
    if !quiet {
        println!("Script done.");
    }
    Ok(if return_code { code } else { 0 })
}

/// The terminal's settings from before it was made raw, put back when it
/// is dropped: also when a write to the log or the output fails.
struct Restore(Option<terminal::Attributes>);

impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(attributes) = &self.0 {
            let _ = terminal::set_attributes(io::stdin(), When::Now, attributes);
        }
    }
}

/// Writes all of `data` to `to`, as far as it takes it.
fn write_all_fd(to: impl AsFd, mut data: &[u8]) {
    while !data.is_empty() {
        match fd::write(&to, data) {
            Ok(0) => return,
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}
