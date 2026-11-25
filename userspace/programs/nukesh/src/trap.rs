// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signals: their names, the handler that notes them, and traps.
//!
//! The handler only records which signal came; the shell runs the trap's
//! command at the next safe point, after the command that was running (as
//! POSIX asks), or at once when the `wait` builtin or `read` is waiting.

use std::sync::atomic::Ordering;

use sysapi::signal::{self as os, Caught};

/// The highest signal number the shell knows.
pub const MAX_SIGNAL: usize = 64;

/// Which signals came, by number.
static CAUGHT: [Caught; MAX_SIGNAL + 1] = [const { Caught::new() }; MAX_SIGNAL + 1];

/// The pseudo-signals `trap` takes besides real ones.
pub const EXIT: usize = 0;
pub const ERR: usize = MAX_SIGNAL + 1;
pub const DEBUG: usize = MAX_SIGNAL + 2;
pub const RETURN: usize = MAX_SIGNAL + 3;
/// How many trap slots there are.
pub const SLOTS: usize = MAX_SIGNAL + 4;

/// The signals by name, without `SIG`, in number order.
pub const SIGNALS: &[(&str, i32)] = &[
    ("HUP", libc::SIGHUP),
    ("INT", libc::SIGINT),
    ("QUIT", libc::SIGQUIT),
    ("ILL", libc::SIGILL),
    ("TRAP", libc::SIGTRAP),
    ("ABRT", libc::SIGABRT),
    ("BUS", libc::SIGBUS),
    ("FPE", libc::SIGFPE),
    ("KILL", libc::SIGKILL),
    ("USR1", libc::SIGUSR1),
    ("SEGV", libc::SIGSEGV),
    ("USR2", libc::SIGUSR2),
    ("PIPE", libc::SIGPIPE),
    ("ALRM", libc::SIGALRM),
    ("TERM", libc::SIGTERM),
    ("STKFLT", libc::SIGSTKFLT),
    ("CHLD", libc::SIGCHLD),
    ("CONT", libc::SIGCONT),
    ("STOP", libc::SIGSTOP),
    ("TSTP", libc::SIGTSTP),
    ("TTIN", libc::SIGTTIN),
    ("TTOU", libc::SIGTTOU),
    ("URG", libc::SIGURG),
    ("XCPU", libc::SIGXCPU),
    ("XFSZ", libc::SIGXFSZ),
    ("VTALRM", libc::SIGVTALRM),
    ("PROF", libc::SIGPROF),
    ("WINCH", libc::SIGWINCH),
    ("IO", libc::SIGIO),
    ("PWR", libc::SIGPWR),
    ("SYS", libc::SIGSYS),
];

/// The name of signal `number`, without `SIG`.
pub fn name(number: i32) -> Option<String> {
    match SIGNALS.iter().find(|(_, n)| *n == number) {
        Some((name, _)) => Some((*name).to_string()),
        None => real_time_name(number),
    }
}

/// The real-time signals, as the C library has them.
fn real_time() -> std::ops::RangeInclusive<i32> {
    libc::SIGRTMIN()..=libc::SIGRTMAX()
}

/// A real-time signal's name, as bash names them: RTMIN+n in the lower
/// half, RTMAX-n in the upper.
fn real_time_name(number: i32) -> Option<String> {
    let range = real_time();
    let (min, max) = (*range.start(), *range.end());
    range.contains(&number).then(|| match number {
        n if n == min => "RTMIN".to_string(),
        n if n == max => "RTMAX".to_string(),
        n if n - min <= (max - min) / 2 => format!("RTMIN+{}", n - min),
        n => format!("RTMAX-{}", max - n),
    })
}

/// Every signal with a name, without `SIG`, in number order.
pub fn names() -> Vec<(String, i32)> {
    let named = SIGNALS.iter().map(|(name, n)| ((*name).to_string(), *n));
    named.chain(real_time().filter_map(|n| real_time_name(n).map(|name| (name, n)))).collect()
}

/// The table of signals `trap -l` and `kill -l` print, as bash prints
/// it: five a line, each followed by a tab but the fifth.
pub fn table() -> String {
    let mut text = String::new();
    for (i, (name, number)) in names().iter().enumerate() {
        text.push_str(&format!("{number:2}) SIG{name}"));
        text.push(if i % 5 == 4 { '\n' } else { '\t' });
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The number of a signal given by name (with or without `SIG`, any case)
/// or by number; for `trap`, also EXIT, ERR, DEBUG and RETURN.
pub fn parse(text: &str, pseudo: bool) -> Option<usize> {
    if let Ok(number) = text.parse::<usize>() {
        return (number <= MAX_SIGNAL && (number > 0 || pseudo)).then_some(number);
    }
    let upper = text.to_ascii_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    if pseudo {
        match bare {
            "EXIT" => return Some(EXIT),
            "ERR" => return Some(ERR),
            "DEBUG" => return Some(DEBUG),
            "RETURN" => return Some(RETURN),
            _ => {}
        }
    }
    if let Some((_, n)) = SIGNALS.iter().find(|(name, _)| *name == bare) {
        return Some(*n as usize);
    }
    // RTMIN+n and RTMAX-n for any n in the range, as bash takes them.
    let range = real_time();
    let number = match bare {
        "RTMIN" => *range.start(),
        "RTMAX" => *range.end(),
        _ => match (bare.strip_prefix("RTMIN+"), bare.strip_prefix("RTMAX-")) {
            (Some(n), _) => range.start() + n.parse::<i32>().ok()?,
            (_, Some(n)) => range.end() - n.parse::<i32>().ok()?,
            _ => return None,
        },
    };
    range.contains(&number).then_some(number as usize)
}

/// A trap slot's name, as `trap -p` prints it.
pub fn slot_name(slot: usize) -> String {
    match slot {
        EXIT => "EXIT".into(),
        ERR => "ERR".into(),
        DEBUG => "DEBUG".into(),
        RETURN => "RETURN".into(),
        n => match name(n as i32) {
            Some(name) => format!("SIG{name}"),
            None => n.to_string(),
        },
    }
}

/// What a signal does to the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Default,
    Ignore,
    /// Noted, for a trap or the shell's own use.
    Catch,
}

pub fn set(signal: i32, disposition: Disposition) {
    // The line editor's terminal library catches SIGWINCH to redraw, and
    // passes the signal on to the handler it found, ours: a handler that
    // is neither ours nor a default stays.
    if signal == libc::SIGWINCH
        && disposition == Disposition::Catch
        && os::disposition(signal).is_ok_and(|now| now == os::Disposition::Other)
    {
        return;
    }
    // A signal that cannot be caught or ignored keeps what it does. No
    // SA_RESTART: a caught signal interrupts `wait` and `read`.
    let _ = match disposition {
        Disposition::Default => os::restore_default(signal),
        Disposition::Ignore => os::ignore(signal),
        Disposition::Catch => match caught(signal) {
            Some(caught) => os::catch(signal, caught, 0),
            None => Ok(()),
        },
    };
}

/// The record of `signal`, a real signal's.
fn caught(signal: i32) -> Option<&'static Caught> {
    usize::try_from(signal).ok().filter(|&n| n > 0).and_then(|n| CAUGHT.get(n))
}

/// Gives SIGPIPE back the disposition it had when the program was
/// started, which Rust's runtime changed: at its default, a writer to a
/// closed pipe ends, as under bash, and the programs the shell runs get
/// it so too.
pub fn restore_sigpipe() {
    if !os::ignored_at_start(libc::SIGPIPE) {
        set(libc::SIGPIPE, Disposition::Default);
    }
}

/// Whether `signal` was ignored when the shell started, which makes a
/// non-interactive shell leave it so (XCU 2.11).
pub fn ignored_at_start(signal: i32) -> bool {
    os::ignored_at_start(signal)
}

/// Unblocks every signal, for a child about to run a program.
pub fn unblock_all() {
    let _ = os::unblock_all();
}

/// Whether any signal came since [`take`] last looked.
pub fn pending() -> bool {
    CAUGHT[1..].iter().any(Caught::is_set)
}

/// The signals that came, each once, in number order.
pub fn take() -> Vec<i32> {
    (1..=MAX_SIGNAL).filter(|&n| CAUGHT[n].take()).map(|n| n as i32).collect()
}

/// Whether `signal` came, forgetting it.
pub fn take_one(signal: i32) -> bool {
    caught(signal).is_some_and(Caught::take)
}

/// Notes `signal` as if it had come, so that its trap runs at the next
/// safe point.
pub fn raise_later(signal: i32) {
    if let Some(caught) = caught(signal) {
        caught.flag().store(true, Ordering::SeqCst);
    }
}

/// The lowest signal that came and has not been taken, if any.
pub fn first_pending() -> Option<i32> {
    (1..=MAX_SIGNAL).find(|&n| CAUGHT[n].is_set()).map(|n| n as i32)
}

/// Whether `signal` came and has not been taken.
pub fn is_pending(signal: i32) -> bool {
    caught(signal).is_some_and(Caught::is_set)
}

/// Whether a signal other than `signal` came and has not been taken.
pub fn pending_other_than(signal: i32) -> bool {
    (1..=MAX_SIGNAL).any(|n| n as i32 != signal && CAUGHT[n].is_set())
}
