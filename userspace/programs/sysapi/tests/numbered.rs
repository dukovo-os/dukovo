// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The descriptor numbers a shell keeps, and how a program it starts
//! finds them, its signals and its signal mask. The table is the whole
//! process's, and standard output is closed on the way, so this test has
//! no harness (`harness = false`): it runs its checks in order on the
//! main thread, and starts itself again where a check needs a new
//! program, with the check's name in `NUMBERED_ROLE`.

#![forbid(unsafe_code)]

use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use sysapi::fd::Standard;
use sysapi::process::{self, Fork};
use sysapi::{fd, numbered, signal};

/// The variable that names the check a started copy of this test makes.
const ROLE: &str = "NUMBERED_ROLE";

fn main() {
    if let Some(role) = std::env::var_os(ROLE) {
        process::exit_now(as_started(role.to_str().unwrap_or("")));
    }
    numbers_kept_for_the_shell_stay_out_of_the_way();
    a_number_held_elsewhere_is_refused();
    a_lent_number_is_neither_closed_nor_replaced();
    a_closed_standard_number_is_closed_for_the_program_started();
    a_redirection_reaches_the_program_started();
    a_handed_down_number_is_taken_once();
    a_program_keeps_the_ignored_signals_and_the_mask();
    sigpipe_ignored_by_the_parent_is_seen();
    a_farewell_ends_the_process_from_the_handler();
    println!("numbered: 9 checks passed");
}

/// The checks a started copy makes: its exit status, 0 when they pass.
fn as_started(role: &str) -> i32 {
    match role {
        "handed-down" => {
            // 7 was left open across exec.
            let mut text = String::new();
            let lent = numbered::lend(7).expect("7 is open");
            std::fs::File::from(lent.as_fd().try_clone_to_owned().unwrap())
                .read_to_string(&mut text)
                .unwrap();
            drop(lent);
            let taken_again = fd::handed_down(7).is_err();
            numbered::close(7).unwrap();
            i32::from(!(text == "handed\n" && taken_again && !numbered::is_open(7)))
        }
        "sigpipe" => i32::from(!signal::ignored_at_start(libc::SIGPIPE)),
        _ => 99,
    }
}

/// Starts this test again in a child, as role `role`, after `prepare`;
/// the child's exit status.
fn start_again(role: &str, prepare: impl FnOnce()) -> i32 {
    match process::fork().unwrap() {
        Fork::Child => {
            prepare();
            let program = std::env::current_exe().unwrap();
            let path = CString::new(program.as_os_str().as_bytes()).unwrap();
            let env = [CString::new(format!("{ROLE}={role}")).unwrap()];
            let error = process::exec(&path, std::slice::from_ref(&path), &env);
            eprintln!("numbered: cannot start again: {error}");
            process::exit_now(98)
        }
        Fork::Parent(child) => process::wait(child).unwrap().code().unwrap_or(-1),
    }
}

/// Runs `script` with sh in a child, after `prepare`; its exit status.
fn sh(script: &str, prepare: impl FnOnce()) -> i32 {
    match process::fork().unwrap() {
        Fork::Child => {
            prepare();
            let path = CString::new("/bin/sh").unwrap();
            let args = [path.clone(), CString::new("-c").unwrap(), CString::new(script).unwrap()];
            let env = [CString::new("PATH=/usr/bin:/bin").unwrap()];
            let error = process::exec(&path, &args, &env);
            eprintln!("numbered: cannot start sh: {error}");
            process::exit_now(98)
        }
        Fork::Parent(child) => process::wait(child).unwrap().code().unwrap_or(-1),
    }
}

fn numbers_kept_for_the_shell_stay_out_of_the_way() {
    numbered::reserve(3..=9).unwrap();
    for n in 3..=9 {
        assert!(!numbered::is_open(n), "{n} holds a placeholder only");
    }
    let file = std::fs::File::open("/dev/null").unwrap();
    assert!(file.as_raw_fd() >= 10, "a file lands past the kept numbers");
    // A redirection at a kept number, then a close: the place is kept.
    let (read, write) = numbered::pipe().unwrap();
    numbered::duplicate_onto(write, 5).unwrap();
    assert!(numbered::is_open(5));
    assert!(
        !fd::close_on_exec(numbered::lend(5).unwrap()).unwrap(),
        "a redirection is kept on exec"
    );
    fd::write(numbered::lend(5).unwrap(), b"x").unwrap();
    let mut byte = [0];
    assert_eq!(fd::read(numbered::lend(read).unwrap(), &mut byte).unwrap(), 1);
    numbered::close(5).unwrap();
    assert!(!numbered::is_open(5));
    assert!(std::fs::File::open("/dev/null").unwrap().as_raw_fd() >= 10);
    numbered::close(read).unwrap();
    numbered::close(write).unwrap();
    assert_eq!(numbered::close(write).unwrap_err().raw_os_error(), Some(libc::EBADF));
}

fn a_number_held_elsewhere_is_refused() {
    let file = std::fs::File::open("/dev/null").unwrap();
    let n = file.as_raw_fd();
    let opened = numbered::open(Path::new("/dev/null"), libc::O_RDONLY, 0).unwrap();
    assert_eq!(numbered::duplicate_onto(opened, n).unwrap_err().raw_os_error(), Some(libc::EBUSY));
    assert_eq!(numbered::close(n).unwrap_err().raw_os_error(), Some(libc::EBADF));
    assert!(numbered::lend(n).is_err());
    assert!(!numbered::is_open(n));
    // The table's own: a copy above, and taken out.
    let copy = numbered::duplicate_above(opened, 20, true).unwrap();
    assert!(copy >= 20);
    let taken = numbered::take(copy).unwrap();
    assert_eq!(taken.as_raw_fd(), copy);
    assert!(!numbered::is_open(copy), "taken, it is the taker's");
    numbered::close(opened).unwrap();
    drop(file);
}

fn a_lent_number_is_neither_closed_nor_replaced() {
    let opened = numbered::open(Path::new("/dev/null"), libc::O_RDONLY, 0).unwrap();
    let other = numbered::open(Path::new("/dev/null"), libc::O_RDONLY, 0).unwrap();
    let lent = numbered::lend(opened).unwrap();
    assert_eq!(numbered::close(opened).unwrap_err().raw_os_error(), Some(libc::EBUSY));
    assert_eq!(
        numbered::duplicate_onto(other, opened).unwrap_err().raw_os_error(),
        Some(libc::EBUSY)
    );
    assert!(numbered::take(opened).is_err());
    drop(lent);
    numbered::duplicate_onto(other, opened).unwrap();
    numbered::close(opened).unwrap();
    numbered::close(other).unwrap();
}

fn a_closed_standard_number_is_closed_for_the_program_started() {
    // Standard output is closed in children only, so that this test's own
    // lines still show.
    let closed = sh("test -e /proc/$$/fd/1", || numbered::close(1).unwrap());
    assert_eq!(closed, 1, "the program finds 1 closed");
    assert_eq!(sh("test -e /proc/$$/fd/1", || {}), 0, "and open when it is open");
    let reopened = sh("test -e /proc/$$/fd/1", || {
        numbered::close(1).unwrap();
        assert!(!numbered::is_open(1) && numbered::lend(1).is_err());
        let null = numbered::open(Path::new("/dev/null"), libc::O_WRONLY, 0).unwrap();
        numbered::duplicate_onto(null, 1).unwrap();
        assert!(numbered::is_open(1));
    });
    assert_eq!(reopened, 0, "a redirection opens it again");
    // A program that cannot start: 1 is open again, on the null device.
    let failed = match process::fork().unwrap() {
        Fork::Child => {
            numbered::close(1).unwrap();
            let path = CString::new("/no/such/program").unwrap();
            let error = process::exec(&path, std::slice::from_ref(&path), &[]);
            let open = fd::write(Standard::Output.fd(), b"").is_ok();
            process::exit_now(if error.raw_os_error() == Some(libc::ENOENT) && open {
                0
            } else {
                1
            })
        }
        Fork::Parent(child) => process::wait(child).unwrap().code(),
    };
    assert_eq!(failed, Some(0));
}

fn a_redirection_reaches_the_program_started() {
    let (read, write) = fd::pipe().unwrap();
    let status = sh("echo reached >&6", || {
        let n = numbered::keep(write.try_clone().unwrap());
        numbered::duplicate_onto(n, 6).unwrap();
        numbered::close(n).unwrap();
    });
    drop(write);
    assert_eq!(status, 0);
    let mut text = String::new();
    std::fs::File::from(read).read_to_string(&mut text).unwrap();
    assert_eq!(text, "reached\n");
}

fn a_handed_down_number_is_taken_once() {
    let (read, write) = fd::pipe().unwrap();
    fd::write(&write, b"handed\n").unwrap();
    drop(write);
    let status = start_again("handed-down", || {
        let n = numbered::keep(read.try_clone().unwrap());
        numbered::duplicate_onto(n, 7).unwrap();
    });
    assert_eq!(status, 0);
}

fn a_program_keeps_the_ignored_signals_and_the_mask() {
    let ignored = sh("kill -USR2 $$; exit 5", || signal::ignore(libc::SIGUSR2).unwrap());
    assert_eq!(ignored, 5, "an ignored signal stays ignored");
    let blocked = sh("kill -USR1 $$; exit 6", || signal::block(&[libc::SIGUSR1]).unwrap());
    assert_eq!(blocked, 6, "a blocked signal stays blocked");
    let unblocked = sh("kill -USR1 $$; exit 6", || {
        signal::block(&[libc::SIGUSR1]).unwrap();
        signal::unblock_all().unwrap();
    });
    assert_ne!(unblocked, 6, "and ends the program once unblocked");
}

fn sigpipe_ignored_by_the_parent_is_seen() {
    assert!(!signal::ignored_at_start(libc::SIGPIPE), "cargo starts tests with its default");
    assert_eq!(signal::disposition(libc::SIGPIPE).unwrap(), signal::Disposition::Ignored);
    // Rust's runtime ignores SIGPIPE, and exec keeps it so.
    assert_eq!(start_again("sigpipe", || {}), 0);
}

fn a_farewell_ends_the_process_from_the_handler() {
    static CAUGHT: signal::Caught = signal::Caught::new();
    let path = std::env::temp_dir().join(format!("sysapi-farewell-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let farewell = signal::Farewell {
        signals: Vec::new(),
        append: Some((path.clone(), b"farewell\n".to_vec())),
        status: 42,
    };
    let ended = match process::fork().unwrap() {
        Fork::Child => {
            signal::catch(libc::SIGHUP, &CAUGHT, 0).unwrap();
            signal::arm_farewell(libc::SIGHUP, farewell.clone()).unwrap();
            let _ = signal::raise(libc::SIGHUP);
            process::exit_now(1)
        }
        Fork::Parent(child) => process::wait(child).unwrap().code(),
    };
    assert_eq!(ended, Some(42));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "farewell\n");
    std::fs::remove_file(&path).unwrap();
    // Put away, the signal is recorded again.
    let recorded = match process::fork().unwrap() {
        Fork::Child => {
            signal::catch(libc::SIGHUP, &CAUGHT, 0).unwrap();
            signal::arm_farewell(libc::SIGHUP, farewell).unwrap();
            signal::disarm_farewell();
            let _ = signal::raise(libc::SIGHUP);
            process::exit_now(if CAUGHT.take() { 0 } else { 1 })
        }
        Fork::Parent(child) => process::wait(child).unwrap().code(),
    };
    assert_eq!(recorded, Some(0));
    assert!(!path.exists());
    assert_eq!(signal::disposition(libc::SIGHUP).unwrap(), signal::Disposition::Default);
}
