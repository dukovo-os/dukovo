// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The calls that need a process with one thread: a fork, and a change to
//! the environment. The test harness would run each check on a thread of
//! its own, so this test has none (`harness = false`) and runs its checks
//! in order on the main thread.

#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

use sysapi::process::{self, Fork};
use sysapi::{signal, terminal};

fn main() {
    a_child_of_a_fork_ends_with_its_own_status();
    any_child_is_waited_for_and_a_stopped_one_is_seen();
    a_new_session_takes_control_of_a_terminal();
    the_environment_changes_while_there_is_one_thread();
    a_joined_thread_does_not_stop_a_fork();
    a_second_thread_stops_both();
    println!("one_thread: 6 checks passed");
}

fn a_child_of_a_fork_ends_with_its_own_status() {
    match process::fork().unwrap() {
        Fork::Child => process::exit_now(7),
        Fork::Parent(child) => assert_eq!(process::wait(child).unwrap().code(), Some(7)),
    }
}

fn any_child_is_waited_for_and_a_stopped_one_is_seen() {
    let child = match process::fork().unwrap() {
        Fork::Child => {
            let _ = signal::raise(libc::SIGSTOP);
            process::exit_now(3)
        }
        Fork::Parent(child) => child,
    };
    let mut stopped = None;
    for _ in 0..500 {
        stopped = process::reap_or_stopped();
        if stopped.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (pid, status) = stopped.expect("the child stops itself");
    assert_eq!((pid, status.stopped_signal()), (child, Some(libc::SIGSTOP)));
    process::kill(child, libc::SIGCONT).unwrap();
    let (pid, status) = process::wait_any().unwrap();
    assert_eq!((pid, status.code()), (child, Some(3)));
}

fn a_new_session_takes_control_of_a_terminal() {
    let pty = terminal::open_pty(None).unwrap();
    match process::fork().unwrap() {
        Fork::Child => {
            let session = process::new_session().unwrap();
            terminal::take_control(&pty.terminal).unwrap();
            let foreground = terminal::foreground_group(&pty.terminal).unwrap();
            process::exit_now(if foreground == session { 0 } else { 1 })
        }
        Fork::Parent(child) => assert_eq!(process::wait(child).unwrap().code(), Some(0)),
    }
}

fn the_environment_changes_while_there_is_one_thread() {
    process::set_env(OsStr::new("SYSAPI_ONE_THREAD"), OsStr::new("set")).unwrap();
    assert_eq!(std::env::var("SYSAPI_ONE_THREAD").unwrap(), "set");
    process::remove_env(OsStr::new("SYSAPI_ONE_THREAD")).unwrap();
    assert!(std::env::var_os("SYSAPI_ONE_THREAD").is_none());
}

/// Linux lists a joined thread in /proc/self/task for some microseconds
/// after the join returns, about once in a hundred joins.
fn a_joined_thread_does_not_stop_a_fork() {
    for _ in 0..2000 {
        std::thread::spawn(|| {}).join().unwrap();
        match process::fork().unwrap() {
            Fork::Child => process::exit_now(0),
            Fork::Parent(child) => assert_eq!(process::wait(child).unwrap().code(), Some(0)),
        }
    }
}

fn a_second_thread_stops_both() {
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let second = std::thread::spawn(move || stopped.recv());
    let fork = process::fork().unwrap_err();
    let set = process::set_env(OsStr::new("SYSAPI_ONE_THREAD"), OsStr::new("1")).unwrap_err();
    stop.send(()).unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(fork.kind(), std::io::ErrorKind::Unsupported);
    assert_eq!(set.kind(), std::io::ErrorKind::Unsupported);
    assert!(std::env::var_os("SYSAPI_ONE_THREAD").is_none());
}
