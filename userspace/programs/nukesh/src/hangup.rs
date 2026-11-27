// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A hang-up at the prompt (ADR 0267). When the terminal goes away while
//! the line editor waits, its terminal library reads nothing forever and
//! never returns, so the shell would spin rather than reach the place
//! where it handles SIGHUP. So before each prompt the shell readies what
//! a hang-up must do, as a farewell (`sysapi::signal::Farewell`) that the
//! SIGHUP handler carries out with only async-signal-safe calls: SIGHUP
//! (and SIGCONT to the stopped) to each job not marked by `disown -h`,
//! the history entries not yet saved appended to `HISTFILE`, and the
//! exit, with status 129. The EXIT trap does not run then; elsewhere
//! SIGHUP is handled as before.

use std::path::PathBuf;

use sysapi::signal::{self, Farewell};

use crate::shell::Shell;
use crate::text;

/// Before the prompt: readies the hang-up, which a SIGHUP the shell
/// catches carries out until [`leave_prompt`].
pub fn enter_prompt(shell: &Shell) {
    let mut signals = Vec::new();
    for job in shell.jobs.list.iter().filter(|j| !j.nohup && !j.done()) {
        for target in job.targets() {
            signals.push((target, libc::SIGHUP));
            if job.stopped() {
                signals.push((target, libc::SIGCONT));
            }
        }
    }
    // A name with a NUL byte names no file.
    let append = (shell.interactive && !shell.private)
        .then(|| shell.history_file())
        .flatten()
        .filter(|path| !path.contains('\0'))
        .and_then(|path| {
            let new = shell.history.unsaved();
            if new.is_empty() {
                return None;
            }
            let data = text::encode(&crate::history::format_entries(new));
            Some((PathBuf::from(text::to_os(&path)), data))
        });
    let farewell = Farewell { signals, append, status: 128 + libc::SIGHUP };
    // SIGHUP is a signal, and the path has no NUL byte: it is armed.
    let _ = signal::arm_farewell(libc::SIGHUP, farewell);
}

/// After the prompt.
pub fn leave_prompt() {
    signal::disarm_farewell();
}
