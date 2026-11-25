// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The calls that need a process with one thread: a fork, and a change to
//! the environment. The test harness would run each check on a thread of
//! its own, so this test has none (`harness = false`) and runs its checks
//! in order on the main thread.

#![forbid(unsafe_code)]

use std::ffi::OsStr;

use sysapi::process::{self, Fork};

fn main() {
    a_child_of_a_fork_ends_with_its_own_status();
    the_environment_changes_while_there_is_one_thread();
    a_second_thread_stops_both();
    println!("one_thread: 3 checks passed");
}

fn a_child_of_a_fork_ends_with_its_own_status() {
    match process::fork().unwrap() {
        Fork::Child => process::exit_now(7),
        Fork::Parent(child) => assert_eq!(process::wait(child).unwrap().code(), Some(7)),
    }
}

fn the_environment_changes_while_there_is_one_thread() {
    process::set_env(OsStr::new("SYSAPI_ONE_THREAD"), OsStr::new("set")).unwrap();
    assert_eq!(std::env::var("SYSAPI_ONE_THREAD").unwrap(), "set");
    process::remove_env(OsStr::new("SYSAPI_ONE_THREAD")).unwrap();
    assert!(std::env::var_os("SYSAPI_ONE_THREAD").is_none());
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
